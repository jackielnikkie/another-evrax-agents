use std::cell::Cell;
use std::path::PathBuf;

use ratatui::style::Color;

// Theme tokens follow opencode's v2 theme assets
// (packages/tui/src/theme/assets/v2/opencode.json). "evrax" keeps the purple
// letter scale; "opencode" restores the original markdown/syntax rainbow.
pub struct Theme {
    pub key: &'static str,
    pub label: &'static str,
    pub base: Color,
    pub raised: Color,
    pub text: Color,
    pub muted: Color,
    pub border: Color,
    pub accent: Color,
    pub interactive: Color,
    pub error: Color,
    pub success: Color,
    pub selection: Color,
    pub action_dim: Color,
    pub diff_add: Color,
    pub diff_rem: Color,
    pub diff_add_bg: Color,
    pub diff_rem_bg: Color,
    pub diff_add_gutter: Color,
    pub diff_rem_gutter: Color,
    pub diff_ctx_gutter: Color,
    pub diff_ctx_text: Color,
    pub md_heading: Color,
    pub md_strong: Color,
    pub md_emphasis: Color,
    pub md_quote: Color,
    pub md_code: Color,
    pub md_list: Color,
    pub md_link: Color,
    pub md_link_label: Color,
    pub md_rule: Color,
    pub syn_comment: Color,
    pub syn_string: Color,
    pub syn_number: Color,
    pub syn_keyword: Color,
    pub syn_function: Color,
    pub syn_type: Color,
    pub syn_variable: Color,
    pub syn_operator: Color,
}

pub const EVRAX: Theme = Theme {
    key: "evrax",
    label: "evrax",
    base: Color::Rgb(0x0a, 0x0a, 0x0a),
    raised: Color::Rgb(0x14, 0x14, 0x14),
    text: Color::Rgb(0xee, 0xee, 0xee),
    muted: Color::Rgb(0x80, 0x80, 0x80),
    border: Color::Rgb(0x48, 0x48, 0x48),
    accent: Color::Rgb(0x9d, 0x7c, 0xd8),
    interactive: Color::Rgb(0xfa, 0xb2, 0x83),
    error: Color::Rgb(0xe0, 0x6c, 0x75),
    success: Color::Rgb(0x7f, 0xd8, 0x8f),
    selection: Color::Rgb(0x33, 0x33, 0x33),
    action_dim: Color::Rgb(0x2a, 0x21, 0x40),
    diff_add: Color::Rgb(0x4f, 0xd6, 0xbe),
    diff_rem: Color::Rgb(0xc5, 0x3b, 0x53),
    diff_add_bg: Color::Rgb(0x20, 0x30, 0x3b),
    diff_rem_bg: Color::Rgb(0x37, 0x22, 0x2c),
    diff_add_gutter: Color::Rgb(0x1b, 0x2b, 0x34),
    diff_rem_gutter: Color::Rgb(0x2d, 0x1f, 0x26),
    diff_ctx_gutter: Color::Rgb(0x14, 0x14, 0x14),
    diff_ctx_text: Color::Rgb(0x82, 0x8b, 0xb8),
    md_heading: Color::Rgb(0x9d, 0x7c, 0xd8), // accent.200
    md_strong: Color::Rgb(0xb3, 0x8f, 0xf4),  // purple.100
    md_emphasis: Color::Rgb(0x9d, 0x7c, 0xd8), // accent.200
    md_quote: Color::Rgb(0x88, 0x69, 0xbd),   // purple.300
    md_code: Color::Rgb(0xb3, 0x8f, 0xf4),    // purple.100
    md_list: Color::Rgb(0x9d, 0x7c, 0xd8),    // accent.200
    md_link: Color::Rgb(0x88, 0x69, 0xbd),    // purple.300
    md_link_label: Color::Rgb(0x9d, 0x7c, 0xd8), // accent.200
    md_rule: Color::Rgb(0x48, 0x48, 0x48),    // dim rail / rule
    syn_comment: Color::Rgb(0x80, 0x80, 0x80), // muted, italic
    syn_string: Color::Rgb(0x9d, 0x7c, 0xd8),  // accent.200
    syn_number: Color::Rgb(0xb3, 0x8f, 0xf4),  // purple.100
    syn_keyword: Color::Rgb(0xb3, 0x8f, 0xf4), // purple.100, italic
    syn_function: Color::Rgb(0x9d, 0x7c, 0xd8), // accent.200
    syn_type: Color::Rgb(0x88, 0x69, 0xbd),    // purple.300
    syn_variable: Color::Rgb(0xee, 0xee, 0xee), // text (unstressed)
    syn_operator: Color::Rgb(0x80, 0x80, 0x80), // muted
};

