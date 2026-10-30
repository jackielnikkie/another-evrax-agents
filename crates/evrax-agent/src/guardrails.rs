//! Tool-call loop guardrails:
//! identical-result streaks and short cycles raise a notice, identical failures
//! block the next call, and byte-identical large results collapse to a stub.

use std::collections::{HashMap, HashSet};

const DUPLICATE_STUB_MIN_CHARS: usize = 512;
const IDENTICAL_NOTICE_STREAK: u32 = 3;
/// Read-only deterministic tools: their content is always complete, so it must
/// never be replaced by the identical-result stub (a stub here reads like an
/// empty/wrong file; the hash+streak notice still fires).
const STUB_EXEMPT_TOOLS: [&str; 4] = ["read", "list", "grep", "glob"];
const FAILURE_BLOCK_THRESHOLD: u32 = 3;
const RECENT_LIMIT: usize = 16;
const CYCLE_MIN: usize = 2;
const CYCLE_MAX: usize = 4;
const CYCLE_LAPS: usize = 3;

struct Seen {
    hash: u64,
    len: usize,
    call: usize,
}

#[derive(Default)]
pub struct Guard {
    last: Option<(String, u64)>,
    streak: u32,
    recent: Vec<String>,
    failures: HashMap<String, u32>,
    seen: HashMap<String, Seen>,
    notice: Option<String>,
    notices: HashSet<&'static str>,
    calls: usize,
}

impl Guard {
    /// Blocks a call that already failed identically several times this turn.
    pub fn blocked(&self, name: &str, raw_args: &str) -> Option<String> {
        let signature = signature(name, raw_args);
        if self.failures.get(&signature).copied().unwrap_or(0) >= FAILURE_BLOCK_THRESHOLD {
            return Some(format!(
                "guardrail: `{name}` gagal {FAILURE_BLOCK_THRESHOLD}× dengan argumen sama — baca error terakhir dan ubah strategi sebelum mengulang."
            ));
        }
        None
    }

    /// Records one executed call. Returns a stub when the result is a
    /// byte-identical repeat of a large earlier result for the same call.
    pub fn observe(
        &mut self,
        name: &str,
        raw_args: &str,
        ok: bool,
        result: &str,
    ) -> Option<String> {
        self.calls += 1;
        let signature = signature(name, raw_args);
        let hash = fnv1a(result.as_bytes());

        let mut stub = None;
        if result.len() >= DUPLICATE_STUB_MIN_CHARS && !STUB_EXEMPT_TOOLS.contains(&name) {
            if let Some(seen) = self.seen.get(&signature)
                && seen.hash == hash
                && seen.len == result.len()
            {
                stub = Some(format!(
                    "[hasil identik dengan panggilan #{} ({} char, {:016x}); isi dihilangkan]",
                    seen.call, seen.len, seen.hash
                ));
            }
            if stub.is_none() {
                self.seen.insert(
                    signature.clone(),
                    Seen {
                        hash,
                        len: result.len(),
                        call: self.calls,
                    },
                );
            }
        }

        if self.last.as_ref() == Some(&(signature.clone(), hash)) {
            self.streak += 1;
        } else {
            self.streak = 1;
            self.last = Some((signature.clone(), hash));
        }
        if self.streak == IDENTICAL_NOTICE_STREAK && self.notices.insert("identical") {
            self.notice = Some(format!(
                "guardrail: `{name}` mengembalikan hasil identik {IDENTICAL_NOTICE_STREAK}× — pertimbangkan pendekatan lain."
            ));
        }

        self.recent.push(signature.clone());
        if self.recent.len() > RECENT_LIMIT {
            self.recent.remove(0);
        }
        if self.cycle_detected() && self.notices.insert("cycle") {
            self.notice = Some(
                "guardrail: pola panggilan berulang terdeteksi (siklus) — ganti strategi.".into(),
            );
        }

        if ok {
            self.failures.remove(&signature);
        } else {
            *self.failures.entry(signature).or_insert(0) += 1;
        }

        stub
    }

