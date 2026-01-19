use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem},
};

pub struct CommandMenu;

impl CommandMenu {
    pub fn new() -> Self {
        Self
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let popup_width = 30u16;
        let popup_height = 6u16;

        let popup_x = (area.width.saturating_sub(popup_width)) / 2;
        let popup_y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);

        let items = vec![
            ListItem::new(Line::from(vec![
                Span::styled(
                    "c",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  Create session"),
            ])),
            ListItem::new(Line::from(vec![
                Span::styled(
                    "s",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  Select session"),
            ])),
        ];

        let list = List::new(items).block(
            Block::default()
                .title(" Command Menu ")
                .borders(Borders::ALL)
                .style(Style::default().bg(Color::Black)),
        );

        frame.render_widget(list, popup_area);
    }
}

impl Default for CommandMenu {
    fn default() -> Self {
        Self::new()
    }
}
