use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph};
use unicode_width::UnicodeWidthChar;

use crate::app::App;
use crate::theme::current;

const LEFT_WIDTH: u16 = 26;
const RIGHT_WIDTH: u16 = 26;
const PANEL_MIN_WIDTH: u16 = 88;
const PANEL_MIN_HEIGHT: u16 = 18;
const SKILL_STEP_TICKS: u64 = 6;
const MCP_STEP_TICKS: u64 = 8;
const TOOL_STEP_TICKS: u64 = 10;

struct Row<'a> {
    glyph: &'static str,
    name: &'a str,
    glyph_color: Color,
    text_color: Color,
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    if area.width < PANEL_MIN_WIDTH || area.height < PANEL_MIN_HEIGHT {
        crate::banner::render(frame, area, app.tick);
        return;
    }
    let columns = Layout::horizontal([
        Constraint::Length(LEFT_WIDTH),
        Constraint::Min(0),
        Constraint::Length(RIGHT_WIDTH),
    ])
    .split(area);
    skills_panel(frame, columns[0], app);
    crate::banner::render(frame, columns[1], app.tick);
    right_panel(frame, columns[2], app);
}

fn skills_panel(frame: &mut Frame, area: Rect, app: &App) {
    let rows: Vec<Row<'_>> = app
        .catalog
        .skills
        .iter()
        .map(|skill| Row {
            glyph: "▸",
            name: &skill.name,
            glyph_color: current().interactive,
            text_color: current().text,
        })
        .collect();
    list_panel(
        frame,
        area,
        format!(" skills {} ", app.catalog.skills.len()),
        rows,
        app.tick,
        SKILL_STEP_TICKS,
        0,
    );
}

fn right_panel(frame: &mut Frame, area: Rect, app: &App) {
    let mcp_height = (app.catalog.mcps.len().clamp(1, 4) + 2) as u16;
    let rows = Layout::vertical([Constraint::Length(mcp_height), Constraint::Min(0)]).split(area);
    mcp_panel(frame, rows[0], app);
    tools_panel(frame, rows[1], app);
}

fn mcp_panel(frame: &mut Frame, area: Rect, app: &App) {
    let rows: Vec<Row<'_>> = app
        .catalog
        .mcps
        .iter()
        .map(|server| Row {
            glyph: if server.enabled { "●" } else { "○" },
            name: &server.name,
            glyph_color: if server.enabled {
                current().success
            } else {
                current().muted
            },
            text_color: if server.enabled {
                current().text
            } else {
                current().muted
            },
        })
        .collect();
    list_panel(
        frame,
        area,
        format!(" mcp {} ", app.catalog.mcps.len()),
        rows,
        app.tick,
        MCP_STEP_TICKS,
        1,
    );
}

fn tools_panel(frame: &mut Frame, area: Rect, app: &App) {
    let rows: Vec<Row<'_>> = app
        .catalog
        .tools
        .iter()
        .map(|tool| Row {
            glyph: "▸",
            name: &tool.name,
            glyph_color: match tool.risk {
                evrax_agent::tools::Risk::Read => current().success,
                evrax_agent::tools::Risk::Mutate => current().interactive,
            },
            text_color: current().text,
        })
        .collect();
    list_panel(
        frame,
        area,
        format!(" tools {} ", app.catalog.tools.len()),
        rows,
        app.tick,
        TOOL_STEP_TICKS,
        2,
    );
}

fn list_panel(
    frame: &mut Frame,
    area: Rect,
    title: String,
    rows: Vec<Row<'_>>,
    tick: u64,
    step_ticks: u64,
    phase: u64,
) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(current().border))
        .title(Span::styled(title, Style::default().fg(current().muted)))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if rows.is_empty() {
        let empty = Row {
            glyph: "Â·",
            name: "none configured",
            glyph_color: current().muted,
            text_color: current().muted,
        };
        draw_rows(frame, inner, &[empty], tick, step_ticks, phase);
        return;
    }
    draw_rows(frame, inner, &rows, tick, step_ticks, phase);
}

fn draw_rows(
    frame: &mut Frame,
    area: Rect,
    rows: &[Row<'_>],
    tick: u64,
    step_ticks: u64,
    phase: u64,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let visible = visible_rows(rows, area.height as usize, tick, step_ticks, phase);
    let name_width = area.width.saturating_sub(2) as usize;
    let lines: Vec<Line<'static>> = visible
        .into_iter()
        .map(|row| {
            Line::from(vec![
                Span::styled(
                    format!("{} ", row.glyph),
                    Style::default().fg(row.glyph_color),
                ),
                Span::styled(
                    fit(row.name, name_width),
                    Style::default().fg(row.text_color),
                ),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn visible_rows<'a>(
    rows: &'a [Row<'a>],
    height: usize,
    tick: u64,
    step_ticks: u64,
    phase: u64,
) -> Vec<&'a Row<'a>> {
    if rows.is_empty() || height == 0 {
        return Vec::new();
    }
    if rows.len() <= height {
        return rows.iter().collect();
    }
    let offset = scroll_offset(rows.len(), height, tick, step_ticks, phase);
    (0..height)
        .map(|index| &rows[(offset + index) % rows.len()])
        .collect()
}

fn scroll_offset(len: usize, height: usize, tick: u64, step_ticks: u64, phase: u64) -> usize {
    if len <= height {
        return 0;
    }
    (((tick / step_ticks.max(1)) as usize) + phase as usize) % len
}

fn fit(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if used + ch_width > width {
            if used < width {
                out.push('…');
            }
            break;
        }
        out.push(ch);
        used += ch_width;
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use evrax_agent::catalog::{McpInfo, SkillInfo};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn skill(index: usize) -> SkillInfo {
        SkillInfo {
            name: format!("skill-{index:02}"),
            description: String::new(),
            path: PathBuf::new(),
            content: String::new(),
        }
    }

    fn app() -> App {
        let mut app = App::new();
        app.catalog.skills = (0..30).map(skill).collect();
        app.catalog.mcps = vec![
            McpInfo {
                name: "serena".into(),
                enabled: true,
                kind: "local".into(),
            },
            McpInfo {
                name: "memory".into(),
                enabled: false,
                kind: "local".into(),
            },
        ];
        app
    }

    fn screen(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..height {
            for x in 0..width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn empty_state_lists_skills_mcps_and_tools() {
        let text = screen(&app(), 120, 40);
        assert!(text.contains("skills 30"), "skills count missing:\n{text}");
        assert!(text.contains("mcp 2"), "mcp count missing:\n{text}");
        assert!(text.contains("tools 13"), "tools count missing:\n{text}");
        assert!(text.contains("serena"), "mcp name missing:\n{text}");
        assert!(text.contains("read"), "tool name missing:\n{text}");
        assert!(text.contains("FAUST55"), "signature missing:\n{text}");
    }

    #[test]
    fn narrow_empty_state_keeps_only_the_banner() {
        let text = screen(&app(), 70, 30);
        assert!(text.contains("E V R A X"), "banner missing:\n{text}");
        assert!(!text.contains("skills 30"), "panels should hide:\n{text}");
    }

    #[test]
    fn slow_scroll_reaches_the_next_item() {
        assert_eq!(scroll_offset(10, 4, 0, 6, 0), 0);
        assert_eq!(scroll_offset(10, 4, 6, 6, 0), 1);
        assert_eq!(scroll_offset(10, 4, 60, 6, 0), 0);
        assert_eq!(scroll_offset(4, 4, 60, 6, 0), 0);
    }
}
