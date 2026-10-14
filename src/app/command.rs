//! Maps a key event to a user-triggered `Command`, decoupled from *how*
//! that command gets carried out (see `app::run_command`).
//!
//! Digits before a chord are a ranger quantifier (`5j`, `12gg`). They are
//! not part of the chord, so `g5` is a failed chord rather than a count.

use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::fs::SortKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Quit,
    /// Ranger `dc`: cumulative size. Hayate counts files under the directory.
    RecountRecursive,
    /// Esc with nothing pending, and Ctrl-c. Stops the running job.
    /// Ctrl-c does not quit; `q` does.
    Cancel,
    SelectPrevious,
    SelectNext,
    MoveHalfUp,
    MoveHalfDown,
    MovePageUp,
    MovePageDown,
    MoveToTop,
    MoveToBottom,
    /// Still available to callers. Ranger's `l` is [`Activate`], not this.
    EnterDirectory,
    /// `l` / Right / Enter: enter a directory, or open a file.
    /// Text files open in `$EDITOR`; other files use `xdg-open` when it exists.
    Activate,
    /// Ranger `E`: edit the file in `$VISUAL` / `$EDITOR`.
    OpenInEditor,
    /// Ranger `r`: open-with prompt.
    OpenWith,
    /// `h` / Left. Backspace is a Hayate extra for the same action.
    OpenParent,
    PreviewLineUp,
    PreviewLineDown,
    ToggleHidden,
    ToggleDirsFirst,
    ToggleSortReverse,
    Sort(SortKey),
    BeginFilter,
    ClearFilter,
    ToggleImages,
    BeginSearch,
    SearchNext,
    SearchPrev,
    BeginFind,
    Yank,
    Cut,
    Paste,
    PasteOverwrite,
    BeginDelete,
    Rename(RenameKind),
    ToggleMark,
    ToggleMarkMove,
    ToggleVisual,
    ClearMarks,
    HistoryBack,
    HistoryForward,
    Cd(CdPlace),
    TabNew,
    TabClose,
    TabNext,
    TabPrev,
    /// 1-based tab number, from Alt-1 .. Alt-9.
    TabGoto(u8),
    BeginBookmarkSet,
    BeginBookmarkGo,
    BeginBookmarkUnset,
    Help,
    BeginConsole,
    BeginShell,
    OpenShell,
    DisplayFile,
    /// Ranger `R`. Re-reads the current directory without leaving it.
    Reload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameKind {
    /// `cw`: the next typed character replaces the name.
    Change,
    /// `a`: cursor sits just before the extension.
    AppendStem,
    /// `A`: cursor at the end.
    AppendEnd,
    /// `I`: cursor at the start.
    Insert,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdPlace {
    Home,
    Root,
    Etc,
    Usr,
    Dev,
    Media,
    Mnt,
    Tmp,
    Srv,
    Opt,
    Var,
}

/// One normalized key. Uppercase letters are stored as lowercase + Shift
/// so `E` and Shift-e are the same stroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyStroke {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

/// Keys typed so far that are a prefix of a ranger chord (`g`, `d`, …)
/// plus an optional quantifier (`5` of `5j`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Chord {
    keys: Vec<KeyStroke>,
    count: Option<u32>,
}

impl Chord {
    pub fn clear(&mut self) {
        self.keys.clear();
        self.count = None;
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.count.is_none()
    }

