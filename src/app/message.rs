//! Everything a background job can report back to the main loop, and the
//! single function that applies it to `AppState`.
//!
//! This is the one place that grows when a new background job is added:
//! add a variant here, and one match arm in `update`. `app::run`'s loop
//! itself never has to change.

use crate::fs::FileEntry;

use super::AppState;

#[derive(Debug)]
pub enum Message {
    /// A batch of entries from the current directory's scanner.
    ScanBatch(Vec<FileEntry>),
    /// The current directory's scanner has sent its last batch.
    ScanFinished,
    /// A batch of entries from the parent directory's scanner.
    ParentScanBatch(Vec<FileEntry>),
    /// The parent directory's scanner has sent its last batch.
    ParentScanFinished,
    /// The recursive file counter finished. `None` means the task was
    /// aborted or panicked before producing a result.
    RecursiveCountFinished(Option<u64>),
}

/// Applies one message to `state`. The single "reducer" for every
/// background job's output.
pub fn update(state: &mut AppState, message: Message) {
    match message {
        Message::ScanBatch(batch) => state.entries.extend(batch),
        Message::ScanFinished => {}
        Message::ParentScanBatch(batch) => state.parent_entries.extend(batch),
        Message::ParentScanFinished => {}
        Message::RecursiveCountFinished(count) => {
            state.is_counting_recursively = false;
            if let Some(count) = count {
                state.recursive_file_count = Some(count);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        AppState::new("/tmp".into())
    }

    fn entry() -> FileEntry {
        FileEntry::new("/tmp/a.txt".into(), false, 0)
    }

    #[test]
    fn scan_batch_extends_entries() {
        let mut state = state();
        update(&mut state, Message::ScanBatch(vec![entry(), entry()]));
        assert_eq!(state.entries.len(), 2);
    }

    #[test]
    fn scan_finished_does_not_touch_entries() {
        let mut state = state();
        update(&mut state, Message::ScanBatch(vec![entry()]));
        update(&mut state, Message::ScanFinished);
        assert_eq!(state.entries.len(), 1);
    }

    #[test]
    fn parent_scan_batch_extends_parent_entries_only() {
        let mut state = state();
        update(&mut state, Message::ParentScanBatch(vec![entry(), entry()]));
        assert_eq!(state.parent_entries.len(), 2);
        assert!(state.entries.is_empty());
    }

    #[test]
    fn parent_scan_finished_does_not_touch_parent_entries() {
        let mut state = state();
        update(&mut state, Message::ParentScanBatch(vec![entry()]));
        update(&mut state, Message::ParentScanFinished);
        assert_eq!(state.parent_entries.len(), 1);
    }

    #[test]
    fn recursive_count_finished_some_sets_count_and_clears_flag() {
        let mut state = state();
        state.is_counting_recursively = true;

        update(&mut state, Message::RecursiveCountFinished(Some(42)));

        assert_eq!(state.recursive_file_count, Some(42));
        assert!(!state.is_counting_recursively);
    }

    #[test]
    fn recursive_count_finished_none_clears_flag_but_keeps_previous_count() {
        let mut state = state();
        state.is_counting_recursively = true;
        state.recursive_file_count = Some(10);

        update(&mut state, Message::RecursiveCountFinished(None));

        assert_eq!(state.recursive_file_count, Some(10));
        assert!(!state.is_counting_recursively);
    }
}
