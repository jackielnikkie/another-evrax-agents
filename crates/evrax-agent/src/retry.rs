//! Provider retry policy: only transient failures are
//! retried, with exponential backoff, jitter and provider-requested delays.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::StatusCode;
use reqwest::header::HeaderMap;

pub const MAX_RETRIES: u32 = 10;
pub const BASE_DELAY_MS: u64 = 2_000;
pub const MAX_DELAY_MS: u64 = 10_000;
pub const MAX_RETRY_AFTER_MS: u64 = 15 * 60 * 1_000;
pub const MAX_TIMEOUT_RETRIES: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Safe to send again: rate limits, provider-side failures, transport.
    Retryable {
        reason: String,
        retry_after_ms: Option<u64>,
        timeout: bool,
    },
    /// Deterministic rejection (auth, quota, invalid request): never retried.
    Fatal { reason: String },
}

impl Failure {
    pub fn reason(&self) -> &str {
        match self {
            Self::Retryable { reason, .. } | Self::Fatal { reason } => reason,
        }
    }
}

/// Retry budget for one provider call.
#[derive(Debug, Default)]
pub struct RetryState {
    pub retries: u32,
    pub timeouts: u32,
}

impl RetryState {
    /// Registers one failure and returns the delay before the next attempt, or
    /// `None` when the failure is fatal or a retry budget is exhausted.
    pub fn next(&mut self, failure: &Failure) -> Option<u64> {
        let Failure::Retryable {
            retry_after_ms,
            timeout,
            ..
        } = failure
        else {
            return None;
        };
        if self.retries >= MAX_RETRIES {
            return None;
        }
        if *timeout {
            if self.timeouts >= MAX_TIMEOUT_RETRIES {
                return None;
            }
            self.timeouts += 1;
        }
        self.retries += 1;
        Some(delay_ms(self.retries, *retry_after_ms))
    }
}

pub fn classify_status(status: StatusCode, headers: &HeaderMap) -> Failure {
    if let Some(force) = header(headers, "x-should-retry") {
        if force.eq_ignore_ascii_case("true") {
            return Failure::Retryable {
                reason: format!("HTTP {status}"),
                retry_after_ms: retry_after_ms(headers),
                timeout: status.as_u16() == 408,
            };
        }
        if force.eq_ignore_ascii_case("false") {
            return Failure::Fatal {
                reason: format!("HTTP {status}"),
            };
        }
    }
    let reason = format!("HTTP {status}");
    match status.as_u16() {
        408 => Failure::Retryable {
            reason,
            retry_after_ms: retry_after_ms(headers),
            timeout: true,
        },
        429 => Failure::Retryable {
            reason,
            retry_after_ms: retry_after_ms(headers),
            timeout: false,
        },
        500..=599 => Failure::Retryable {
            reason,
            retry_after_ms: retry_after_ms(headers),
            timeout: false,
        },
        _ => Failure::Fatal { reason },
    }
}

