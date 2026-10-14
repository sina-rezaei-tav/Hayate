//! Line editor shared by search, find, filter, rename, and the console.
//!
//! The cursor is a character index, so backspace on `héllo` deletes one
//! scalar, not one byte.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineKind {
    Search,
    Find,
    Filter,
    Rename(PathBuf),
    Console,
    Shell,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinePrompt {
    pub kind: LineKind,
    pub title: &'static str,
    pub buffer: String,
    pub cursor: usize,
    /// The next printable character replaces the buffer. Used by `cw`.
    pub replace_on_type: bool,
    /// Selection to restore when find or the prompt is cancelled.
    pub origin: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineAction {
    None,
    Close,
    Changed,
    Submit,
}

impl LinePrompt {
    pub fn new(kind: LineKind, title: &'static str, buffer: impl Into<String>, cursor: usize, replace_on_type: bool, origin: usize) -> Self {
        let buffer = buffer.into();
        let cursor = cursor.min(char_len(&buffer));
        Self {
            kind,
            title,
            buffer,
            cursor,
            replace_on_type,
            origin,
        }
    }
}

pub fn on_key(prompt: &mut LinePrompt, key: KeyEvent) -> LineAction {
    if key.kind != KeyEventKind::Press {
        return LineAction::None;
    }
    match (key.code, key.modifiers) {
        (KeyCode::Esc, KeyModifiers::NONE) => LineAction::Close,
        (KeyCode::Enter, KeyModifiers::NONE) => LineAction::Submit,
        (KeyCode::Backspace, KeyModifiers::NONE) => {
            backspace(prompt);
            LineAction::Changed
        }
        (KeyCode::Left, KeyModifiers::NONE) => {
            prompt.replace_on_type = false;
            prompt.cursor = prompt.cursor.saturating_sub(1);
            LineAction::None
        }
        (KeyCode::Right, KeyModifiers::NONE) => {
            prompt.replace_on_type = false;
            let len = char_len(&prompt.buffer);
            prompt.cursor = (prompt.cursor + 1).min(len);
            LineAction::None
        }
        (KeyCode::Home, KeyModifiers::NONE) => {
            prompt.replace_on_type = false;
            prompt.cursor = 0;
            LineAction::None
        }
        (KeyCode::End, KeyModifiers::NONE) => {
            prompt.replace_on_type = false;
            prompt.cursor = char_len(&prompt.buffer);
            LineAction::None
        }
        (KeyCode::Char('u'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            prompt.buffer.clear();
            prompt.cursor = 0;
            prompt.replace_on_type = false;
            LineAction::Changed
        }
        (KeyCode::Char(ch), modifiers) if modifiers == KeyModifiers::NONE || modifiers == KeyModifiers::SHIFT => {
            insert(prompt, ch);
            LineAction::Changed
        }
        _ => LineAction::None,
    }
}

fn insert(prompt: &mut LinePrompt, ch: char) {
    if prompt.replace_on_type {
        prompt.buffer.clear();
        prompt.cursor = 0;
        prompt.replace_on_type = false;
    }
    let index = byte_index(&prompt.buffer, prompt.cursor);
    prompt.buffer.insert(index, ch);
    prompt.cursor += 1;
}

fn backspace(prompt: &mut LinePrompt) {
    prompt.replace_on_type = false;
    if prompt.cursor == 0 {
        return;
    }
    let end = byte_index(&prompt.buffer, prompt.cursor);
    prompt.cursor -= 1;
    let start = byte_index(&prompt.buffer, prompt.cursor);
    prompt.buffer.replace_range(start..end, "");
}

pub fn char_len(text: &str) -> usize {
    text.chars().count()
}

fn byte_index(text: &str, cursor: usize) -> usize {
    text.char_indices().nth(cursor).map(|(index, _)| index).unwrap_or(text.len())
}

/// Cursor sitting on the last dot, so `a` edits `file.txt` before `.txt`.
/// A leading-dot name such as `.gitignore` has no extension; the cursor
/// goes to the end.
pub fn stem_cursor(name: &str) -> usize {
    let chars: Vec<char> = name.chars().collect();
    match chars.iter().rposition(|ch| *ch == '.') {
        Some(0) | None => chars.len(),
        Some(index) => index,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmPrompt {
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAction {
    None,
    Cancel,
    Confirm,
}

pub fn on_confirm_key(key: KeyEvent) -> ConfirmAction {
    if key.kind != KeyEventKind::Press {
        return ConfirmAction::None;
    }
    match (key.code, key.modifiers) {
        (KeyCode::Char('y' | 'Y'), KeyModifiers::NONE | KeyModifiers::SHIFT) => ConfirmAction::Confirm,
        (KeyCode::Esc | KeyCode::Char('n' | 'N'), KeyModifiers::NONE | KeyModifiers::SHIFT) => ConfirmAction::Cancel,
        _ => ConfirmAction::None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelpView {
    pub scroll: u16,
    pub count: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagerView {
    pub scroll: u16,
    pub count: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OneKey {
    BookmarkSet,
    BookmarkGo,
    BookmarkUnset,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn prompt(buffer: &str, cursor: usize) -> LinePrompt {
        LinePrompt::new(LineKind::Console, "console", buffer, cursor, false, 0)
    }

    #[test]
    fn backspace_deletes_one_unicode_scalar() {
        let mut prompt = prompt("héllo", 2);
        assert_eq!(on_key(&mut prompt, key(KeyCode::Backspace, KeyModifiers::NONE)), LineAction::Changed);
        assert_eq!(prompt.buffer, "hllo");
        assert_eq!(prompt.cursor, 1);
    }

    #[test]
    fn backspace_at_the_start_does_nothing() {
        let mut prompt = prompt("ab", 0);
        on_key(&mut prompt, key(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(prompt.buffer, "ab");
        assert_eq!(prompt.cursor, 0);
    }

    #[test]
    fn the_cursor_cannot_walk_off_either_end() {
        let mut prompt = prompt("ab", 0);
        on_key(&mut prompt, key(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(prompt.cursor, 0);
        on_key(&mut prompt, key(KeyCode::End, KeyModifiers::NONE));
        on_key(&mut prompt, key(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(prompt.cursor, 2);
    }

    #[test]
    fn insert_goes_through_the_cursor_and_replace_on_type_clears_first() {
        let mut prompt = prompt("ab", 1);
        on_key(&mut prompt, key(KeyCode::Char('X'), KeyModifiers::SHIFT));
        assert_eq!(prompt.buffer, "aXb");
        assert_eq!(prompt.cursor, 2);

        let mut prompt = LinePrompt::new(LineKind::Rename(PathBuf::from("/tmp/a")), "rename", "old", 0, true, 0);
        on_key(&mut prompt, key(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_eq!(prompt.buffer, "n");
        assert!(!prompt.replace_on_type);
    }

    #[test]
    fn an_arrow_cancels_replace_on_type() {
        let mut prompt = LinePrompt::new(LineKind::Rename(PathBuf::from("/tmp/a")), "rename", "old", 0, true, 0);
        on_key(&mut prompt, key(KeyCode::Right, KeyModifiers::NONE));
        on_key(&mut prompt, key(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(prompt.buffer, "oxld");
    }

    #[test]
    fn ctrl_u_clears_and_enter_submits_and_escape_closes() {
        let mut prompt = prompt("abc", 2);
        on_key(&mut prompt, key(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(prompt.buffer, "");
        assert_eq!(on_key(&mut prompt, key(KeyCode::Enter, KeyModifiers::NONE)), LineAction::Submit);
        assert_eq!(on_key(&mut prompt, key(KeyCode::Esc, KeyModifiers::NONE)), LineAction::Close);
    }

    #[test]
    fn q_is_a_character_not_a_close() {
        let mut prompt = prompt("", 0);
        assert_eq!(on_key(&mut prompt, key(KeyCode::Char('q'), KeyModifiers::NONE)), LineAction::Changed);
        assert_eq!(prompt.buffer, "q");
    }

    #[test]
    fn stem_cursor_stops_before_the_last_extension() {
        assert_eq!(stem_cursor("file.txt"), 4);
        assert_eq!(stem_cursor("a.tar.gz"), 5);
        assert_eq!(stem_cursor(".gitignore"), ".gitignore".chars().count());
        assert_eq!(stem_cursor("README"), 6);
        assert_eq!(stem_cursor("héllo.txt"), 5);
    }

    #[test]
    fn confirm_accepts_only_y() {
        assert_eq!(on_confirm_key(key(KeyCode::Char('y'), KeyModifiers::NONE)), ConfirmAction::Confirm);
        assert_eq!(on_confirm_key(key(KeyCode::Char('Y'), KeyModifiers::SHIFT)), ConfirmAction::Confirm);
        assert_eq!(on_confirm_key(key(KeyCode::Enter, KeyModifiers::NONE)), ConfirmAction::None);
        assert_eq!(on_confirm_key(key(KeyCode::Char('q'), KeyModifiers::NONE)), ConfirmAction::None);
        assert_eq!(on_confirm_key(key(KeyCode::Esc, KeyModifiers::NONE)), ConfirmAction::Cancel);
        assert_eq!(on_confirm_key(key(KeyCode::Char('n'), KeyModifiers::NONE)), ConfirmAction::Cancel);
    }
}