pub const OPENCODE: Theme = Theme {
    key: "opencode",
    label: "opencode",
    base: Color::Rgb(0x0a, 0x0a, 0x0a),
    raised: Color::Rgb(0x14, 0x14, 0x14),
    text: Color::Rgb(0xee, 0xee, 0xee),
    muted: Color::Rgb(0x80, 0x80, 0x80),
    border: Color::Rgb(0x48, 0x48, 0x48),
    accent: Color::Rgb(0xfa, 0xb2, 0x83), // interactive.200: the chrome runs orange
    interactive: Color::Rgb(0xfa, 0xb2, 0x83),
    error: Color::Rgb(0xe0, 0x6c, 0x75),
    success: Color::Rgb(0x7f, 0xd8, 0x8f),
    selection: Color::Rgb(0x3d, 0x1d, 0x06), // orange.800 tint
    action_dim: Color::Rgb(0x3d, 0x1d, 0x06), // orange.800 tint
    diff_add: Color::Rgb(0x4f, 0xd6, 0xbe),
    diff_rem: Color::Rgb(0xc5, 0x3b, 0x53),
    diff_add_bg: Color::Rgb(0x20, 0x30, 0x3b),
    diff_rem_bg: Color::Rgb(0x37, 0x22, 0x2c),
    diff_add_gutter: Color::Rgb(0x1b, 0x2b, 0x34),
    diff_rem_gutter: Color::Rgb(0x2d, 0x1f, 0x26),
    diff_ctx_gutter: Color::Rgb(0x14, 0x14, 0x14),
    diff_ctx_text: Color::Rgb(0x82, 0x8b, 0xb8),
    md_heading: Color::Rgb(0x9d, 0x7c, 0xd8), // accent.200
    md_strong: Color::Rgb(0xf5, 0xa7, 0x42),  // #f5a742
    md_emphasis: Color::Rgb(0xe5, 0xc0, 0x7b), // #e5c07b
    md_quote: Color::Rgb(0xe5, 0xc0, 0x7b),   // #e5c07b
    md_code: Color::Rgb(0x7f, 0xd8, 0x8f),    // green.200
    md_list: Color::Rgb(0xfa, 0xb2, 0x83),    // interactive.200
    md_link: Color::Rgb(0xfa, 0xb2, 0x83),    // interactive.200
    md_link_label: Color::Rgb(0x56, 0xb6, 0xc2), // cyan.200
    md_rule: Color::Rgb(0x80, 0x80, 0x80),    // neutral.400
    syn_comment: Color::Rgb(0x80, 0x80, 0x80), // neutral.400, italic
    syn_string: Color::Rgb(0x7f, 0xd8, 0x8f),  // green.200
    syn_number: Color::Rgb(0xf5, 0xa7, 0x42),  // #f5a742
    syn_keyword: Color::Rgb(0x9d, 0x7c, 0xd8), // accent.200, italic
    syn_function: Color::Rgb(0xfa, 0xb2, 0x83), // interactive.200
    syn_type: Color::Rgb(0xe5, 0xc0, 0x7b),    // #e5c07b
    syn_variable: Color::Rgb(0xe0, 0x6c, 0x75), // red.200
    syn_operator: Color::Rgb(0x56, 0xb6, 0xc2), // cyan.200
};

