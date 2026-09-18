//! Maps a key event to a user-triggered `Command`, decoupled from *how*
//! that command gets carried out (see `app::run_command`).
//!
//! This is the other place that grows when a new key-triggered feature is
//! added: add a `Command` variant and one match arm here.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Quit,
    RecountRecursive,
    /// Stops whichever on-demand background job is currently running
    /// (e.g. a recursive count started by accident), without quitting the
    /// app or touching the always-running directory scan.
    Cancel,
    /// Moves the highlighted entry in the "current" pane up one.
    SelectPrevious,
    /// Moves the highlighted entry in the "current" pane down one.
    SelectNext,
    /// Enters the highlighted directory (`l` / Right / Enter).
    EnterDirectory,
    /// Moves up to the parent directory (`h` / Left / Backspace).
    OpenParent,
    /// Scrolls the preview pane up by one page.
    PreviewPageUp,
    /// Scrolls the preview pane down by one page.
    PreviewPageDown,
}

/// Looks up which `Command`, if any, `key` should trigger.
///
/// Ignores everything but `Press`: without this, Windows (and Unix
/// terminals with the Kitty keyboard protocol enabled) also deliver
/// `Release`/`Repeat` for the same physical keystroke, which would
/// double-apply whatever action the key maps to.
pub fn command_for_key(key: KeyEvent) -> Option<Command> {
    if key.kind != KeyEventKind::Press {
        return None;
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), KeyModifiers::NONE) => Some(Command::Quit),
        (KeyCode::Char('c'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Command::Quit)
        }
        (KeyCode::Char('r'), KeyModifiers::NONE) => Some(Command::RecountRecursive),
        (KeyCode::Esc, KeyModifiers::NONE) => Some(Command::Cancel),
        (KeyCode::Up | KeyCode::Char('k'), KeyModifiers::NONE) => Some(Command::SelectPrevious),
        (KeyCode::Down | KeyCode::Char('j'), KeyModifiers::NONE) => Some(Command::SelectNext),
        (KeyCode::Right | KeyCode::Enter | KeyCode::Char('l'), KeyModifiers::NONE) => {
            Some(Command::EnterDirectory)
        }
        (KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h'), KeyModifiers::NONE) => {
            Some(Command::OpenParent)
        }
        (KeyCode::PageUp, KeyModifiers::NONE)
        | (KeyCode::Char('u'), KeyModifiers::CONTROL) => Some(Command::PreviewPageUp),
        (KeyCode::PageDown, KeyModifiers::NONE)
        | (KeyCode::Char('d'), KeyModifiers::CONTROL) => Some(Command::PreviewPageDown),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn key_with_kind(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
        KeyEvent::new_with_kind(code, modifiers, kind)
    }

    #[test]
    fn q_quits() {
        assert_eq!(
            command_for_key(key(KeyCode::Char('q'), KeyModifiers::NONE)),
            Some(Command::Quit)
        );
    }

    #[test]
    fn ctrl_c_quits() {
        assert_eq!(
            command_for_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Command::Quit)
        );
    }

    #[test]
    fn ctrl_shift_c_also_quits() {
        assert_eq!(
            command_for_key(key(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )),
            Some(Command::Quit)
        );
    }

    #[test]
    fn bare_r_triggers_recount() {
        assert_eq!(
            command_for_key(key(KeyCode::Char('r'), KeyModifiers::NONE)),
            Some(Command::RecountRecursive)
        );
    }

    #[test]
    fn modified_r_does_not_trigger_recount() {
        assert_eq!(
            command_for_key(key(KeyCode::Char('r'), KeyModifiers::SHIFT)),
            None
        );
    }

    #[test]
    fn bare_esc_triggers_cancel() {
        assert_eq!(
            command_for_key(key(KeyCode::Esc, KeyModifiers::NONE)),
            Some(Command::Cancel)
        );
    }

    #[test]
    fn modified_esc_does_not_trigger_cancel() {
        assert_eq!(
            command_for_key(key(KeyCode::Esc, KeyModifiers::SHIFT)),
            None
        );
    }

    #[test]
    fn up_and_k_select_previous() {
        assert_eq!(
            command_for_key(key(KeyCode::Up, KeyModifiers::NONE)),
            Some(Command::SelectPrevious)
        );
        assert_eq!(
            command_for_key(key(KeyCode::Char('k'), KeyModifiers::NONE)),
            Some(Command::SelectPrevious)
        );
    }

    #[test]
    fn down_and_j_select_next() {
        assert_eq!(
            command_for_key(key(KeyCode::Down, KeyModifiers::NONE)),
            Some(Command::SelectNext)
        );
        assert_eq!(
            command_for_key(key(KeyCode::Char('j'), KeyModifiers::NONE)),
            Some(Command::SelectNext)
        );
    }

    #[test]
    fn modified_j_and_k_do_not_move_selection() {
        assert_eq!(
            command_for_key(key(KeyCode::Char('j'), KeyModifiers::SHIFT)),
            None
        );
        assert_eq!(
            command_for_key(key(KeyCode::Char('k'), KeyModifiers::SHIFT)),
            None
        );
    }

    #[test]
    fn l_enter_and_right_enter_directory() {
        for code in [KeyCode::Char('l'), KeyCode::Enter, KeyCode::Right] {
            assert_eq!(
                command_for_key(key(code, KeyModifiers::NONE)),
                Some(Command::EnterDirectory),
                "{code:?}"
            );
        }
    }

    #[test]
    fn h_backspace_and_left_open_parent() {
        for code in [KeyCode::Char('h'), KeyCode::Backspace, KeyCode::Left] {
            assert_eq!(
                command_for_key(key(code, KeyModifiers::NONE)),
                Some(Command::OpenParent),
                "{code:?}"
            );
        }
    }

    #[test]
    fn modified_h_and_l_do_not_navigate() {
        assert_eq!(
            command_for_key(key(KeyCode::Char('h'), KeyModifiers::SHIFT)),
            None
        );
        assert_eq!(
            command_for_key(key(KeyCode::Char('l'), KeyModifiers::SHIFT)),
            None
        );
    }

    #[test]
    fn page_up_and_ctrl_u_scroll_preview_up() {
        assert_eq!(
            command_for_key(key(KeyCode::PageUp, KeyModifiers::NONE)),
            Some(Command::PreviewPageUp)
        );
        assert_eq!(
            command_for_key(key(KeyCode::Char('u'), KeyModifiers::CONTROL)),
            Some(Command::PreviewPageUp)
        );
    }

    #[test]
    fn page_down_and_ctrl_d_scroll_preview_down() {
        assert_eq!(
            command_for_key(key(KeyCode::PageDown, KeyModifiers::NONE)),
            Some(Command::PreviewPageDown)
        );
        assert_eq!(
            command_for_key(key(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            Some(Command::PreviewPageDown)
        );
    }

    /// Exhaustively sweeps every letter against every defined modifier
    /// combination (`KeyModifiers` has 6 real bits) and checks the exact
    /// expected `Command`, positive and negative, in one table-driven test.
    #[test]
    fn sweeps_every_letter_and_modifier_combination() {
        for bits in 0..=0b0011_1111u8 {
            let modifiers = KeyModifiers::from_bits_truncate(bits);

            for c in ('a'..='z').chain('A'..='Z') {
                let is_bare_q = c == 'q' && modifiers == KeyModifiers::NONE;
                let is_ctrl_c = c == 'c' && modifiers.contains(KeyModifiers::CONTROL);
                let is_bare_r = c == 'r' && modifiers == KeyModifiers::NONE;
                let is_bare_k = c == 'k' && modifiers == KeyModifiers::NONE;
                let is_bare_j = c == 'j' && modifiers == KeyModifiers::NONE;
                let is_bare_l = c == 'l' && modifiers == KeyModifiers::NONE;
                let is_bare_h = c == 'h' && modifiers == KeyModifiers::NONE;
                let is_ctrl_u = c == 'u' && modifiers == KeyModifiers::CONTROL;
                let is_ctrl_d = c == 'd' && modifiers == KeyModifiers::CONTROL;

                let expected = if is_bare_q || is_ctrl_c {
                    Some(Command::Quit)
                } else if is_bare_r {
                    Some(Command::RecountRecursive)
                } else if is_bare_k {
                    Some(Command::SelectPrevious)
                } else if is_bare_j {
                    Some(Command::SelectNext)
                } else if is_bare_l {
                    Some(Command::EnterDirectory)
                } else if is_bare_h {
                    Some(Command::OpenParent)
                } else if is_ctrl_u {
                    Some(Command::PreviewPageUp)
                } else if is_ctrl_d {
                    Some(Command::PreviewPageDown)
                } else {
                    None
                };

                assert_eq!(
                    command_for_key(key(KeyCode::Char(c), modifiers)),
                    expected,
                    "key '{c}' with modifiers {modifiers:?}"
                );
            }
        }
    }

    #[test]
    fn release_and_repeat_kinds_return_none() {
        assert_eq!(
            command_for_key(key_with_kind(
                KeyCode::Char('q'),
                KeyModifiers::NONE,
                KeyEventKind::Release
            )),
            None
        );
        assert_eq!(
            command_for_key(key_with_kind(
                KeyCode::Char('r'),
                KeyModifiers::NONE,
                KeyEventKind::Repeat
            )),
            None
        );
    }
}