    pub fn count(&self) -> Option<u32> {
        self.count
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub command: Command,
    pub count: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    /// Release/repeat, a failed chord, or Esc that only cancelled a prefix.
    Ignored,
    /// The keys so far are a prefix (`g` before `gg`) or a quantifier.
    Pending,
    Command(Resolved),
}

/// Pushes `key` onto `chord` and resolves ranger bindings.
///
/// A failed continuation clears the chord *and* the quantifier, and does
/// not run the second key on its own (`gx` is not `x`). Esc clears a
/// pending chord or quantifier; Esc with nothing pending cancels the
/// running job. Ctrl-c always cancels and never quits.
pub fn feed(chord: &mut Chord, key: KeyEvent) -> Feed {
    if key.kind != KeyEventKind::Press {
        return Feed::Ignored;
    }
    if is_ctrl_c(key) {
        chord.clear();
        return Feed::Command(Resolved {
            command: Command::Cancel,
            count: None,
        });
    }
    if key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE {
        if chord.is_empty() {
            return Feed::Command(Resolved {
                command: Command::Cancel,
                count: None,
            });
        }
        chord.clear();
        return Feed::Ignored;
    }
    if chord.keys.is_empty()
        && key.modifiers == KeyModifiers::NONE
        && let KeyCode::Char(digit) = key.code
        && digit.is_ascii_digit()
    {
        let value = digit.to_digit(10).unwrap_or(0);
        let next = chord.count.unwrap_or(0).saturating_mul(10).saturating_add(value);
        chord.count = Some(next);
        return Feed::Pending;
    }

    chord.keys.push(normalize(key));
    match lookup(&chord.keys) {
        Lookup::Command(command) => {
            let count = chord.count;
            chord.clear();
            Feed::Command(Resolved { command, count })
        }
        Lookup::Prefix => Feed::Pending,
        Lookup::None => {
            chord.clear();
            Feed::Ignored
        }
    }
}

/// Single-key helper. Chords (`gg`, `dc`) and quantifiers need [`feed`].
pub fn command_for_key(key: KeyEvent) -> Option<Command> {
    let mut chord = Chord::default();
    match feed(&mut chord, key) {
        Feed::Command(resolved) => Some(resolved.command),
        Feed::Ignored | Feed::Pending => None,
    }
}

pub fn help_lines() -> Vec<(&'static str, &'static str)> {
    let mut seen = HashSet::new();
    let mut lines = Vec::new();
    for binding in bindings() {
        if seen.insert(binding.label) {
            lines.push((binding.label, binding.blurb));
        }
    }
    lines
}

fn is_ctrl_c(key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&'c') => key.modifiers.contains(KeyModifiers::CONTROL),
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
    for binding in bindings() {
        if binding.keys == seq {
            return Lookup::Command(binding.command);
        }
        if binding.keys.starts_with(seq) {
            prefix = true;
        }
    }
    if prefix { Lookup::Prefix } else { Lookup::None }
}

