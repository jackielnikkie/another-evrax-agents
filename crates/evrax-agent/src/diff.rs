use evrax_core::{DiffKind, DiffLine};
use similar::{ChangeTag, TextDiff};

/// Line diff for the TUI diff card; capped so a whole-file rewrite stays sane.
pub fn diff_lines(old: &str, new: &str) -> Vec<DiffLine> {
    let diff = TextDiff::from_lines(old, new);
    let mut lines: Vec<DiffLine> = Vec::new();
    for change in diff.iter_all_changes() {
        let kind = match change.tag() {
            ChangeTag::Delete => DiffKind::Rem,
            ChangeTag::Insert => DiffKind::Add,
            ChangeTag::Equal => DiffKind::Ctx,
        };
        if lines.len() >= MAX_DIFF_LINES {
            break;
        }
        lines.push(DiffLine {
            kind,
            text: change.value().trim_end_matches(['\n', '\r']).to_string(),
        });
    }
    lines
}

const MAX_DIFF_LINES: usize = 200;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_added_and_removed_lines() {
        let lines = diff_lines("a\nb\nc\n", "a\nx\nc\n");
        assert!(
            lines
                .iter()
                .any(|l| l.kind == DiffKind::Add && l.text == "x")
        );
        assert!(
            lines
                .iter()
                .any(|l| l.kind == DiffKind::Rem && l.text == "b")
        );
    }

    #[test]
    fn caps_output() {
        let big: String = (0..1000).map(|i| format!("line {i}\n")).collect();
        let lines = diff_lines("", &big);
        assert_eq!(lines.len(), MAX_DIFF_LINES);
    }
}