pub const CATPPUCCIN: Theme = Theme {
    key: "catppuccin",
    label: "catppuccin",
    base: Color::Rgb(0x1e, 0x1e, 0x2e),
    raised: Color::Rgb(0x18, 0x18, 0x25),
    text: Color::Rgb(0xcd, 0xd6, 0xf4),
    muted: Color::Rgb(0x93, 0x99, 0xb2),
    border: Color::Rgb(0x31, 0x32, 0x44),
    accent: Color::Rgb(0xcb, 0xa6, 0xf7),
    interactive: Color::Rgb(0xfa, 0xb3, 0x87),
    error: Color::Rgb(0xf3, 0x8b, 0xa8),
    success: Color::Rgb(0xa6, 0xe3, 0xa1),
    selection: Color::Rgb(0x45, 0x47, 0x5a),
    action_dim: Color::Rgb(0x31, 0x32, 0x44),
    diff_add: Color::Rgb(0xa6, 0xe3, 0xa1),
    diff_rem: Color::Rgb(0xf3, 0x8b, 0xa8),
    diff_add_bg: Color::Rgb(0x24, 0x31, 0x2b),
    diff_rem_bg: Color::Rgb(0x3c, 0x2a, 0x32),
    diff_add_gutter: Color::Rgb(0x1e, 0x2a, 0x25),
    diff_rem_gutter: Color::Rgb(0x32, 0x23, 0x2a),
    diff_ctx_gutter: Color::Rgb(0x18, 0x18, 0x25),
    diff_ctx_text: Color::Rgb(0x93, 0x99, 0xb2),
    md_heading: Color::Rgb(0xcb, 0xa6, 0xf7),
    md_strong: Color::Rgb(0xfa, 0xb3, 0x87),
    md_emphasis: Color::Rgb(0xf9, 0xe2, 0xaf),
    md_quote: Color::Rgb(0xf9, 0xe2, 0xaf),
    md_code: Color::Rgb(0xa6, 0xe3, 0xa1),
    md_list: Color::Rgb(0x89, 0xb4, 0xfa),
    md_link: Color::Rgb(0x89, 0xb4, 0xfa),
    md_link_label: Color::Rgb(0x89, 0xdc, 0xeb),
    md_rule: Color::Rgb(0xa6, 0xad, 0xc8),
    syn_comment: Color::Rgb(0x93, 0x99, 0xb2),
    syn_string: Color::Rgb(0xa6, 0xe3, 0xa1),
    syn_number: Color::Rgb(0xfa, 0xb3, 0x87),
    syn_keyword: Color::Rgb(0xcb, 0xa6, 0xf7),
    syn_function: Color::Rgb(0x89, 0xb4, 0xfa),
    syn_type: Color::Rgb(0xf9, 0xe2, 0xaf),
    syn_variable: Color::Rgb(0xf3, 0x8b, 0xa8),
    syn_operator: Color::Rgb(0x89, 0xdc, 0xeb),
};

