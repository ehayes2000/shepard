use std::io::{self, Write};

use anyhow::Result;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor};
use crossterm::ExecutableCommand;

use crate::app::{App, InputMode};

pub fn draw_overlay(stdout: &mut io::Stdout, app: &App) -> Result<()> {
    let (cols, rows) = app.terminal_size;

    let overlay_width = 50.min(cols.saturating_sub(4)) as usize;
    let overlay_height = (app.sessions.len() + 6).min(rows.saturating_sub(4) as usize);
    let start_x = ((cols as usize).saturating_sub(overlay_width)) / 2;
    let start_y = ((rows as usize).saturating_sub(overlay_height)) / 2;

    stdout.execute(Hide)?;

    // Top border
    stdout.execute(MoveTo(start_x as u16, start_y as u16))?;
    stdout.execute(SetBackgroundColor(Color::Black))?;
    stdout.execute(SetForegroundColor(Color::Cyan))?;
    stdout.execute(Print(format!(
        "┌{:─<width$}┐",
        "",
        width = overlay_width - 2
    )))?;

    // Title
    stdout.execute(MoveTo(start_x as u16, (start_y + 1) as u16))?;
    let title = " Shepard ";
    let padding = (overlay_width - 2 - title.len()) / 2;
    stdout.execute(Print(format!(
        "│{:padding$}{}{:rest$}│",
        "",
        title,
        "",
        padding = padding,
        rest = overlay_width - 2 - padding - title.len()
    )))?;

    // Separator
    stdout.execute(MoveTo(start_x as u16, (start_y + 2) as u16))?;
    stdout.execute(Print(format!(
        "├{:─<width$}┤",
        "",
        width = overlay_width - 2
    )))?;

    // Sessions
    let session_area_start = start_y + 3;
    for (i, session) in app.sessions.iter().enumerate() {
        stdout.execute(MoveTo(start_x as u16, (session_area_start + i) as u16))?;

        let marker = if Some(i) == app.active_session { "*" } else { " " };
        let selected = if i == app.selected_index { ">" } else { " " };
        let status = if session.alive { "●" } else { "○" };

        if i == app.selected_index {
            stdout.execute(SetBackgroundColor(Color::DarkGrey))?;
        } else {
            stdout.execute(SetBackgroundColor(Color::Black))?;
        }

        if session.alive {
            stdout.execute(SetForegroundColor(Color::Green))?;
        } else {
            stdout.execute(SetForegroundColor(Color::Red))?;
        }

        let content = format!("{}{} {} {}", selected, marker, status, session.name);
        let padded = format!("│ {:<width$} │", content, width = overlay_width - 4);
        stdout.execute(Print(&padded[..padded.len().min(overlay_width + 2)]))?;
    }

    // Empty message if no sessions
    if app.sessions.is_empty() {
        stdout.execute(MoveTo(start_x as u16, session_area_start as u16))?;
        stdout.execute(SetBackgroundColor(Color::Black))?;
        stdout.execute(SetForegroundColor(Color::DarkGrey))?;
        let msg = "(no sessions - press 'n')";
        let padded = format!("│ {:^width$} │", msg, width = overlay_width - 4);
        stdout.execute(Print(padded))?;
    }

    // Separator before status
    let status_y = session_area_start + app.sessions.len().max(1);
    stdout.execute(MoveTo(start_x as u16, status_y as u16))?;
    stdout.execute(SetForegroundColor(Color::Cyan))?;
    stdout.execute(Print(format!(
        "├{:─<width$}┤",
        "",
        width = overlay_width - 2
    )))?;

    // Status line
    stdout.execute(MoveTo(start_x as u16, (status_y + 1) as u16))?;
    stdout.execute(SetForegroundColor(Color::White))?;
    let status = match app.input_mode {
        InputMode::Normal => app.status_message.clone(),
        InputMode::CreatingSession => format!("Branch: {}█", app.input_buffer),
    };
    let status_display: String = status.chars().take(overlay_width - 4).collect();
    let padded = format!("│ {:<width$} │", status_display, width = overlay_width - 4);
    stdout.execute(Print(padded))?;

    // Bottom border
    stdout.execute(MoveTo(start_x as u16, (status_y + 2) as u16))?;
    stdout.execute(SetForegroundColor(Color::Cyan))?;
    stdout.execute(Print(format!(
        "└{:─<width$}┘",
        "",
        width = overlay_width - 2
    )))?;

    stdout.execute(ResetColor)?;
    stdout.execute(Show)?;
    stdout.flush()?;

    Ok(())
}
