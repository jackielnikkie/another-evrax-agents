use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::current;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SegStyle {
    Normal,
    Heading,
    Heading1,
    Bold,
    Italic,
    Quote,
    Code,
    Link,
    LinkLabel,
    SynComment,
    SynString,
    SynNumber,
    SynKeyword,
    SynFunction,
    SynType,
    SynVariable,
    SynOperator,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Seg {
    pub text: String,
    pub style: SegStyle,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Paragraph(Vec<Seg>),
    Heading(u8, Vec<Seg>),
    Bullet(Vec<Seg>),
    Numbered(u32, Vec<Seg>),
    Quote(Vec<Seg>),
    Code(String, Vec<String>),
    Rule,
}

pub fn parse(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph: Option<String> = None;
    let mut in_fence = false;
    let mut fence_lang = String::new();
    let mut code: Vec<String> = Vec::new();

    for raw in text.split('\n') {
        if in_fence {
            if raw.trim_start().starts_with("```") {
                blocks.push(Block::Code(
                    std::mem::take(&mut fence_lang),
                    std::mem::take(&mut code),
                ));
                in_fence = false;
            } else {
                code.push(raw.to_string());
            }
            continue;
        }
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            flush_paragraph(&mut paragraph, &mut blocks);
            continue;
        }
        if let Some(fence) = trimmed.strip_prefix("```") {
            flush_paragraph(&mut paragraph, &mut blocks);
            in_fence = true;
            fence_lang = fence.trim().to_string();
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            flush_paragraph(&mut paragraph, &mut blocks);
            let level = hashes as u8;
            blocks.push(Block::Heading(level, inline(trimmed[hashes..].trim())));
            continue;
        }
        if is_rule(trimmed) {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
            continue;
        }
        if let Some(rest) = strip_bullet(trimmed) {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(Block::Bullet(inline(rest)));
            continue;
        }
        if let Some((number, rest)) = strip_numbered(trimmed) {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(Block::Numbered(number, inline(rest)));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("> ") {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(Block::Quote(inline(rest)));
            continue;
        }
        match &mut paragraph {
            Some(pending) => {
                pending.push(' ');
                pending.push_str(trimmed);
            }
            None => paragraph = Some(trimmed.to_string()),
        }
    }
    if in_fence {
        blocks.push(Block::Code(fence_lang, code));
    }
    flush_paragraph(&mut paragraph, &mut blocks);
    blocks
}

fn flush_paragraph(paragraph: &mut Option<String>, blocks: &mut Vec<Block>) {
    if let Some(text) = paragraph.take() {
        blocks.push(Block::Paragraph(inline(&text)));
    }
}

fn strip_bullet(text: &str) -> Option<&str> {
    ["- ", "* ", "+ "]
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix))
}

fn strip_numbered(text: &str) -> Option<(u32, &str)> {
    let digits: String = text.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = text[digits.len()..].strip_prefix(". ")?;
    Some((digits.parse().ok()?, rest))
}

fn is_rule(text: &str) -> bool {
    for marker in ['-', '*', '_'] {
        let count = text.chars().take_while(|c| *c == marker).count();
        if count >= 3 && count == text.chars().count() {
            return true;
        }
    }
    false
}

