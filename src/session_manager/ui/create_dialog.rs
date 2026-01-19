use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

pub struct CreateDialog {
    input: String,
}

impl CreateDialog {
    pub fn new() -> Self {
        Self {
            input: String::new(),
        }
    }

    pub fn clear(&mut self) {
        self.input.clear();
    }

    pub fn push(&mut self, c: char) {
        self.input.push(c);
    }

    pub fn pop(&mut self) -> Option<char> {
        self.input.pop()
    }

    pub fn take_input(&mut self) -> String {
        std::mem::take(&mut self.input)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let popup_width = 40u16;
        let popup_height = 5u16;

        let popup_x = (area.width.saturating_sub(popup_width)) / 2;
        let popup_y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);

        let block = Block::default()
            .title(" Create Session ")
            .borders(Borders::ALL)
            .style(Style::default().bg(Color::Black));

        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        let display_text = if self.input.is_empty() {
            Line::from(vec![
                Span::styled("Name: ", Style::default().fg(Color::Gray)),
                Span::styled("_", Style::default().fg(Color::Yellow)),
            ])
        } else {
            Line::from(vec![
                Span::styled("Name: ", Style::default().fg(Color::Gray)),
                Span::raw(&self.input),
                Span::styled("_", Style::default().fg(Color::Yellow)),
            ])
        };

        let paragraph = Paragraph::new(display_text);
        frame.render_widget(paragraph, inner);
    }
}

impl Default for CreateDialog {
    fn default() -> Self {
        Self::new()
    }
}
