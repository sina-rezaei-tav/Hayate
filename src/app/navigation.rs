//! Pure directory-change logic. Returns which scans the loop should spawn;
//! it never touches Tokio or the filesystem itself.

use std::path::{Path, PathBuf};

use super::AppState;

/// Scans to start after a successful directory change. Either field being
/// `None` means that listing was reused from already-scanned state and
/// does not need a new walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanPlan {
    pub current: Option<PathBuf>,
    pub parent: Option<PathBuf>,
}

/// Enters the highlighted directory. No-op if nothing is selected or the
/// selection is a file.
///
/// Reuses the current listing as the new parent pane, so we do not
/// re-walk a directory we just displayed.
pub fn enter_selected(state: &mut AppState) -> Option<ScanPlan> {
    let entry = state.selected_entry()?;
    if !entry.is_dir {
        return None;
    }
    let new_dir = entry.path.clone();
    if new_dir == state.current_dir {
        return None;
    }

    begin_generation(state);
    state.parent_entries = std::mem::take(&mut state.entries);
    state.current_dir = new_dir.clone();
    state.selected = state.history.get(&new_dir).copied().unwrap_or(0);

    Some(ScanPlan {
        current: Some(new_dir),
        parent: None,
    })
}

/// Moves up to the parent directory. No-op at the filesystem root.
///
/// Reuses the parent listing as the new current pane when it is already
/// populated; otherwise the caller must scan current too.
pub fn open_parent(state: &mut AppState) -> Option<ScanPlan> {
    let parent = usable_parent(&state.current_dir)?;
    let left = state.current_dir.clone();

    begin_generation(state);
    let reused = std::mem::take(&mut state.parent_entries);
    state.current_dir = parent.clone();
    state.entries = reused;
    state.selected = select_left_child_or_history(state, &left);

    Some(ScanPlan {
        current: if state.entries.is_empty() {
            Some(parent.clone())
        } else {
            None
        },
        parent: usable_parent(&parent),
    })
}

fn begin_generation(state: &mut AppState) {
    state.history.insert(state.current_dir.clone(), state.selected);
    state.scan_generation = state.scan_generation.wrapping_add(1);
    state.recursive_file_count = None;
    state.is_counting_recursively = false;
}

fn usable_parent(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    if parent.as_os_str().is_empty() {
        None
    } else {
        Some(parent.to_path_buf())
    }
}

/// Scans to start when the app first opens `state.current_dir`.
pub fn initial_scan_plan(state: &AppState) -> ScanPlan {
    ScanPlan {
        current: Some(state.current_dir.clone()),
        parent: usable_parent(&state.current_dir),
    }
}

fn select_left_child_or_history(state: &AppState, left: &Path) -> usize {
    if let Some(name) = left.file_name() {
        let name = name.to_string_lossy();
        if let Some(index) = state.entries.iter().position(|entry| entry.name.as_str() == name) {
            return index;
        }
    }
    state.history.get(&state.current_dir).copied().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use crate::fs::FileEntry;

    use super::*;

    fn dir(path: &str) -> FileEntry {
        FileEntry::new(path.into(), true, 0)
    }

    fn file(path: &str) -> FileEntry {
        FileEntry::new(path.into(), false, 0)
    }

    fn state_in_project() -> AppState {
        let mut state = AppState::new("/tmp/project".into());
        state.entries = vec![dir("/tmp/project/src"), file("/tmp/project/README.md")];
        state.parent_entries = vec![dir("/tmp/project"), dir("/tmp/other")];
        state.selected = 0;
        state
    }

    #[test]
    fn entering_a_file_is_a_noop() {
        let mut state = state_in_project();
        state.selected = 1;
        let generation = state.scan_generation;

        assert!(enter_selected(&mut state).is_none());
        assert_eq!(state.current_dir, PathBuf::from("/tmp/project"));
        assert_eq!(state.scan_generation, generation);
        assert_eq!(state.entries.len(), 2);
    }

    #[test]
    fn entering_nothing_selected_is_a_noop() {
        let mut state = AppState::new("/tmp".into());
        assert!(enter_selected(&mut state).is_none());
    }

    #[test]
    fn entering_a_directory_reuses_current_listing_as_parent() {
        let mut state = state_in_project();

        let plan = enter_selected(&mut state).expect("should navigate");

        assert_eq!(state.current_dir, PathBuf::from("/tmp/project/src"));
        assert_eq!(state.scan_generation, 1);
        assert!(state.entries.is_empty());
        assert_eq!(state.parent_entries.len(), 2);
        assert_eq!(state.parent_entries[0].name, "src");
        assert_eq!(state.history.get(&PathBuf::from("/tmp/project")), Some(&0));
        assert_eq!(
            plan,
            ScanPlan {
                current: Some("/tmp/project/src".into()),
                parent: None,
            }
        );
        assert!(state.recursive_file_count.is_none());
        assert!(!state.is_counting_recursively);
    }

    #[test]
    fn entering_restores_saved_selection() {
        let mut state = state_in_project();
        state.history.insert("/tmp/project/src".into(), 7);

        enter_selected(&mut state).unwrap();

        assert_eq!(state.selected, 7);
    }

    #[test]
    fn opening_parent_reuses_parent_listing_and_highlights_where_we_came_from() {
        let mut state = state_in_project();
        state.selected = 1; // README, so history records that.

        let plan = open_parent(&mut state).expect("should navigate");

        assert_eq!(state.current_dir, PathBuf::from("/tmp"));
        assert_eq!(state.entries.len(), 2);
        assert_eq!(state.entries[0].name, "project");
        assert_eq!(state.selected, 0, "the directory we left should be highlighted");
        assert!(state.parent_entries.is_empty());
        assert_eq!(state.history.get(&PathBuf::from("/tmp/project")), Some(&1));
        assert_eq!(
            plan,
            ScanPlan {
                current: None,
                parent: Some("/".into()),
            }
        );
    }

    #[test]
    fn opening_parent_scans_current_if_the_parent_listing_was_still_empty() {
        let mut state = AppState::new("/tmp/project".into());
        state.parent_entries.clear();

        let plan = open_parent(&mut state).unwrap();

        assert_eq!(
            plan,
            ScanPlan {
                current: Some("/tmp".into()),
                parent: Some("/".into()),
            }
        );
    }

    #[test]
    fn opening_parent_at_root_is_a_noop() {
        let mut state = AppState::new("/".into());
        let generation = state.scan_generation;

        assert!(open_parent(&mut state).is_none());
        assert_eq!(state.scan_generation, generation);
    }

    #[test]
    fn initial_scan_plan_covers_current_and_parent() {
        let state = AppState::new("/tmp/project".into());
        assert_eq!(
            initial_scan_plan(&state),
            ScanPlan {
                current: Some("/tmp/project".into()),
                parent: Some("/tmp".into()),
            }
        );
    }

    #[test]
    fn initial_scan_plan_at_root_has_no_parent() {
        let state = AppState::new("/".into());
        assert_eq!(
            initial_scan_plan(&state),
            ScanPlan {
                current: Some("/".into()),
                parent: None,
            }
        );
    }

    #[test]
    fn round_trip_restores_selection_in_the_child() {
        let mut state = state_in_project();
        state.selected = 0;
        enter_selected(&mut state).unwrap();
        state.entries = vec![file("/tmp/project/src/main.rs"), file("/tmp/project/src/lib.rs")];
        state.selected = 1;

        open_parent(&mut state).unwrap();
        enter_selected(&mut state).unwrap();

        assert_eq!(state.current_dir, PathBuf::from("/tmp/project/src"));
        assert_eq!(state.selected, 1);
    }
}
