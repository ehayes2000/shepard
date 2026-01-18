mod config;
mod pty_widget;
mod session;
mod session_manager;
mod workflows;

use session_manager::TuiSessionManager;

fn main() -> anyhow::Result<()> {
    let mut manager = TuiSessionManager::new()?;

    // Try to resume a previous session, otherwise open command menu
    if !manager.try_resume()? {
        manager.open_command_menu();
    }

    manager.run()?;

    Ok(())
}
