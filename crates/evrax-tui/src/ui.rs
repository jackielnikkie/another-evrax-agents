use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block as WidgetBlock, BorderType, Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState,
};

use evrax_core::DiffKind;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{APPROVAL_OPTIONS, AgentMode, App, ChatEntry, ToolStatus, TunnelEditKind};
use crate::markdown;
use crate::theme::current;

const SIDEBAR_WIDTH: u16 = 42;
const CONTENT_PREFERRED_WIDTH: u16 = 64;
const SIDEBAR_MIN_TOTAL_WIDTH: u16 = SIDEBAR_WIDTH + CONTENT_PREFERRED_WIDTH;
const APPROVAL_PANEL_HEIGHT: u16 = 4;

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(
        WidgetBlock::default().style(Style::default().bg(current().base)),
        area,
    );

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    header(frame, rows[0]);
    let messages_rect = body(frame, rows[1], app);
    footer(frame, rows[2], app);
    app.selectable_area = messages_rect;
    apply_selection(frame, app);
    palette(frame, app);
    session_modal(frame, app);
    theme_modal(frame, app);
    remote_modal(frame, app);
    tunnel_modal(frame, app);
    preview_modal(frame, app);
}

fn sidebar_visible(app: &App, width: u16) -> bool {
    !app.messages.is_empty() && width >= SIDEBAR_MIN_TOTAL_WIDTH && app.maximized.unwrap_or(true)
}

fn apply_selection(frame: &mut Frame, app: &mut App) {
    let area = app.selectable_area;
    if area.width == 0 || area.height == 0 {
        return;
    }
    let min_x = area.left();
    let max_x = area.right().saturating_sub(1);
    let min_y = area.top();
    let max_y = area.bottom().saturating_sub(1);
    if let Some((x0, y0, x1, y1)) = app.selection.normalized() {
        let sx = x0.clamp(min_x, max_x);
        let sy = y0.clamp(min_y, max_y);
        let ex = x1.clamp(min_x, max_x);
        let ey = y1.clamp(min_y, max_y);
        let (fx, fy) = (sx.min(ex), sy.min(ey));
        let (lx, ly) = (sx.max(ex), sy.max(ey));
        let buf = frame.buffer_mut();
        for y in fy..=ly {
            let start = if y == fy { fx } else { min_x };
            let end = if y == ly { lx } else { max_x };
            for x in start..=end {
                buf[(x, y)].set_bg(current().selection);
            }
        }
    }
    if let Some((x0, y0, x1, y1)) = app.selection.take_copy_request() {
        let text = extract_text(frame.buffer_mut(), area, (x0, y0), (x1, y1));
        let chars = text.chars().count();
        if chars > 0 {
            app.copied_text = Some(text);
            app.set_flash(format!("copied {chars} chars"));
        } else {
            app.selection.clear();
        }
    }
}

fn extract_text(buf: &Buffer, area: Rect, start: (u16, u16), end: (u16, u16)) -> String {
    if area.width == 0 || area.height == 0 {
        return String::new();
    }
    let min_x = area.left();
    let max_x = area.right().saturating_sub(1);
    let min_y = area.top();
    let max_y = area.bottom().saturating_sub(1);
    let sx = start.0.clamp(min_x, max_x);
    let sy = start.1.clamp(min_y, max_y);
    let ex = end.0.clamp(min_x, max_x);
    let ey = end.1.clamp(min_y, max_y);
    let (fx, fy) = (sx.min(ex), sy.min(ey));
    let (lx, ly) = (sx.max(ex), sy.max(ey));
    let mut out = String::new();
    for y in fy..=ly {
        let from = if y == fy { fx } else { min_x };
        let to = if y == ly { lx } else { max_x };
        let mut row = String::new();
        for x in from..=to {
            let symbol = buf[(x, y)].symbol();
            if !symbol.is_empty() {
                row.push_str(symbol);
            }
        }
        out.push_str(row.trim_end());
        if y != ly {
            out.push('\n');
        }
    }
    out
}

fn header(frame: &mut Frame, area: Rect) {
    let line = Line::from(vec![
        Span::styled(
            " evrax",
            Style::default()
                .fg(current().accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  v{}", env!("CARGO_PKG_VERSION")),
            Style::default().fg(current().muted),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(current().base)),
        area,
    );
}

fn body(frame: &mut Frame, area: Rect, app: &mut App) -> Rect {
    let columns = if sidebar_visible(app, area.width) {
        Layout::horizontal([Constraint::Min(0), Constraint::Length(SIDEBAR_WIDTH)]).split(area)
    } else {
        Layout::horizontal([Constraint::Min(0)]).split(area)
    };

    let messages_rect = chat(frame, columns[0], app);
    if columns.len() > 1 {
        sidebar(frame, columns[1], app);
    }
    messages_rect
}

fn chat(frame: &mut Frame, area: Rect, app: &mut App) -> Rect {
    let panel = if app.approval.is_some() && area.height >= 12 {
        APPROVAL_PANEL_HEIGHT
    } else {
        0
    };
    let composer = if app.busy {
        3
    } else {
        app.composer_height(area.width.saturating_sub(4) as usize)
    };
    let rows = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(panel),
        Constraint::Length(composer),
        Constraint::Length(1),
    ])
    .split(area);
    messages(frame, rows[0], app);
    if panel > 0 {
        approval_panel(frame, rows[1], app);
    }
    input(frame, rows[2], app);
    if app.slash.is_some() {
        slash_menu(frame, rows[0], rows[2].y, app);
    }
    metadata(frame, rows[3], app);
    rows[0]
}

fn messages(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.messages.is_empty() {
        crate::welcome::render(frame, area, app);
        app.frame_max_scroll = 0;
        return;
    }
    // Per-entry line cache: only the tail entry (streaming text, running tool)
    // is rebuilt per frame, so render cost stays flat as the transcript grows.
    let (content_height, offset, visible) = {
        let lines = app.transcript.sync(
            &app.messages,
            app.tick,
            app.content_rev,
            area.width,
            crate::theme::index(),
        );
        let max_scroll = lines.len().saturating_sub(area.height as usize) as u16;
        let offset = max_scroll.saturating_sub(app.scroll_from_bottom);
        let end = (offset as usize + area.height as usize + 1).min(lines.len());
        app.frame_max_scroll = max_scroll;
        (lines.len(), offset, lines[offset as usize..end].to_vec())
    };
    frame.render_widget(Paragraph::new(visible), area);

    if content_height > area.height as usize && area.width > 2 {
        let mut state = ScrollbarState::new(content_height)
            .viewport_content_length(area.height as usize)
            .position(offset as usize);
        let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(Style::default().fg(current().border))
            .thumb_style(Style::default().fg(current().muted));
        frame.render_stateful_widget(bar, area, &mut state);
    }
}

/// Flattened transcript lines with incremental rebuilds. Mutations always
/// touch the last entry, so entries below the tail are frozen once built.
#[derive(Default)]
pub struct TranscriptCache {
    width: usize,
    theme: usize,
    /// Entry count at the last sync; `usize::MAX` marks "force full rebuild".
    prev_n: usize,
    /// Number of frozen (non-tail) entry blocks in `lines`.
    frozen: usize,
    /// Line index where the tail entry block starts.
    tail_start: usize,
    /// Last tick/revision the tail was rendered for; unchanged means the
    /// cached tail (including spinner frame) is still current.
    last_tick: u64,
    last_rev: u64,
    lines: Vec<Line<'static>>,
    #[cfg(test)]
    rebuilds: usize,
    #[cfg(test)]
    tail_rebuilds: usize,
}

impl TranscriptCache {
    /// Drop all cached lines (session switch or any other content reset).
    pub fn invalidate(&mut self) {
        self.prev_n = usize::MAX;
    }

    pub fn sync(
        &mut self,
        entries: &[ChatEntry],
        tick: u64,
        rev: u64,
        width: u16,
        theme: usize,
    ) -> &Vec<Line<'static>> {
        let width = width.max(1) as usize;
        let n = entries.len();
        if n < self.prev_n || width != self.width || theme != self.theme {
            self.theme = theme;
            self.rebuild(entries, tick, rev, width);
            return &self.lines;
        }
        let mut tail_dirty = tick != self.last_tick || rev != self.last_rev;
        if n > self.prev_n {
            self.lines.truncate(self.tail_start);
            for entry in &entries[self.frozen..n.saturating_sub(1)] {
                self.lines.extend(entry_lines(entry, tick, width));
                self.lines.push(Line::from(""));
            }
            self.frozen = n.saturating_sub(1);
            self.tail_start = self.lines.len();
            self.prev_n = n;
            tail_dirty = true;
        }
        if n > 0 && tail_dirty {
            self.lines.truncate(self.tail_start);
            self.lines.extend(entry_lines(&entries[n - 1], tick, width));
            self.lines.push(Line::from(""));
            self.last_tick = tick;
            self.last_rev = rev;
            #[cfg(test)]
            {
                self.tail_rebuilds += 1;
            }
        }
        &self.lines
    }

    fn rebuild(&mut self, entries: &[ChatEntry], tick: u64, rev: u64, width: usize) {
        #[cfg(test)]
        {
            self.rebuilds += 1;
            self.tail_rebuilds += 1;
        }
        self.width = width;
        self.prev_n = entries.len();
        self.last_tick = tick;
        self.last_rev = rev;
        self.lines = vec![Line::from("")];
        let Some((last, head)) = entries.split_last() else {
            self.frozen = 0;
            self.tail_start = self.lines.len();
            return;
        };
        for entry in head {
            self.lines.extend(entry_lines(entry, tick, width));
            self.lines.push(Line::from(""));
        }
        self.frozen = head.len();
        self.tail_start = self.lines.len();
        self.lines.extend(entry_lines(last, tick, width));
        self.lines.push(Line::from(""));
    }
}

