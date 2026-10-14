//! Selection math against the *visible* listing.
//!
//! `AppState.selected` stays an index into the full scan so a streaming
//! directory can keep an index that is still past the end. Motions walk
//! only the rows the user can see, so `j` never lands on a hidden file
//! while hidden files are off.

use super::state::AppState;
use crate::fs::{FileEntry, SortSpec};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct View {
    /// Ranger's default is off. The scan still contains dotfiles; this only
    /// decides which rows motions and the panes show.
    pub show_hidden: bool,
    pub sort: SortSpec,
    /// Literal substring. Empty is treated as no filter.
    pub filter: Option<String>,
    pub preview_images: bool,
}

impl View {
    pub fn shows(&self, entry: &FileEntry) -> bool {
        if !self.show_hidden && entry.is_hidden {
            return false;
        }
        match self.filter.as_deref() {
            Some(pattern) if !pattern.is_empty() => entry.name.contains(pattern),
            _ => true,
        }
    }

    /// The parent pane always keeps the directory we are inside, even when
    /// that directory is hidden and hidden files are off. Otherwise leaving
    /// `~/.config` would show a parent list with no highlight.
    pub fn shows_parent(&self, entry: &FileEntry, current_name: Option<&str>) -> bool {
        if current_name == Some(entry.name.as_str()) {
            return true;
        }
        self.shows(entry)
    }
}

pub fn visible_positions(entries: &[FileEntry], view: &View, pin: Option<&std::path::Path>) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| view.shows(entry) || pin.is_some_and(|pin| pin == entry.path.as_path()))
        .map(|(index, _)| index)
        .collect()
}

pub fn highlighted_index(selected: usize, entries: &[FileEntry], view: &View, pin: Option<&std::path::Path>) -> Option<usize> {
    let positions = visible_positions(entries, view, pin);
    visible_slot(selected, entries.len(), &positions).map(|slot| positions[slot])
}

fn visible_slot(selected: usize, len: usize, positions: &[usize]) -> Option<usize> {
    if positions.is_empty() || len == 0 {
        return None;
    }
    let clamped = selected.min(len - 1);
    if let Some(slot) = positions.iter().position(|index| *index == clamped) {
        return Some(slot);
    }
    positions.iter().rposition(|index| *index < clamped).or(Some(0))
}

/// `None` and `Some(0)` both mean "no quantifier" (ranger treats 0 as falsy).
pub fn repeat(count: Option<u32>) -> usize {
    match count {
        Some(n) if n > 0 => match usize::try_from(n) {
            Ok(n) => n,
            Err(_) => usize::MAX,
        },
        _ => 1,
    }
}

pub fn move_by(state: &mut AppState, delta: isize) {
    let positions = visible_positions(&state.entries, &state.view, state.pinned.as_deref());
    let Some(slot) = visible_slot(state.selected, state.entries.len(), &positions) else {
        return;
    };
    let last = positions.len() as isize - 1;
    let next = (slot as isize).saturating_add(delta).clamp(0, last) as usize;
    state.selected = positions[next];
    let still_pinned = state.pinned.as_ref().is_some_and(|pin| {
        state.entries.get(state.selected).is_some_and(|entry| entry.path == *pin)
    });
    if !still_pinned {
        state.pinned = None;
    }
}

/// `absent_last` is ranger's `G`: no count jumps to the bottom, a count
/// jumps to that 1-based visible row. `gg` uses `absent_last = false`.
pub fn move_to_line(state: &mut AppState, count: Option<u32>, absent_last: bool) {
    let positions = visible_positions(&state.entries, &state.view, state.pinned.as_deref());
    if positions.is_empty() {
        return;
    }
    let slot = match count {
        Some(n) if n > 0 => match usize::try_from(n) {
            Ok(n) => n.saturating_sub(1).min(positions.len() - 1),
            Err(_) => positions.len() - 1,
        },
        _ if absent_last => positions.len() - 1,
        _ => 0,
    };
    state.selected = positions[slot];
    state.pinned = None;
}

