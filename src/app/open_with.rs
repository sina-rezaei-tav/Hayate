//! "Open with" picker: a typed command plus a capped PATH filter.
//!
//! Listing PATH is I/O and belongs in a background job. Filtering, row
//! construction, and key handling are pure so the modal can be tested
//! without a TTY.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::PathBuf;

use compact_str::CompactString;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio_util::sync::CancellationToken;

use super::editor::{self, EditorError, EditorLaunch};

/// Maximum PATH binaries kept in memory. Bounds a huge PATH without trying
/// to present every executable as a menu.
pub const CANDIDATE_LIMIT: usize = 4096;

/// Maximum binary rows shown after filtering. The typed-command row is extra.
pub const DISPLAY_LIMIT: usize = 20;

/// Shown when the query is empty, in this order, and only if they exist on PATH.
pub const CURATED: &[&str] = &["nvim", "vim", "vi", "nano", "xdg-open", "less"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionMode {
    Browser,
    OpenWith(OpenWithPrompt),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenWithPrompt {
    pub target: PathBuf,
    pub target_name: CompactString,
    pub query: String,
    pub candidates: Vec<CompactString>,
    pub selected: usize,
    pub generation: u64,
    pub listing_complete: bool,
    pub listing_error: Option<String>,
}

impl OpenWithPrompt {
    pub fn new(target: PathBuf, generation: u64) -> Self {
        let target_name = target
            .file_name()
            .map(|name| CompactString::from(name.to_string_lossy()))
            .unwrap_or_else(|| CompactString::from(target.to_string_lossy()));
        Self {
            target,
            target_name,
            query: String::new(),
            candidates: Vec::new(),
            selected: 0,
            generation,
            listing_complete: false,
            listing_error: None,
        }
    }

    pub fn visible_rows(&self) -> Vec<OpenWithRow> {
        visible_rows(&self.query, &self.candidates)
    }

    pub fn clamp_selected(&mut self) {
        let len = self.visible_rows().len();
        if len == 0 {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(len - 1);
        }
    }

    pub fn set_candidates(&mut self, names: Vec<CompactString>) {
        self.candidates = names;
        self.listing_complete = true;
        self.listing_error = None;
        self.clamp_selected();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenWithRow {
    /// The raw typed command, including flags (`code --wait`).
    Custom(String),
    Binary(CompactString),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenWithAction {
    None,
    Close,
    Launch(EditorLaunch),
    /// Stay in the modal; the loop copies this into `AppState::notice`.
    Notice(String),
}

pub fn visible_rows(query: &str, candidates: &[CompactString]) -> Vec<OpenWithRow> {
    let trimmed = query.trim();
    let mut rows = Vec::new();
    if !trimmed.is_empty() {
        rows.push(OpenWithRow::Custom(trimmed.to_string()));
    }
    rows.extend(
        matching_binaries(trimmed, candidates)
            .into_iter()
            .map(OpenWithRow::Binary),
    );
    rows
}

fn matching_binaries(query: &str, candidates: &[CompactString]) -> Vec<CompactString> {
    if query.is_empty() {
        return CURATED
            .iter()
            .filter_map(|name| {
                candidates
                    .iter()
                    .find(|candidate| candidate.as_str() == *name)
                    .cloned()
            })
            .collect();
    }

    let needle = query
        .split_whitespace()
        .next()
        .unwrap_or(query)
        .to_ascii_lowercase();
    let mut prefix = Vec::new();
    let mut contains = Vec::new();
    for name in candidates {
        let lower = name.as_str().to_ascii_lowercase();
        if lower.starts_with(&needle) {
            prefix.push(name.clone());
        } else if lower.contains(&needle) {
            contains.push(name.clone());
        }
    }
    prefix.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    contains.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    prefix
        .into_iter()
        .chain(contains)
        .take(DISPLAY_LIMIT)
        .collect()
}

pub fn confirm(prompt: &OpenWithPrompt) -> Result<EditorLaunch, EditorError> {
    let rows = prompt.visible_rows();
    if rows.is_empty() {
        return Err(EditorError::Empty);
    }
    let index = prompt.selected.min(rows.len() - 1);
    let command = match &rows[index] {
        OpenWithRow::Custom(query) => query.as_str(),
        OpenWithRow::Binary(name) => name.as_str(),
    };
    editor::launch_spec(command, &prompt.target)
}

/// Modal keymap. `j`/`k` move the picker, not the file list. `q` types `q`.
pub fn handle_key(prompt: &mut OpenWithPrompt, key: KeyEvent) -> OpenWithAction {
    if key.kind != KeyEventKind::Press {
        return OpenWithAction::None;
    }

    match (key.code, key.modifiers) {
        (KeyCode::Esc, KeyModifiers::NONE) => OpenWithAction::Close,
        (KeyCode::Enter, KeyModifiers::NONE) => match confirm(prompt) {
            Ok(spec) => OpenWithAction::Launch(spec),
            Err(err) => OpenWithAction::Notice(err.to_string()),
        },
        (KeyCode::Up | KeyCode::Char('k'), KeyModifiers::NONE) => {
            prompt.selected = prompt.selected.saturating_sub(1);
            prompt.clamp_selected();
            OpenWithAction::None
        }
        (KeyCode::Down | KeyCode::Char('j'), KeyModifiers::NONE) => {
            if !prompt.visible_rows().is_empty() {
                prompt.selected = prompt.selected.saturating_add(1);
                prompt.clamp_selected();
            }
            OpenWithAction::None
        }
        (KeyCode::Backspace, KeyModifiers::NONE) => {
            prompt.query.pop();
            prompt.selected = 0;
            OpenWithAction::None
        }
        (KeyCode::Char('u'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            prompt.query.clear();
            prompt.selected = 0;
            OpenWithAction::None
        }
        (KeyCode::Char(ch), modifiers)
            if modifiers == KeyModifiers::NONE || modifiers == KeyModifiers::SHIFT =>
        {
            prompt.query.push(ch);
            prompt.selected = 0;
            OpenWithAction::None
        }
        _ => OpenWithAction::None,
    }
}

/// Unique executable names on `PATH`, first directory wins (same as `which`).
/// Stops at [`CANDIDATE_LIMIT`] or when `cancel` fires.
pub fn list_path_executables(
    path_var: Option<&OsStr>,
    cancel: &CancellationToken,
) -> Vec<CompactString> {
    let Some(path_var) = path_var else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for dir in std::env::split_paths(path_var) {
        if cancel.is_cancelled() {
            break;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries {
            if cancel.is_cancelled() || names.len() >= CANDIDATE_LIMIT {
                break;
            }
            let Ok(entry) = entry else {
                continue;
            };
            let os_name = entry.file_name();
            let name = os_name.to_string_lossy();
            if name.starts_with('.') || name.is_empty() {
                continue;
            }
            if !seen.insert(CompactString::from(name.as_ref())) {
                continue;
            }
            if !editor::is_runnable(&entry.path()) {
                seen.remove(name.as_ref());
                continue;
            }
            names.push(CompactString::from(name.as_ref()));
        }
        if names.len() >= CANDIDATE_LIMIT {
            break;
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn names(list: &[&str]) -> Vec<CompactString> {
        list.iter().map(|name| CompactString::from(*name)).collect()
    }

    fn prompt_with(candidates: &[&str]) -> OpenWithPrompt {
        let mut prompt = OpenWithPrompt::new(PathBuf::from("/tmp/notes.txt"), 1);
        prompt.set_candidates(names(candidates));
        prompt
    }

    #[test]
    fn empty_query_shows_curated_binaries_that_exist_on_path() {
        let rows = visible_rows("", &names(&["less", "cat", "vim", "firefox"]));
        assert_eq!(
            rows,
            vec![
                OpenWithRow::Binary("vim".into()),
                OpenWithRow::Binary("less".into()),
            ]
        );
    }

    #[test]
    fn empty_query_hides_curated_names_that_are_missing() {
        let rows = visible_rows("", &names(&["cat", "grep"]));
        assert!(rows.is_empty());
    }

    #[test]
    fn non_empty_query_puts_the_typed_command_first() {
        let rows = visible_rows("mpv --fs", &names(&["mpv", "vim"]));
        assert_eq!(rows[0], OpenWithRow::Custom("mpv --fs".into()));
        assert!(rows.contains(&OpenWithRow::Binary("mpv".into())));
    }

    #[test]
    fn prefix_matches_rank_above_substring_matches() {
        let rows = visible_rows("vi", &names(&["nvim", "vim", "vi", "evil"]));
        let binaries: Vec<_> = rows
            .into_iter()
            .filter_map(|row| match row {
                OpenWithRow::Binary(name) => Some(name.to_string()),
                OpenWithRow::Custom(_) => None,
            })
            .collect();
        assert_eq!(binaries, vec!["vi", "vim", "evil", "nvim"]);
    }

    #[test]
    fn matching_is_capped() {
        let candidates: Vec<CompactString> = (0..50)
            .map(|i| CompactString::from(format!("tool-{i:02}")))
            .collect();
        let rows = visible_rows("tool", &candidates);
        let binaries = rows
            .iter()
            .filter(|row| matches!(row, OpenWithRow::Binary(_)))
            .count();
        assert_eq!(binaries, DISPLAY_LIMIT);
    }

    #[test]
    fn confirm_empty_rows_is_an_error() {
        let prompt = prompt_with(&["cat"]);
        assert_eq!(confirm(&prompt), Err(EditorError::Empty));
    }

    #[test]
    fn confirm_custom_row_keeps_flags_and_appends_the_file() {
        let mut prompt = prompt_with(&["code"]);
        prompt.query = "code --wait".into();
        let spec = confirm(&prompt).unwrap();
        assert_eq!(spec.program, std::ffi::OsString::from("code"));
        assert_eq!(
            spec.args,
            vec![
                std::ffi::OsString::from("--wait"),
                std::ffi::OsString::from("/tmp/notes.txt"),
            ]
        );
    }

    #[test]
    fn confirm_binary_row_appends_the_file() {
        let mut prompt = prompt_with(&["vim", "nano"]);
        prompt.selected = 1;
        let spec = confirm(&prompt).unwrap();
        assert_eq!(spec.program, std::ffi::OsString::from("nano"));
        assert_eq!(spec.args, vec![std::ffi::OsString::from("/tmp/notes.txt")]);
    }

    #[test]
    fn typing_resets_the_highlight_to_the_typed_command() {
        let mut prompt = prompt_with(&["vim", "nano"]);
        prompt.selected = 1;
        let action = handle_key(&mut prompt, key(KeyCode::Char('m'), KeyModifiers::NONE));
        assert_eq!(action, OpenWithAction::None);
        assert_eq!(prompt.query, "m");
        assert_eq!(prompt.selected, 0);
        assert_eq!(prompt.visible_rows()[0], OpenWithRow::Custom("m".into()));
    }

    #[test]
    fn j_and_k_move_the_picker_not_past_the_ends() {
        let mut prompt = prompt_with(&["nvim", "vim"]);
        assert_eq!(prompt.visible_rows().len(), 2);

        handle_key(&mut prompt, key(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(prompt.selected, 1);
        handle_key(&mut prompt, key(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(prompt.selected, 1, "must not wrap past the last row");

        handle_key(&mut prompt, key(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(prompt.selected, 0);
        handle_key(&mut prompt, key(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(prompt.selected, 0, "must not wrap past the first row");
    }

    #[test]
    fn q_types_into_the_query_instead_of_quitting() {
        let mut prompt = prompt_with(&["vim"]);
        handle_key(&mut prompt, key(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(prompt.query, "q");
    }

    #[test]
    fn esc_closes_the_modal() {
        let mut prompt = prompt_with(&["vim"]);
        assert_eq!(
            handle_key(&mut prompt, key(KeyCode::Esc, KeyModifiers::NONE)),
            OpenWithAction::Close
        );
    }

    #[test]
    fn enter_on_empty_picker_returns_a_notice() {
        let mut prompt = prompt_with(&["cat"]);
        assert_eq!(
            handle_key(&mut prompt, key(KeyCode::Enter, KeyModifiers::NONE)),
            OpenWithAction::Notice(EditorError::Empty.to_string())
        );
    }

    #[test]
    fn ctrl_u_clears_the_query() {
        let mut prompt = prompt_with(&["vim"]);
        prompt.query = "nvim".into();
        handle_key(
            &mut prompt,
            key(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        assert!(prompt.query.is_empty());
        assert_eq!(prompt.selected, 0);
    }

    #[test]
    fn backspace_edits_the_query() {
        let mut prompt = prompt_with(&["vim"]);
        prompt.query = "vi".into();
        handle_key(&mut prompt, key(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(prompt.query, "v");
    }

    #[test]
    fn release_kind_is_ignored() {
        let mut prompt = prompt_with(&["vim"]);
        let key = KeyEvent::new_with_kind(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        handle_key(&mut prompt, key);
        assert!(prompt.query.is_empty());
    }

    #[cfg(unix)]
    fn make_executable(dir: &Path, name: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn list_path_executables_keeps_the_first_path_directory_win() {
        let early = tempfile::tempdir().unwrap();
        let late = tempfile::tempdir().unwrap();
        make_executable(early.path(), "vim");
        make_executable(late.path(), "vim");
        make_executable(late.path(), "nvim");
        std::fs::write(early.path().join("not-exec"), b"x").unwrap();

        let path = std::env::join_paths([early.path(), late.path()]).unwrap();
        let names = list_path_executables(Some(path.as_os_str()), &CancellationToken::new());
        let set: HashSet<_> = names.iter().map(|name| name.as_str()).collect();
        assert!(set.contains("vim"));
        assert!(set.contains("nvim"));
        assert!(!set.contains("not-exec"));
        assert_eq!(
            names.iter().filter(|name| name.as_str() == "vim").count(),
            1,
            "the later PATH vim must not appear twice"
        );
    }

    #[cfg(unix)]
    #[test]
    fn list_path_executables_stops_when_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        make_executable(dir.path(), "vim");
        let path = std::env::join_paths([dir.path()]).unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let names = list_path_executables(Some(path.as_os_str()), &cancel);
        assert!(names.is_empty());
    }
}