/// Reference implementation used to verify the incremental cache.
#[cfg(test)]
fn conversation_lines(entries: &[ChatEntry], tick: u64, width: u16) -> Vec<Line<'static>> {
    let width = width.max(1) as usize;
    let mut lines: Vec<Line<'static>> = vec![Line::from("")];
    for entry in entries {
        lines.extend(entry_lines(entry, tick, width));
        lines.push(Line::from(""));
    }
    lines
}

fn entry_lines(entry: &ChatEntry, tick: u64, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    {
        match entry {
            ChatEntry::User(text) => {
                for raw in wrap_text(text, width.saturating_sub(4)) {
                    lines.push(Line::from(vec![
                        Span::styled("  ▌ ", Style::default().fg(current().accent)),
                        Span::styled(raw, Style::default().fg(current().text)),
                    ]));
                }
            }
            ChatEntry::Assistant(text) => {
                lines.extend(markdown_lines(text, width));
            }
            ChatEntry::Thinking { summary, seconds } => {
                let segments = wrap_text(summary, width.saturating_sub(14));
                let last = segments.len().saturating_sub(1);
                for (i, raw) in segments.into_iter().enumerate() {
                    let head = if i == 0 { "   ▸ Thought: " } else { "     " };
                    let mut spans = vec![
                        Span::styled(
                            head,
                            Style::default()
                                .fg(current().muted)
                                .add_modifier(Modifier::ITALIC),
                        ),
                        Span::styled(
                            raw,
                            Style::default()
                                .fg(current().muted)
                                .add_modifier(Modifier::ITALIC),
                        ),
                    ];
                    if i == last {
                        spans.push(Span::styled(
                            format!(" · {seconds:.1}s"),
                            Style::default().fg(current().border),
                        ));
                    }
                    lines.push(Line::from(spans));
                }
            }
            ChatEntry::Tool(tool) => {
                let (glyph, color) = match tool.status {
                    ToolStatus::Running => (spinner(tick), current().interactive),
                    ToolStatus::Done => ("✓", current().success),
                    ToolStatus::Error => ("✗", current().error),
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("   {glyph} "), Style::default().fg(color)),
                    Span::styled(
                        format!("{}  ", tool.name),
                        Style::default()
                            .fg(current().text)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(tool.summary.clone(), Style::default().fg(current().muted)),
                ]));
                if !tool.detail.is_empty() {
                    for raw in wrap_text(&tool.detail, width.saturating_sub(5)) {
                        lines.push(Line::from(Span::styled(
                            format!("     {raw}"),
                            Style::default().fg(current().muted),
                        )));
                    }
                }
            }
            ChatEntry::Diff { path, lines: diff } => {
                let avail = width.saturating_sub(7);
                let shown = if path.width() > avail {
                    let mut cut: String = path.chars().take(avail.saturating_sub(1)).collect();
                    cut.push('…');
                    cut
                } else {
                    path.clone()
                };
                let mut header = vec![
                    Span::styled("   ", Style::default()),
                    Span::styled(
                        "│ ",
                        Style::default()
                            .fg(current().border)
                            .bg(current().diff_ctx_gutter),
                    ),
                    Span::styled(
                        shown.clone(),
                        Style::default()
                            .fg(current().muted)
                            .bg(current().diff_ctx_gutter),
                    ),
                ];
                let header_used = 5 + shown.width();
                if header_used < width {
                    header.push(Span::styled(
                        " ".repeat(width - header_used),
                        Style::default().bg(current().diff_ctx_gutter),
                    ));
                }
                lines.push(Line::from(header));
                for line in diff {
                    let (sign, sign_color, content_bg, gutter_bg) = match line.kind {
                        DiffKind::Add => (
                            "+",
                            current().diff_add,
                            current().diff_add_bg,
                            current().diff_add_gutter,
                        ),
                        DiffKind::Rem => (
                            "-",
                            current().diff_rem,
                            current().diff_rem_bg,
                            current().diff_rem_gutter,
                        ),
                        DiffKind::Ctx => {
                            for raw in wrap_text(&line.text, width.saturating_sub(7).max(1)) {
                                lines.push(Line::from(vec![
                                    Span::styled("   ", Style::default()),
                                    Span::styled(
                                        "│ ",
                                        Style::default()
                                            .fg(current().border)
                                            .bg(current().diff_ctx_gutter),
                                    ),
                                    Span::styled("  ", Style::default().fg(current().muted)),
                                    Span::styled(raw, Style::default().fg(current().diff_ctx_text)),
                                ]));
                            }
                            continue;
                        }
                    };
                    for raw in wrap_text(&line.text, width.saturating_sub(7).max(1)) {
                        let mut spans = vec![
                            Span::styled("   ", Style::default()),
                            Span::styled("┊ ", Style::default().fg(current().border).bg(gutter_bg)),
                            Span::styled(
                                format!("{sign} "),
                                Style::default().fg(sign_color).bg(content_bg),
                            ),
                            Span::styled(
                                raw.clone(),
                                Style::default().fg(current().text).bg(content_bg),
                            ),
                        ];
                        let used = 7 + raw.width();
                        if used < width {
                            spans.push(Span::styled(
                                " ".repeat(width - used),
                                Style::default().bg(content_bg),
                            ));
                        }
                        lines.push(Line::from(spans));
                    }
                }
            }
        }
    }
    lines
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Paragraph,
    Heading,
    List,
    Quote,
    Code,
    Rule,
}

fn block_kind(block: &markdown::Block) -> BlockKind {
    match block {
        markdown::Block::Paragraph(_) => BlockKind::Paragraph,
        markdown::Block::Heading(..) => BlockKind::Heading,
        markdown::Block::Bullet(_) | markdown::Block::Numbered(..) => BlockKind::List,
        markdown::Block::Quote(_) => BlockKind::Quote,
        markdown::Block::Code(..) => BlockKind::Code,
        markdown::Block::Rule => BlockKind::Rule,
    }
}

fn needs_gap(prev: BlockKind, current: BlockKind) -> bool {
    match current {
        BlockKind::Heading | BlockKind::Quote | BlockKind::Code | BlockKind::Rule => true,
        BlockKind::List => prev != BlockKind::List,
        BlockKind::Paragraph => matches!(
            prev,
            BlockKind::Heading
                | BlockKind::List
                | BlockKind::Quote
                | BlockKind::Code
                | BlockKind::Rule
        ),
    }
}

fn markdown_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    let width = width.max(4);
    let blocks = markdown::parse(text);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut prev: Option<BlockKind> = None;
    let mut index = 0;
    while index < blocks.len() {
        let kind = block_kind(&blocks[index]);
        if let Some(prev_kind) = prev
            && needs_gap(prev_kind, kind)
        {
            out.push(Line::default());
        }
        match &blocks[index] {
            markdown::Block::Paragraph(segs) => {
                out.extend(markdown::wrap_segs(
                    segs,
                    width - 3,
                    "   ",
                    "   ",
                    Style::default(),
                ));
            }
            markdown::Block::Heading(level, segs) => {
                let segs = restyle(segs, |style| match style {
                    markdown::SegStyle::Normal if *level == 1 => markdown::SegStyle::Heading1,
                    markdown::SegStyle::Normal => markdown::SegStyle::Heading,
                    other => other,
                });
                out.extend(markdown::wrap_segs(
                    &segs,
                    width - 3,
                    "   ",
                    "   ",
                    Style::default(),
                ));
            }
            markdown::Block::Bullet(_) | markdown::Block::Numbered(..) => {
                let mut run_end = index;
                while run_end < blocks.len()
                    && matches!(
                        blocks[run_end],
                        markdown::Block::Bullet(_) | markdown::Block::Numbered(..)
                    )
                {
                    run_end += 1;
                }
                let marker_width = blocks[index..run_end]
                    .iter()
                    .map(|block| match block {
                        markdown::Block::Numbered(number, _) => format!("{number}.").len(),
                        _ => 1,
                    })
                    .max()
                    .unwrap_or(1);
                for block in &blocks[index..run_end] {
                    let (marker, segs) = match block {
                        markdown::Block::Numbered(number, segs) => (format!("{number}."), segs),
                        markdown::Block::Bullet(segs) => ("-".to_string(), segs),
                        _ => unreachable!(),
                    };
                    let lead = format!("   {:>width$} ", marker, width = marker_width);
                    let cont = " ".repeat(3 + marker_width + 1);
                    out.extend(markdown::wrap_segs(
                        segs,
                        width.saturating_sub(3 + marker_width + 1),
                        &lead,
                        &cont,
                        Style::default().fg(current().md_list),
                    ));
                }
                index = run_end;
                prev = Some(BlockKind::List);
                continue;
            }
            markdown::Block::Quote(segs) => {
                let segs = restyle(segs, |style| match style {
                    markdown::SegStyle::Normal => markdown::SegStyle::Quote,
                    other => other,
                });
                out.extend(markdown::wrap_segs(
                    &segs,
                    width.saturating_sub(5),
                    "   │ ",
                    "   │ ",
                    Style::default().fg(current().md_rule),
                ));
            }
            markdown::Block::Code(lang, code_lines) => {
                let inner = width.saturating_sub(3);
                for code_line in code_lines {
                    if code_line.trim().is_empty() {
                        out.push(Line::default());
                        continue;
                    }
                    for chunk in chunk_segs(&markdown::highlight(lang, code_line), inner) {
                        let mut spans = vec![Span::styled("   ", Style::default())];
                        spans.extend(chunk.iter().map(|seg| {
                            Span::styled(seg.text.clone(), markdown::style_of(seg.style))
                        }));
                        out.push(Line::from(spans));
                    }
                }
            }
            markdown::Block::Rule => {
                out.push(Line::from(vec![
                    Span::styled("   ", Style::default()),
                    Span::styled(
                        "─".repeat(width.saturating_sub(3)),
                        Style::default().fg(current().md_rule),
                    ),
                ]));
            }
        }
        prev = Some(kind);
        index += 1;
    }
    out
}

