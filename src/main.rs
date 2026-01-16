mod app;
mod input;
mod session;
mod ui;

use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::cursor::MoveTo;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    self, disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;

use app::{App, InputMode};
use input::{key_to_bytes, TOGGLE_KEY};
use ui::draw_overlay;

fn main() -> Result<()> {
    // Set up panic hook to restore terminal on panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // Restore terminal
        let _ = disable_raw_mode();
        let _ = io::stdout().execute(LeaveAlternateScreen);
        original_hook(panic_info);
    }));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;

    let size = terminal::size()?;
    let mut app = App::new(size)?;

    let result = run_app(&mut app, &mut stdout);

    disable_raw_mode()?;
    stdout.execute(LeaveAlternateScreen)?;

    result
}

fn run_app(app: &mut App, stdout: &mut io::Stdout) -> Result<()> {
    loop {
        // Drain PTY output for all sessions and render active session
        let mut should_render = app.needs_full_render;
        app.needs_full_render = false;

        for (i, session) in app.sessions.iter_mut().enumerate() {
            let had_data = session.drain_output();
            if !app.show_overlay && app.active_session == Some(i) && had_data {
                should_render = true;
            }
        }

        if should_render && !app.show_overlay {
            if let Some(idx) = app.active_session {
                app.sessions[idx].render_screen(stdout)?;
            }
        }

        if app.show_overlay {
            draw_overlay(stdout, app)?;
        }

        if event::poll(Duration::from_millis(10))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    // Cmd+B toggles overlay
                    if key.code == KeyCode::Char(TOGGLE_KEY)
                        && key.modifiers.contains(KeyModifiers::SUPER)
                    {
                        app.show_overlay = !app.show_overlay;
                        if !app.show_overlay {
                            if let Some(idx) = app.active_session {
                                app.switch_to_session(idx, stdout)?;
                            }
                        }
                        continue;
                    }

                    if app.show_overlay {
                        handle_overlay_input(app, key.code, key.modifiers, stdout)?;
                    } else if let Some(idx) = app.active_session {
                        let bytes = key_to_bytes(key.code, key.modifiers);
                        if !bytes.is_empty() {
                            app.sessions[idx].write_input(&bytes);
                        }
                    }
                }
                Event::Resize(cols, rows) => {
                    app.terminal_size = (cols, rows);
                    if let Some(idx) = app.active_session {
                        app.sessions[idx].resize(cols, rows);
                    }
                }
                Event::Paste(text) => {
                    if !app.show_overlay {
                        if let Some(idx) = app.active_session {
                            app.sessions[idx].write_input(text.as_bytes());
                        }
                    }
                }
                _ => {}
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn handle_overlay_input(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    stdout: &mut io::Stdout,
) -> Result<()> {
    match app.input_mode {
        InputMode::Normal => match code {
            KeyCode::Char('q') => app.should_quit = true,
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                app.should_quit = true;
            }
            KeyCode::Char('n') => {
                app.input_mode = InputMode::CreatingSession;
                app.input_buffer.clear();
                app.status_message = "Enter branch/session name:".to_string();
            }
            KeyCode::Char('d') => {
                if !app.sessions.is_empty() {
                    app.delete_session(app.selected_index);
                }
            }
            KeyCode::Enter => {
                if !app.sessions.is_empty() {
                    let idx = app.selected_index;
                    app.switch_to_session(idx, stdout)?;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => app.select_next(),
            KeyCode::Up | KeyCode::Char('k') => app.select_previous(),
            KeyCode::Esc => {
                if let Some(idx) = app.active_session {
                    app.switch_to_session(idx, stdout)?;
                }
            }
            _ => {}
        },
        InputMode::CreatingSession => match code {
            KeyCode::Enter => {
                if !app.input_buffer.is_empty() {
                    let name = app.input_buffer.clone();
                    app.input_buffer.clear();
                    app.input_mode = InputMode::Normal;
                    app.create_session(name)?;
                    if !app.show_overlay {
                        stdout.execute(Clear(ClearType::All))?;
                        stdout.execute(MoveTo(0, 0))?;
                    }
                }
            }
            KeyCode::Esc => {
                app.input_mode = InputMode::Normal;
                app.input_buffer.clear();
                app.status_message =
                    "n: new | Enter: switch | d: delete | Cmd+b: toggle | q: quit".to_string();
            }
            KeyCode::Backspace => {
                app.input_buffer.pop();
            }
            KeyCode::Char(c) => {
                app.input_buffer.push(c);
            }
            _ => {}
        },
    }
    Ok(())
}
