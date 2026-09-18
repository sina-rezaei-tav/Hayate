//! Everything a background job can report back to the main loop, and the
//! single function that applies it to `AppState`.
//!
//! This is the one place that grows when a new background job is added:
//! add a variant here, and one match arm in `update`. `app::run`'s loop
//! itself never has to change.

use std::path::PathBuf;

use crate::fs::FileEntry;
use crate::preview::{FilePreview, PreviewPayload};

use super::AppState;

#[derive(Debug)]
pub enum Message {
    /// A batch of entries from the current directory's scanner.
    ScanBatch(u64, Vec<FileEntry>),
    /// The current directory's scanner has sent its last batch.
    ScanFinished(u64),
    /// `read_dir` failed for the current directory.
    ScanFailed(u64, String),
    /// A batch of entries from the parent directory's scanner.
    ParentScanBatch(u64, Vec<FileEntry>),
    /// The parent directory's scanner has sent its last batch.
    ParentScanFinished(u64),
    /// `read_dir` failed for the parent directory.
    ParentScanFailed(u64, String),
    /// The recursive file counter finished. The generation must match
    /// `AppState::count_generation`; `None` means the task was aborted or
    /// panicked before producing a result.
    RecursiveCountFinished(u64, Option<u64>),
    /// A background preview read finished for `path`. Applied only if that
    /// path is still the highlighted file.
    PreviewReady(PathBuf, PreviewPayload),
}