fn chunk_segs(segs: &[markdown::Seg], width: usize) -> Vec<Vec<markdown::Seg>> {
    let width = width.max(1);
    let mut chunks: Vec<Vec<markdown::Seg>> = Vec::new();
    let mut current: Vec<markdown::Seg> = Vec::new();
    let mut current_width = 0usize;
    for seg in segs {
        let mut text = String::new();
        for ch in seg.text.chars() {
            let ch_width = ch.width().unwrap_or(0);
            if current_width + ch_width > width && current_width > 0 {
                if !text.is_empty() {
                    current.push(markdown::Seg {
                        text: std::mem::take(&mut text),
                        style: seg.style,
                    });
                }
                chunks.push(std::mem::take(&mut current));
                current_width = 0;
            }
            text.push(ch);
            current_width += ch_width;
        }
        if !text.is_empty() {
            current.push(markdown::Seg {
                text,
                style: seg.style,
            });
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn restyle(
    segs: &[markdown::Seg],
    map: impl Fn(markdown::SegStyle) -> markdown::SegStyle,
) -> Vec<markdown::Seg> {
    segs.iter()
        .map(|seg| markdown::Seg {
            text: seg.text.clone(),
            style: map(seg.style),
        })
        .collect()
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let mut line = String::new();
        let mut line_width = 0usize;
        let mut has_words = false;
        for word in raw.split_whitespace() {
            has_words = true;
            let word_width = word.width();
            if word_width > width {
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                }
                let mut chunk = String::new();
                let mut chunk_width = 0usize;
                for ch in word.chars() {
                    let ch_width = ch.width().unwrap_or(0);
                    if chunk_width + ch_width > width && chunk_width > 0 {
                        out.push(std::mem::take(&mut chunk));
                        chunk_width = 0;
                    }
                    chunk.push(ch);
                    chunk_width += ch_width;
                }
                line = chunk;
                line_width = chunk_width;
            } else if line.is_empty() {
                line.push_str(word);
                line_width = word_width;
            } else if line_width + 1 + word_width <= width {
                line.push(' ');
                line.push_str(word);
                line_width += 1 + word_width;
            } else {
                out.push(std::mem::take(&mut line));
                line.push_str(word);
                line_width = word_width;
            }
        }
        if has_words {
            out.push(line);
        } else {
            out.push(String::new());
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn spinner(tick: u64) -> &'static str {
    ["·", "•", "●", "•"][(tick % 4) as usize]
}

fn input(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.height == 0 {
        return;
    }
    let border_color = match app.mode {
        AgentMode::Build => current().border,
        AgentMode::Plan => current().accent,
    };
    let block = WidgetBlock::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 {
        return;
    }

    if app.busy {
        let line = Line::from(vec![
            Span::styled(
                format!("{} ", spinner(app.tick)),
                Style::default().fg(current().interactive),
            ),
            Span::styled(
                "streaming…  esc to cancel",
                Style::default().fg(current().muted),
            ),
        ]);
        frame.render_widget(Paragraph::new(line), inner);
        return;
    }

    // Attachment chips (pasted screenshot paths) render above the text.
    let chip_rows = render_attachment_chips(frame, inner, app);
    let text_area = Rect {
        x: inner.x,
        y: inner.y + chip_rows,
        width: inner.width,
        height: inner.height.saturating_sub(chip_rows),
    };
    if text_area.height == 0 {
        return;
    }

    if app.input.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Ask anything…",
                Style::default().fg(current().muted),
            ))),
            text_area,
        );
        frame.set_cursor_position((text_area.x, text_area.y));
        return;
    }

    let width = text_area.width as usize;
    let max_rows = text_area.height as usize;
    let rows = app.input.visual_rows(width);
    let (cursor_row, cursor_col) = app.input.cursor_visual(width);
    let first = if cursor_row >= max_rows {
        cursor_row + 1 - max_rows
    } else {
        0
    };
    let last = (first + max_rows).min(rows.len());

    let rendered: Vec<Line> = rows[first..last]
        .iter()
        .map(|row| {
            Line::from(Span::styled(
                row.as_str(),
                Style::default().fg(current().text),
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(rendered), text_area);
    if cursor_row >= first && cursor_row < last {
        let x = text_area.x + (cursor_col.min(width.saturating_sub(1))) as u16;
        let y = text_area.y + (cursor_row - first) as u16;
        frame.set_cursor_position((x, y));
    }
}

/// Renders the pasted image chips at the top of the composer. Kitty-capable
/// terminals get a real thumbnail (PNG fed straight to the graphics protocol,
/// queued into `app.pending_kitty` for the main loop to flush); everywhere else
/// a `🖼 name` placeholder chip is drawn. Returns the number of composer rows
/// consumed.
fn render_attachment_chips(frame: &mut Frame, inner: Rect, app: &mut App) -> u16 {
    app.chip_rects.clear();
    app.chip_x_rects.clear();
    if app.attachments.is_empty() || inner.height == 0 || inner.width < 8 {
        return 0;
    }
    let protocol = crate::image::detect_protocol();
    let max_chips = (inner.height.saturating_sub(1) as usize).min(app.attachments.len());

    if protocol == crate::image::ImageProtocol::Kitty {
        // Reserve rows for the thumbnails; the pixels are drawn out-of-band via
        // the kitty graphics protocol after the frame is presented.
        let thumb_rows = 4u16;
        let mut used = 0u16;
        for (i, attachment) in app.attachments.iter().take(max_chips).enumerate() {
            if used + thumb_rows > inner.height.saturating_sub(1) {
                break;
            }
            let data = match std::fs::read(&attachment.path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let cols = (inner.width.saturating_sub(2)).max(2);
            let rows = if attachment.height > 0 && attachment.width > 0 {
                (attachment.height as u64 * cols as u64 / attachment.width as u64)
                    .clamp(1, thumb_rows as u64) as u16
            } else {
                thumb_rows
            };
            let esc = crate::image::kitty_transmit(attachment.id, &data, cols, rows);
            app.pending_kitty.push(esc);
            app.chip_rects.push(Rect {
                x: inner.x,
                y: inner.y + used,
                width: inner.width,
                height: rows,
            });
            app.chip_x_rects.push(Rect {
                x: inner.x + inner.width.saturating_sub(3),
                y: inner.y + used,
                width: 3,
                height: 1,
            });
            let _ = i;
            used += rows + 1;
        }
        return used.min(inner.height.saturating_sub(1));
    }

    for (i, attachment) in app.attachments.iter().take(max_chips).enumerate() {
        let row = Rect {
            x: inner.x,
            y: inner.y + i as u16,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Clear, row);
        let label = attachment.label();
        let style = Style::default()
            .fg(current().interactive)
            .bg(current().raised);
        let pad = (inner.width as usize)
            .saturating_sub(label.width() + 2)
            .saturating_sub(1);
        let line = Line::from(vec![
            Span::styled(format!(" {label}{}", " ".repeat(pad)), style),
            Span::styled("x", style),
        ]);
        frame.render_widget(Paragraph::new(line), row);
        app.chip_rects.push(row);
        app.chip_x_rects.push(Rect {
            x: inner.x + inner.width.saturating_sub(3),
            y: inner.y + i as u16,
            width: 3,
            height: 1,
        });
    }
    max_chips as u16
}

fn metadata(frame: &mut Frame, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }
    let (label, color) = match app.mode {
        AgentMode::Build => ("Build", current().interactive),
        AgentMode::Plan => ("Plan", current().accent),
    };
    let win = match app.maximized {
        Some(true) => "max",
        Some(false) => "win",
        None => "-",
    };
    let line = Line::from(vec![
        Span::styled(
            format!("  {label}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" · {} · {win}", app.backend_label),
            Style::default().fg(current().muted),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn sidebar(frame: &mut Frame, area: Rect, app: &App) {
    let block = WidgetBlock::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(current().border))
        .title(Span::styled(
            " sessions ",
            Style::default().fg(current().muted),
        ))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let separator = "─".repeat(inner.width as usize);
    let lines = vec![
        Line::from(vec![
            Span::styled("▸ ", Style::default().fg(current().interactive)),
            Span::styled(app.display_name(), Style::default().fg(current().text)),
        ]),
        Line::from(Span::styled(
            format!("  {} · {} saved", app.backend_label, app.sessions.len()),
            Style::default().fg(current().muted),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("▸ ", Style::default().fg(current().interactive)),
            Span::styled("new session", Style::default().fg(current().text)),
            Span::styled("   ctrl+n", Style::default().fg(current().muted)),
        ]),
        Line::from(vec![
            Span::styled("▸ ", Style::default().fg(current().interactive)),
            Span::styled("session list", Style::default().fg(current().text)),
            Span::styled(" ctrl+o", Style::default().fg(current().muted)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            separator,
            Style::default().fg(current().border),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "shortcuts",
            Style::default()
                .fg(current().text)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "  esc      cancel turn",
            Style::default().fg(current().muted),
        )),
        Line::from(Span::styled(
            "  shift+tab  plan / build",
            Style::default().fg(current().muted),
        )),
        Line::from(Span::styled(
            "  /        commands",
            Style::default().fg(current().muted),
        )),
        Line::from(Span::styled(
            "  ctrl+y   copy reply",
            Style::default().fg(current().muted),
        )),
        Line::from(Span::styled(
            "  drag     select + copy",
            Style::default().fg(current().muted),
        )),
        Line::from(Span::styled(
            "  ctrl+c   quit",
            Style::default().fg(current().muted),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn footer(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = if app.busy {
        let status = match app.iteration {
            Some((n, max)) => format!("working · iterasi {n}/{max}"),
            None => "working".to_string(),
        };
        vec![
            Span::styled(
                format!(" {} ", spinner(app.tick)),
                Style::default().fg(current().interactive),
            ),
            Span::styled(status, Style::default().fg(current().muted)),
            Span::styled("  ·  ", Style::default().fg(current().border)),
            Span::styled("esc ", Style::default().fg(current().interactive)),
            Span::styled("cancel", Style::default().fg(current().muted)),
            Span::styled("  ·  ", Style::default().fg(current().border)),
            Span::styled("ctrl+c ", Style::default().fg(current().interactive)),
            Span::styled("quit", Style::default().fg(current().muted)),
        ]
    } else {
        vec![
            Span::styled(" / ", Style::default().fg(current().interactive)),
            Span::styled("commands", Style::default().fg(current().muted)),
        ]
    };
    if let Some((text, _)) = &app.flash {
        spans.push(Span::styled("  ·  ", Style::default().fg(current().border)));
        spans.push(Span::styled(
            text.clone(),
            Style::default().fg(current().success),
        ));
    }
    let hint = Line::from(spans);

    let cwd = format!(" {} ", shorten_path(&app.cwd, 40));
    let cwd_width = (cwd.chars().count() as u16).min(area.width);
    let columns =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(cwd_width)]).split(area);

    frame.render_widget(Paragraph::new(hint), columns[0]);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            cwd,
            Style::default().fg(current().muted),
        )))
        .alignment(Alignment::Right),
        columns[1],
    );
}

fn modal(frame: &mut Frame, title: &str, width: u16, height: u16) -> Option<Rect> {
    let screen = frame.area();
    let width = width.min(screen.width.saturating_sub(4));
    // Clamp instead of bailing: a too-tall dialog on a short terminal should
    // shrink (and its list window) rather than disappear entirely.
    let height = height.min(screen.height);
    if width < 12 || height < 3 {
        return None;
    }
    let x = screen.x + screen.width.saturating_sub(width) / 2;
    let y = screen.y + screen.height.saturating_sub(height) / 3;
    let rect = Rect::new(x, y, width, height);
    frame.render_widget(Clear, rect);
    let block = WidgetBlock::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(current().border))
        .style(Style::default().bg(current().raised))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(current().muted),
        ));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    (inner.width >= 4 && inner.height >= 1).then_some(inner)
}

/// Window of at most `cap` rows that keeps `selected` visible.
/// Returns (offset, visible). Keeps the selection as the last row when moving
/// down, like a normal scrolling list.
fn dialog_window(total: usize, selected: usize, cap: usize) -> (usize, usize) {
    if total == 0 {
        return (0, 0);
    }
    let visible = total.min(cap.max(1));
    let offset = selected
        .saturating_sub(visible - 1)
        .min(total.saturating_sub(visible));
    (offset, visible)
}

fn modal_row(inner: Rect, label: &str, selected: bool) -> Line<'static> {
    let text = format!(" {label}");
    if selected {
        let pad = (inner.width as usize).saturating_sub(text.width());
        Line::from(Span::styled(
            format!("{text}{}", " ".repeat(pad)),
            Style::default().fg(current().text).bg(current().selection),
        ))
    } else {
        Line::from(Span::styled(
            text,
            Style::default().fg(current().muted).bg(current().raised),
        ))
    }
}

/// A centered, non-interactive line: footer hints and display-only values.
fn centered_line(inner: Rect, text: &str, style: Style) -> Line<'static> {
    let pad = (inner.width as usize).saturating_sub(text.width()) / 2;
    Line::from(Span::styled(format!("{}{}", " ".repeat(pad), text), style))
}