    pub fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }

    fn cycle_detected(&self) -> bool {
        for period in CYCLE_MIN..=CYCLE_MAX {
            if self.recent.len() < period * CYCLE_LAPS {
                continue;
            }
            let tail = &self.recent[self.recent.len() - period * CYCLE_LAPS..];
            let first = &tail[..period];
            if tail.chunks(period).all(|lap| lap == first) {
                return true;
            }
        }
        false
    }
}

/// Canonical call identity: tool name plus normalized JSON arguments.
fn signature(name: &str, raw_args: &str) -> String {
    let args = serde_json::from_str::<serde_json::Value>(raw_args)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| raw_args.trim().to_string());
    format!("{name}\u{0}{args}")
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_identical_results_collapse_to_stub() {
        let mut guard = Guard::default();
        let result = "x".repeat(600);
        assert!(
            guard
                .observe("shell", r#"{"command":"a"}"#, true, &result)
                .is_none()
        );
        let stub = guard
            .observe("shell", r#"{ "command": "a" }"#, true, &result)
            .expect("second identical result should be stubbed");
        assert!(stub.contains("panggilan #1"), "{stub}");
    }

    #[test]
    fn read_only_tools_are_never_stubbed() {
        let mut guard = Guard::default();
        let content = "y".repeat(600);
        for name in STUB_EXEMPT_TOOLS {
            assert!(
                guard
                    .observe(name, r#"{"path":"a.rs"}"#, true, &content)
                    .is_none(),
                "{name} must not stub on the first call"
            );
            let second = guard.observe(name, r#"{"path":"a.rs"}"#, true, &content);
            assert!(
                second.is_none(),
                "{name} returned a stub for identical read-only content: {second:?}"
            );
        }
        // Regression: read a.rs -> read b.rs -> read a.rs keeps returning a.rs.
        let mut guard = Guard::default();
        let a = "a".repeat(600);
        let b = "b".repeat(600);
        assert!(
            guard
                .observe("read", r#"{"path":"a.rs"}"#, true, &a)
                .is_none()
        );
        assert!(
            guard
                .observe("read", r#"{"path":"b.rs"}"#, true, &b)
                .is_none()
        );
        assert!(
            guard
                .observe("read", r#"{"path":"a.rs"}"#, true, &a)
                .is_none()
        );
    }

    #[test]
    fn repeated_identical_results_raise_one_notice() {
        let mut guard = Guard::default();
        for _ in 0..IDENTICAL_NOTICE_STREAK {
            assert!(
                guard
                    .observe("list", r#"{"pattern":"."}"#, true, "same")
                    .is_none()
            );
        }
        let notice = guard.take_notice().expect("streak notice");
        assert!(notice.contains("hasil identik"), "{notice}");
        assert!(guard.take_notice().is_none());
    }

    #[test]
    fn repeated_failures_block_the_next_call() {
        let mut guard = Guard::default();
        let args = r#"{"path":"missing.txt"}"#;
        for _ in 0..FAILURE_BLOCK_THRESHOLD {
            guard.observe("read", args, false, "error: not found");
        }
        let block = guard
            .blocked("read", args)
            .expect("blocked after 3 failures");
        assert!(block.contains("ubah strategi"), "{block}");
        assert!(guard.blocked("read", r#"{"path":"other.txt"}"#).is_none());
    }

    #[test]
    fn alternating_pattern_raises_cycle_notice() {
        let mut guard = Guard::default();
        for lap in 0..CYCLE_LAPS {
            guard.observe("read", r#"{"path":"a"}"#, true, &format!("a{lap}"));
            guard.observe("read", r#"{"path":"b"}"#, true, &format!("b{lap}"));
        }
        let notice = guard.take_notice().expect("cycle notice");
        assert!(notice.contains("siklus"), "{notice}");
    }
}
