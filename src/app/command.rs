//! Maps a key event to a user-triggered `Command`, decoupled from *how*
//! that command gets carried out (see `app::run_command`).
//!
//! This is the other place that grows when a new key-triggered feature is
//! added: add a `Command` variant and one match arm here.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Quit,
    /// Ranger `dc`: cumulative size. Hayate counts files under the directory.
    RecountRecursive,
    /// Esc with no pending chord, and Ctrl-c. Stops the running job.
    /// Ctrl-c does not quit; `q` does.
    Cancel,
    SelectPrevious,
    SelectNext,
    /// Half a page up (`K`, Ctrl-u).
    MoveHalfUp,
    /// Half a page down (`J`, Ctrl-d).
    MoveHalfDown,
    /// One page up (PageUp, Ctrl-b).
    MovePageUp,
    /// One page down (PageDown, Ctrl-f).
    MovePageDown,
    /// `gg` and Home.
    MoveToTop,
    /// `G` and End.
    MoveToBottom,
    /// Still available to callers. Ranger's `l` is [`Activate`], not this.
    EnterDirectory,
    /// `l` / Right / Enter: enter a directory, or open a file with the
    /// default program. Until rifle exists, a file opens in `$EDITOR`.
    Activate,
    /// Ranger `E`: edit the file in `$VISUAL` / `$EDITOR`.
    OpenInEditor,
    /// Ranger `r`: open-with prompt.
    OpenWith,
    /// `h` / Left. Backspace is a Hayate extra for the same action.
    OpenParent,
    /// Ranger Ctrl-k: scroll the preview column up one line.
    PreviewLineUp,
    /// Ranger Ctrl-j: scroll the preview column down one line.
    PreviewLineDown,
}

/// One normalized key. Uppercase letters are stored as lowercase + Shift
/// so `E` and Shift-e are the same stroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyStroke {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

/// Keys typed so far that are a prefix of a ranger chord (`g`, `d`, …).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Chord {
    keys: Vec<KeyStroke>,
}

impl Chord {
    pub fn clear(&mut self) {
        self.keys.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    /// Release/repeat, a failed chord, or Esc that only cancelled a prefix.
    Ignored,
    /// The keys so far are a prefix (`g` before `gg`). Keep them.
    Pending,
    Command(Command),
}

/// Pushes `key` onto `chord` and resolves ranger bindings.
///
/// A failed continuation clears the chord and does not run the second key
/// on its own (`gx` is not `x`). Esc clears a pending chord; Esc with an
/// empty chord cancels the running job. Ctrl-c always cancels and never quits.
pub fn feed(chord: &mut Chord, key: KeyEvent) -> Feed {
    if key.kind != KeyEventKind::Press {
        return Feed::Ignored;
    }
    if is_ctrl_c(key) {
        chord.clear();
        return Feed::Command(Command::Cancel);
    }
    if key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE {
        if chord.is_empty() {
            return Feed::Command(Command::Cancel);
        }
        chord.clear();
        return Feed::Ignored;
    }

    chord.keys.push(normalize(key));
    match lookup(&chord.keys) {
        Lookup::Command(command) => {
            chord.clear();
            Feed::Command(command)
        }
        Lookup::Prefix => Feed::Pending,
        Lookup::None => {
            chord.clear();
            Feed::Ignored
        }
    }
}

/// Single-key helper. Chords (`gg`, `dc`) need [`feed`] with a kept [`Chord`].
pub fn command_for_key(key: KeyEvent) -> Option<Command> {
    let mut chord = Chord::default();
    match feed(&mut chord, key) {
        Feed::Command(command) => Some(command),
        Feed::Ignored | Feed::Pending => None,
    }
}

fn is_ctrl_c(key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&'c') => {
            key.modifiers.contains(KeyModifiers::CONTROL)
        }
        _ => false,
    }
}

fn normalize(key: KeyEvent) -> KeyStroke {
    match key.code {
        KeyCode::Char(ch) if ch.is_ascii_uppercase() => KeyStroke {
            code: KeyCode::Char(ch.to_ascii_lowercase()),
            modifiers: key.modifiers | KeyModifiers::SHIFT,
        },
        code => KeyStroke {
            code,
            modifiers: key.modifiers,
        },
    }
}