fn theme_row(inner: Rect, idx: usize, selected: bool) -> Line<'static> {
    let theme = &crate::theme::THEMES[idx];
    let label = format!(" {}", theme.label);
    let pad = (inner.width as usize).saturating_sub(label.width() + 2);
    let bg = if selected {
        current().selection
    } else {
        current().raised
    };
    let fg = if selected {
        current().text
    } else {
        current().muted
    };
    Line::from(vec![
        Span::styled("●", Style::default().fg(theme.accent).bg(bg)),
        Span::styled("●", Style::default().fg(theme.interactive).bg(bg)),
        Span::styled(
            format!("{label}{}", " ".repeat(pad)),
            Style::default().fg(fg).bg(bg),
        ),
    ])
}

fn palette(frame: &mut Frame, app: &App) {
    let Some(state) = &app.palette else {
        return;
    };
    let entries = app.filtered_commands();
    let screen = frame.area();
    // Total modal height is bound by the screen so it never gets clipped off.
    let rows = (entries.len().max(1) as u16)
        .min(crate::app::DIALOG_PAGE as u16)
        .min(screen.height.saturating_sub(4));
    let Some(inner) = modal(frame, "commands", 58, rows + 4) else {
        return;
    };
    // Two rows at the top are the query line and separator; the list fills the
    // rest of the inner area.
    let list_capacity = inner.height.saturating_sub(2) as usize;
    let selected = state.selected;
    let (offset, visible) = dialog_window(entries.len(), selected, list_capacity);

    let query_line = Line::from(vec![
        Span::styled(
            "› ",
            Style::default().fg(current().accent).bg(current().raised),
        ),
        Span::styled(
            state.query.clone(),
            Style::default().fg(current().text).bg(current().raised),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(query_line),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let cursor_x = inner.x + 2 + state.query.width() as u16;
    frame.set_cursor_position((cursor_x.min(inner.right().saturating_sub(1)), inner.y));

    let separator = Line::from(Span::styled(
        "─".repeat(inner.width as usize),
        Style::default().fg(current().border).bg(current().raised),
    ));
    frame.render_widget(
        Paragraph::new(separator),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );

    if entries.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " no matching commands",
                Style::default().fg(current().muted).bg(current().raised),
            ))),
            Rect::new(inner.x, inner.y + 2, inner.width, 1),
        );
        return;
    }

    let list: Vec<Line> = entries
        .iter()
        .skip(offset)
        .take(visible)
        .enumerate()
        .map(|(i, (label, _))| modal_row(inner, label, (i + offset) == selected))
        .collect();
    let list_h = visible as u16;
    frame.render_widget(
        Paragraph::new(list),
        Rect::new(inner.x, inner.y + 2, inner.width, list_h),
    );
}

fn slash_menu(frame: &mut Frame, area: Rect, composer_top: u16, app: &App) {
    let options = app.filtered_slash();
    let rows = (options.len().max(1) as u16).min(8);
    let height = rows + 2;
    let width = area.width.min(62);
    if width < 20 || height > area.height || composer_top < area.y + height {
        return;
    }
    let rect = Rect::new(area.x, composer_top - height, width, height);
    frame.render_widget(Clear, rect);
    let block = WidgetBlock::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(current().border))
        .style(Style::default().bg(current().raised))
        .title(Span::styled(
            " commands ",
            Style::default().fg(current().muted),
        ));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.width < 4 {
        return;
    }
    if options.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " no matching commands",
                Style::default().fg(current().muted).bg(current().raised),
            ))),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        return;
    }
    let selected = app.slash.as_ref().map(|menu| menu.selected).unwrap_or(0);
    let (offset, visible) = dialog_window(options.len(), selected, inner.height as usize);
    let lines: Vec<Line> = options
        .iter()
        .skip(offset)
        .take(visible)
        .enumerate()
        .map(|(i, (name, description, _))| {
            slash_row(inner, name, description, (i + offset) == selected)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn slash_row(inner: Rect, name: &str, description: &str, selected: bool) -> Line<'static> {
    let bg = if selected {
        current().selection
    } else {
        current().raised
    };
    let label = format!(" /{name}");
    let label_pad = 18usize.saturating_sub(label.width());
    let used = label.width() + label_pad + 1 + description.width();
    let pad = (inner.width as usize).saturating_sub(used);
    Line::from(vec![
        Span::styled(
            format!("{label}{}", " ".repeat(label_pad)),
            Style::default()
                .fg(if selected {
                    current().text
                } else {
                    current().interactive
                })
                .bg(bg),
        ),
        Span::styled(" ", Style::default().bg(bg)),
        Span::styled(
            description.to_string(),
            Style::default().fg(current().muted).bg(bg),
        ),
        Span::styled(" ".repeat(pad), Style::default().bg(bg)),
    ])
}

fn approval_panel(frame: &mut Frame, area: Rect, app: &App) {
    let Some(approval) = &app.approval else {
        return;
    };
    if area.height < 3 || area.width < 20 {
        return;
    }
    frame.render_widget(Clear, area);
    frame.render_widget(
        WidgetBlock::default().style(Style::default().bg(current().raised)),
        area,
    );

    let bar = |bg: Color| Span::styled("▌", Style::default().fg(current().accent).bg(bg));
    let body_rect = Rect {
        height: area.height.saturating_sub(1),
        ..area
    };
    let lines = vec![
        Line::from(vec![
            bar(current().raised),
            Span::styled(
                " △ ",
                Style::default()
                    .fg(current().interactive)
                    .bg(current().raised),
            ),
            Span::styled(
                "Permission required",
                Style::default().fg(current().text).bg(current().raised),
            ),
        ]),
        Line::from(vec![
            bar(current().raised),
            Span::styled(
                format!("   {}  ", approval.name),
                Style::default()
                    .fg(current().text)
                    .bg(current().raised)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                approval.summary.clone(),
                Style::default().fg(current().muted).bg(current().raised),
            ),
        ]),
        Line::from(bar(current().raised)),
    ];
    frame.render_widget(Paragraph::new(lines), body_rect);

    let footer_rect = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);
    let mut spans = vec![
        Span::styled(
            "▌",
            Style::default().fg(current().accent).bg(current().base),
        ),
        Span::styled(" ", Style::default().bg(current().base)),
    ];
    for (i, label) in APPROVAL_OPTIONS.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" ", Style::default().bg(current().base)));
        }
        spans.push(if i == approval.selected {
            Span::styled(
                format!(" {label} "),
                Style::default().fg(current().base).bg(current().accent),
            )
        } else {
            Span::styled(
                format!(" {label} "),
                Style::default()
                    .fg(current().accent)
                    .bg(current().action_dim),
            )
        });
    }
    let hints = vec![
        Span::styled("⇆ ", Style::default().fg(current().text).bg(current().base)),
        Span::styled(
            "select",
            Style::default().fg(current().muted).bg(current().base),
        ),
        Span::styled("  ", Style::default().bg(current().base)),
        Span::styled(
            "enter ",
            Style::default().fg(current().text).bg(current().base),
        ),
        Span::styled(
            "confirm",
            Style::default().fg(current().muted).bg(current().base),
        ),
    ];
    let used: usize = spans
        .iter()
        .chain(hints.iter())
        .map(|span| span.content.width())
        .sum();
    let pad = (area.width as usize).saturating_sub(used);
    spans.push(Span::styled(
        " ".repeat(pad),
        Style::default().bg(current().base),
    ));
    spans.extend(hints);
    frame.render_widget(Paragraph::new(Line::from(spans)), footer_rect);
}

