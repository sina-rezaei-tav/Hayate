use std::collections::HashMap;
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
    /// Intended highlight index into `entries`. May temporarily sit past
    /// `entries.len()` while a scan is still streaming in; use
    /// [`selected_index`](Self::selected_index) / [`selected_entry`](Self::selected_entry)
    /// to read a clamped value.
    pub selected: usize,
    /// Last highlighted index per visited directory, restored on revisit.
    pub history: HashMap<PathBuf, usize>,
    /// Incremented on every directory change. Scan messages carry the
    /// generation they were started with so a cancelled walk cannot apply
    /// late batches to the new listing.
    pub scan_generation: u64,
    /// Cached contents of the highlighted file. The UI never reads disk
    /// for this; a background job fills it in.
    pub preview: crate::preview::FilePreview,
    /// Vertical offset (in wrapped lines) of the preview pane. Reset when
    /// the highlighted file changes.
    pub preview_scroll: u16,
    /// Last drawn terminal size, used to page the preview by a real pane
    /// of lines instead of a guessed constant.
    pub frame_width: u16,
    pub frame_height: u16,
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
            history: HashMap::new(),
            scan_generation: 0,
            preview: crate::preview::FilePreview::Idle,
            preview_scroll: 0,
            frame_width: 80,
            frame_height: 24,
            recursive_file_count: None,
            is_counting_recursively: false,
        }
    }

    /// Clamped highlight index, or `None` if the list is empty.
    pub fn selected_index(&self) -> Option<usize> {
        if self.entries.is_empty() {
            None
        } else {
            Some(self.selected.min(self.entries.len() - 1))
        }
    }

    /// The currently highlighted entry, or `None` if the list is empty.
    pub fn selected_entry(&self) -> Option<&FileEntry> {
        self.selected_index().and_then(|i| self.entries.get(i))
    }

    /// Moves the preview by `delta` wrapped lines, clamped to `limit`
    /// (the maximum scroll computed from the current pane size).
    pub fn scroll_preview(&mut self, delta: i32, limit: u16) {
        let next = i32::from(self.preview_scroll).saturating_add(delta);
        self.preview_scroll = next.clamp(0, i32::from(limit)) as u16;
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

    #[test]
    fn selected_index_clamps_until_the_scan_catches_up() {
        let mut state = AppState::new("/tmp".into());
        state.selected = 10;
        state.entries = vec![
            FileEntry::new("/tmp/a.txt".into(), false, 0),
            FileEntry::new("/tmp/b.txt".into(), false, 0),
        ];

        assert_eq!(state.selected_index(), Some(1));
        assert_eq!(state.selected_entry().map(|e| e.name.as_str()), Some("b.txt"));
        assert_eq!(state.selected, 10, "the intended index is kept for later batches");
    }

    #[test]
    fn scroll_preview_clamps_to_the_limit() {
        let mut state = AppState::new("/tmp".into());
        state.scroll_preview(5, 3);
        assert_eq!(state.preview_scroll, 3);
        state.scroll_preview(-10, 3);
        assert_eq!(state.preview_scroll, 0);
    }
}
