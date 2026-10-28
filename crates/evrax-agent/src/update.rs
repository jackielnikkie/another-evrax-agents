//! Update check against the GitHub releases API.

use std::time::Duration;

const API_RELEASE_URL: &str =
    "https://api.github.com/repos/jackielnikkie/another-evrax-agents/releases/latest";
/// The web endpoint redirects to `/releases/tag/<tag>`; unlike the API it has
/// no unauthenticated rate limit, so it backs up the API path.
const WEB_RELEASE_URL: &str =
    "https://github.com/jackielnikkie/another-evrax-agents/releases/latest";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateCheck {
    pub latest: String,
    pub url: String,
}

/// Returns `Some` only when `latest` is strictly newer than `current`.
///
/// Tries the GitHub API first (gives the exact `html_url`), then falls back to
/// the unauthenticated web redirect when the API is rate-limited or blocked
/// (HTTP 403), which is common behind shared IPs.
pub async fn check_update(current: &str) -> Result<Option<UpdateCheck>, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .read_timeout(Duration::from_secs(8))
        .no_proxy()
        .build()
        .map_err(|e| e.to_string())?;

    let release = match fetch_api(&client).await {
        Ok(release) => release,
        Err(api_error) => fetch_web(&client)
            .await
            .map_err(|web_error| format!("{api_error}; fallback failed: {web_error}"))?,
    };
    if is_newer(&release.latest, current) {
        Ok(Some(release))
    } else {
        Ok(None)
    }
}

async fn fetch_api(client: &reqwest::Client) -> Result<UpdateCheck, String> {
    let response = client
        .get(API_RELEASE_URL)
        .header("User-Agent", "evrax")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "HTTP {} when checking for updates",
            status.as_u16()
        ));
    }
    let body = response.text().await.map_err(|e| e.to_string())?;
    parse_release(&body)
}

/// Follows the web redirect manually so the tag can be read from the
/// `Location` header without the API.
async fn fetch_web(client: &reqwest::Client) -> Result<UpdateCheck, String> {
    let response = client
        .get(WEB_RELEASE_URL)
        .header("User-Agent", "evrax")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| "no release redirect".to_string())?;
    parse_redirect_location(location)
}

/// Pure version comparison: true only when `latest` is strictly newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let trimmed = s.trim_start_matches(['v', 'V']);
    let mut parts = trimmed.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn parse_release(body: &str) -> Result<UpdateCheck, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "unexpected release payload".to_string())?;
    let latest = value
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "unexpected release payload".to_string())?
        .to_string();
    let url = value
        .get("html_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "unexpected release payload".to_string())?
        .to_string();
    if latest.is_empty() || url.is_empty() {
        return Err("unexpected release payload".to_string());
    }
    Ok(UpdateCheck { latest, url })
}

/// Extracts the tag from a `/releases/tag/<tag>` redirect target.
fn parse_redirect_location(location: &str) -> Result<UpdateCheck, String> {
    let marker = "/releases/tag/";
    let index = location
        .find(marker)
        .ok_or_else(|| "unexpected release redirect".to_string())?;
    let tag = location[index + marker.len()..]
        .split(['?', '#', '/'])
        .next()
        .unwrap_or("")
        .trim();
    if tag.is_empty() {
        return Err("unexpected release redirect".to_string());
    }
    let url = location
        .split(['?', '#'])
        .next()
        .unwrap_or(location)
        .to_string();
    Ok(UpdateCheck {
        latest: tag.to_string(),
        url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_table() {
        // newer
        assert!(is_newer("v1.10.0", "v1.9.9"));
        assert!(is_newer("v0.1.4", "v0.1.3"));
        assert!(is_newer("v0.2.0", "v0.1.9"));
        // equal
        assert!(!is_newer("v0.1.4", "v0.1.4"));
        // older
        assert!(!is_newer("v0.1.3", "v0.1.4"));
        // no v prefix
        assert!(is_newer("1.2.3", "1.2.2"));
        // capital V prefix
        assert!(is_newer("V1.0.1", "V1.0.0"));
        // garbage
        assert!(!is_newer("not-a-version", "v0.1.4"));
        assert!(!is_newer("v0.1.4", "latest"));
        assert!(!is_newer("v0.1", "v0.1.4"));
        assert!(!is_newer("v0.1.4.1", "v0.1.4"));
        assert!(!is_newer("", ""));
        assert!(!is_newer("v0.1.4-beta", "v0.1.3"));
    }

    const RELEASE_JSON: &str = r#"{
  "url": "https://api.github.com/repos/jackielnikkie/another-evrax-agents/releases/123456",
  "html_url": "https://github.com/jackielnikkie/another-evrax-agents/releases/tag/v0.1.5",
  "id": 123456,
  "tag_name": "v0.1.5",
  "name": "v0.1.5",
  "draft": false,
  "prerelease": false,
  "assets": []
}"#;

    #[test]
    fn parse_release_reads_tag_and_url() {
        let release = parse_release(RELEASE_JSON).expect("valid payload");
        assert_eq!(release.latest, "v0.1.5");
        assert_eq!(
            release.url,
            "https://github.com/jackielnikkie/another-evrax-agents/releases/tag/v0.1.5"
        );
    }

    #[test]
    fn parse_redirect_location_reads_tag() {
        let location = "https://github.com/jackielnikkie/another-evrax-agents/releases/tag/v0.1.4";
        let release = parse_redirect_location(location).expect("valid redirect");
        assert_eq!(release.latest, "v0.1.4");
        assert_eq!(release.url, location);
        // trailing query/hash stripped
        let with_query =
            parse_redirect_location("https://github.com/o/r/releases/tag/v9.9.9?foo=bar")
                .expect("valid redirect with query");
        assert_eq!(with_query.latest, "v9.9.9");
        assert_eq!(with_query.url, "https://github.com/o/r/releases/tag/v9.9.9");
        // no tag
        assert!(parse_redirect_location("https://github.com/o/r/releases").is_err());
        assert!(parse_redirect_location("https://example.com/").is_err());
    }

    #[test]
    fn parse_release_rejects_bad_payloads() {
        assert!(parse_release("").is_err());
        assert!(parse_release("{not json").is_err());
        assert!(parse_release("{}").is_err());
        assert!(
            parse_release(r#"{"tag_name":"v0.1.5"}"#).is_err(),
            "missing html_url"
        );
        assert!(
            parse_release(r#"{"html_url":"https://example.com"}"#).is_err(),
            "missing tag_name"
        );
        assert!(
            parse_release(r#"{"tag_name":"v0.1.5","html_url":""}"#).is_err(),
            "empty html_url"
        );
        assert!(
            parse_release(r#"{"tag_name":"","html_url":"https://example.com"}"#).is_err(),
            "empty tag_name"
        );
    }
}