pub const DRACULA: Theme = Theme {
    key: "dracula",
    label: "dracula",
    base: Color::Rgb(0x28, 0x2a, 0x36),
    raised: Color::Rgb(0x21, 0x22, 0x2c),
    text: Color::Rgb(0xf8, 0xf8, 0xf2),
    muted: Color::Rgb(0x62, 0x72, 0xa4),
    border: Color::Rgb(0x44, 0x47, 0x5a),
    accent: Color::Rgb(0xbd, 0x93, 0xf9),
    interactive: Color::Rgb(0xff, 0x79, 0xc6),
    error: Color::Rgb(0xff, 0x55, 0x55),
    success: Color::Rgb(0x50, 0xfa, 0x7b),
    selection: Color::Rgb(0x44, 0x47, 0x5a),
    action_dim: Color::Rgb(0x34, 0x37, 0x46),
    diff_add: Color::Rgb(0x50, 0xfa, 0x7b),
    diff_rem: Color::Rgb(0xff, 0x55, 0x55),
    diff_add_bg: Color::Rgb(0x1a, 0x3a, 0x1a),
    diff_rem_bg: Color::Rgb(0x3a, 0x1a, 0x1a),
    diff_add_gutter: Color::Rgb(0x1a, 0x3a, 0x1a),
    diff_rem_gutter: Color::Rgb(0x3a, 0x1a, 0x1a),
    diff_ctx_gutter: Color::Rgb(0x21, 0x22, 0x2c),
    diff_ctx_text: Color::Rgb(0x62, 0x72, 0xa4),
    md_heading: Color::Rgb(0xbd, 0x93, 0xf9),
    md_strong: Color::Rgb(0xff, 0xb8, 0x6c),
    md_emphasis: Color::Rgb(0xf1, 0xfa, 0x8c),
    md_quote: Color::Rgb(0x62, 0x72, 0xa4),
    md_code: Color::Rgb(0x50, 0xfa, 0x7b),
    md_list: Color::Rgb(0xbd, 0x93, 0xf9),
    md_link: Color::Rgb(0x8b, 0xe9, 0xfd),
    md_link_label: Color::Rgb(0xff, 0x79, 0xc6),
    md_rule: Color::Rgb(0x62, 0x72, 0xa4),
    syn_comment: Color::Rgb(0x62, 0x72, 0xa4),
    syn_string: Color::Rgb(0xf1, 0xfa, 0x8c),
    syn_number: Color::Rgb(0xbd, 0x93, 0xf9),
    syn_keyword: Color::Rgb(0xff, 0x79, 0xc6),
    syn_function: Color::Rgb(0x50, 0xfa, 0x7b),
    syn_type: Color::Rgb(0x8b, 0xe9, 0xfd),
    syn_variable: Color::Rgb(0xf8, 0xf8, 0xf2),
    syn_operator: Color::Rgb(0xff, 0x79, 0xc6),
};

pub const GRUVBOX: Theme = Theme {
    key: "gruvbox",
    label: "gruvbox",
    base: Color::Rgb(0x28, 0x28, 0x28),
    raised: Color::Rgb(0x3c, 0x38, 0x36),
    text: Color::Rgb(0xeb, 0xdb, 0xb2),
    muted: Color::Rgb(0x92, 0x83, 0x74),
    border: Color::Rgb(0x66, 0x5c, 0x54),
    accent: Color::Rgb(0xfe, 0x80, 0x19),
    interactive: Color::Rgb(0xfa, 0xbd, 0x2f),
    error: Color::Rgb(0xfb, 0x49, 0x34),
    success: Color::Rgb(0xb8, 0xbb, 0x26),
    selection: Color::Rgb(0x50, 0x49, 0x45),
    action_dim: Color::Rgb(0x3c, 0x38, 0x36),
    diff_add: Color::Rgb(0x98, 0x97, 0x1a),
    diff_rem: Color::Rgb(0xcc, 0x24, 0x1d),
    diff_add_bg: Color::Rgb(0x32, 0x30, 0x2f),
    diff_rem_bg: Color::Rgb(0x32, 0x29, 0x29),
    diff_add_gutter: Color::Rgb(0x2a, 0x28, 0x27),
    diff_rem_gutter: Color::Rgb(0x2a, 0x22, 0x22),
    diff_ctx_gutter: Color::Rgb(0x3c, 0x38, 0x36),
    diff_ctx_text: Color::Rgb(0x92, 0x83, 0x74),
    md_heading: Color::Rgb(0x83, 0xa5, 0x98),
    md_strong: Color::Rgb(0xfe, 0x80, 0x19),
    md_emphasis: Color::Rgb(0xd3, 0x86, 0x9b),
    md_quote: Color::Rgb(0x92, 0x83, 0x74),
    md_code: Color::Rgb(0xfa, 0xbd, 0x2f),
    md_list: Color::Rgb(0x83, 0xa5, 0x98),
    md_link: Color::Rgb(0x8e, 0xc0, 0x7c),
    md_link_label: Color::Rgb(0xb8, 0xbb, 0x26),
    md_rule: Color::Rgb(0x92, 0x83, 0x74),
    syn_comment: Color::Rgb(0x92, 0x83, 0x74),
    syn_string: Color::Rgb(0xfa, 0xbd, 0x2f),
    syn_number: Color::Rgb(0xd3, 0x86, 0x9b),
    syn_keyword: Color::Rgb(0xfb, 0x49, 0x34),
    syn_function: Color::Rgb(0xb8, 0xbb, 0x26),
    syn_type: Color::Rgb(0x8e, 0xc0, 0x7c),
    syn_variable: Color::Rgb(0x83, 0xa5, 0x98),
    syn_operator: Color::Rgb(0xfe, 0x80, 0x19),
};

