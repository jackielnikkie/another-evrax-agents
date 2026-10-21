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
}