pub fn classify_error(error: &reqwest::Error) -> Failure {
    Failure::Retryable {
        reason: "jaringan".into(),
        retry_after_ms: None,
        timeout: error.is_timeout(),
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

/// `Retry-After` may be milliseconds (`retry-after-ms`) or whole seconds.
/// HTTP-date values are ignored; the exponential delay still applies.
pub fn retry_after_ms(headers: &HeaderMap) -> Option<u64> {
    if let Some(value) = header(headers, "retry-after-ms")
        && let Ok(ms) = value.trim().parse::<u64>()
    {
        return Some(ms.min(MAX_RETRY_AFTER_MS));
    }
    if let Some(value) = header(headers, "retry-after")
        && let Ok(seconds) = value.trim().parse::<u64>()
    {
        return Some(seconds.saturating_mul(1_000).min(MAX_RETRY_AFTER_MS));
    }
    None
}

/// Exponential 2s, 4s, 8s, then 10s per gap; a provider-requested delay wins
/// (capped at 15 minutes).
pub fn delay_ms(attempt: u32, retry_after_ms: Option<u64>) -> u64 {
    let shift = attempt.saturating_sub(1).min(10);
    let exponential = BASE_DELAY_MS
        .saturating_mul(1u64 << shift)
        .min(MAX_DELAY_MS);
    exponential.max(retry_after_ms.unwrap_or(0).min(MAX_RETRY_AFTER_MS))
}

/// Adds up to 250 ms of jitter so parallel clients do not retry in lockstep.
pub fn sleep(delay_ms: u64) -> Duration {
    let jitter = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::from(elapsed.subsec_millis()))
        .unwrap_or(0)
        % 250;
    Duration::from_millis(delay_ms + jitter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(*name, HeaderValue::from_static(value));
        }
        headers
    }

    #[test]
    fn retries_only_transient_statuses() {
        let none = HeaderMap::new();
        assert!(matches!(
            classify_status(StatusCode::TOO_MANY_REQUESTS, &none),
            Failure::Retryable { timeout: false, .. }
        ));
        assert!(matches!(
            classify_status(StatusCode::INTERNAL_SERVER_ERROR, &none),
            Failure::Retryable { .. }
        ));
        assert!(matches!(
            classify_status(StatusCode::REQUEST_TIMEOUT, &none),
            Failure::Retryable { timeout: true, .. }
        ));
        assert!(matches!(
            classify_status(StatusCode::FORBIDDEN, &none),
            Failure::Fatal { .. }
        ));
        assert!(matches!(
            classify_status(StatusCode::BAD_REQUEST, &none),
            Failure::Fatal { .. }
        ));
        assert!(matches!(
            classify_status(StatusCode::UNAUTHORIZED, &none),
            Failure::Fatal { .. }
        ));
    }

    #[test]
    fn honors_should_retry_override() {
        let force = headers(&[("x-should-retry", "true")]);
        assert!(matches!(
            classify_status(StatusCode::FORBIDDEN, &force),
            Failure::Retryable { .. }
        ));
        let deny = headers(&[("x-should-retry", "false")]);
        assert!(matches!(
            classify_status(StatusCode::TOO_MANY_REQUESTS, &deny),
            Failure::Fatal { .. }
        ));
    }

    #[test]
    fn parses_retry_after_headers() {
        assert_eq!(
            retry_after_ms(&headers(&[("retry-after-ms", "750")])),
            Some(750)
        );
        assert_eq!(
            retry_after_ms(&headers(&[("retry-after", "3")])),
            Some(3_000)
        );
        assert_eq!(
            retry_after_ms(&headers(&[("retry-after", "999999")])),
            Some(MAX_RETRY_AFTER_MS)
        );
        assert_eq!(retry_after_ms(&HeaderMap::new()), None);
    }

    #[test]
    fn schedule_is_exponential_then_capped() {
        assert_eq!(delay_ms(1, None), 2_000);
        assert_eq!(delay_ms(2, None), 4_000);
        assert_eq!(delay_ms(3, None), 8_000);
        assert_eq!(delay_ms(4, None), 10_000);
        assert_eq!(delay_ms(9, None), 10_000);
        assert_eq!(delay_ms(1, Some(30_000)), 30_000);
    }

    #[test]
    fn retry_state_bounds_attempts_and_timeouts() {
        let transient = Failure::Retryable {
            reason: "HTTP 500".into(),
            retry_after_ms: None,
            timeout: false,
        };
        let mut state = RetryState::default();
        for expected in 1..=MAX_RETRIES {
            assert!(state.next(&transient).is_some());
            assert_eq!(state.retries, expected);
        }
        assert_eq!(state.next(&transient), None);

        let timeout = Failure::Retryable {
            reason: "jaringan".into(),
            retry_after_ms: None,
            timeout: true,
        };
        let mut state = RetryState::default();
        for _ in 0..MAX_TIMEOUT_RETRIES {
            assert!(state.next(&timeout).is_some());
        }
        assert_eq!(state.next(&timeout), None);

        let fatal = Failure::Fatal {
            reason: "HTTP 403".into(),
        };
        assert_eq!(RetryState::default().next(&fatal), None);
    }
}
