use std::path::Path;
use std::sync::Arc;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders},
};
use vt100::Screen;

use crate::pty_widget::PtyWidget;
use super::super::session_pair::SessionView;

pub struct MainView;

impl MainView {
    pub fn new() -> Self {
        Self
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        frame: &mut Frame,
        screen: Option<&Arc<Screen>>,
        active_name: Option<&str>,
        active_path: Option<&Path>,
        active_view: SessionView,
        background_count: usize,
        bottom_left: Line<'static>,
        bottom_center: Option<Line<'static>>,
    ) -> Rect {
        let area = frame.area();

        let top_title = match active_name {
            Some(name) => {
                let view_indicator = match active_view {
                    SessionView::Claude => "",
                    SessionView::Shell => " [shell]",
                };
                format!(" {}{} ", name, view_indicator)
            }
            None => " No Session ".to_string(),
        };

        let total_sessions = background_count + if active_name.is_some() { 1 } else { 0 };
        let session_count_text = if total_sessions > 1 {
            format!(" {} Sessions ", total_sessions)
        } else {
            String::new()
        };

        let bottom_right = active_path
            .map(|p| format!(" {} ", path_relative_to_home(p)))
            .unwrap_or_default();

        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::White))
            .title(Line::from(top_title).left_aligned());

        // Bottom left: hotkeys
        block = block.title_bottom(bottom_left.left_aligned());

        // Bottom center: status message (if any)
        if let Some(center) = bottom_center {
            block = block.title_bottom(center.centered());
        }

        // Bottom right: session count + path
        let right_text = if session_count_text.is_empty() {
            bottom_right
        } else if bottom_right.is_empty() {
            session_count_text
        } else {
            format!("{} │{}", session_count_text, bottom_right)
        };
        if !right_text.is_empty() {
            block = block.title_bottom(Line::from(right_text).right_aligned());
        }

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if let Some(screen) = screen {
            let widget = PtyWidget::new(screen.as_ref());
            frame.render_widget(widget, inner);
        }

        inner
    }
}

impl Default for MainView {
    fn default() -> Self {
        Self::new()
    }
}

fn path_relative_to_home(path: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(relative) = path.strip_prefix(&home) {
            return format!("~/{}", relative.display());
        }
    path.display().to_string()
}