/// Applies one message to `state`. The single "reducer" for every
/// background job's output.
pub fn update(state: &mut AppState, message: Message) {
    match message {
        Message::ScanBatch(generation, batch) if generation == state.scan_generation => {
            let follow = state.selected_entry().map(|entry| entry.name.clone());
            state.entries.extend(batch);
            crate::fs::sort_listing(&mut state.entries);
            state.sync_selection_after_listing_change(follow.as_deref());
        }
        Message::ParentScanBatch(generation, batch) if generation == state.scan_generation => {
            state.parent_entries.extend(batch);
            crate::fs::sort_listing(&mut state.parent_entries);
        }
        Message::ScanFinished(generation) if generation == state.scan_generation => {
            state.current_listing_complete = true;
        }
        Message::ParentScanFinished(generation) if generation == state.scan_generation => {
            state.parent_listing_complete = true;
        }
        Message::ScanFailed(generation, err) if generation == state.scan_generation => {
            state.current_scan_error = Some(err);
        }
        Message::ParentScanFailed(generation, err) if generation == state.scan_generation => {
            state.parent_scan_error = Some(err);
        }
        Message::ScanBatch(_, _)
        | Message::ScanFinished(_)
        | Message::ScanFailed(_, _)
        | Message::ParentScanBatch(_, _)
        | Message::ParentScanFinished(_)
        | Message::ParentScanFailed(_, _) => {}
        Message::RecursiveCountFinished(generation, count) if generation == state.count_generation => {
            state.is_counting_recursively = false;
            if let Some(count) = count {
                state.recursive_file_count = Some(count);
            }
        }
        Message::RecursiveCountFinished(_, _) => {}
        Message::PreviewReady(path, payload) => {
            let still_selected = state
                .selected_entry()
                .is_some_and(|entry| !entry.is_dir && entry.path == path);
            if still_selected {
                state.preview = match payload {
                    PreviewPayload::Text { content, truncated } => {
                        FilePreview::Text {
                            path,
                            content,
                            truncated,
                        }
                    }
                    PreviewPayload::Binary => FilePreview::Binary { path },
                    PreviewPayload::Error(message) => FilePreview::Error { path, message },
                };
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
        update(&mut state, Message::ScanBatch(0, vec![entry(), entry()]));
        assert_eq!(state.entries.len(), 2);
    }

    #[test]
    fn scan_batches_are_kept_sorted_dirs_first_then_by_name() {
        fn names(state: &AppState) -> Vec<&str> {
            state.entries.iter().map(|entry| entry.name.as_str()).collect()
        }

        let mut state = state();
        update(
            &mut state,
            Message::ScanBatch(
                0,
                vec![
                    FileEntry::new("/tmp/z.txt".into(), false, 0),
                    FileEntry::new("/tmp/m".into(), true, 0),
                    FileEntry::new("/tmp/a.txt".into(), false, 0),
                    FileEntry::new("/tmp/b".into(), true, 0),
                ],
            ),
        );

        assert_eq!(names(&state), ["b", "m", "a.txt", "z.txt"]);
    }

    #[test]
    fn scan_finished_does_not_touch_entries() {
        let mut state = state();
        update(&mut state, Message::ScanBatch(0, vec![entry()]));
        update(&mut state, Message::ScanFinished(0));
        assert_eq!(state.entries.len(), 1);
        assert!(state.current_listing_complete);
    }

    #[test]
    fn parent_scan_batch_extends_parent_entries_only() {
        let mut state = state();
        update(&mut state, Message::ParentScanBatch(0, vec![entry(), entry()]));
        assert_eq!(state.parent_entries.len(), 2);
        assert!(state.entries.is_empty());
    }

    #[test]
    fn parent_scan_finished_does_not_touch_parent_entries() {
        let mut state = state();
        update(&mut state, Message::ParentScanBatch(0, vec![entry()]));
        update(&mut state, Message::ParentScanFinished(0));
        assert_eq!(state.parent_entries.len(), 1);
        assert!(state.parent_listing_complete);
    }

    #[test]
    fn stale_scan_batches_are_ignored() {
        let mut state = state();
        state.scan_generation = 2;

        update(&mut state, Message::ScanBatch(1, vec![entry()]));
        update(&mut state, Message::ParentScanBatch(1, vec![entry()]));
        update(&mut state, Message::ScanFinished(1));
        update(&mut state, Message::ParentScanFinished(1));

        assert!(state.entries.is_empty());
        assert!(state.parent_entries.is_empty());
    }

    #[test]
    fn recursive_count_finished_some_sets_count_and_clears_flag() {
        let mut state = state();
        state.is_counting_recursively = true;

        update(&mut state, Message::RecursiveCountFinished(0, Some(42)));

        assert_eq!(state.recursive_file_count, Some(42));
        assert!(!state.is_counting_recursively);
    }

    #[test]
    fn recursive_count_finished_none_clears_flag_but_keeps_previous_count() {
        let mut state = state();
        state.is_counting_recursively = true;
        state.recursive_file_count = Some(10);

        update(&mut state, Message::RecursiveCountFinished(0, None));

        assert_eq!(state.recursive_file_count, Some(10));
        assert!(!state.is_counting_recursively);
    }

    #[test]
    fn stale_recursive_count_finished_does_not_clear_a_newer_count() {
        let mut state = state();
        state.is_counting_recursively = true;
        state.count_generation = 2;

        update(&mut state, Message::RecursiveCountFinished(1, None));

        assert!(state.is_counting_recursively);
    }

    #[test]
    fn scan_failed_records_an_error() {
        let mut state = state();
        update(&mut state, Message::ScanFailed(0, "permission denied".into()));
        assert_eq!(state.current_scan_error.as_deref(), Some("permission denied"));
    }

    #[test]
    fn preview_ready_applies_when_that_file_is_still_selected() {
        let mut state = state();
        state.entries = vec![FileEntry::new("/tmp/a.txt".into(), false, 0)];
        state.preview = FilePreview::Loading("/tmp/a.txt".into());

        update(
            &mut state,
            Message::PreviewReady(
                "/tmp/a.txt".into(),
                PreviewPayload::Text {
                    content: "hello".into(),
                    truncated: false,
                },
            ),
        );

        assert!(matches!(
            state.preview,
            FilePreview::Text { ref content, .. } if content == "hello"
        ));
    }

    #[test]
    fn preview_ready_is_ignored_after_the_selection_moves() {
        let mut state = state();
        state.entries = vec![
            FileEntry::new("/tmp/a.txt".into(), false, 0),
            FileEntry::new("/tmp/b.txt".into(), false, 0),
        ];
        state.selected = 1;
        state.preview = FilePreview::Loading("/tmp/b.txt".into());

        update(
            &mut state,
            Message::PreviewReady(
                "/tmp/a.txt".into(),
                PreviewPayload::Text {
                    content: "stale".into(),
                    truncated: false,
                },
            ),
        );

        assert!(matches!(state.preview, FilePreview::Loading(_)));
    }

    #[test]
    fn a_scan_batch_restores_the_remembered_file_by_name() {
        let mut state = state();
        state.history.insert("/tmp".into(), "b.txt".into());

        update(
            &mut state,
            Message::ScanBatch(
                0,
                vec![
                    FileEntry::new("/tmp/z.txt".into(), false, 0),
                    FileEntry::new("/tmp/a.txt".into(), false, 0),
                    FileEntry::new("/tmp/b.txt".into(), false, 0),
                ],
            ),
        );

        assert_eq!(state.selected_entry().map(|e| e.name.as_str()), Some("b.txt"));
    }

    #[test]
    fn a_count_from_the_previous_directory_is_ignored() {
        let mut state = state();
        state.count_generation = 3;

        update(&mut state, Message::RecursiveCountFinished(2, Some(99)));

        assert!(state.recursive_file_count.is_none());
        assert!(!state.is_counting_recursively);
    }

    #[test]
    fn a_live_selection_is_not_overwritten_by_history_when_a_batch_arrives() {
        let mut state = state();
        state.history.insert("/tmp".into(), "b.txt".into());
        state.entries = vec![FileEntry::new("/tmp/a.txt".into(), false, 0)];
        state.selected = 0;

        update(
            &mut state,
            Message::ScanBatch(
                0,
                vec![
                    FileEntry::new("/tmp/z.txt".into(), false, 0),
                    FileEntry::new("/tmp/b.txt".into(), false, 0),
                ],
            ),
        );

        assert_eq!(state.selected_entry().map(|e| e.name.as_str()), Some("a.txt"));
    }

    #[test]
    fn stale_scan_failed_is_ignored() {
        let mut state = state();
        state.scan_generation = 2;

        update(&mut state, Message::ScanFailed(1, "gone".into()));
        update(&mut state, Message::ParentScanFailed(1, "gone".into()));

        assert!(state.current_scan_error.is_none());
        assert!(state.parent_scan_error.is_none());
    }

    #[test]
    fn parent_scan_failed_records_an_error() {
        let mut state = state();
        update(&mut state, Message::ParentScanFailed(0, "permission denied".into()));
        assert_eq!(state.parent_scan_error.as_deref(), Some("permission denied"));
    }
}