fn session_modal(frame: &mut Frame, app: &App) {
    let Some(dialog) = &app.session_dialog else {
        return;
    };
    let rows = app.session_rows();
    let total = rows.len().max(1) as u16;
    let cap = total
        .min(crate::app::DIALOG_PAGE as u16)
        .min(frame.area().height);
    let Some(inner) = modal(frame, "sessions", 52, cap + 2) else {
        return;
    };
    let (offset, visible) = dialog_window(rows.len(), dialog.selected, inner.height as usize);
    let list: Vec<Line> = rows
        .iter()
        .skip(offset)
        .take(visible)
        .enumerate()
        .map(|(i, label)| modal_row(inner, label, (i + offset) == dialog.selected))
        .collect();
    frame.render_widget(Paragraph::new(list), inner);
}

fn theme_modal(frame: &mut Frame, app: &App) {
    let Some(dialog) = &app.theme_dialog else {
        return;
    };
    let total = crate::theme::count();
    let cap = (total as u16)
        .min(crate::app::DIALOG_PAGE as u16)
        .min(frame.area().height);
    let Some(inner) = modal(frame, "themes", 44, cap + 3) else {
        return;
    };
    let (offset, visible) = dialog_window(
        total,
        dialog.selected,
        inner.height.saturating_sub(1) as usize,
    );
    let mut list: Vec<Line> = (0..total)
        .skip(offset)
        .take(visible)
        .map(|i| theme_row(inner, i, i == dialog.selected))
        .collect();
    list.push(Line::from(Span::styled(
        " enter save · esc revert",
        Style::default().fg(current().muted).bg(current().raised),
    )));
    frame.render_widget(Paragraph::new(list), inner);
}

fn remote_modal(frame: &mut Frame, app: &App) {
    let Some(dialog) = &app.remote_dialog else {
        return;
    };
    let Some(state) = &app.remote else {
        return;
    };

    let onoff = if state.enabled { "on" } else { "off" };
    let permit = match state.permit {
        evrax_remote::Permit::Read => "read",
        evrax_remote::Permit::Write => "read/write",
    };
    let editing = dialog.editing.as_deref();
    let password = match editing {
        Some(buf) if !buf.is_empty() => buf.to_string(),
        Some(_) => String::new(),
        None => "•".repeat(8),
    };

    let height = app.remote_row_count() as u16 + 3;
    let Some(inner) = modal(frame, "remote web", 60, height) else {
        return;
    };

    // Interactive rows first, then the display-only rows as one centered block
    // (shared left offset keeps the labels aligned). The cursor never lands on
    // the block, so it reads as information instead of menu items.
    let interactive = [
        format!("enabled      {onoff}"),
        format!("permit       {permit}"),
        format!("password     {password}"),
        format!("tunnel       {}", app.tunnel_label()),
    ];
    let info = [
        format!("bind         {} ·", state.config.bind),
        format!("port         {} ·", state.config.port),
        format!("url          {} ·", app.remote_display_url()),
        format!("session      {} ·", state.session_id),
    ];
    let info_width = info
        .iter()
        .map(|row| row.as_str().width())
        .max()
        .unwrap_or(0);
    let info_pad = " ".repeat((inner.width as usize).saturating_sub(info_width) / 2);

    let mut list: Vec<Line> = [0usize, 1, 2, 6]
        .iter()
        .zip(interactive.iter())
        .map(|(i, label)| modal_row(inner, label, editing.is_none() && *i == dialog.selected))
        .collect();
    list.extend(info.iter().map(|row| {
        Line::from(Span::styled(
            format!("{info_pad}{row}"),
            Style::default().fg(current().muted).bg(current().raised),
        ))
    }));
    let hint = if editing.is_some() {
        " type password · enter save · esc cancel"
    } else {
        " enter edit/toggle · esc close"
    };
    list.push(centered_line(
        inner,
        hint.trim(),
        Style::default().fg(current().muted).bg(current().raised),
    ));
    frame.render_widget(Paragraph::new(list), inner);

    if let Some(buf) = editing {
        // Park the caret at the end of the password value (row index 2). The
        // value starts after the leading row space (1) + "password" (8) + pad (5).
        let x = inner.x + 14 + buf.width() as u16;
        let y = inner.y + 2;
        frame.set_cursor_position((x.min(inner.right().saturating_sub(1)), y));
    }
}

fn tunnel_modal(frame: &mut Frame, app: &mut App) {
    let Some(dialog) = &app.tunnel_dialog else {
        return;
    };
    let selected = dialog.selected;
    let editing = dialog.editing.clone();
    let kind = dialog.kind;
    let rows = app.tunnel_rows();
    let status = app.tunnel_label();
    let height = (rows.len() + 4) as u16;
    let Some(inner) = modal(frame, "tunnel", 60, height) else {
        return;
    };

    let mut list: Vec<Line> = rows
        .iter()
        .enumerate()
        .map(|(i, label)| modal_row(inner, label, editing.is_none() && i == selected))
        .collect();
    match editing.as_deref() {
        Some(buf) => {
            let label = match kind {
                TunnelEditKind::Hostname => " hostname     ",
                TunnelEditKind::ManualUrl => " url          ",
            };
            list.push(Line::from(Span::styled(
                format!("{label}{buf}"),
                Style::default().fg(current().text).bg(current().raised),
            )));
        }
        // Display-only status: centered and dim so it never reads as a menu row.
        None => list.push(centered_line(
            inner,
            &status,
            Style::default().fg(current().muted).bg(current().raised),
        )),
    }
    let hint = match (editing.is_some(), kind) {
        (true, TunnelEditKind::Hostname) => {
            "paste hostname (domain di akun cloudflare) · enter run · esc cancel"
        }
        (true, TunnelEditKind::ManualUrl) => "paste url · enter save · esc cancel · empty clears",
        (false, _) => "enter: start/stop · click url: copy · esc close",
    };
    list.push(centered_line(
        inner,
        hint,
        Style::default().fg(current().muted).bg(current().raised),
    ));
    frame.render_widget(Paragraph::new(list), inner);

    // Clickable area of the status/url line (a click copies the tunnel URL).
    app.tunnel_url_rect = if editing.is_none() {
        Some(Rect::new(
            inner.x,
            inner.y + rows.len() as u16,
            inner.width,
            1,
        ))
    } else {
        None
    };

    if let Some(buf) = editing.as_deref() {
        // Caret after the url value on the input line (first line past the menu).
        let x = inner.x + 14 + buf.width() as u16;
        let y = inner.y + rows.len() as u16;
        frame.set_cursor_position((x.min(inner.right().saturating_sub(1)), y));
    }
}

