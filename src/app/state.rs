use std::path::PathBuf;

use crate::fs::FileEntry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    pub should_quit: bool,
    pub current_dir: PathBuf,
    pub entries: Vec<FileEntry>,
    /// `current_dir`'s parent's entries, for the Miller-column "parent"
    /// pane. Empty (rather than `Option`) when `current_dir` has no parent
    /// (filesystem root) or the scan simply hasn't delivered anything yet.
    pub parent_entries: Vec<FileEntry>,
    /// Index into `entries` of the highlighted item. Not `Option<usize>`:
    /// an empty list and "nothing selected" are the same rendering case,
    /// so callers just check `entries.is_empty()` rather than unwrapping.
    pub selected: usize,
    /// Result of the last recursive file count (triggered by pressing 'r'),
    /// if one has completed.
    pub recursive_file_count: Option<u64>,
    /// Whether a recursive count is currently running in the background.
    pub is_counting_recursively: bool,
}

impl AppState {
    pub fn new(current_dir: PathBuf) -> Self {
        Self {
            should_quit: false,
            current_dir,
            entries: Vec::new(),
            parent_entries: Vec::new(),
            selected: 0,
            recursive_file_count: None,
            is_counting_recursively: false,
        }
    }

    /// The currently highlighted entry, or `None` if the list is empty.
    pub fn selected_entry(&self) -> Option<&FileEntry> {
        self.entries.get(self.selected)
    }
}

impl Default for AppState {
    fn default() -> Self {
        // Falls back to "." rather than unwrapping, since the cwd can be
        // unreadable (e.g. removed out from under the process).
        Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_not_quitting_with_the_given_dir() {
        let state = AppState::new(PathBuf::from("/tmp"));
        assert!(!state.should_quit);
        assert_eq!(state.current_dir, PathBuf::from("/tmp"));
    }

    #[test]
    fn default_starts_not_quitting() {
        assert!(!AppState::default().should_quit);
    }

    #[test]
    fn selected_entry_is_none_when_empty() {
        assert!(AppState::new("/tmp".into()).selected_entry().is_none());
    }

    #[test]
    fn selected_entry_returns_the_entry_at_the_selected_index() {
        let mut state = AppState::new("/tmp".into());
        state.entries = vec![
            FileEntry::new("/tmp/a.txt".into(), false, 0),
            FileEntry::new("/tmp/b.txt".into(), false, 0),
        ];
        state.selected = 1;

        assert_eq!(state.selected_entry().map(|e| e.name.as_str()), Some("b.txt"));
    }
}
