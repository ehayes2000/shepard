mod pty_widget;
mod session;
mod session_manager;

use session_manager::TuiSessionManager;

fn main() -> anyhow::Result<()> {
    let mut manager = TuiSessionManager::new()?;

    manager.add_session("htop", "htop", &[])?;
    manager.add_session("claude", "claude", &[])?;

    manager.run()?;

    Ok(())
}
