mod session;
mod session_manager;

use session_manager::*;

const CTRL_B: u8 = 0x02;

fn main() -> anyhow::Result<()> {
    let mut manager = SessionManager::new();

    let switch = HotkeyCallback {
        key: CTRL_B,
        callback: Box::new(|manager| manager.switch_to_next().expect("switch to next")),
    };

    // Create sessions

    manager.add_session_active("btop", "btop", &["--force-utf"])?;
    manager.add_session_active("claude", "claude", &[])?;

    manager.with_hotkeys(vec![switch]);

    eprintln!("Starting session manager...");
    eprintln!("Sessions: {:?}", manager.session_names());
    eprintln!("Press Ctrl+B to switch sessions, Ctrl+Q to quit\n");

    manager.run()?;

    eprintln!("\nSession manager exited.");
    Ok(())
}
