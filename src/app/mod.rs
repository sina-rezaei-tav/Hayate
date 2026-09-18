//! Central application state and the main event/render loop.

pub mod state;

pub use state::AppState;

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};

use crate::event::{AppEvent, EventHandler};
use crate::ui;
use crate::util::Tui;

/// Drives the app until `state.should_quit` is set: draws a frame, then
/// waits for the next `AppEvent` and applies it to `state`.
///
/// Takes `tui` by reference (rather than owning it) so the caller keeps
/// control of it and can call `Tui::restore` after `run` returns, whether it
/// returned `Ok` or `Err`.
pub async fn run(mut state: AppState, tui: &mut Tui, mut events: EventHandler) -> anyhow::Result<()> {
    while !state.should_quit {
        tui.draw(|frame| ui::render(frame, &state))?;

        match events.next().await? {
            AppEvent::Tick => {}
            AppEvent::Input(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                handle_key(&mut state, key.code, key.modifiers);
            }
            AppEvent::Input(_) => {}
            AppEvent::Error(message) => return Err(anyhow::anyhow!(message)),
        }
    }

    Ok(())
}

fn handle_key(state: &mut AppState, code: KeyCode, modifiers: KeyModifiers) {
    let is_quit_key = matches!(code, KeyCode::Char('q'))
        || (modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c')));

    if is_quit_key {
        state.should_quit = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        AppState::new("/tmp".into())
    }

    #[test]
    fn q_quits() {
        let mut state = state();
        handle_key(&mut state, KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(state.should_quit);
    }

    #[test]
    fn ctrl_c_quits() {
        let mut state = state();
        handle_key(&mut state, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(state.should_quit);
    }

    #[test]
    fn plain_c_does_not_quit() {
        let mut state = state();
        handle_key(&mut state, KeyCode::Char('c'), KeyModifiers::NONE);
        assert!(!state.should_quit);
    }

    #[test]
    fn unrelated_keys_do_not_quit() {
        let mut state = state();
        handle_key(&mut state, KeyCode::Char('j'), KeyModifiers::NONE);
        assert!(!state.should_quit);
    }
}
