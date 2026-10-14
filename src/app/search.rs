//! Literal name search and find.
//!
//! Ranger compiles the pattern as a regular expression. A hostile pattern
//! can hang the UI thread on backtracking, so Hayate matches a literal
//! substring (`/`, `n`, `N`) or a literal prefix (`f`) instead.

use crate::fs::FileEntry;

use super::motion::visible_positions;
use super::state::AppState;

pub fn next_match(entries: &[FileEntry], positions: &[usize], current: usize, pattern: &str, forward: bool) -> Option<usize> {
    if pattern.is_empty() || positions.is_empty() {
        return None;
    }
    let hits: Vec<usize> = positions
        .iter()
        .copied()
        .filter(|index| entries[*index].name.contains(pattern))
        .collect();
    if hits.is_empty() {
        return None;
    }
    if forward {
        hits.iter().copied().find(|index| *index > current).or_else(|| hits.first().copied())
    } else {
        hits.iter().copied().rev().find(|index| *index < current).or_else(|| hits.last().copied())
    }
}

/// Prefix find starting at `origin`, including `origin` itself, then
/// wrapping. `None` means nothing matches; the caller keeps `origin`.
pub fn find_prefix(entries: &[FileEntry], positions: &[usize], origin: usize, prefix: &str) -> Option<usize> {
    if positions.is_empty() {
        return None;
    }
    if prefix.is_empty() {
        return visible_slot_index(origin, entries.len(), positions);
    }
    let start = positions.iter().position(|index| *index >= origin).unwrap_or(0);
    for step in 0..positions.len() {
        let index = positions[(start + step) % positions.len()];
        if entries[index].name.starts_with(prefix) {
            return Some(index);
        }
    }
    None
}

fn visible_slot_index(origin: usize, len: usize, positions: &[usize]) -> Option<usize> {
    if positions.is_empty() || len == 0 {
        return None;
    }
    let clamped = origin.min(len - 1);
    if positions.contains(&clamped) {
        return Some(clamped);
    }
    positions.iter().copied().find(|index| *index > clamped).or_else(|| positions.first().copied())
}

pub fn search_from_selection(state: &mut AppState, pattern: &str, forward: bool) -> bool {
    if pattern.is_empty() {
        return false;
    }
    let positions = visible_positions(&state.entries, &state.view, state.pinned.as_deref());
    let current = super::motion::highlighted_index(state.selected, &state.entries, &state.view, state.pinned.as_deref()).unwrap_or(0);
    state.search_pattern = Some(pattern.to_string());
    state.pinned = None;
    match next_match(&state.entries, &positions, current, pattern, forward) {
        Some(index) => {
            state.selected = index;
            state.search_pattern = Some(pattern.to_string());
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::FileEntry;

    fn files(names: &[&str]) -> Vec<FileEntry> {
        names
            .iter()
            .map(|name| FileEntry::new(format!("/tmp/{name}").into(), false, 0))
            .collect()
    }

    #[test]
    fn search_moves_forward_and_wraps() {
        let entries = files(&["aa", "ab", "ba"]);
        let positions: Vec<_> = (0..entries.len()).collect();
        // "ba" contains the substring "a", so it is a hit, not a gap.
        assert_eq!(next_match(&entries, &positions, 0, "a", true), Some(1));
        assert_eq!(next_match(&entries, &positions, 1, "a", true), Some(2));
        assert_eq!(next_match(&entries, &positions, 2, "a", true), Some(0));
    }

    #[test]
    fn search_wraps_past_names_that_do_not_contain_the_substring() {
        let entries = files(&["aa", "zz", "ab"]);
        let positions: Vec<_> = (0..entries.len()).collect();
        assert_eq!(next_match(&entries, &positions, 2, "a", true), Some(0));
        assert_eq!(next_match(&entries, &positions, 0, "a", false), Some(2));
    }

    #[test]
    fn the_only_match_wraps_to_itself() {
        let entries = files(&["aa", "zz"]);
        assert_eq!(next_match(&entries, &[0, 1], 0, "aa", true), Some(0));
        assert_eq!(next_match(&entries, &[0, 1], 0, "aa", false), Some(0));
    }

    #[test]
    fn search_backward_wraps_to_the_last_hit() {
        let entries = files(&["aa", "bb", "ab"]);
        let positions: Vec<_> = (0..entries.len()).collect();
        assert_eq!(next_match(&entries, &positions, 0, "a", false), Some(2));
    }

    #[test]
    fn search_is_case_sensitive_and_literal() {
        let entries = files(&["Readme", "readme", "a.b"]);
        let positions: Vec<_> = (0..entries.len()).collect();
        assert_eq!(next_match(&entries, &positions, 2, "Read", true), Some(0));
        assert_eq!(next_match(&entries, &positions, 2, "a.b", true), Some(2));
        assert_eq!(next_match(&entries, &positions, 0, "a.b", true), Some(2));
        assert!(next_match(&entries, &positions, 0, "", true).is_none());
        assert!(next_match(&entries, &positions, 0, "nope", true).is_none());
    }

    #[test]
    fn search_ignores_hidden_positions_it_was_not_given() {
        let entries = files(&[".secret", "visible"]);
        assert_eq!(next_match(&entries, &[1], 1, "secret", true), None);
        assert_eq!(next_match(&entries, &[0, 1], 1, "secret", true), Some(0));
    }

    #[test]
    fn find_includes_the_origin_and_restores_it_for_an_empty_prefix() {
        let entries = files(&["aa", "ab", "ba"]);
        let positions: Vec<_> = (0..3).collect();
        assert_eq!(find_prefix(&entries, &positions, 1, ""), Some(1));
        assert_eq!(find_prefix(&entries, &positions, 0, "ab"), Some(1));
        assert_eq!(find_prefix(&entries, &positions, 2, "a"), Some(0));
        assert!(find_prefix(&entries, &positions, 0, "z").is_none());
    }

    #[test]
    fn find_does_not_match_a_substring_in_the_middle() {
        let entries = files(&["xxab", "ab"]);
        assert_eq!(find_prefix(&entries, &[0, 1], 0, "ab"), Some(1));
    }
}