pub const NORD: Theme = Theme {
    key: "nord",
    label: "nord",
    base: Color::Rgb(0x2e, 0x34, 0x40),
    raised: Color::Rgb(0x3b, 0x42, 0x52),
    text: Color::Rgb(0xec, 0xef, 0xf4),
    muted: Color::Rgb(0x8b, 0x95, 0xa7),
    border: Color::Rgb(0x43, 0x4c, 0x5e),
    accent: Color::Rgb(0x88, 0xc0, 0xd0),
    interactive: Color::Rgb(0x8f, 0xbc, 0xbb),
    error: Color::Rgb(0xbf, 0x61, 0x6a),
    success: Color::Rgb(0xa3, 0xbe, 0x8c),
    selection: Color::Rgb(0x4c, 0x56, 0x6a),
    action_dim: Color::Rgb(0x3b, 0x42, 0x52),
    diff_add: Color::Rgb(0xa3, 0xbe, 0x8c),
    diff_rem: Color::Rgb(0xbf, 0x61, 0x6a),
    diff_add_bg: Color::Rgb(0x3b, 0x42, 0x52),
    diff_rem_bg: Color::Rgb(0x3b, 0x42, 0x52),
    diff_add_gutter: Color::Rgb(0x3b, 0x42, 0x52),
    diff_rem_gutter: Color::Rgb(0x3b, 0x42, 0x52),
    diff_ctx_gutter: Color::Rgb(0x3b, 0x42, 0x52),
    diff_ctx_text: Color::Rgb(0x8b, 0x95, 0xa7),
    md_heading: Color::Rgb(0x88, 0xc0, 0xd0),
    md_strong: Color::Rgb(0xeb, 0xcb, 0x8b),
    md_emphasis: Color::Rgb(0xd0, 0x87, 0x70),
    md_quote: Color::Rgb(0x8b, 0x95, 0xa7),
    md_code: Color::Rgb(0xa3, 0xbe, 0x8c),
    md_list: Color::Rgb(0x88, 0xc0, 0xd0),
    md_link: Color::Rgb(0x81, 0xa1, 0xc1),
    md_link_label: Color::Rgb(0x8f, 0xbc, 0xbb),
    md_rule: Color::Rgb(0x8b, 0x95, 0xa7),
    syn_comment: Color::Rgb(0x8b, 0x95, 0xa7),
    syn_string: Color::Rgb(0xa3, 0xbe, 0x8c),
    syn_number: Color::Rgb(0xb4, 0x8e, 0xad),
    syn_keyword: Color::Rgb(0x81, 0xa1, 0xc1),
    syn_function: Color::Rgb(0x88, 0xc0, 0xd0),
    syn_type: Color::Rgb(0x8f, 0xbc, 0xbb),
    syn_variable: Color::Rgb(0x8f, 0xbc, 0xbb),
    syn_operator: Color::Rgb(0x81, 0xa1, 0xc1),
};

