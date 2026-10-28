use std::time::Duration;

use regex::Regex;
use serde_json::Value;

use super::{ToolOutcome, arg_str};

const MAX_BYTES: usize = 1024 * 1024;

pub(super) async fn run(args: &Value) -> Result<ToolOutcome, String> {
    let raw = arg_str(args, "url")?;
    let parsed = reqwest::Url::parse(&raw).map_err(|e| format!("bad url: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("only http(s) URLs are allowed".into());
    }
    let format = args
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("text")
        .to_ascii_lowercase();
    let max_chars = args
        .get("max_chars")
        .and_then(Value::as_u64)
        .unwrap_or(16_000)
        .clamp(200, 100_000) as usize;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent(concat!("evrax/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = client
        .get(parsed.clone())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    let mut bytes = Vec::new();
    let mut capped = false;
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() >= MAX_BYTES {
            capped = true;
            break;
        }
    }
    let body = String::from_utf8_lossy(&bytes).to_string();
    let body = if format == "raw" {
        body
    } else {
        html_to_text(&body)
    };
    let body = truncate_chars(&body, max_chars);
    let mut marker = String::new();
    if capped {
        marker.push_str("\n[response body capped at 1 MiB]");
    }
    Ok(ToolOutcome {
        ok: status.is_success(),
        detail: format!(
            "GET {} -> HTTP {status}\ncontent-type: {content_type}\n\n{}{marker}",
            parsed.as_str(),
            if body.trim().is_empty() {
                "(empty body)"
            } else {
                body.trim_end()
            }
        ),
        diff: None,
    })
}

pub(super) fn html_to_text(html: &str) -> String {
    let script = Regex::new(r"(?is)<script[^>]*>.*?</script>|<style[^>]*>.*?</style>")
        .unwrap()
        .replace_all(html, " ");
    let without_tags = Regex::new(r"(?is)<[^>]+>")
        .unwrap()
        .replace_all(&script, " ");
    let compact = without_tags
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    let mut out = String::new();
    let mut spaces = 0;
    let mut newlines = 0;
    for ch in compact.chars() {
        match ch {
            ' ' | '\t' | '\r' | '\n' => {
                if ch == '\n' {
                    newlines += 1;
                } else {
                    spaces += 1;
                }
            }
            other => {
                if newlines > 0 {
                    out.push_str("\n".repeat(newlines.min(2)).as_str());
                    newlines = 0;
                } else if spaces > 0 {
                    out.push(' ');
                }
                spaces = 0;
                out.push(other);
            }
        }
    }
    out
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out = text.chars().take(max).collect::<String>();
    out.push_str("…[truncated]");
    out
}

const MAX_HITS: usize = 8;
const MAX_SEARCH_CHARS: usize = 8_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

pub(super) async fn search(args: &Value) -> Result<ToolOutcome, String> {
    let query = arg_str(args, "query")?;
    if query.trim().is_empty() {
        return Err("`query` must not be empty".into());
    }
    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        urlencode(query.trim())
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent(concat!("evrax/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let body = client
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    let hits = parse_ddg(&body);
    if hits.is_empty() {
        return Ok(ToolOutcome {
            ok: false,
            detail: "no results".into(),
            diff: None,
        });
    }
    let mut out = String::new();
    for (i, hit) in hits.iter().enumerate() {
        out.push_str(&format!("{}. {}\n   {}\n", i + 1, hit.title, hit.url));
        if !hit.snippet.is_empty() {
            out.push_str(&format!("   {}\n", hit.snippet));
        }
    }
    let out = truncate_chars(out.trim_end(), MAX_SEARCH_CHARS);
    Ok(ToolOutcome {
        ok: true,
        detail: format!("{} results\n{out}", hits.len()),
        diff: None,
    })
}

/// Parse a DuckDuckGo HTML endpoint body into ranked hits. Pure (offline).
pub(super) fn parse_ddg(html: &str) -> Vec<SearchHit> {
    let anchor_re = Regex::new(r#"(?is)<a\b([^>]*)>(.*?)</a>"#).unwrap();
    let class_re = Regex::new(r##"(?i)class\s*=\s*"([^"]*)""##).unwrap();
    let href_re = Regex::new(r##"(?i)href\s*=\s*"([^"]*)""##).unwrap();
    let mut titles: Vec<(String, String)> = Vec::new();
    let mut snippets: Vec<String> = Vec::new();
    for cap in anchor_re.captures_iter(html) {
        let attrs = &cap[1];
        let class = class_re
            .captures(attrs)
            .map(|c| c[1].to_lowercase())
            .unwrap_or_default();
        let inner = &cap[2];
        if class.contains("result__a") {
            if let Some(href) = href_re.captures(attrs).map(|c| c[1].to_string()) {
                titles.push((href, clean_fragment(inner)));
            }
        } else if class.contains("result__snippet") {
            snippets.push(clean_fragment(inner));
        }
    }
    let mut hits = Vec::new();
    for (i, (href, title)) in titles.iter().enumerate() {
        if hits.len() >= MAX_HITS {
            break;
        }
        let Some(url) = resolve_href(href) else {
            continue;
        };
        if url.contains("/y.js") {
            continue;
        }
        hits.push(SearchHit {
            title: title.clone(),
            url,
            snippet: snippets.get(i).cloned().unwrap_or_default(),
        });
    }
    hits
}

/// Decode a `//duckduckgo.com/l/?uddg=<encoded>` redirect wrapper, if present.
pub(super) fn decode_ddg_redirect(href: &str) -> Option<String> {
    let href = href.replace("&amp;", "&");
    let marker = "uddg=";
    let start = href.find(marker)? + marker.len();
    let rest = &href[start..];
    let end = rest.find('&').unwrap_or(rest.len());
    let decoded = percent_decode(&rest[..end]);
    if decoded.is_empty() {
        None
    } else {
        Some(decoded)
    }
}

fn resolve_href(href: &str) -> Option<String> {
    if let Some(decoded) = decode_ddg_redirect(href) {
        return Some(decoded);
    }
    let h = href.replace("&amp;", "&");
    let h = h.trim();
    if !(h.starts_with("http://") || h.starts_with("https://")) {
        return None;
    }
    if h.contains("duckduckgo.com/l/") {
        return None;
    }
    Some(h.to_string())
}

fn clean_fragment(fragment: &str) -> String {
    let stripped = Regex::new(r"(?is)<[^>]+>")
        .unwrap()
        .replace_all(fragment, " ");
    let decoded = stripped
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn urlencode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_to_text_strips_scripts_and_tags() {
        let text = html_to_text("<h1>Hello</h1><script>evil()</script><p>World</p>");
        assert_eq!(text.trim(), "Hello World");
    }

    #[tokio::test]
    async fn rejects_non_http_urls() {
        let err = run(&serde_json::json!({"url":"file:///tmp/x"}))
            .await
            .unwrap_err();
        assert!(err.contains("http"));
    }

    #[tokio::test]
    async fn fetches_a_local_page() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let address = server.server_addr().to_ip().unwrap();
        let thread = std::thread::spawn(move || {
            let request = server.recv().unwrap();
            request
                .respond(tiny_http::Response::from_string("<h1>hello</h1>"))
                .unwrap();
        });
        let url = format!("http://{address}/");
        let out = run(&serde_json::json!({"url":url})).await.unwrap();
        thread.join().unwrap();
        assert!(out.ok);
        assert!(out.detail.contains("hello"));
    }

    const FIXTURE: &str = r##"
<div class="result results_links results_links_deep web-result">
  <div class="links_main links_deep result__body">
    <h2 class="result__title">
      <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Frust&amp;rut=abc">Rust Programming Language</a>
    </h2>
    <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Frust"><b>Rust</b> is a systems language.</a>
  </div>
</div>
<div class="result results_links results_links_deep web-result">
  <div class="links_main links_deep result__body">
    <h2 class="result__title">
      <a rel="nofollow" class="result__a" href="https://doc.rust-lang.org/book/">The Book</a>
    </h2>
    <a class="result__snippet" href="#">Learn Rust &amp; more &#x27;today&#x27;.</a>
  </div>
</div>
<a rel="nofollow" class="result__a" href="https://duckduckgo.com/y.js?ad">Ad</a>
"##;

    #[test]
    fn parse_ddg_extracts_results() {
        let hits = parse_ddg(FIXTURE);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Rust Programming Language");
        assert_eq!(hits[0].url, "https://example.com/rust");
        assert_eq!(hits[0].snippet, "Rust is a systems language.");
        assert_eq!(hits[1].title, "The Book");
        assert_eq!(hits[1].url, "https://doc.rust-lang.org/book/");
        assert_eq!(hits[1].snippet, "Learn Rust & more 'today'.");
    }

    #[test]
    fn decode_ddg_redirect_decodes_uddg() {
        assert_eq!(
            decode_ddg_redirect("//duckduckgo.com/l/?uddg=https%3A%2F%2Fa.dev%2Fx&rut=1"),
            Some("https://a.dev/x".to_string())
        );
        assert_eq!(decode_ddg_redirect("https://example.com/plain"), None);
    }

    #[test]
    fn parse_ddg_empty() {
        assert!(parse_ddg("<html><body>no results</body></html>").is_empty());
    }

    #[test]
    fn urlencode_encodes_reserved() {
        assert_eq!(urlencode("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(urlencode("safe-._~"), "safe-._~");
    }
}
