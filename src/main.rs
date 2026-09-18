//! Hayate: an async, Ratatui-based terminal file manager.
//! See `ARCITECTURE.md` for the full design.

pub mod app;
pub mod config;
pub mod event;
pub mod fs;
pub mod preview;
pub mod ui;
pub mod util;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Ok(())
}
