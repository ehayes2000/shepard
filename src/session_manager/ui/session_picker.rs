use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, List, ListItem, ListState},
};

const MAX_VISIBLE: usize = 8;

pub struct SessionPicker {
    state: ListState,
}

impl SessionPicker {
    pub fn new() -> Self {
        Self {
            state: ListState::default(),
        }
    }

    pub fn select(&mut self, index: Option<usize>) {
        self.state.select(index);
    }

    pub fn selected(&self) -> Option<usize> {
        self.state.selected()
    }

    pub fn move_selection(&mut self, delta: i32, count: usize) {
        if count == 0 {
            return;
        }
        let current = self.state.selected().unwrap_or(0) as i32;
        let next = (current + delta).rem_euclid(count as i32) as usize;
        self.state.select(Some(next));
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, session_names: &[String]) {
        if session_names.is_empty() {
            return;
        }

        let has_overflow = session_names.len() > MAX_VISIBLE;
        let visible_count = session_names.len().min(MAX_VISIBLE);

        let max_name_len = session_names.iter().map(|s| s.len()).max().unwrap_or(10);
        let popup_width = (max_name_len as u16 + 6)
            .max(24)
            .min(area.width.saturating_sub(4));
        let popup_height = (visible_count as u16 + 2).min(area.height.saturating_sub(2));

        let popup_x = (area.width.saturating_sub(popup_width)) / 2;
        let popup_y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);

        let items: Vec<ListItem> = session_names
            .iter()
            .map(|name| ListItem::new(name.as_str()))
            .collect();

        let title = if has_overflow {
            " Select Session (...) "
        } else {
            " Select Session "
        };

        let list = List::new(items)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .style(Style::default().bg(Color::Black)),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");

        frame.render_stateful_widget(list, popup_area, &mut self.state);
    }
}

impl Default for SessionPicker {
    fn default() -> Self {
        Self::new()
    }
}
