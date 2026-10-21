use std::f32::consts::TAU;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::theme::current;

const WIDE: &str = include_str!("banner_wide.txt");
const COMPACT: &str = include_str!("banner_compact.txt");

const BODY: Color = Color::Rgb(0x22, 0x17, 0x32);
const MID: Color = Color::Rgb(0x4e, 0x37, 0x68);
const RIM: Color = Color::Rgb(0x96, 0x60, 0xff);
const FACE: Color = Color::Rgb(0xfa, 0xf6, 0xee);

struct Art {
    rows: Vec<&'static [u8]>,
    width: u16,
    height: u16,
}

impl Art {
    fn parse(src: &'static str) -> Self {
        let rows: Vec<&'static [u8]> = src.lines().map(str::as_bytes).collect();
        let width = rows.iter().map(|row| row.len()).max().unwrap_or(0) as u16 + 2;
        let height = (rows.len() as u16).div_ceil(2);
        Self {
            rows,
            width,
            height,
        }
    }

    fn pixel(&self, x: u16, y: u16, blink: bool) -> Option<Color> {
        let gx = x.checked_sub(1)?;
        let row = self.rows.get(y as usize)?;
        let byte = *row.get(gx as usize)?;
        if blink && byte == b'W' && is_eye(gx, y) {
            return Some(BODY);
        }
        Some(match byte {
            b'W' => FACE,
            b'R' => RIM,
            b'M' => MID,
            b'B' => BODY,
            _ => return None,
        })
    }
}

fn is_eye(x: u16, y: u16) -> bool {
    ((10..=11).contains(&x) && (15..=16).contains(&y))
        || ((17..=19).contains(&x) && (13..=14).contains(&y))
}

pub fn render(frame: &mut Frame, area: Rect, tick: u64) {
    if area.width < 24 || area.height < 6 {
        return;
    }
    let wide = Art::parse(WIDE);
    let compact = Art::parse(COMPACT);
    let art = if area.width >= wide.width && area.height >= wide.height + 3 {
        wide
    } else if area.width >= compact.width && area.height >= compact.height + 3 {
        compact
    } else {
        plain(frame, area);
        return;
    };
    let total = art.height + 3;
    let left = area.left() + (area.width - art.width) / 2;
    let top = area.top() + (area.height - total) / 2;
    let bob = u16::from((tick / 12) % 2 == 1 && top > area.top());
    let art_top = top.saturating_sub(bob);

    draw_glow(frame, Rect::new(left, art_top, art.width, art.height), tick);
    draw_art(frame, &art, left, art_top, tick, area);
    draw_text(frame, area, top + art.height + 1);
    draw_sparks(frame, area, left, art_top, art.width, art.height, tick);
}

fn draw_glow(frame: &mut Frame, rect: Rect, tick: u64) {
    let base = current().base;
    let accent = current().accent;
    let pulse = 0.5 + 0.5 * ((tick % 32) as f32 / 32.0 * TAU).sin();
    let strength = 0.12 + 0.08 * pulse;
    let buf = frame.buffer_mut();
    for y in 0..rect.height {
        for x in 0..rect.width {
            let nx = (x as f32 + 0.5 - rect.width as f32 / 2.0) / (rect.width as f32 * 0.56);
            let ny = (y as f32 + 0.5 - rect.height as f32 * 0.52) / (rect.height as f32 * 0.58);
            let distance = (nx * nx + ny * ny).sqrt().min(1.0);
            let weight = (1.0 - distance).powf(1.7) * strength;
            buf[(rect.x + x, rect.y + y)].set_bg(mix(base, accent, weight));
        }
    }
}

fn draw_art(frame: &mut Frame, art: &Art, left: u16, top: u16, tick: u64, area: Rect) {
    let blink = art.width >= 46 && tick % 20 < 2;
    let buf = frame.buffer_mut();
    for row in 0..art.height {
        for col in 0..art.width {
            let x = left + col;
            let y = top + row;
            if x < area.left() || x >= area.right() || y < area.top() || y >= area.bottom() {
                continue;
            }
            let upper = art.pixel(col, row * 2, blink);
            let lower = art.pixel(col, row * 2 + 1, blink);
            let cell = &mut buf[(x, y)];
            match (upper, lower) {
                (None, None) => {}
                (Some(color), None) => {
                    cell.set_symbol("▀");
                    cell.set_fg(color);
                }
                (None, Some(color)) => {
                    cell.set_symbol("▄");
                    cell.set_fg(color);
                }
                (Some(fg), Some(bg)) => {
                    cell.set_symbol("▀");
                    cell.set_fg(fg);
                    cell.set_bg(bg);
                }
            }
        }
    }
}