pub const TOKYONIGHT: Theme = Theme {
    key: "tokyonight",
    label: "tokyonight",
    base: Color::Rgb(0x1a, 0x1b, 0x26),
    raised: Color::Rgb(0x1e, 0x20, 0x30),
    text: Color::Rgb(0xc8, 0xd3, 0xf5),
    muted: Color::Rgb(0x82, 0x8b, 0xb8),
    border: Color::Rgb(0x73, 0x7a, 0xa2),
    accent: Color::Rgb(0x82, 0xaa, 0xff),
    interactive: Color::Rgb(0xc0, 0x99, 0xff),
    error: Color::Rgb(0xff, 0x75, 0x7f),
    success: Color::Rgb(0xc3, 0xe8, 0x8d),
    selection: Color::Rgb(0x22, 0x24, 0x36),
    action_dim: Color::Rgb(0x29, 0x2e, 0x42),
    diff_add: Color::Rgb(0x4f, 0xd6, 0xbe),
    diff_rem: Color::Rgb(0xc5, 0x3b, 0x53),
    diff_add_bg: Color::Rgb(0x20, 0x30, 0x3b),
    diff_rem_bg: Color::Rgb(0x37, 0x22, 0x2c),
    diff_add_gutter: Color::Rgb(0x1b, 0x2b, 0x34),
    diff_rem_gutter: Color::Rgb(0x2d, 0x1f, 0x26),
    diff_ctx_gutter: Color::Rgb(0x1e, 0x20, 0x30),
    diff_ctx_text: Color::Rgb(0x82, 0x8b, 0xb8),
    md_heading: Color::Rgb(0xc0, 0x99, 0xff),
    md_strong: Color::Rgb(0xff, 0x96, 0x6c),
    md_emphasis: Color::Rgb(0xff, 0xc7, 0x77),
    md_quote: Color::Rgb(0xff, 0xc7, 0x77),
    md_code: Color::Rgb(0xc3, 0xe8, 0x8d),
    md_list: Color::Rgb(0x82, 0xaa, 0xff),
    md_link: Color::Rgb(0x82, 0xaa, 0xff),
    md_link_label: Color::Rgb(0x86, 0xe1, 0xfc),
    md_rule: Color::Rgb(0x82, 0x8b, 0xb8),
    syn_comment: Color::Rgb(0x82, 0x8b, 0xb8),
    syn_string: Color::Rgb(0xc3, 0xe8, 0x8d),
    syn_number: Color::Rgb(0xff, 0x96, 0x6c),
    syn_keyword: Color::Rgb(0xc0, 0x99, 0xff),
    syn_function: Color::Rgb(0x82, 0xaa, 0xff),
    syn_type: Color::Rgb(0xff, 0xc7, 0x77),
    syn_variable: Color::Rgb(0xff, 0x75, 0x7f),
    syn_operator: Color::Rgb(0x86, 0xe1, 0xfc),
};

pub static THEMES: &[Theme] = &[
    EVRAX, OPENCODE, CATPPUCCIN, DRACULA, GRUVBOX, NORD, TOKYONIGHT,
];

thread_local! {
    static CURRENT: Cell<usize> = const { Cell::new(0) };
}

pub fn current() -> &'static Theme {
    &THEMES[CURRENT.with(|cell| cell.get()).min(THEMES.len() - 1)]
}

pub fn index() -> usize {
    CURRENT.with(|cell| cell.get()).min(THEMES.len() - 1)
}

pub fn set_index(idx: usize) {
    CURRENT.with(|cell| cell.set(idx.min(THEMES.len() - 1)));
}

pub fn count() -> usize {
    THEMES.len()
}

pub fn label() -> &'static str {
    current().label
}

pub fn by_key(key: &str) -> Option<usize> {
    THEMES.iter().position(|theme| theme.key == key)
}

fn config_file() -> Option<PathBuf> {
    let dir = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(dir).join("evrax").join("theme"))
}

pub fn load_saved() -> Option<usize> {
    let saved = std::fs::read_to_string(config_file()?).ok()?;
    by_key(saved.trim())
}

#[cfg(not(test))]
pub fn save_current() {
    let Some(path) = config_file() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, current().key);
}

#[cfg(test)]
pub fn save_current() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_keys_are_unique() {
        let mut keys: Vec<&str> = THEMES.iter().map(|t| t.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), THEMES.len(), "theme keys must be unique");
        assert_eq!(count(), 7);
    }

    #[test]
    fn set_index_roundtrips_by_key() {
        set_index(0);
        for (i, theme) in THEMES.iter().enumerate() {
            set_index(i);
            assert_eq!(current().key, theme.key, "index {i}");
            assert_eq!(by_key(theme.key), Some(i), "key {}", theme.key);
        }
        set_index(0);
    }
}
