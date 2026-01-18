mod session;
mod session_manager;

use session_manager::*;

fn main() -> anyhow::Result<()> {
    let mut manager = SessionManager::new();

    // Create sessions

    manager.add_session_active("btop", "htop", &[])?;
    manager.add_session_active("claude", "claude", &[])?;

    eprintln!("Starting session manager...");
    eprintln!("Sessions: {:?}", manager.session_names());
    eprintln!("Press Ctrl+B to switch sessions, Ctrl+Q to quit\n");

    manager.run()?;

    eprintln!("\nSession manager exited.");
    Ok(())
}
