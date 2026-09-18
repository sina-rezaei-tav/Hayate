//! Merges Crossterm input and a fixed-rate tick into `AppEvent`s consumed by
//! the main loop.

pub mod stream;

pub use stream::EventHandler;

#[derive(Debug, Clone)]
pub enum AppEvent {
    Input(crossterm::event::Event),
    Tick,
    Error(String),
}