/// Pull `selected` onto a row the user can see. Used after a filter or
/// `zh`, not during a streaming scan (that must keep an index past the end).
pub fn snap_to_visible(state: &mut AppState) {
    if let Some(index) = highlighted_index(state.selected, &state.entries, &state.view, state.pinned.as_deref()) {
        state.selected = index;
    }
    state.pinned = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::FileEntry;

    fn file(name: &str) -> FileEntry {
        FileEntry::new(format!("/tmp/{name}").into(), false, 0)
    }

    fn state_with(names: &[&str]) -> AppState {
        let mut state = AppState::new("/tmp".into());
        state.entries = names.iter().copied().map(file).collect();
        state
    }

    #[test]
    fn repeat_treats_missing_and_zero_as_one_step() {
        assert_eq!(repeat(None), 1);
        assert_eq!(repeat(Some(0)), 1);
        assert_eq!(repeat(Some(5)), 5);
    }

    #[test]
    fn hidden_files_are_skipped_until_show_hidden_is_on() {
        let mut state = state_with(&[".secret", "a.txt", ".other", "b.txt"]);
        assert_eq!(state.selected_entry().map(|entry| entry.name.as_str()), Some("a.txt"));

        move_by(&mut state, 1);
        assert_eq!(state.selected_entry().map(|entry| entry.name.as_str()), Some("b.txt"));
        move_by(&mut state, 1);
        assert_eq!(state.selected_entry().map(|entry| entry.name.as_str()), Some("b.txt"));

        state.view.show_hidden = true;
        state.selected = 0;
        assert_eq!(state.selected_entry().map(|entry| entry.name.as_str()), Some(".secret"));
    }

    #[test]
    fn a_filter_hides_non_matching_names_without_dropping_them() {
        let mut state = state_with(&["aa", "ab", "bb"]);
        state.view.filter = Some("a".into());
        snap_to_visible(&mut state);
        move_by(&mut state, 1);
        assert_eq!(state.selected_entry().map(|entry| entry.name.as_str()), Some("ab"));
        assert_eq!(state.entries.len(), 3, "the scan still holds every name");

        state.view.filter = None;
        snap_to_visible(&mut state);
        assert_eq!(state.selected_entry().map(|entry| entry.name.as_str()), Some("ab"));
    }

    #[test]
    fn an_empty_filter_shows_everything() {
        let view = View {
            filter: Some(String::new()),
            ..View::default()
        };
        let entries = vec![file("a.txt")];
        assert_eq!(visible_positions(&entries, &view, None), vec![0]);
    }

    #[test]
    fn jumping_with_a_count_is_one_based_and_clamps() {
        let mut state = state_with(&["a", "b", "c", "d"]);
        move_to_line(&mut state, Some(3), false);
        assert_eq!(state.selected_entry().map(|entry| entry.name.as_str()), Some("c"));
        move_to_line(&mut state, Some(99), false);
        assert_eq!(state.selected_entry().map(|entry| entry.name.as_str()), Some("d"));
        move_to_line(&mut state, Some(0), true);
        assert_eq!(
            state.selected_entry().map(|entry| entry.name.as_str()),
            Some("d"),
            "a zero count does not turn G into a jump to line 0"
        );
        move_to_line(&mut state, None, false);
        assert_eq!(state.selected_entry().map(|entry| entry.name.as_str()), Some("a"));
    }

    #[test]
    fn a_count_skips_hidden_rows() {
        let mut state = state_with(&[".a", "b", ".c", "d", "e"]);
        move_by(&mut state, 2);
        assert_eq!(state.selected_entry().map(|entry| entry.name.as_str()), Some("e"));
    }

    #[test]
    fn moving_in_an_empty_or_fully_filtered_list_does_not_panic() {
        let mut state = state_with(&[".only"]);
        move_by(&mut state, 1);
        move_to_line(&mut state, Some(3), true);
        assert!(state.selected_entry().is_none());
    }

    #[test]
    fn the_parent_pane_keeps_the_current_directory_when_it_is_hidden() {
        let view = View::default();
        let entry = file(".config");
        assert!(!view.shows(&entry));
        assert!(view.shows_parent(&entry, Some(".config")));
        assert!(!view.shows_parent(&entry, Some("other")));
    }

    #[test]
    fn an_index_past_the_end_still_highlights_the_last_visible_row() {
        let entries = vec![file("a"), file("b"), file("c")];
        assert_eq!(highlighted_index(10, &entries, &View::default(), None), Some(2));
    }
}