enum Lookup {
    Command(Command),
    Prefix,
    None,
}

fn lookup(seq: &[KeyStroke]) -> Lookup {
    let mut prefix = false;
    for (keys, command) in bindings() {
        if *keys == seq {
            return Lookup::Command(*command);
        }
        if keys.starts_with(seq) {
            prefix = true;
        }
    }
    if prefix { Lookup::Prefix } else { Lookup::None }
}

fn stroke(code: KeyCode, modifiers: KeyModifiers) -> KeyStroke {
    KeyStroke { code, modifiers }
}

fn ch(c: char) -> KeyStroke {
    stroke(KeyCode::Char(c), KeyModifiers::NONE)
}

fn shift(c: char) -> KeyStroke {
    stroke(KeyCode::Char(c), KeyModifiers::SHIFT)
}

fn ctrl(c: char) -> KeyStroke {
    stroke(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn bindings() -> &'static [(&'static [KeyStroke], Command)] {
    use std::sync::OnceLock;
    static BINDS: OnceLock<Vec<(&'static [KeyStroke], Command)>> = OnceLock::new();
    BINDS.get_or_init(|| {
        let leak = |keys: Vec<KeyStroke>| -> &'static [KeyStroke] { keys.leak() };
        vec![
            (leak(vec![ch('q')]), Command::Quit),
            (leak(vec![ch('r')]), Command::OpenWith),
            (leak(vec![ch('j')]), Command::SelectNext),
            (leak(vec![stroke(KeyCode::Down, KeyModifiers::NONE)]), Command::SelectNext),
            (leak(vec![ch('k')]), Command::SelectPrevious),
            (leak(vec![stroke(KeyCode::Up, KeyModifiers::NONE)]), Command::SelectPrevious),
            (leak(vec![ch('h')]), Command::OpenParent),
            (leak(vec![stroke(KeyCode::Left, KeyModifiers::NONE)]), Command::OpenParent),
            (leak(vec![stroke(KeyCode::Backspace, KeyModifiers::NONE)]), Command::OpenParent),
            (leak(vec![ch('l')]), Command::Activate),
            (leak(vec![stroke(KeyCode::Right, KeyModifiers::NONE)]), Command::Activate),
            (leak(vec![stroke(KeyCode::Enter, KeyModifiers::NONE)]), Command::Activate),
            (leak(vec![shift('e')]), Command::OpenInEditor),
            (leak(vec![shift('j')]), Command::MoveHalfDown),
            (leak(vec![shift('k')]), Command::MoveHalfUp),
            (leak(vec![ctrl('d')]), Command::MoveHalfDown),
            (leak(vec![ctrl('u')]), Command::MoveHalfUp),
            (leak(vec![ctrl('f')]), Command::MovePageDown),
            (leak(vec![ctrl('b')]), Command::MovePageUp),
            (leak(vec![stroke(KeyCode::PageDown, KeyModifiers::NONE)]), Command::MovePageDown),
            (leak(vec![stroke(KeyCode::PageUp, KeyModifiers::NONE)]), Command::MovePageUp),
            (leak(vec![shift('g')]), Command::MoveToBottom),
            (leak(vec![stroke(KeyCode::End, KeyModifiers::NONE)]), Command::MoveToBottom),
            (leak(vec![stroke(KeyCode::Home, KeyModifiers::NONE)]), Command::MoveToTop),
            (leak(vec![ch('g'), ch('g')]), Command::MoveToTop),
            (leak(vec![ch('d'), ch('c')]), Command::RecountRecursive),
            (leak(vec![ctrl('j')]), Command::PreviewLineDown),
            (leak(vec![ctrl('k')]), Command::PreviewLineUp),
        ]
    })
    .as_slice()
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
    fn ctrl_c_cancels_and_does_not_quit() {
        assert_eq!(
            command_for_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Command::Cancel)
        );
    }

    #[test]
    fn ctrl_shift_c_also_cancels() {
        assert_eq!(
            command_for_key(key(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )),
            Some(Command::Cancel)
        );
    }

    #[test]
    fn bare_r_opens_with() {
        assert_eq!(
            command_for_key(key(KeyCode::Char('r'), KeyModifiers::NONE)),
            Some(Command::OpenWith)
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
    fn shift_j_and_k_page_the_list_not_the_selection_by_one() {
        assert_eq!(
            command_for_key(key(KeyCode::Char('j'), KeyModifiers::SHIFT)),
            Some(Command::MoveHalfDown)
        );
        assert_eq!(
            command_for_key(key(KeyCode::Char('k'), KeyModifiers::SHIFT)),
            Some(Command::MoveHalfUp)
        );
    }

    #[test]
    fn l_right_and_enter_activate() {
        for code in [KeyCode::Char('l'), KeyCode::Right, KeyCode::Enter] {
            assert_eq!(
                command_for_key(key(code, KeyModifiers::NONE)),
                Some(Command::Activate),
                "{code:?}"
            );
        }
    }

    #[test]
    fn shift_e_edits() {
        assert_eq!(
            command_for_key(key(KeyCode::Char('e'), KeyModifiers::SHIFT)),
            Some(Command::OpenInEditor)
        );
        assert_eq!(
            command_for_key(key(KeyCode::Char('E'), KeyModifiers::NONE)),
            Some(Command::OpenInEditor)
        );
        assert_eq!(
            command_for_key(key(KeyCode::Char('e'), KeyModifiers::NONE)),
            None,
            "bare e is not ranger's edit key"
        );
    }

    #[test]
    fn bare_o_is_not_open_with() {
        assert_eq!(
            command_for_key(key(KeyCode::Char('o'), KeyModifiers::NONE)),
            None
        );
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
    fn ctrl_u_and_ctrl_d_move_half_a_page() {
        assert_eq!(
            command_for_key(key(KeyCode::Char('u'), KeyModifiers::CONTROL)),
            Some(Command::MoveHalfUp)
        );
        assert_eq!(
            command_for_key(key(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            Some(Command::MoveHalfDown)
        );
    }

    #[test]
    fn page_keys_and_ctrl_b_f_move_a_full_page() {
        assert_eq!(
            command_for_key(key(KeyCode::PageUp, KeyModifiers::NONE)),
            Some(Command::MovePageUp)
        );
        assert_eq!(
            command_for_key(key(KeyCode::PageDown, KeyModifiers::NONE)),
            Some(Command::MovePageDown)
        );
        assert_eq!(
            command_for_key(key(KeyCode::Char('b'), KeyModifiers::CONTROL)),
            Some(Command::MovePageUp)
        );
        assert_eq!(
            command_for_key(key(KeyCode::Char('f'), KeyModifiers::CONTROL)),
            Some(Command::MovePageDown)
        );
    }

    #[test]
    fn ctrl_j_and_ctrl_k_scroll_the_preview_one_line() {
        assert_eq!(
            command_for_key(key(KeyCode::Char('j'), KeyModifiers::CONTROL)),
            Some(Command::PreviewLineDown)
        );
        assert_eq!(
            command_for_key(key(KeyCode::Char('k'), KeyModifiers::CONTROL)),
            Some(Command::PreviewLineUp)
        );
    }

    #[test]
    fn gg_goes_to_the_top_and_a_failed_g_chord_is_dropped() {
        let mut chord = Chord::default();
        assert_eq!(
            feed(&mut chord, key(KeyCode::Char('g'), KeyModifiers::NONE)),
            Feed::Pending
        );
        assert_eq!(
            feed(&mut chord, key(KeyCode::Char('g'), KeyModifiers::NONE)),
            Feed::Command(Command::MoveToTop)
        );
        assert!(chord.is_empty());

        assert_eq!(
            feed(&mut chord, key(KeyCode::Char('g'), KeyModifiers::NONE)),
            Feed::Pending
        );
        assert_eq!(
            feed(&mut chord, key(KeyCode::Char('x'), KeyModifiers::NONE)),
            Feed::Ignored
        );
        assert!(chord.is_empty());
    }

    #[test]
    fn esc_clears_a_pending_chord_without_cancelling() {
        let mut chord = Chord::default();
        feed(&mut chord, key(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(
            feed(&mut chord, key(KeyCode::Esc, KeyModifiers::NONE)),
            Feed::Ignored
        );
        assert!(chord.is_empty());
    }

    #[test]
    fn shift_g_jumps_to_the_bottom_immediately() {
        assert_eq!(
            command_for_key(key(KeyCode::Char('G'), KeyModifiers::NONE)),
            Some(Command::MoveToBottom)
        );
        assert_eq!(
            command_for_key(key(KeyCode::Char('g'), KeyModifiers::SHIFT)),
            Some(Command::MoveToBottom)
        );
    }

    #[test]
    fn dc_recounts() {
        let mut chord = Chord::default();
        assert_eq!(
            feed(&mut chord, key(KeyCode::Char('d'), KeyModifiers::NONE)),
            Feed::Pending
        );
        assert_eq!(
            feed(&mut chord, key(KeyCode::Char('c'), KeyModifiers::NONE)),
            Feed::Command(Command::RecountRecursive)
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
                let is_ctrl_c = c.eq_ignore_ascii_case(&'c') && modifiers.contains(KeyModifiers::CONTROL);
                let is_bare_r = c == 'r' && modifiers == KeyModifiers::NONE;
                let is_bare_k = c == 'k' && modifiers == KeyModifiers::NONE;
                let is_bare_j = c == 'j' && modifiers == KeyModifiers::NONE;
                let is_half_down = (c == 'J' && (modifiers == KeyModifiers::NONE || modifiers == KeyModifiers::SHIFT))
                    || (c == 'j' && modifiers == KeyModifiers::SHIFT);
                let is_half_up = (c == 'K' && (modifiers == KeyModifiers::NONE || modifiers == KeyModifiers::SHIFT))
                    || (c == 'k' && modifiers == KeyModifiers::SHIFT);
                let is_bottom = (c == 'G' && (modifiers == KeyModifiers::NONE || modifiers == KeyModifiers::SHIFT))
                    || (c == 'g' && modifiers == KeyModifiers::SHIFT);
                let is_bare_l = c == 'l' && modifiers == KeyModifiers::NONE;
                let is_bare_h = c == 'h' && modifiers == KeyModifiers::NONE;
                let is_edit = (c == 'E' && (modifiers == KeyModifiers::NONE || modifiers == KeyModifiers::SHIFT))
                    || (c == 'e' && modifiers == KeyModifiers::SHIFT);
                let is_ctrl_u = c == 'u' && modifiers == KeyModifiers::CONTROL;
                let is_ctrl_d = c == 'd' && modifiers == KeyModifiers::CONTROL;
                let is_ctrl_f = c == 'f' && modifiers == KeyModifiers::CONTROL;
                let is_ctrl_b = c == 'b' && modifiers == KeyModifiers::CONTROL;
                let is_preview_down = c == 'j' && modifiers == KeyModifiers::CONTROL;
                let is_preview_up = c == 'k' && modifiers == KeyModifiers::CONTROL;

                let expected = if is_ctrl_c {
                    Some(Command::Cancel)
                } else if is_bare_q {
                    Some(Command::Quit)
                } else if is_bare_r {
                    Some(Command::OpenWith)
                } else if is_half_down || is_ctrl_d {
                    Some(Command::MoveHalfDown)
                } else if is_half_up || is_ctrl_u {
                    Some(Command::MoveHalfUp)
                } else if is_bottom {
                    Some(Command::MoveToBottom)
                } else if is_bare_k {
                    Some(Command::SelectPrevious)
                } else if is_bare_j {
                    Some(Command::SelectNext)
                } else if is_preview_down {
                    Some(Command::PreviewLineDown)
                } else if is_preview_up {
                    Some(Command::PreviewLineUp)
                } else if is_ctrl_f {
                    Some(Command::MovePageDown)
                } else if is_ctrl_b {
                    Some(Command::MovePageUp)
                } else if is_bare_l {
                    Some(Command::Activate)
                } else if is_edit {
                    Some(Command::OpenInEditor)
                } else if is_bare_h {
                    Some(Command::OpenParent)
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
