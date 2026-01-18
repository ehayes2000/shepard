mod session;
mod session_manager;

use session_manager::*;

fn main() -> anyhow::Result<()> {
    let mut manager = SessionManager::new();

    // Create sessions

    manager.add_session_active("claude", "claude", &[])?;
    // manager.add_session_active("btop", "lazygit", &["-p", "/Users/eric/Code/shepard"])?;
    manager.add_session_active("btop", "btop", &["--force-utf"])?;

    eprintln!("Starting session manager...");
    eprintln!("Sessions: {:?}", manager.session_names());
    eprintln!("Press Ctrl+B to switch sessions, Ctrl+Q to quit\n");

    manager.run()?;

    eprintln!("\nSession manager exited.");
    Ok(())
}
