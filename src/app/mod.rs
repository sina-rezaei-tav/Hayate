//! Central application state and the main event/render loop.

pub mod state;

pub use state::AppState;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

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
            AppEvent::Input(Event::Key(key)) => handle_key(&mut state, key),
            AppEvent::Input(_) => {}
            AppEvent::Error(message) => return Err(anyhow::anyhow!(message)),
        }
    }

    Ok(())
}

/// Applies a key event to `state`.
///
/// Ignores everything but `Press`: without this, Windows (and Unix
/// terminals with the Kitty keyboard protocol enabled) also deliver
/// `Release`/`Repeat` for the same physical keystroke, which would
/// double-apply whatever action the key maps to.
fn handle_key(state: &mut AppState, key: KeyEvent) {
    if key.kind != KeyEventKind::Press {
        return;
    }

    let is_quit_key = (key.modifiers == KeyModifiers::NONE && matches!(key.code, KeyCode::Char('q')))
        || (key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')));

    if is_quit_key {
        state.should_quit = true;
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyEventKind;

    use super::*;

    fn state() -> AppState {
        AppState::new("/tmp".into())
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn key_with_kind(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
        KeyEvent::new_with_kind(code, modifiers, kind)
    }

    #[test]
    fn q_quits() {
        let mut state = state();
        handle_key(&mut state, key(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(state.should_quit);
    }

    #[test]
    fn ctrl_c_quits() {
        let mut state = state();
        handle_key(&mut state, key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(state.should_quit);
    }

    #[test]
    fn plain_c_does_not_quit() {
        let mut state = state();
        handle_key(&mut state, key(KeyCode::Char('c'), KeyModifiers::NONE));
        assert!(!state.should_quit);
    }

    #[test]
    fn unrelated_keys_do_not_quit() {
        let mut state = state();

        for bits in 0..=0b0111_1111 {
            let modifiers = KeyModifiers::from_bits_truncate(bits);

            for c in ('a'..='z').chain('A'..='Z') {
                // Exclude exact quit keys safely
                let is_quit_key = (c == 'q' && modifiers == KeyModifiers::NONE)
                    || (c == 'c' && modifiers.contains(KeyModifiers::CONTROL));

                if is_quit_key {
                    continue;
                }

                handle_key(&mut state, key(KeyCode::Char(c), modifiers));
                assert!(
                    !state.should_quit,
                    "Key combination '{c}' with modifiers {modifiers:?} unexpectedly set should_quit to true"
                );
            }
        }
    }

    #[test]
    fn release_and_repeat_kinds_are_ignored_even_for_quit_keys() {
        let mut release = state();
        handle_key(
            &mut release,
            key_with_kind(KeyCode::Char('q'), KeyModifiers::NONE, KeyEventKind::Release),
        );
        assert!(!release.should_quit);

        let mut repeat = state();
        handle_key(
            &mut repeat,
            key_with_kind(KeyCode::Char('q'), KeyModifiers::NONE, KeyEventKind::Repeat),
        );
        assert!(!repeat.should_quit);
    }
}