pub fn inline(text: &str) -> Vec<Seg> {
    let mut segs: Vec<Seg> = Vec::new();
    let mut plain = String::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        if let Some(rest) = rest.strip_prefix("**")
            && let Some(end) = rest.find("**")
            && end > 0
        {
            push_plain(&mut segs, &mut plain);
            segs.push(Seg {
                text: rest[..end].to_string(),
                style: SegStyle::Bold,
            });
            i += 2 + end + 2;
            continue;
        }
        if let Some(rest) = rest.strip_prefix('`')
            && let Some(end) = rest.find('`')
            && end > 0
        {
            push_plain(&mut segs, &mut plain);
            segs.push(Seg {
                text: rest[..end].to_string(),
                style: SegStyle::Code,
            });
            i += 1 + end + 1;
            continue;
        }
        if let Some(rest) = rest.strip_prefix('[')
            && let Some(close) = rest.find("](")
            && let Some(par) = rest[close + 2..].find(')')
        {
            push_plain(&mut segs, &mut plain);
            let label = &rest[..close];
            let url = &rest[close + 2..close + 2 + par];
            segs.push(Seg {
                text: label.to_string(),
                style: SegStyle::LinkLabel,
            });
            if label != url {
                segs.push(Seg {
                    text: format!(" ({url})"),
                    style: SegStyle::Link,
                });
            }
            i += 1 + close + 2 + par + 1;
            continue;
        }
        if let Some(rest) = rest.strip_prefix('*')
            && let Some(end) = italic_close(rest, b'*')
        {
            push_plain(&mut segs, &mut plain);
            segs.push(Seg {
                text: rest[..end].to_string(),
                style: SegStyle::Italic,
            });
            i += 1 + end + 1;
            continue;
        }
        if rest.starts_with('_')
            && (i == 0
                || !text[..i]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric()))
            && let Some(rest) = rest.strip_prefix('_')
            && let Some(end) = italic_close(rest, b'_')
        {
            push_plain(&mut segs, &mut plain);
            segs.push(Seg {
                text: rest[..end].to_string(),
                style: SegStyle::Italic,
            });
            i += 1 + end + 1;
            continue;
        }
        let ch = rest.chars().next().unwrap_or_default();
        plain.push(ch);
        i += ch.len_utf8();
    }
    push_plain(&mut segs, &mut plain);
    segs
}

fn push_plain(segs: &mut Vec<Seg>, plain: &mut String) {
    if !plain.is_empty() {
        segs.push(Seg {
            text: std::mem::take(plain),
            style: SegStyle::Normal,
        });
    }
}

fn italic_close(text: &str, marker: u8) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.is_empty() || bytes[0].is_ascii_whitespace() || bytes[0] == marker {
        return None;
    }
    for (idx, _) in text.char_indices() {
        if idx == 0 || bytes[idx] != marker {
            continue;
        }
        if marker == b'*' && bytes.get(idx + 1) == Some(&b'*') {
            continue;
        }
        if marker == b'_' {
            let next_ok = text[idx + 1..]
                .chars()
                .next()
                .map(|c| !c.is_alphanumeric())
                .unwrap_or(true);
            if !next_ok {
                continue;
            }
        }
        return Some(idx);
    }
    None
}

