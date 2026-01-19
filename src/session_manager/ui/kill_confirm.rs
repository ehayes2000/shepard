use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

pub struct KillConfirmDialog {
    session_name: String,
}

impl KillConfirmDialog {
    pub fn new() -> Self {
        Self {
            session_name: String::new(),
        }
    }

    pub fn set_session_name(&mut self, name: &str) {
        self.session_name = name.to_string();
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let lines = vec![
            Line::from(format!("Kill session '{}'?", self.session_name)),
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    "y",
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" - Yes, kill it"),
            ]),
            Line::from(vec![
                Span::styled(
                    "n",
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" / "),
                Span::styled(
                    "Esc",
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" - Cancel"),
            ]),
        ];

        let max_line_len = lines
            .iter()
            .map(|l| l.width())
            .max()
            .unwrap_or(20);

        let popup_width = (max_line_len as u16 + 4).min(area.width.saturating_sub(4));
        let popup_height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));

        let popup_x = (area.width.saturating_sub(popup_width)) / 2;
        let popup_y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);

        let paragraph = Paragraph::new(lines).block(
            Block::default()
                .title(" Kill Session ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::White))
                .style(Style::default().bg(Color::Black)),
        );

        frame.render_widget(paragraph, popup_area);
    }
}

impl Default for KillConfirmDialog {
    fn default() -> Self {
        Self::new()
    }
}
