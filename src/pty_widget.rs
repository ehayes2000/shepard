use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Widget,
};
use vt100::Screen;

/// A widget that renders a vt100 terminal screen
pub struct PtyWidget<'a> {
    screen: &'a Screen,
    dimmed: bool,
}

impl<'a> PtyWidget<'a> {
    pub fn new(screen: &'a Screen) -> Self {
        Self { screen, dimmed: false }
    }

    pub fn dimmed(mut self, dimmed: bool) -> Self {
        self.dimmed = dimmed;
        self
    }
}

impl Widget for PtyWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Render within bounds - use minimum of screen size and area size
        let (screen_rows, screen_cols) = self.screen.size();
        let rows = area.height.min(screen_rows);
        let cols = area.width.min(screen_cols);
        for row in 0..rows {
            for col in 0..cols {
                if let Some(cell) = self.screen.cell(row, col) {
                    let mut style = vt100_to_ratatui_style(cell);
                    if self.dimmed {
                        style = style.add_modifier(Modifier::DIM);
                    }
                    let x = area.x + col;
                    let y = area.y + row;

                    if x < buf.area.width && y < buf.area.height {
                        let contents = cell.contents();
                        if contents.is_empty() {
                            buf[(x, y)].set_char(' ').set_style(style);
                        } else {
                            buf.set_string(x, y, contents, style);
                        }
                    }
                }
            }
        }
    }
}

fn vt100_to_ratatui_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();

    style = style.fg(vt100_color_to_ratatui(cell.fgcolor()));
    style = style.bg(vt100_color_to_ratatui(cell.bgcolor()));

    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }

    style
}

fn vt100_color_to_ratatui(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(idx) => Color::Indexed(idx),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