const KEYWORDS: &[&str] = &[
    "fn",
    "function",
    "let",
    "const",
    "var",
    "mut",
    "pub",
    "use",
    "struct",
    "enum",
    "impl",
    "trait",
    "for",
    "while",
    "loop",
    "if",
    "else",
    "match",
    "return",
    "break",
    "continue",
    "in",
    "as",
    "async",
    "await",
    "move",
    "self",
    "Self",
    "crate",
    "super",
    "mod",
    "static",
    "ref",
    "where",
    "dyn",
    "unsafe",
    "type",
    "def",
    "class",
    "import",
    "from",
    "export",
    "default",
    "new",
    "try",
    "except",
    "catch",
    "finally",
    "raise",
    "throw",
    "with",
    "lambda",
    "pass",
    "elif",
    "then",
    "fi",
    "do",
    "done",
    "esac",
    "case",
    "local",
    "echo",
    "yield",
    "global",
    "nonlocal",
    "del",
    "not",
    "and",
    "or",
    "is",
    "interface",
    "extends",
    "implements",
    "package",
    "void",
    "go",
    "defer",
    "chan",
    "select",
    "range",
    "nil",
    "None",
    "True",
    "False",
    "true",
    "false",
    "null",
    "undefined",
    "this",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Comment {
    Slash,
    Hash,
}

fn comment_flavor(lang: &str) -> Option<Comment> {
    match lang.trim().to_ascii_lowercase().as_str() {
        "rust" | "rs" | "js" | "javascript" | "jsx" | "ts" | "typescript" | "tsx" | "go"
        | "golang" | "c" | "cpp" | "c++" | "h" | "hpp" | "java" | "kt" | "kotlin" | "swift"
        | "cs" | "csharp" | "json" | "jsonc" | "zig" | "scala" | "dart" => Some(Comment::Slash),
        "py" | "python" | "sh" | "bash" | "zsh" | "shell" | "fish" | "toml" | "yaml" | "yml"
        | "rb" | "ruby" | "php" | "pl" | "perl" | "julia" | "r" | "dockerfile" | "make" | "awk" => {
            Some(Comment::Hash)
        }
        _ => None,
    }
}

pub fn highlight(lang: &str, line: &str) -> Vec<Seg> {
    let Some(comment) = comment_flavor(lang) else {
        return vec![Seg {
            text: line.to_string(),
            style: SegStyle::Normal,
        }];
    };
    let mut segs: Vec<Seg> = Vec::new();
    let mut plain = String::new();
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        if comment == Comment::Slash && rest.starts_with("//") {
            push_plain(&mut segs, &mut plain);
            segs.push(Seg {
                text: rest.to_string(),
                style: SegStyle::SynComment,
            });
            break;
        }
        if comment == Comment::Hash && rest.starts_with('#') {
            push_plain(&mut segs, &mut plain);
            segs.push(Seg {
                text: rest.to_string(),
                style: SegStyle::SynComment,
            });
            break;
        }
        if comment == Comment::Slash && rest.starts_with("/*") {
            let len = rest[2..]
                .find("*/")
                .map(|pos| 2 + pos + 2)
                .unwrap_or(rest.len());
            push_plain(&mut segs, &mut plain);
            segs.push(Seg {
                text: rest[..len].to_string(),
                style: SegStyle::SynComment,
            });
            i += len;
            continue;
        }
        let ch = rest.chars().next().unwrap_or_default();
        if ch == '"' || ch == '\'' || ch == '`' {
            let mut len = rest.len();
            let mut escaped = false;
            for (idx, c) in rest.char_indices().skip(1) {
                if escaped {
                    escaped = false;
                    continue;
                }
                if c == '\\' {
                    escaped = true;
                    continue;
                }
                if c == ch {
                    len = idx + c.len_utf8();
                    break;
                }
            }
            push_plain(&mut segs, &mut plain);
            segs.push(Seg {
                text: rest[..len].to_string(),
                style: SegStyle::SynString,
            });
            i += len;
            continue;
        }
        if ch.is_ascii_digit() {
            let len = rest
                .char_indices()
                .take_while(|(_, c)| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
                .map(|(idx, c)| idx + c.len_utf8())
                .last()
                .unwrap_or(1);
            push_plain(&mut segs, &mut plain);
            segs.push(Seg {
                text: rest[..len].to_string(),
                style: SegStyle::SynNumber,
            });
            i += len;
            continue;
        }
        if ch.is_alphabetic() || ch == '_' {
            let len = rest
                .char_indices()
                .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
                .map(|(idx, c)| idx + c.len_utf8())
                .last()
                .unwrap_or(1);
            let word = &rest[..len];
            let style = if KEYWORDS.contains(&word) {
                SegStyle::SynKeyword
            } else if word.chars().next().is_some_and(|c| c.is_uppercase()) {
                SegStyle::SynType
            } else if rest[len..].trim_start().starts_with('(') {
                SegStyle::SynFunction
            } else {
                SegStyle::SynVariable
            };
            push_plain(&mut segs, &mut plain);
            segs.push(Seg {
                text: word.to_string(),
                style,
            });
            i += len;
            continue;
        }
        if "+-*/%=<>!&|^~?:".contains(ch) {
            push_plain(&mut segs, &mut plain);
            segs.push(Seg {
                text: ch.to_string(),
                style: SegStyle::SynOperator,
            });
            i += ch.len_utf8();
            continue;
        }
        plain.push(ch);
        i += ch.len_utf8();
    }
    push_plain(&mut segs, &mut plain);
    segs
}

pub fn style_of(style: SegStyle) -> Style {
    let base = Style::default();
    match style {
        SegStyle::Normal => base.fg(current().text),
        SegStyle::Heading => base.fg(current().md_heading).add_modifier(Modifier::BOLD),
        SegStyle::Heading1 => base
            .fg(current().md_heading)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        SegStyle::Bold => base.fg(current().md_strong).add_modifier(Modifier::BOLD),
        SegStyle::Italic => base
            .fg(current().md_emphasis)
            .add_modifier(Modifier::ITALIC),
        SegStyle::Quote => base.fg(current().md_quote).add_modifier(Modifier::ITALIC),
        SegStyle::Code => base.fg(current().md_code),
        SegStyle::Link => base
            .fg(current().md_link)
            .add_modifier(Modifier::UNDERLINED),
        SegStyle::LinkLabel => base
            .fg(current().md_link_label)
            .add_modifier(Modifier::UNDERLINED),
        SegStyle::SynComment => base
            .fg(current().syn_comment)
            .add_modifier(Modifier::ITALIC),
        SegStyle::SynString => base.fg(current().syn_string),
        SegStyle::SynNumber => base.fg(current().syn_number),
        SegStyle::SynKeyword => base
            .fg(current().syn_keyword)
            .add_modifier(Modifier::ITALIC),
        SegStyle::SynFunction => base.fg(current().syn_function),
        SegStyle::SynType => base.fg(current().syn_type),
        SegStyle::SynVariable => base.fg(current().syn_variable),
        SegStyle::SynOperator => base.fg(current().syn_operator),
    }
}

pub fn wrap_segs(
    segs: &[Seg],
    width: usize,
    first: &str,
    cont: &str,
    prefix: Style,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let words: Vec<Seg> = segs
        .iter()
        .flat_map(|seg| {
            seg.text.split_whitespace().map(|word| Seg {
                text: word.to_string(),
                style: seg.style,
            })
        })
        .collect();

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut line_width = 0usize;

    let finish = |spans: Vec<Span<'static>>, out: &mut Vec<Line<'static>>| {
        let lead = if out.is_empty() { first } else { cont };
        let mut line = vec![Span::styled(lead.to_string(), prefix)];
        line.extend(spans);
        out.push(Line::from(line));
    };

    for word in words {
        let word_width = word.text.width();
        if word_width > width {
            if !current.is_empty() {
                finish(std::mem::take(&mut current), &mut out);
                line_width = 0;
            }
            let mut chunk = String::new();
            let mut chunk_width = 0usize;
            for ch in word.text.chars() {
                let ch_width = ch.width().unwrap_or(0);
                if chunk_width + ch_width > width && chunk_width > 0 {
                    finish(
                        vec![Span::styled(
                            std::mem::take(&mut chunk),
                            style_of(word.style),
                        )],
                        &mut out,
                    );
                    chunk_width = 0;
                }
                chunk.push(ch);
                chunk_width += ch_width;
            }
            if !chunk.is_empty() {
                current.push(Span::styled(chunk, style_of(word.style)));
                line_width = chunk_width;
            }
            continue;
        }
        let space = usize::from(!current.is_empty());
        if line_width + space + word_width <= width {
            if space == 1 {
                current.push(Span::styled(" ", Style::default()));
                line_width += 1;
            }
            current.push(Span::styled(word.text, style_of(word.style)));
            line_width += word_width;
        } else {
            finish(std::mem::take(&mut current), &mut out);
            line_width = word_width;
            current.push(Span::styled(word.text, style_of(word.style)));
        }
    }
    if !current.is_empty() {
        finish(current, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_text_as_single_paragraph() {
        let blocks = parse("hello world\nsecond line");
        assert_eq!(
            blocks,
            vec![Block::Paragraph(vec![Seg {
                text: "hello world second line".into(),
                style: SegStyle::Normal,
            }])]
        );
    }

    #[test]
    fn parses_headings_bullets_and_numbered_lists() {
        let blocks = parse("## Title\n- one\n- two\n1. first\n2. second");
        assert_eq!(blocks.len(), 5);
        assert!(matches!(&blocks[0], Block::Heading(2, segs) if segs[0].text == "Title"));
        assert!(matches!(&blocks[1], Block::Bullet(segs) if segs[0].text == "one"));
        assert!(matches!(&blocks[2], Block::Bullet(segs) if segs[0].text == "two"));
        assert!(matches!(&blocks[3], Block::Numbered(1, segs) if segs[0].text == "first"));
        assert!(matches!(&blocks[4], Block::Numbered(2, segs) if segs[0].text == "second"));
    }

    #[test]
    fn parses_quotes_and_keeps_code_fences_verbatim() {
        let blocks = parse("> careful\n```rust\nlet x = **not bold**;\n```");
        assert!(matches!(&blocks[0], Block::Quote(segs) if segs[0].text == "careful"));
        match &blocks[1] {
            Block::Code(lang, lines) => {
                assert_eq!(lang, "rust");
                assert_eq!(lines, &vec!["let x = **not bold**;".to_string()]);
            }
            other => panic!("expected code block, got {other:?}"),
        }
    }

    #[test]
    fn parses_horizontal_rules() {
        let blocks = parse("above\n\n---\n\nbelow");
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[1], Block::Rule);
    }

    #[test]
    fn parses_inline_styles() {
        let segs = inline("a **b** c `d` e *f* [g](http://h)");
        let styles: Vec<(String, SegStyle)> =
            segs.iter().map(|s| (s.text.clone(), s.style)).collect();
        assert!(styles.contains(&("b".into(), SegStyle::Bold)));
        assert!(styles.contains(&("d".into(), SegStyle::Code)));
        assert!(styles.contains(&("f".into(), SegStyle::Italic)));
        assert!(styles.contains(&("g".into(), SegStyle::LinkLabel)));
        assert!(styles.contains(&(" (http://h)".into(), SegStyle::Link)));
    }

    #[test]
    fn unclosed_markers_stay_literal() {
        let segs = inline("2 * 3 and **bold");
        let text: String = segs.iter().map(|s| s.text.clone()).collect();
        assert_eq!(text, "2 * 3 and **bold");
        assert!(segs.iter().all(|s| s.style == SegStyle::Normal));
    }

    #[test]
    fn highlight_rust_classifies_tokens() {
        let segs = highlight("rust", "let x = 1; // note");
        let find = |needle: &str| segs.iter().find(|s| s.text == needle).map(|s| s.style);
        assert_eq!(find("let"), Some(SegStyle::SynKeyword));
        assert_eq!(find("1"), Some(SegStyle::SynNumber));
        assert_eq!(find("// note"), Some(SegStyle::SynComment));
        assert_eq!(find("="), Some(SegStyle::SynOperator));
    }

    #[test]
    fn highlight_strings_and_functions() {
        let segs = highlight("js", "const s = greet(\"hi\");");
        let find = |needle: &str| segs.iter().find(|s| s.text == needle).map(|s| s.style);
        assert_eq!(find("const"), Some(SegStyle::SynKeyword));
        assert_eq!(find("greet"), Some(SegStyle::SynFunction));
        assert_eq!(find("\"hi\""), Some(SegStyle::SynString));
    }

    #[test]
    fn highlight_unknown_language_stays_plain() {
        let segs = highlight("", "let x = 1;");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].style, SegStyle::Normal);
    }

    #[test]
    fn wrap_segs_keeps_style_across_lines() {
        let segs = vec![Seg {
            text: "alpha beta gamma delta epsilon zeta".into(),
            style: SegStyle::Bold,
        }];
        let lines = wrap_segs(&segs, 20, "   ", "   ", Style::default());
        assert!(lines.len() >= 2, "should wrap");
        for line in &lines {
            let content: Vec<&Span> = line
                .spans
                .iter()
                .skip(1)
                .filter(|span| !span.content.trim().is_empty())
                .collect();
            assert!(
                content
                    .iter()
                    .all(|span| span.style.add_modifier.contains(Modifier::BOLD)),
                "bold must survive wrapping: {line:?}"
            );
        }
    }

    #[test]
    fn wrap_segs_chunks_words_longer_than_width() {
        let segs = vec![Seg {
            text: "abcdefghij".into(),
            style: SegStyle::Code,
        }];
        let lines = wrap_segs(&segs, 4, "", "", Style::default());
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>()
            })
            .collect();
        assert_eq!(rendered, vec!["abcd", "efgh", "ij"]);
    }
}
