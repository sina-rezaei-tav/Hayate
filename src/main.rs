//! Hayate: an async, Ratatui-based terminal file manager.
//!
//! See `ARCITECTURE.md` for the full design. Module responsibilities:
//! - `app`:    Central `AppState`, navigation history, selection, preview cache.
//! - `config`: User configuration loading/parsing.
//! - `event`:  Crossterm input + tick stream, emitted as `AppEvent` over `mpsc`.
//! - `fs`:     Cancellable async directory scanning (Tokio tasks).
//! - `preview`: Async file/image preview generation.
//! - `ui`:     Pure, side-effect-free Ratatui rendering (Miller columns layout).
//! - `util`:   Shared helpers (formatting, path utilities, etc.).

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
