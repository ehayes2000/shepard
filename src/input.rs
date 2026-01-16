use crossterm::event::{KeyCode, KeyModifiers};

pub const TOGGLE_KEY: char = 'b';

pub fn key_to_bytes(code: KeyCode, modifiers: KeyModifiers) -> Vec<u8> {
    match code {
        KeyCode::Char(c) => {
            if modifiers.contains(KeyModifiers::CONTROL) {
                vec![(c as u8) & 0x1f]
            } else {
                let mut buf = [0u8; 4];
                c.encode_utf8(&mut buf).as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![127],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Esc => vec![27],
        KeyCode::Up => vec![27, b'[', b'A'],
        KeyCode::Down => vec![27, b'[', b'B'],
        KeyCode::Right => vec![27, b'[', b'C'],
        KeyCode::Left => vec![27, b'[', b'D'],
        KeyCode::Home => vec![27, b'[', b'H'],
        KeyCode::End => vec![27, b'[', b'F'],
        KeyCode::PageUp => vec![27, b'[', b'5', b'~'],
        KeyCode::PageDown => vec![27, b'[', b'6', b'~'],
        KeyCode::Delete => vec![27, b'[', b'3', b'~'],
        KeyCode::Insert => vec![27, b'[', b'2', b'~'],
        KeyCode::F(n) => match n {
            1 => vec![27, b'O', b'P'],
            2 => vec![27, b'O', b'Q'],
            3 => vec![27, b'O', b'R'],
            4 => vec![27, b'O', b'S'],
            5..=12 => {
                let codes = [15, 17, 18, 19, 20, 21, 23, 24];
                let idx = (n - 5) as usize;
                if idx < codes.len() {
                    format!("\x1b[{}~", codes[idx]).into_bytes()
                } else {
                    vec![]
                }
            }
            _ => vec![],
        },
        _ => vec![],
    }
}
