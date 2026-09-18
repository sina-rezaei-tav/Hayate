//! Hayate: an async, Ratatui-based terminal file manager.
//! See `ARCITECTURE.md` for the full design.

pub mod app;
pub mod config;
pub mod event;
pub mod fs;
pub mod preview;
pub mod ui;
pub mod util;

use app::AppState;
use event::EventHandler;
use util::Tui;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut tui = Tui::init()?;
    let events = EventHandler::new();
    let state = AppState::default();

    let result = app::run(state, &mut tui, events).await;
    tui.restore()?;

    result
}
