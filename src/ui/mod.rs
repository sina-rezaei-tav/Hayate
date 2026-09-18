//! Pure Ratatui rendering: reads `AppState`, writes to `Frame` only. No I/O,
//! no state mutation.
//!
//! `layout` splits the frame into the 3-pane Miller-column view; `panes`
//! holds each pane's own render function.

pub mod layout;
pub mod panes;

pub use layout::render;