fn draw_text(frame: &mut Frame, area: Rect, y: u16) {
    if y >= area.bottom() {
        return;
    }
    let accent = current().accent;
    let interactive = current().interactive;
    let mut spans = Vec::new();
    for (index, letter) in ['E', 'V', 'R', 'A', 'X'].into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            letter.to_string(),
            Style::default()
                .fg(mix(accent, interactive, index as f32 / 4.0))
                .add_modifier(Modifier::BOLD),
        ));
    }
    let line = Line::from(spans);
    let rect = Rect::new(area.left(), y, area.width, 1);
    frame.render_widget(
        Paragraph::new(line)
            .alignment(Alignment::Center)
            .style(Style::default().bg(current().base)),
        rect,
    );
    if y + 1 < area.bottom() {
        let line = Line::from(vec![
            Span::styled("FAUST", Style::default().fg(current().muted)),
            Span::styled(
                "55",
                Style::default()
                    .fg(interactive)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        let rect = Rect::new(area.left(), y + 1, area.width, 1);
        frame.render_widget(
            Paragraph::new(line)
                .alignment(Alignment::Center)
                .style(Style::default().bg(current().base)),
            rect,
        );
    }
}

fn draw_sparks(
    frame: &mut Frame,
    area: Rect,
    left: u16,
    top: u16,
    width: u16,
    height: u16,
    tick: u64,
) {
    let accent = current().accent;
    let interactive = current().interactive;
    let phase = (tick / 4) % 4;
    let sparks = [
        (left.saturating_sub(2), top + 4, accent),
        (left + width + 1, top + 7, interactive),
        (
            left.saturating_sub(1),
            top + height.saturating_sub(4),
            interactive,
        ),
        (left + width, top + height.saturating_sub(3), accent),
    ];
    let buf = frame.buffer_mut();
    for (index, (x, y, color)) in sparks.into_iter().enumerate() {
        if (phase as usize + index).is_multiple_of(3) {
            continue;
        }
        if x < area.left() || x >= area.right() || y < area.top() || y >= area.bottom() {
            continue;
        }
        buf[(x, y)]
            .set_symbol("✦")
            .set_fg(color)
            .set_bg(current().base);
    }
}

fn plain(frame: &mut Frame, area: Rect) {
    if area.height < 2 || area.width < 12 {
        return;
    }
    let lines = vec![
        Line::from(Span::styled(
            "E V R A X",
            Style::default()
                .fg(current().accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("FAUST", Style::default().fg(current().muted)),
            Span::styled("55", Style::default().fg(current().interactive)),
        ]),
    ];
    let rect = Rect::new(area.left(), area.top(), area.width, 2);
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .style(Style::default().bg(current().base)),
        rect,
    );
}

fn mix(left: Color, right: Color, amount: f32) -> Color {
    let amount = amount.clamp(0.0, 1.0);
    let (lr, lg, lb) = rgb(left);
    let (rr, rg, rb) = rgb(right);
    Color::Rgb(
        (lr as f32 + (rr as f32 - lr as f32) * amount) as u8,
        (lg as f32 + (rg as f32 - lg as f32) * amount) as u8,
        (lb as f32 + (rb as f32 - lb as f32) * amount) as u8,
    )
}

fn rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0xaa, 0xaa, 0xaa),
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::render;

    fn screen(width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), 0))
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
    fn wide_banner_has_wordmark_and_signature() {
        let text = screen(100, 40);
        assert!(text.contains("E V R A X"), "wordmark missing:\n{text}");
        assert!(text.contains("FAUST55"), "signature missing:\n{text}");
    }

    #[test]
    fn small_terminal_degrades_to_wordmark() {
        let text = screen(40, 8);
        assert!(text.contains("E V R A X"), "fallback missing:\n{text}");
        assert!(
            text.contains("FAUST55"),
            "fallback signature missing:\n{text}"
        );
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        let _ = screen(20, 4);
    }
}
