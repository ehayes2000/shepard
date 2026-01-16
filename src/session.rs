use std::io::{Read, Write as IoWrite};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;

use anyhow::{Context, Result};
use crossterm::cursor::MoveTo;
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::QueueableCommand;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tattoy_wezterm_term::color::{ColorAttribute, ColorPalette};
use tattoy_wezterm_term::{Intensity, Terminal, TerminalConfiguration, TerminalSize, Underline};

#[derive(Debug)]
struct TermConfig {
    scrollback_size: usize,
}

impl TermConfig {
    fn new() -> Self {
        Self {
            scrollback_size: 10000,
        }
    }
}

impl TerminalConfiguration for TermConfig {
    fn scrollback_size(&self) -> usize {
        self.scrollback_size
    }

    fn color_palette(&self) -> ColorPalette {
        ColorPalette::default()
    }
}

pub struct Session {
    pub name: String,
    pub worktree_path: PathBuf,
    pub alive: bool,
    pty_master: Box<dyn portable_pty::MasterPty + Send>,
    pty_writer: Box<dyn IoWrite + Send>,
    output_receiver: Receiver<Vec<u8>>,
    terminal: Terminal,
}

impl Session {
    pub fn new(name: String, worktree_path: PathBuf, terminal_size: (u16, u16)) -> Result<Self> {
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows: terminal_size.1,
                cols: terminal_size.0,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY")?;

        let mut cmd = CommandBuilder::new("claude");
        cmd.cwd(&worktree_path);
        for (key, value) in std::env::vars() {
            cmd.env(key, value);
        }

        pty_pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn claude")?;

        let mut reader = pty_pair.master.try_clone_reader().unwrap();
        let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = mpsc::channel();

        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let writer = pty_pair.master.take_writer().unwrap();

        let term_size = TerminalSize {
            rows: terminal_size.1 as usize,
            cols: terminal_size.0 as usize,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 96,
        };

        let config = Arc::new(TermConfig::new());
        let terminal = Terminal::new(term_size, config, "shepard", "1.0", Box::new(Vec::new()));

        Ok(Self {
            name,
            worktree_path,
            alive: true,
            pty_master: pty_pair.master,
            pty_writer: writer,
            output_receiver: rx,
            terminal,
        })
    }

    pub fn write_input(&mut self, data: &[u8]) {
        let _ = self.pty_writer.write_all(data);
        let _ = self.pty_writer.flush();
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let _ = self.pty_master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });

        self.terminal.resize(TerminalSize {
            rows: rows as usize,
            cols: cols as usize,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 96,
        });
    }

    pub fn drain_output(&mut self) -> bool {
        let mut had_data = false;
        loop {
            match self.output_receiver.try_recv() {
                Ok(data) => {
                    had_data = true;
                    self.terminal.advance_bytes(data);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.alive = false;
                    break;
                }
            }
        }
        had_data
    }

    pub fn render_screen(&mut self, stdout: &mut impl IoWrite) -> Result<()> {
        // Make sure we've processed all pending data before getting cursor position
        self.drain_output();

        let screen = self.terminal.screen();
        let rows = screen.physical_rows;

        // Convert visible row range (0..rows) to physical row range
        let phys_range = screen.phys_range(&(0..rows as i64));
        let lines = screen.lines_in_phys_range(phys_range);

        // Track current attributes to minimize escape sequences
        let mut current_fg: Option<Color> = None;
        let mut current_bg: Option<Color> = None;

        for (row_idx, line) in lines.iter().enumerate() {
            // Position at start of line
            write!(stdout, "\x1b[{};1H", row_idx + 1)?;

            let mut current_col = 0usize;

            // Use visible_cells() which handles both compressed and uncompressed storage
            for cell in line.visible_cells() {
                let cell_col = cell.cell_index();

                // If there's a gap, fill with spaces
                while current_col < cell_col {
                    write!(stdout, " ")?;
                    current_col += 1;
                }

                let attrs = cell.attrs();

                // Handle foreground color
                let fg = convert_color_attr(&attrs.foreground());
                if fg != current_fg {
                    if let Some(color) = &fg {
                        write!(stdout, "{}", SetForegroundColor(*color))?;
                    } else {
                        write!(stdout, "\x1b[39m")?; // Default foreground
                    }
                    current_fg = fg;
                }

                // Handle background color
                let bg = convert_color_attr(&attrs.background());
                if bg != current_bg {
                    if let Some(color) = &bg {
                        write!(stdout, "{}", SetBackgroundColor(*color))?;
                    } else {
                        write!(stdout, "\x1b[49m")?; // Default background
                    }
                    current_bg = bg;
                }

                let text = cell.str();
                let width = cell.width();
                if text.is_empty() {
                    write!(stdout, " ")?;
                } else {
                    write!(stdout, "{}", text)?;
                }
                current_col += width;
            }

            // Reset colors and clear to end of line
            write!(stdout, "\x1b[0m\x1b[K")?;
            current_fg = None;
            current_bg = None;
        }

        // cursor_pos() returns wrong x value (always 0) for Claude Code - wezterm-term bug.
        // Don't explicitly position cursor - let it stay where last write left it.
        // This is a workaround until the cursor tracking bug is fixed.

        stdout.flush()?;
        Ok(())
    }
}

fn convert_color_attr(attr: &ColorAttribute) -> Option<Color> {
    match attr {
        ColorAttribute::Default => None,
        ColorAttribute::PaletteIndex(idx) => {
            // Map ANSI palette indices to crossterm colors
            match idx {
                0 => Some(Color::Black),
                1 => Some(Color::DarkRed),
                2 => Some(Color::DarkGreen),
                3 => Some(Color::DarkYellow),
                4 => Some(Color::DarkBlue),
                5 => Some(Color::DarkMagenta),
                6 => Some(Color::DarkCyan),
                7 => Some(Color::Grey),
                8 => Some(Color::DarkGrey),
                9 => Some(Color::Red),
                10 => Some(Color::Green),
                11 => Some(Color::Yellow),
                12 => Some(Color::Blue),
                13 => Some(Color::Magenta),
                14 => Some(Color::Cyan),
                15 => Some(Color::White),
                n => Some(Color::AnsiValue(*n)),
            }
        }
        ColorAttribute::TrueColorWithDefaultFallback(c)
        | ColorAttribute::TrueColorWithPaletteFallback(c, _) => {
            let (r, g, b, _) = c.to_srgb_u8();
            Some(Color::Rgb { r, g, b })
        }
    }
}