fn preview_modal(frame: &mut Frame, app: &mut App) {
    let Some(index) = app.preview else {
        return;
    };
    let Some(attachment) = app.attachments.get(index) else {
        app.preview = None;
        return;
    };
    // Width: clamp to screen, height: fit name + dims + path + hint + border.
    let width = 64u16.min(frame.area().width.saturating_sub(4)).max(12);
    let height = 8u16;
    let Some(inner) = modal(frame, "image preview", width, height) else {
        return;
    };

    if crate::image::detect_protocol() == crate::image::ImageProtocol::Kitty {
        if let Ok(data) = std::fs::read(&attachment.path) {
            let cols = inner.width.saturating_sub(2).max(2);
            let rows = inner.height.saturating_sub(2).max(1);
            let esc = crate::image::kitty_transmit(attachment.id, &data, cols, rows);
            app.pending_kitty.push(esc);
        }
        let hint = Line::from(Span::styled(
            " esc close",
            Style::default().fg(current().muted).bg(current().raised),
        ));
        let footer = Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(1),
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(hint), footer);
        return;
    }

    let lines = vec![
        Line::from(Span::styled(
            attachment.name.clone(),
            Style::default().fg(current().text).bg(current().raised),
        )),
        Line::from(Span::styled(
            format!("{}×{}", attachment.width, attachment.height),
            Style::default().fg(current().muted).bg(current().raised),
        )),
        Line::from(Span::styled(
            attachment.path.clone(),
            Style::default().fg(current().muted).bg(current().raised),
        )),
        Line::from(Span::styled(
            " esc close",
            Style::default().fg(current().muted).bg(current().raised),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn shorten_path(path: &str, max: usize) -> String {
    let len = path.chars().count();
    if len <= max {
        return path.to_string();
    }
    let tail: String = path.chars().skip(len - (max - 1)).collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, ToolCall};
    use evrax_core::{AgentEvent, DiffLine};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn render_text(width: u16, height: u16) -> String {
        let mut app = App::new();
        render_text_with(&mut app, width, height)
    }

    /// Chat transcript covering every entry kind the renderer supports.
    fn sample_conversation() -> Vec<ChatEntry> {
        vec![
            ChatEntry::User("render markdown in the chat".to_string()),
            ChatEntry::Thinking {
                summary: "parsing markdown blocks".to_string(),
                seconds: 1.6,
            },
            ChatEntry::Tool(ToolCall {
                name: "read".to_string(),
                summary: "crates/evrax-tui/src/markdown.rs".to_string(),
                status: ToolStatus::Done,
                detail: "96 lines".to_string(),
            }),
            ChatEntry::Diff {
                path: "crates/evrax-tui/src/markdown.rs".to_string(),
                lines: vec![
                    DiffLine {
                        kind: DiffKind::Rem,
                        text: "let raw = text.to_string();".to_string(),
                    },
                    DiffLine {
                        kind: DiffKind::Add,
                        text: "for block in markdown::parse(text) {".to_string(),
                    },
                ],
            },
            ChatEntry::Tool(ToolCall {
                name: "shell".to_string(),
                summary: "cargo test".to_string(),
                status: ToolStatus::Error,
                detail: "1 failed".to_string(),
            }),
            ChatEntry::Assistant("One case still fails on wrapped code rows.".to_string()),
        ]
    }

    fn render_sample(width: u16, height: u16) -> String {
        let mut app = App::new();
        app.messages = sample_conversation();
        render_text_with(&mut app, width, height)
    }

    fn render_text_with(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        buffer_text(&terminal)
    }

    #[test]
    fn empty_chat_shows_faust_banner() {
        let text = render_text(100, 40);
        assert!(
            text.contains("E V R A X"),
            "banner wordmark missing:\n{text}"
        );
        assert!(
            text.contains("FAUST55"),
            "banner signature missing:\n{text}"
        );
    }

    #[test]
    fn renders_brand_sidebar_and_hints() {
        let mut app = App::new();
        app.messages.push(ChatEntry::User("hi".to_string()));
        let text = render_text_with(&mut app, 110, 28);
        assert!(text.contains("evrax"), "brand missing:\n{text}");
        assert!(text.contains("sessions"), "sidebar missing:\n{text}");
        assert!(
            text.contains("new session"),
            "sidebar action missing:\n{text}"
        );
        assert!(text.contains("Ask anything"), "input missing:\n{text}");
        assert!(text.contains("quit"), "footer hint missing:\n{text}");
    }

    #[test]
    fn remote_modal_centers_info_rows() {
        let mut app = App::new();
        app.open_remote_dialog();
        {
            let state = app.remote.as_mut().unwrap();
            state.config.bind = "10.0.0.1".into();
            state.config.port = 5555;
            state.session_id = "sess_x".into();
        }
        app.remote_dialog.as_mut().unwrap().selected = 0;
        let text = render_text_with(&mut app, 100, 30);
        assert!(text.contains("10.0.0.1 ·"), "bind marker missing:\n{text}");
        assert!(text.contains("sess_x ·"), "session marker missing:\n{text}");

        // Centered rows start well right of the dialog's left border.
        for needle in ["10.0.0.1 ·", "sess_x ·"] {
            let line = text
                .lines()
                .find(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("{needle} line missing:\n{text}"));
            assert!(
                line.find(needle).unwrap() >= 40,
                "info row should be centered: {line:?}"
            );
        }
        let hint = text
            .lines()
            .find(|l| l.contains("enter edit/toggle"))
            .expect("toggle hint missing");
        assert!(
            hint.find("enter edit/toggle").unwrap() >= 30,
            "hint should be centered: {hint:?}"
        );

        // Info rows are never cursor targets.
        for _ in 0..6 {
            app.remote_move(1);
            let row = app.remote_dialog.as_ref().unwrap().selected;
            assert!(
                !app.remote_row_is_info(row),
                "cursor landed on info row {row}"
            );
        }
    }

    #[test]
    fn tunnel_modal_centers_status_and_hint() {
        let mut app = App::new();
        app.open_remote_dialog();
        {
            let state = app.remote.as_mut().unwrap();
            state.config.tunnel_mode = "manual".into();
            state.config.public_url = "https://evrax.example.com".into();
        }
        app.open_tunnel_dialog();
        let text = render_text_with(&mut app, 100, 30);
        let status = text
            .lines()
            .find(|l| l.contains("manual · https://evrax.example.com"))
            .expect("status line missing");
        assert!(
            status.find("manual ·").unwrap() >= 30,
            "status should be centered: {status:?}"
        );
        let hint = text
            .lines()
            .find(|l| l.contains("click url: copy"))
            .expect("hint missing");
        assert!(
            hint.find("enter: start/stop").unwrap() >= 24,
            "hint should be centered: {hint:?}"
        );
    }

    #[test]
    fn hides_sidebar_on_narrow_terminal() {
        let mut app = App::new();
        app.messages.push(ChatEntry::User("hi".to_string()));
        let text = render_text_with(&mut app, 100, 20);
        assert!(
            !text.contains("sessions"),
            "sidebar should hide at 100 cols:\n{text}"
        );
        assert!(text.contains("Ask anything"), "input missing:\n{text}");
        let mut app = App::new();
        app.messages.push(ChatEntry::User("hi".to_string()));
        let wide = render_text_with(&mut app, 110, 20);
        assert!(
            wide.contains("sessions"),
            "sidebar should show at 110 cols:\n{wide}"
        );
    }

    #[test]
    fn renders_full_conversation() {
        let text = render_sample(100, 60);
        assert!(text.contains('▌'), "user bar missing:\n{text}");
        assert!(text.contains("Thought:"), "thinking missing:\n{text}");
        assert!(text.contains('✓'), "done tool glyph missing:\n{text}");
        assert!(text.contains('✗'), "error tool glyph missing:\n{text}");
        assert!(text.contains('┊'), "diff gutter missing:\n{text}");
        assert!(text.contains("+ "), "diff add line missing:\n{text}");
        assert!(text.contains("- "), "diff rem line missing:\n{text}");
        assert!(text.contains("cargo test"), "tool summary missing:\n{text}");
    }

    #[test]
    fn renders_typed_input() {
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let mut app = App::new();
        for c in "fix the failing test".chars() {
            app.input.insert_char(c);
        }
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("fix the failing test"),
            "input missing:\n{text}"
        );
        let footer = text.lines().last().unwrap_or_default().to_string();
        assert!(
            !footer.contains("enter"),
            "send hint should be gone:\n{footer}"
        );
    }

    #[test]
    fn footer_keeps_only_essential_hints() {
        let text = render_text(100, 24);
        let footer = text.lines().last().unwrap_or_default().to_string();
        assert!(
            footer.contains("commands"),
            "commands hint missing:\n{footer}"
        );
        assert!(
            !footer.contains("ctrl+p"),
            "ctrl+p hint should be gone:\n{footer}"
        );
        assert!(
            !footer.contains("send"),
            "send hint still present:\n{footer}"
        );
        assert!(
            !footer.contains("drag"),
            "drag hint still present:\n{footer}"
        );
        assert!(
            !footer.contains("shift+tab"),
            "mode hint still present:\n{footer}"
        );
        assert!(
            !footer.contains("quit"),
            "quit hint still present:\n{footer}"
        );
        assert!(
            !footer.contains("copy reply"),
            "copy hint still present:\n{footer}"
        );
    }

    #[test]
    fn palette_renders_and_filters_commands() {
        let mut app = App::new();
        app.toggle_palette();
        let text = render_text_with(&mut app, 100, 30);
        assert!(text.contains("commands"), "palette title missing:\n{text}");
        assert!(
            text.contains("Switch to plan mode"),
            "palette list missing:\n{text}"
        );
        assert!(text.contains("Quit evrax"), "palette list missing:\n{text}");
        for c in "quit".chars() {
            app.palette_type(c);
        }
        let text = render_text_with(&mut app, 100, 30);
        assert!(
            text.contains("Quit evrax"),
            "filtered list missing:\n{text}"
        );
        assert!(
            !text.contains("New session"),
            "filter should hide other commands:\n{text}"
        );
        for c in "zzz".chars() {
            app.palette_type(c);
        }
        let text = render_text_with(&mut app, 100, 30);
        assert!(
            text.contains("no matching commands"),
            "empty state missing:\n{text}"
        );
    }

    #[test]
    fn slash_menu_renders_above_composer() {
        let mut app = App::new();
        app.input.insert_char('/');
        app.slash_sync();
        let text = render_text_with(&mut app, 100, 24);
        assert!(text.contains("/new"), "slash options missing:\n{text}");
        assert!(
            text.contains("start a new session"),
            "description missing:\n{text}"
        );
        assert!(
            text.contains("/sessions"),
            "session aliases missing:\n{text}"
        );
        let rows: Vec<&str> = text.lines().collect();
        let menu_row = rows
            .iter()
            .position(|row| row.contains("/new"))
            .expect("menu row");
        let composer_row = rows
            .iter()
            .position(|row| row.contains("Build ·"))
            .expect("metadata row");
        assert!(menu_row < composer_row, "menu must sit above the composer");
    }

    #[test]
    fn theme_modal_lists_themes() {
        let mut app = App::new();
        app.open_theme_dialog();
        let text = render_text_with(&mut app, 100, 30);
        assert!(text.contains("themes"), "title missing:\n{text}");
        assert!(text.contains("evrax"), "evrax option missing:\n{text}");
        assert!(
            text.contains("opencode"),
            "opencode option missing:\n{text}"
        );
        assert!(text.contains("dracula"), "dracula option missing:\n{text}");
        app.theme_close();
    }

    #[test]
    fn theme_switch_repaints_markdown() {
        let mut app = App::new();
        app.messages.clear();
        app.messages
            .push(ChatEntry::Assistant("hello **bold** world".into()));
        let original = crate::theme::index();
        crate::theme::set_index(crate::theme::by_key("opencode").unwrap());
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let repainted = (0..buffer.area.height).any(|y| {
            (0..buffer.area.width).any(|x| buffer[(x, y)].fg == crate::theme::OPENCODE.md_strong)
        });
        crate::theme::set_index(original);
        assert!(repainted, "opencode strong token should repaint markdown");
    }

    #[test]
    fn theme_switch_repaints_chrome() {
        let mut app = App::new();
        app.apply_event(AgentEvent::ApprovalRequested {
            name: "edit".into(),
            summary: "src/ui.rs".into(),
            key: "edit".into(),
        });
        let original = crate::theme::index();
        crate::theme::set_index(crate::theme::by_key("opencode").unwrap());
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut button_bg = None;
        for y in 0..buffer.area.height {
            let row: String = (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            if let Some(idx) = row.find("Allow once") {
                button_bg = Some(buffer[(idx as u16, y)].bg);
                break;
            }
        }
        crate::theme::set_index(original);
        assert_eq!(
            button_bg,
            Some(crate::theme::OPENCODE.accent),
            "selected approval button should follow the active theme globally"
        );
    }

    #[test]
    fn approval_panel_renders_inline() {
        let mut app = App::new();
        app.apply_event(AgentEvent::ApprovalRequested {
            name: "edit".into(),
            summary: "crates/evrax-tui/src/ui.rs".into(),
            key: "edit".into(),
        });
        let text = render_text_with(&mut app, 100, 30);
        assert!(
            text.contains("Permission required"),
            "header missing:\n{text}"
        );
        assert!(text.contains("edit"), "tool name missing:\n{text}");
        let rows: Vec<&str> = text.lines().collect();
        let option_row = rows
            .iter()
            .find(|row| row.contains("Allow once"))
            .expect("options row missing");
        assert!(
            option_row.contains("Always allow") && option_row.contains("Reject"),
            "options must sit in a single docked row:\n{option_row}"
        );
        assert!(
            option_row.contains("select") && option_row.contains("confirm"),
            "keyboard hints missing:\n{option_row}"
        );
        let header_row = rows
            .iter()
            .position(|row| row.contains("Permission required"))
            .expect("header row missing");
        assert!(
            header_row > rows.len() / 2,
            "panel should dock near the bottom (row {header_row} of {})",
            rows.len()
        );
    }

    #[test]
    fn session_dialog_lists_sessions() {
        let mut app = App::new();
        for c in "perbaiki streaming sse".chars() {
            app.input.insert_char(c);
        }
        app.submit_prompt();
        app.new_session();
        app.open_sessions();
        let text = render_text_with(&mut app, 100, 30);
        assert!(text.contains("sessions"), "modal title missing:\n{text}");
        assert!(
            text.contains("new session (current)"),
            "current session missing:\n{text}"
        );
        assert!(
            text.contains("perbaiki streaming sse"),
            "titled session missing:\n{text}"
        );
        assert!(
            text.contains("new session"),
            "new session row missing:\n{text}"
        );
    }

    #[test]
    fn transcript_cache_grows_from_empty_transcript() {
        let theme = crate::theme::index();
        let mut cache = TranscriptCache::default();
        let empty: [ChatEntry; 0] = [];
        cache.sync(&empty, 0, 0, 80, theme);
        let mut entries = vec![ChatEntry::User("hai bos".to_string())];
        let out = cache.sync(&entries, 0, 1, 80, theme).clone();
        assert_eq!(out, conversation_lines(&entries, 0, 80));
        assert_eq!(
            cache.rebuilds, 1,
            "growth from empty must extend, not rebuild"
        );
        assert_eq!(out.first().unwrap().width(), 0);
        entries.push(ChatEntry::Assistant("sip".to_string()));
        let out = cache.sync(&entries, 0, 2, 80, theme).clone();
        assert_eq!(out, conversation_lines(&entries, 0, 80));
        assert_eq!(cache.rebuilds, 1);
    }

    #[test]
    fn transcript_cache_matches_plain_render_and_reuses_prefix() {
        let entries = sample_conversation();
        let theme = crate::theme::index();
        let mut cache = TranscriptCache::default();
        let synced = cache.sync(&entries, 3, 0, 80, theme).clone();
        assert_eq!(synced, conversation_lines(&entries, 3, 80));
        assert_eq!(cache.rebuilds, 1);
        for line in &synced {
            assert!(line.width() <= 80, "line too wide: {line:?}");
        }
        cache.sync(&entries, 3, 0, 80, theme);
        assert_eq!(cache.rebuilds, 1, "same content must not rebuild");
        assert_eq!(
            cache.tail_rebuilds, 1,
            "same tick+rev must not rebuild the tail"
        );
        let mut grown = sample_conversation();
        grown.push(ChatEntry::Assistant("tambahan baru".to_string()));
        let out = cache.sync(&grown, 3, 1, 80, theme).clone();
        assert_eq!(cache.rebuilds, 1, "new entries extend the cache");
        assert_eq!(out, conversation_lines(&grown, 3, 80));
        let shrunk = &grown[..grown.len() - 1];
        cache.sync(shrunk, 3, 2, 80, theme);
        assert_eq!(cache.rebuilds, 2, "shrink (retention) forces a rebuild");
        cache.sync(shrunk, 3, 2, 40, theme);
        assert_eq!(cache.rebuilds, 3, "width change forces a rebuild");
        cache.sync(shrunk, 3, 2, 40, theme + 1);
        assert_eq!(cache.rebuilds, 4, "theme change forces a rebuild");
        cache.invalidate();
        cache.sync(shrunk, 3, 2, 40, theme + 1);
        assert_eq!(cache.rebuilds, 5, "invalidate forces a rebuild");
    }

    #[test]
    fn transcript_cache_rebuilds_only_streaming_tail() {
        let theme = crate::theme::index();
        let mut cache = TranscriptCache::default();
        let mut entries = vec![
            ChatEntry::User("halo".to_string()),
            ChatEntry::Assistant("sep".to_string()),
        ];
        cache.sync(&entries, 1, 0, 80, theme);
        let after_prefix = cache.lines.len();
        entries[1] = ChatEntry::Assistant("kata ".repeat(60));
        let out = cache.sync(&entries, 1, 1, 80, theme).clone();
        assert_eq!(cache.rebuilds, 1, "tail edit must not rebuild the prefix");
        assert!(out.len() > after_prefix, "streaming must grow the tail");
        assert_eq!(out, conversation_lines(&entries, 1, 80));
        let same_len = out.len();
        let same = cache.sync(&entries, 1, 1, 80, theme).clone();
        assert_eq!(same.len(), same_len);
        assert_eq!(cache.tail_rebuilds, 2, "idle repaint reuses the tail");
    }

    #[test]
    fn scrollbar_shows_only_when_content_overflows() {
        let overflowing = render_sample(100, 14);
        assert!(
            overflowing.contains('█'),
            "scrollbar thumb missing:\n{overflowing}"
        );
        let mut app = App::new();
        app.messages.clear();
        app.messages.push(ChatEntry::User("hi".into()));
        let short = render_text_with(&mut app, 100, 24);
        assert!(!short.contains('█'), "scrollbar should hide:\n{short}");
    }

    #[test]
    fn footer_reflects_busy_state() {
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let mut app = App::new();
        app.busy = true;
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("working"), "busy footer missing:\n{text}");
        assert!(text.contains("cancel"), "cancel hint missing:\n{text}");
    }

    #[test]
    fn footer_shows_iteration_budget() {
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let mut app = App::new();
        app.busy = true;
        app.iteration = Some((3, 500));
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("iterasi 3/500"),
            "iteration status missing:\n{text}"
        );
    }

    #[test]
    fn scrolling_moves_viewport() {
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        let mut app = App::new();
        app.messages = sample_conversation();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let at_bottom = buffer_text(&terminal);

        app.scroll_view(5, true, u16::MAX);
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let scrolled = buffer_text(&terminal);
        assert_ne!(
            scrolled, at_bottom,
            "viewport should change when scrolled up"
        );

        app.scroll_view(5, false, u16::MAX);
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let back = buffer_text(&terminal);
        assert_eq!(back, at_bottom, "scrolling back should restore view");
    }

    #[test]
    fn sidebar_requires_maximized_window() {
        let mut app = App::new();
        app.messages.push(ChatEntry::User("hi".to_string()));
        app.maximized = Some(false);
        let windowed = render_text_with(&mut app, 140, 28);
        assert!(
            !windowed.contains("sessions"),
            "windowed should hide sidebar:\n{windowed}"
        );
        app.maximized = Some(true);
        let maximized = render_text_with(&mut app, 140, 28);
        assert!(
            maximized.contains("sessions"),
            "maximized should show sidebar:\n{maximized}"
        );
        let mut narrow = App::new();
        narrow.messages.push(ChatEntry::User("hi".to_string()));
        narrow.maximized = Some(true);
        let narrow_text = render_text_with(&mut narrow, 100, 28);
        assert!(
            !narrow_text.contains("sessions"),
            "narrow maximized should still hide sidebar:\n{narrow_text}"
        );
    }

    #[test]
    fn tick_advances_animation() {
        let mut app = App::new();
        assert_eq!(app.tick, 0);
        app.tick();
        assert_eq!(app.tick, 1);
    }

    #[test]
    fn input_scrolls_to_keep_cursor_visible() {
        let mut app = App::new();
        app.messages.clear();
        for c in "START".chars() {
            app.input.insert_char(c);
        }
        for _ in 0..300 {
            app.input.insert_char('x');
        }
        let text = render_text_with(&mut app, 20, 20);
        assert!(!text.contains("START"), "head should scroll out:\n{text}");
        assert!(text.contains("xxxx"), "tail should stay visible:\n{text}");
    }

    #[test]
    fn selection_extracts_visible_text_on_copy_request() {
        let mut app = App::new();
        app.messages.clear();
        app.messages.push(ChatEntry::User("hello world".into()));
        app.selection.begin((4, 2));
        app.selection.update((8, 2));
        app.selection.finish();
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let copied = app
            .copied_text
            .take()
            .expect("copy request should fill text");
        assert_eq!(copied, "hello");
        assert!(app.flash.is_some(), "flash should announce copy");
    }

    #[test]
    fn selection_ignores_sidebar_text() {
        let mut app = App::new();
        app.maximized = Some(true);
        app.messages.clear();
        app.messages.push(ChatEntry::User("hello world".into()));
        app.selection.begin((2, 2));
        app.selection.update((139, 2));
        app.selection.finish();
        let mut terminal = Terminal::new(TestBackend::new(140, 28)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let copied = app.copied_text.take().expect("copy text");
        assert!(
            copied.contains("hello world"),
            "chat text should be copied: {copied:?}"
        );
        assert!(
            !copied.contains("sessions"),
            "sidebar must stay out of selection: {copied:?}"
        );
    }

    #[test]
    fn selection_beyond_frame_edges_is_clamped() {
        let mut app = App::new();
        app.messages.clear();
        app.messages.push(ChatEntry::User("hello world".into()));
        app.selection.begin((2, 2));
        app.selection.update((500, 500));
        app.selection.finish();
        let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let copied = app.copied_text.take().expect("copy text");
        assert!(copied.contains("hello world"), "copied: {copied:?}");
    }

    #[test]
    fn assistant_text_wraps_with_hanging_indent() {
        let mut app = App::new();
        app.messages.clear();
        app.messages.push(ChatEntry::Assistant(
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi"
                .into(),
        ));
        let text = render_text_with(&mut app, 40, 24);
        let words = [
            "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa",
            "lambda", "mu", "nu", "xi", "omicron", "pi",
        ];
        let rows: Vec<&str> = text
            .lines()
            .filter(|row| words.iter().any(|w| row.contains(w)))
            .collect();
        assert!(rows.len() >= 2, "text should wrap across rows:\n{text}");
        for row in &rows {
            assert!(
                row.starts_with("   ") && !row.starts_with("    "),
                "wrapped row must keep the 3-col indent: {row:?}"
            );
        }
    }

    #[test]
    fn assistant_markdown_gets_styles() {
        let mut app = App::new();
        app.messages.clear();
        app.messages.push(ChatEntry::Assistant(
            "## Judul\n**bold** word and `code` span".into(),
        ));
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let find = |needle: &str| -> (u16, u16) {
            for y in 0..buffer.area.height {
                let row: String = (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect();
                if let Some(idx) = row.find(needle) {
                    return (idx as u16, y);
                }
            }
            panic!("{needle} not rendered");
        };
        let (hx, hy) = find("Judul");
        assert_eq!(
            buffer[(hx, hy)].fg,
            current().md_heading,
            "heading should use the heading token"
        );
        assert!(
            buffer[(hx, hy)].modifier.contains(Modifier::BOLD),
            "heading should be bold"
        );
        let heading_row: String = (0..buffer.area.width)
            .map(|x| buffer[(x, hy)].symbol())
            .collect();
        assert!(
            !heading_row.contains('#'),
            "heading marker should be concealed: {heading_row:?}"
        );
        let (bx, by) = find("bold");
        assert_eq!(
            buffer[(bx, by)].fg,
            current().md_strong,
            "strong should use the strong token"
        );
        assert!(
            buffer[(bx, by)].modifier.contains(Modifier::BOLD),
            "bold modifier missing"
        );
        let (cx, cy) = find("code");
        assert_eq!(
            buffer[(cx, cy)].fg,
            current().md_code,
            "inline code should use the code token"
        );
    }

    #[test]
    fn code_fence_renders_syntax_colors() {
        let mut app = App::new();
        app.messages.clear();
        app.messages.push(ChatEntry::Assistant(
            "```rust\nlet x = 1; // note\n```".into(),
        ));
        let mut terminal = Terminal::new(TestBackend::new(60, 10)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let find = |needle: &str| -> (u16, u16) {
            for y in 0..buffer.area.height {
                let row: String = (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect();
                if let Some(idx) = row.find(needle) {
                    return (idx as u16, y);
                }
            }
            panic!("{needle} not rendered");
        };
        let (kx, ky) = find("let");
        assert_eq!(
            buffer[(kx, ky)].fg,
            current().syn_keyword,
            "keyword should use the syntax token"
        );
        assert!(
            buffer[(kx, ky)].modifier.contains(Modifier::ITALIC),
            "rust keywords are italic in opencode"
        );
        let (nx, ny) = find("1;");
        assert_eq!(buffer[(nx, ny)].fg, current().syn_number);
        let (mx, my) = find("// note");
        assert_eq!(
            buffer[(mx, my)].fg,
            current().syn_comment,
            "comment should be muted italic"
        );
        assert_eq!(
            buffer[(kx, ky)].bg,
            current().base,
            "code sits on the base background, no band"
        );
    }

    #[test]
    fn composer_renders_multiple_lines() {
        let mut app = App::new();
        app.messages.clear();
        app.input.insert_str("first line");
        app.input.insert_newline();
        app.input.insert_str("second line");
        let text = render_text_with(&mut app, 80, 24);
        assert!(text.contains("first line"), "line 1 missing:\n{text}");
        assert!(text.contains("second line"), "line 2 missing:\n{text}");
    }

    #[test]
    fn metadata_line_reflects_mode() {
        let mut app = App::new();
        app.set_backend_label("bebas");
        app.maximized = Some(true);
        let build = render_text_with(&mut app, 100, 24);
        assert!(build.contains("Build"), "metadata missing build:\n{build}");
        assert!(
            build.contains("Build · bebas · max"),
            "metadata should show the real backend: {build}"
        );
        app.cycle_mode();
        let plan = render_text_with(&mut app, 100, 24);
        assert!(plan.contains("Plan"), "metadata missing plan:\n{plan}");
        assert!(
            !plan.contains("Build"),
            "build label should be gone:\n{plan}"
        );
        app.maximized = Some(false);
        let windowed = render_text_with(&mut app, 100, 24);
        assert!(
            windowed.contains("Plan · bebas · win"),
            "window state should show: {windowed}"
        );
    }

    #[test]
    fn diff_lines_get_opencode_backgrounds() {
        let mut app = App::new();
        app.messages.clear();
        app.messages.push(ChatEntry::Diff {
            path: "src/ui.rs".into(),
            lines: vec![
                DiffLine {
                    kind: DiffKind::Add,
                    text: "let x = 1;".into(),
                },
                DiffLine {
                    kind: DiffKind::Rem,
                    text: "let y = 2;".into(),
                },
                DiffLine {
                    kind: DiffKind::Ctx,
                    text: "let z = 3;".into(),
                },
            ],
        });
        let mut terminal = Terminal::new(TestBackend::new(60, 14)).unwrap();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let find = |needle: &str| -> (u16, u16) {
            for y in 0..buffer.area.height {
                let row: String = (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect();
                if let Some(idx) = row.find(needle) {
                    return (idx as u16, y);
                }
            }
            panic!("{needle} not rendered");
        };
        let (ax, ay) = find("let x = 1;");
        assert_eq!(
            buffer[(ax, ay)].bg,
            current().diff_add_bg,
            "add line content bg"
        );
        assert_eq!(
            buffer[(3, ay)].bg,
            current().diff_add_gutter,
            "add line gutter bg"
        );
        assert_eq!(
            buffer[(buffer.area.width - 1, ay)].bg,
            current().diff_add_bg,
            "add line band should span the full row"
        );
        let (rx, ry) = find("let y = 2;");
        assert_eq!(
            buffer[(rx, ry)].bg,
            current().diff_rem_bg,
            "rem line content bg"
        );
        assert_eq!(
            buffer[(3, ry)].bg,
            current().diff_rem_gutter,
            "rem line gutter bg"
        );
        let (cx, cy) = find("let z = 3;");
        assert_eq!(
            buffer[(cx, cy)].fg,
            current().diff_ctx_text,
            "ctx text color"
        );
        assert_eq!(
            buffer[(cx, cy)].bg,
            current().base,
            "ctx content keeps base bg"
        );
        assert_eq!(
            buffer[(3, cy)].bg,
            current().diff_ctx_gutter,
            "ctx gutter bg"
        );
        let (px, py) = find("src/ui.rs");
        assert_eq!(
            buffer[(px, py)].bg,
            current().diff_ctx_gutter,
            "path header band"
        );
    }

    #[test]
    fn shortens_long_paths() {
        assert_eq!(shorten_path("C:\\short", 20), "C:\\short");
        let long = "C:\\Users\\nzeex\\some\\very\\long\\project\\path";
        let shortened = shorten_path(long, 20);
        assert_eq!(shortened.chars().count(), 20);
        assert!(shortened.starts_with('…'));
    }
}