struct Binding {
    keys: &'static [KeyStroke],
    command: Command,
    label: &'static str,
    blurb: &'static str,
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

fn alt_digit(n: u8) -> KeyStroke {
    stroke(KeyCode::Char((b'0' + n) as char), KeyModifiers::ALT)
}

fn bindings() -> &'static [Binding] {
    use std::sync::OnceLock;
    static BINDS: OnceLock<Vec<Binding>> = OnceLock::new();
    BINDS.get_or_init(|| {
        let row = |keys: Vec<KeyStroke>, command: Command, label: &'static str, blurb: &'static str| Binding {
            keys: keys.leak(),
            command,
            label,
            blurb,
        };
        let mut rows = vec![
            row(vec![ch('q')], Command::Quit, "q", "quit"),
            row(vec![ch('r')], Command::OpenWith, "r", "open with a command"),
            row(vec![ch('j')], Command::SelectNext, "j", "move down"),
            row(vec![stroke(KeyCode::Down, KeyModifiers::NONE)], Command::SelectNext, "Down", "move down"),
            row(vec![ch('k')], Command::SelectPrevious, "k", "move up"),
            row(vec![stroke(KeyCode::Up, KeyModifiers::NONE)], Command::SelectPrevious, "Up", "move up"),
            row(vec![ch('h')], Command::OpenParent, "h", "parent directory"),
            row(vec![stroke(KeyCode::Left, KeyModifiers::NONE)], Command::OpenParent, "Left", "parent directory"),
            row(vec![stroke(KeyCode::Backspace, KeyModifiers::NONE)], Command::OpenParent, "Backspace", "parent directory"),
            row(vec![ch('l')], Command::Activate, "l", "enter or open"),
            row(vec![stroke(KeyCode::Right, KeyModifiers::NONE)], Command::Activate, "Right", "enter or open"),
            row(vec![stroke(KeyCode::Enter, KeyModifiers::NONE)], Command::Activate, "Enter", "enter or open"),
            row(vec![shift('e')], Command::OpenInEditor, "E", "edit in $EDITOR"),
            row(vec![shift('r')], Command::Reload, "R", "reload this directory"),
            row(vec![shift('j')], Command::MoveHalfDown, "J", "half page down"),
            row(vec![shift('k')], Command::MoveHalfUp, "K", "half page up"),
            row(vec![ctrl('d')], Command::MoveHalfDown, "Ctrl-d", "half page down"),
            row(vec![ctrl('u')], Command::MoveHalfUp, "Ctrl-u", "half page up"),
            row(vec![ctrl('f')], Command::MovePageDown, "Ctrl-f", "page down"),
            row(vec![ctrl('b')], Command::MovePageUp, "Ctrl-b", "page up"),
            row(vec![stroke(KeyCode::PageDown, KeyModifiers::NONE)], Command::MovePageDown, "PageDown", "page down"),
            row(vec![stroke(KeyCode::PageUp, KeyModifiers::NONE)], Command::MovePageUp, "PageUp", "page up"),
            row(vec![shift('g')], Command::MoveToBottom, "G", "bottom, or line N"),
            row(vec![stroke(KeyCode::End, KeyModifiers::NONE)], Command::MoveToBottom, "End", "bottom, or line N"),
            row(vec![stroke(KeyCode::Home, KeyModifiers::NONE)], Command::MoveToTop, "Home", "top, or line N"),
            row(vec![ch('g'), ch('g')], Command::MoveToTop, "gg", "top, or line N"),
            row(vec![ch('d'), ch('c')], Command::RecountRecursive, "dc", "count files recursively"),
            row(vec![ctrl('j')], Command::PreviewLineDown, "Ctrl-j", "preview one line down"),
            row(vec![ctrl('k')], Command::PreviewLineUp, "Ctrl-k", "preview one line up"),
            row(vec![ch('z'), ch('h')], Command::ToggleHidden, "zh", "toggle hidden files"),
            row(vec![ch('z'), ch('d')], Command::ToggleDirsFirst, "zd", "toggle directories first"),
            row(vec![ch('z'), ch('f')], Command::BeginFilter, "zf", "filter names"),
            row(vec![ch('z'), ch('z')], Command::ClearFilter, "zz", "clear the filter"),
            row(vec![ch('z'), ch('i')], Command::ToggleImages, "zi", "toggle image preview"),
            row(vec![ch('o'), ch('n')], Command::Sort(SortKey::Name), "on", "sort by name"),
            row(vec![ch('o'), ch('s')], Command::Sort(SortKey::Size), "os", "sort by size"),
            row(vec![ch('o'), ch('m')], Command::Sort(SortKey::Mtime), "om", "sort by time"),
            row(vec![ch('o'), ch('e')], Command::Sort(SortKey::Extension), "oe", "sort by extension"),
            row(vec![ch('o'), ch('t')], Command::Sort(SortKey::Type), "ot", "sort by kind"),
            row(vec![ch('o'), ch('r')], Command::ToggleSortReverse, "or", "reverse the sort"),
            row(vec![ch('/')], Command::BeginSearch, "/", "search names"),
            row(vec![ch('n')], Command::SearchNext, "n", "next match"),
            row(vec![shift('n')], Command::SearchPrev, "N", "previous match"),
            row(vec![ch('f')], Command::BeginFind, "f", "find by prefix"),
            row(vec![ch('y'), ch('y')], Command::Yank, "yy", "yank"),
            row(vec![ch('d'), ch('d')], Command::Cut, "dd", "cut"),
            row(vec![ch('p'), ch('p')], Command::Paste, "pp", "paste"),
            row(vec![ch('p'), ch('o')], Command::PasteOverwrite, "po", "paste, replacing"),
            row(vec![ch('d'), shift('d')], Command::BeginDelete, "dD", "delete, after confirm"),
            row(vec![ch('c'), ch('w')], Command::Rename(RenameKind::Change), "cw", "rename"),
            row(vec![ch('a')], Command::Rename(RenameKind::AppendStem), "a", "rename before the extension"),
            row(vec![shift('a')], Command::Rename(RenameKind::AppendEnd), "A", "rename at the end"),
            row(vec![shift('i')], Command::Rename(RenameKind::Insert), "I", "rename at the start"),
            row(vec![ch(' ')], Command::ToggleMark, "Space", "toggle mark"),
            row(vec![ch('v')], Command::ToggleMarkMove, "v", "mark and move down"),
            row(vec![shift('v')], Command::ToggleVisual, "V", "visual mark"),
            row(vec![ch('u'), ch('v')], Command::ClearMarks, "uv", "clear marks"),
            row(vec![shift('h')], Command::HistoryBack, "H", "back"),
            row(vec![shift('l')], Command::HistoryForward, "L", "forward"),
            row(vec![ch('g'), ch('h')], Command::Cd(CdPlace::Home), "gh", "go to home"),
            row(vec![ch('g'), ch('/')], Command::Cd(CdPlace::Root), "g/", "go to /"),
            row(vec![ch('g'), ch('r')], Command::Cd(CdPlace::Root), "gr", "go to /"),
            row(vec![ch('g'), ch('e')], Command::Cd(CdPlace::Etc), "ge", "go to /etc"),
            row(vec![ch('g'), ch('u')], Command::Cd(CdPlace::Usr), "gu", "go to /usr"),
            row(vec![ch('g'), ch('d')], Command::Cd(CdPlace::Dev), "gd", "go to /dev"),
            row(vec![ch('g'), ch('m')], Command::Cd(CdPlace::Media), "gm", "go to /media"),
            row(vec![ch('g'), shift('m')], Command::Cd(CdPlace::Mnt), "gM", "go to /mnt"),
            row(vec![ch('g'), ch('p')], Command::Cd(CdPlace::Tmp), "gp", "go to /tmp"),
            row(vec![ch('g'), ch('s')], Command::Cd(CdPlace::Srv), "gs", "go to /srv"),
            row(vec![ch('g'), ch('o')], Command::Cd(CdPlace::Opt), "go", "go to /opt"),
            row(vec![ch('g'), ch('v')], Command::Cd(CdPlace::Var), "gv", "go to /var"),
            row(vec![ch('g'), ch('n')], Command::TabNew, "gn", "new tab"),
            row(vec![ctrl('n')], Command::TabNew, "Ctrl-n", "new tab"),
            row(vec![ch('g'), ch('c')], Command::TabClose, "gc", "close tab"),
            row(vec![ctrl('w')], Command::TabClose, "Ctrl-w", "close tab"),
            row(vec![ch('g'), ch('t')], Command::TabNext, "gt", "next tab"),
            row(vec![stroke(KeyCode::Tab, KeyModifiers::NONE)], Command::TabNext, "Tab", "next tab"),
            row(vec![ch('g'), shift('t')], Command::TabPrev, "gT", "previous tab"),
            row(vec![stroke(KeyCode::BackTab, KeyModifiers::NONE)], Command::TabPrev, "Shift-Tab", "previous tab"),
            row(vec![ch('m')], Command::BeginBookmarkSet, "m", "set bookmark"),
            row(vec![ch('\'')], Command::BeginBookmarkGo, "'", "go to bookmark"),
            row(vec![ch('`')], Command::BeginBookmarkGo, "`", "go to bookmark"),
            row(vec![ch('u'), ch('m')], Command::BeginBookmarkUnset, "um", "unset bookmark"),
            row(vec![ch('?')], Command::Help, "?", "help"),
            row(vec![shift('/')], Command::Help, "?", "help"),
            row(vec![stroke(KeyCode::Char('?'), KeyModifiers::SHIFT)], Command::Help, "?", "help"),
            row(vec![ch(':')], Command::BeginConsole, ":", "console"),
            row(vec![stroke(KeyCode::Char(':'), KeyModifiers::SHIFT)], Command::BeginConsole, ":", "console"),
            row(vec![ch('!')], Command::BeginShell, "!", "shell command"),
            row(vec![stroke(KeyCode::Char('!'), KeyModifiers::SHIFT)], Command::BeginShell, "!", "shell command"),
            row(vec![ch('s')], Command::BeginShell, "s", "shell command"),
            row(vec![shift('s')], Command::OpenShell, "S", "interactive shell"),
            row(vec![ch('i')], Command::DisplayFile, "i", "pager"),
        ];
        for n in 1..=9 {
            let label: &'static str = match n {
                1 => "Alt-1",
                2 => "Alt-2",
                3 => "Alt-3",
                4 => "Alt-4",
                5 => "Alt-5",
                6 => "Alt-6",
                7 => "Alt-7",
                8 => "Alt-8",
                _ => "Alt-9",
            };
            rows.push(row(vec![alt_digit(n)], Command::TabGoto(n), label, "go to tab"));
        }
        rows
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

    fn done(command: Command) -> Feed {
        Feed::Command(Resolved { command, count: None })
    }

    fn done_count(command: Command, count: u32) -> Feed {
        Feed::Command(Resolved {
            command,
            count: Some(count),
        })
    }

    #[test]
    fn q_quits_and_ctrl_c_cancels() {
        assert_eq!(command_for_key(key(KeyCode::Char('q'), KeyModifiers::NONE)), Some(Command::Quit));
        assert_eq!(
            command_for_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Command::Cancel)
        );
        assert_eq!(
            command_for_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)),
            Some(Command::Cancel)
        );
    }

    #[test]
    fn bare_r_opens_with_and_shift_r_does_not() {
        assert_eq!(command_for_key(key(KeyCode::Char('r'), KeyModifiers::NONE)), Some(Command::OpenWith));
        assert_eq!(command_for_key(key(KeyCode::Char('r'), KeyModifiers::SHIFT)), Some(Command::Reload));
    }

    #[test]
    fn bare_esc_cancels_and_modified_esc_does_not() {
        assert_eq!(command_for_key(key(KeyCode::Esc, KeyModifiers::NONE)), Some(Command::Cancel));
        assert_eq!(command_for_key(key(KeyCode::Esc, KeyModifiers::SHIFT)), None);
    }

    #[test]
    fn movement_keys_match_ranger() {
        assert_eq!(command_for_key(key(KeyCode::Char('j'), KeyModifiers::NONE)), Some(Command::SelectNext));
        assert_eq!(command_for_key(key(KeyCode::Down, KeyModifiers::NONE)), Some(Command::SelectNext));
        assert_eq!(command_for_key(key(KeyCode::Char('k'), KeyModifiers::NONE)), Some(Command::SelectPrevious));
        assert_eq!(command_for_key(key(KeyCode::Char('J'), KeyModifiers::NONE)), Some(Command::MoveHalfDown));
        assert_eq!(command_for_key(key(KeyCode::Char('K'), KeyModifiers::NONE)), Some(Command::MoveHalfUp));
        assert_eq!(command_for_key(key(KeyCode::Char('l'), KeyModifiers::NONE)), Some(Command::Activate));
        assert_eq!(command_for_key(key(KeyCode::Right, KeyModifiers::NONE)), Some(Command::Activate));
        assert_eq!(command_for_key(key(KeyCode::Enter, KeyModifiers::NONE)), Some(Command::Activate));
        assert_eq!(command_for_key(key(KeyCode::Char('h'), KeyModifiers::NONE)), Some(Command::OpenParent));
        assert_eq!(command_for_key(key(KeyCode::Backspace, KeyModifiers::NONE)), Some(Command::OpenParent));
        assert_eq!(command_for_key(key(KeyCode::Char('E'), KeyModifiers::NONE)), Some(Command::OpenInEditor));
        assert_eq!(command_for_key(key(KeyCode::Char('e'), KeyModifiers::NONE)), None);
        assert_eq!(command_for_key(key(KeyCode::Char('o'), KeyModifiers::NONE)), None);
    }

    #[test]
    fn a_quantifier_is_pending_until_the_command_and_zero_is_kept() {
        let mut chord = Chord::default();
        assert_eq!(feed(&mut chord, key(KeyCode::Char('5'), KeyModifiers::NONE)), Feed::Pending);
        assert_eq!(chord.count(), Some(5));
        assert_eq!(feed(&mut chord, key(KeyCode::Char('j'), KeyModifiers::NONE)), done_count(Command::SelectNext, 5));
        assert!(chord.is_empty());

        assert_eq!(feed(&mut chord, key(KeyCode::Char('0'), KeyModifiers::NONE)), Feed::Pending);
        assert_eq!(feed(&mut chord, key(KeyCode::Char('j'), KeyModifiers::NONE)), done_count(Command::SelectNext, 0));
    }

    #[test]
    fn a_quantifier_applies_to_a_chord_and_a_failed_chord_drops_it() {
        let mut chord = Chord::default();
        feed(&mut chord, key(KeyCode::Char('1'), KeyModifiers::NONE));
        feed(&mut chord, key(KeyCode::Char('2'), KeyModifiers::NONE));
        assert_eq!(chord.count(), Some(12));
        assert_eq!(feed(&mut chord, key(KeyCode::Char('g'), KeyModifiers::NONE)), Feed::Pending);
        assert_eq!(feed(&mut chord, key(KeyCode::Char('g'), KeyModifiers::NONE)), done_count(Command::MoveToTop, 12));

        feed(&mut chord, key(KeyCode::Char('4'), KeyModifiers::NONE));
        feed(&mut chord, key(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(feed(&mut chord, key(KeyCode::Char('x'), KeyModifiers::NONE)), Feed::Ignored);
        assert!(chord.is_empty(), "gx must not leave the 4 behind");
    }

    #[test]
    fn a_digit_inside_a_chord_is_not_a_quantifier() {
        let mut chord = Chord::default();
        feed(&mut chord, key(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(feed(&mut chord, key(KeyCode::Char('5'), KeyModifiers::NONE)), Feed::Ignored);
        assert!(chord.count().is_none());
        assert!(chord.is_empty());
    }

    #[test]
    fn escape_clears_a_quantifier_without_cancelling() {
        let mut chord = Chord::default();
        feed(&mut chord, key(KeyCode::Char('5'), KeyModifiers::NONE));
        assert_eq!(feed(&mut chord, key(KeyCode::Esc, KeyModifiers::NONE)), Feed::Ignored);
        assert!(chord.is_empty());
        assert_eq!(feed(&mut chord, key(KeyCode::Char('j'), KeyModifiers::NONE)), done(Command::SelectNext));
    }

    #[test]
    fn a_huge_quantifier_saturates_instead_of_overflowing() {
        let mut chord = Chord::default();
        for _ in 0..20 {
            feed(&mut chord, key(KeyCode::Char('9'), KeyModifiers::NONE));
        }
        assert_eq!(chord.count(), Some(u32::MAX));
    }

    #[test]
    fn shift_digit_and_alt_digit_do_not_start_a_quantifier() {
        let mut chord = Chord::default();
        assert_eq!(feed(&mut chord, key(KeyCode::Char('5'), KeyModifiers::SHIFT)), Feed::Ignored);
        assert!(chord.is_empty());
        assert_eq!(
            feed(&mut chord, key(KeyCode::Char('1'), KeyModifiers::ALT)),
            done(Command::TabGoto(1))
        );
    }

    #[test]
    fn gg_and_dc_and_a_failed_chord_do_not_run_the_second_key() {
        let mut chord = Chord::default();
        assert_eq!(feed(&mut chord, key(KeyCode::Char('g'), KeyModifiers::NONE)), Feed::Pending);
        assert_eq!(feed(&mut chord, key(KeyCode::Char('g'), KeyModifiers::NONE)), done(Command::MoveToTop));
        feed(&mut chord, key(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(feed(&mut chord, key(KeyCode::Char('x'), KeyModifiers::NONE)), Feed::Ignored);
        assert!(chord.is_empty());

        assert_eq!(feed(&mut chord, key(KeyCode::Char('d'), KeyModifiers::NONE)), Feed::Pending);
        assert_eq!(feed(&mut chord, key(KeyCode::Char('c'), KeyModifiers::NONE)), done(Command::RecountRecursive));
    }

    #[test]
    fn esc_clears_a_pending_chord_without_cancelling() {
        let mut chord = Chord::default();
        feed(&mut chord, key(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(feed(&mut chord, key(KeyCode::Esc, KeyModifiers::NONE)), Feed::Ignored);
        assert!(chord.is_empty());
    }

    #[test]
    fn two_key_chords_resolve_to_the_ranger_command() {
        let specs = [
            (&[('y', false), ('y', false)][..], Command::Yank),
            (&[('d', false), ('d', false)], Command::Cut),
            (&[('d', false), ('D', false)], Command::BeginDelete),
            (&[('p', false), ('p', false)], Command::Paste),
            (&[('p', false), ('o', false)], Command::PasteOverwrite),
            (&[('c', false), ('w', false)], Command::Rename(RenameKind::Change)),
            (&[('z', false), ('h', false)], Command::ToggleHidden),
            (&[('z', false), ('i', false)], Command::ToggleImages),
            (&[('z', false), ('z', false)], Command::ClearFilter),
            (&[('o', false), ('s', false)], Command::Sort(SortKey::Size)),
            (&[('o', false), ('t', false)], Command::Sort(SortKey::Type)),
            (&[('o', false), ('e', false)], Command::Sort(SortKey::Extension)),
            (&[('o', false), ('r', false)], Command::ToggleSortReverse),
            (&[('u', false), ('v', false)], Command::ClearMarks),
            (&[('u', false), ('m', false)], Command::BeginBookmarkUnset),
            (&[('g', false), ('e', false)], Command::Cd(CdPlace::Etc)),
            (&[('g', false), ('M', false)], Command::Cd(CdPlace::Mnt)),
            (&[('g', false), ('n', false)], Command::TabNew),
            (&[('g', false), ('T', false)], Command::TabPrev),
        ];
        for (keys, command) in specs {
            let mut chord = Chord::default();
            assert_eq!(
                feed(&mut chord, key(KeyCode::Char(keys[0].0), KeyModifiers::NONE)),
                Feed::Pending,
                "{}",
                keys[0].0
            );
            assert_eq!(
                feed(&mut chord, key(KeyCode::Char(keys[1].0), KeyModifiers::NONE)),
                done(command),
                "{}{}",
                keys[0].0,
                keys[1].0
            );
        }
    }

    #[test]
    fn help_lists_every_binding_and_does_not_invent_blank_rows() {
        let lines = help_lines();
        assert!(lines.iter().all(|(label, blurb)| !label.is_empty() && !blurb.is_empty()));
        let labels: HashSet<&str> = lines.iter().map(|(label, _)| *label).collect();
        for label in ["q", "dc", "yy", "dd", "dD", "pp", "zh", "gg", "?", ":", "E", "r"] {
            assert!(labels.contains(label), "help is missing {label}");
        }
    }

    #[test]
    fn release_and_repeat_kinds_return_none() {
        assert_eq!(
            command_for_key(key_with_kind(KeyCode::Char('q'), KeyModifiers::NONE, KeyEventKind::Release)),
            None
        );
        assert_eq!(
            command_for_key(key_with_kind(KeyCode::Char('r'), KeyModifiers::NONE, KeyEventKind::Repeat)),
            None
        );
    }

    /// Every letter against every modifier bit. This is the net that catches
    /// a new binding stealing a key the sweep did not mean to give away.
    #[test]
    fn sweeps_every_letter_and_modifier_combination() {
        for bits in 0..=0b0011_1111u8 {
            let modifiers = KeyModifiers::from_bits_truncate(bits);
            for c in ('a'..='z').chain('A'..='Z') {
                assert_eq!(
                    command_for_key(key(KeyCode::Char(c), modifiers)),
                    expected_letter(c, modifiers),
                    "key '{c}' with modifiers {modifiers:?}"
                );
            }
        }
    }

    fn expected_letter(c: char, modifiers: KeyModifiers) -> Option<Command> {
        let bare = modifiers == KeyModifiers::NONE;
        let shift = modifiers == KeyModifiers::SHIFT;
        let ctrl = modifiers == KeyModifiers::CONTROL;
        let lower = c.to_ascii_lowercase();

        if c.eq_ignore_ascii_case(&'c') && modifiers.contains(KeyModifiers::CONTROL) {
            return Some(Command::Cancel);
        }
        if ctrl {
            return match c {
                'd' => Some(Command::MoveHalfDown),
                'u' => Some(Command::MoveHalfUp),
                'f' => Some(Command::MovePageDown),
                'b' => Some(Command::MovePageUp),
                'j' => Some(Command::PreviewLineDown),
                'k' => Some(Command::PreviewLineUp),
                'n' => Some(Command::TabNew),
                'w' => Some(Command::TabClose),
                _ => None,
            };
        }
        let shifted = (c.is_ascii_uppercase() && (bare || shift)) || (c.is_ascii_lowercase() && shift);
        if shifted {
            return match lower {
                'j' => Some(Command::MoveHalfDown),
                'k' => Some(Command::MoveHalfUp),
                'g' => Some(Command::MoveToBottom),
                'e' => Some(Command::OpenInEditor),
                'h' => Some(Command::HistoryBack),
                'l' => Some(Command::HistoryForward),
                'n' => Some(Command::SearchPrev),
                'a' => Some(Command::Rename(RenameKind::AppendEnd)),
                'i' => Some(Command::Rename(RenameKind::Insert)),
                's' => Some(Command::OpenShell),
                'v' => Some(Command::ToggleVisual),
                'r' => Some(Command::Reload),
                _ => None,
            };
        }
        if bare {
            return match lower {
                'q' => Some(Command::Quit),
                'r' => Some(Command::OpenWith),
                'j' => Some(Command::SelectNext),
                'k' => Some(Command::SelectPrevious),
                'h' => Some(Command::OpenParent),
                'l' => Some(Command::Activate),
                'n' => Some(Command::SearchNext),
                'f' => Some(Command::BeginFind),
                'a' => Some(Command::Rename(RenameKind::AppendStem)),
                'i' => Some(Command::DisplayFile),
                's' => Some(Command::BeginShell),
                'v' => Some(Command::ToggleMarkMove),
                'm' => Some(Command::BeginBookmarkSet),
                _ => None,
            };
        }
        None
    }
}
