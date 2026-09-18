//! `[2] Current` pane: the active directory's entries, with the selected
//! item highlighted.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem};

use crate::app::AppState;

pub fn render(frame: &mut Frame, state: &AppState, area: Rect) {
    let title = format!(
        "[2] Current ({} entries) — {}",
        state.entries.len(),
        recount_status(state)
    );
    let block = Block::default().borders(Borders::ALL).title(title);

    let items: Vec<ListItem> = state
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let label = entry_label(entry);
            let style = if Some(index) == state.selected_index() {
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(label)).style(style)
        })
        .collect();

    frame.render_widget(List::new(items).block(block), area);
}

fn entry_label(entry: &crate::fs::FileEntry) -> String {
    if entry.is_dir {
        format!("{}/", entry.name)
    } else {
        entry.name.to_string()
    }
}

/// Status of the recursive file count (triggered by 'r', cancellable with
/// Esc). Shown in this pane's title since the count is a property of the
/// whole current directory, not of any single selected entry.
fn recount_status(state: &AppState) -> String {
    if state.is_counting_recursively {
        "counting recursively... (Esc to cancel)".to_string()
    } else if let Some(count) = state.recursive_file_count {
        format!("{count} files recursively (r to recount)")
    } else {
        "press 'r' to count files recursively".to_string()
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use crate::fs::FileEntry;

    use super::*;

    fn entries(names: &[&str]) -> Vec<FileEntry> {
        names
            .iter()
            .map(|name| FileEntry::new(format!("/tmp/{name}").into(), false, 0))
            .collect()
    }

    /// Renders `render` into a fresh buffer and returns, per row within
    /// `area`, the trimmed text and whether any cell in that row carries
    /// the `REVERSED` modifier. Coordinate-independent: works regardless of
    /// exactly which row a given entry lands on.
    fn rows_with_reversed_flag(state: &AppState, area: Rect) -> Vec<(String, bool)> {
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        terminal.draw(|frame| render(frame, state, area)).unwrap();
        let buffer: &Buffer = terminal.backend().buffer();

        (area.top()..area.bottom())
            .map(|y| {
                let mut text = String::new();
                let mut reversed = false;
                for x in area.left()..area.right() {
                    let cell = buffer.cell((x, y)).unwrap();
                    text.push_str(cell.symbol());
                    if cell.modifier.contains(Modifier::REVERSED) {
                        reversed = true;
                    }
                }
                (text.trim().to_string(), reversed)
            })
            .collect()
    }

    #[test]
    fn title_shows_entry_count_and_recount_prompt_by_default() {
        let mut state = AppState::new("/tmp".into());
        state.entries = entries(&["a.txt", "b.txt"]);

        let rows = rows_with_reversed_flag(&state, Rect::new(0, 0, 60, 6));

        assert!(rows.iter().any(|(text, _)| text.contains("[2] Current (2 entries)")));
        assert!(rows.iter().any(|(text, _)| text.contains("press 'r' to count")));
    }

    #[test]
    fn title_shows_counting_in_progress() {
        let mut state = AppState::new("/tmp".into());
        state.is_counting_recursively = true;

        let rows = rows_with_reversed_flag(&state, Rect::new(0, 0, 60, 6));

        assert!(rows.iter().any(|(text, _)| text.contains("counting recursively")));
    }

    #[test]
    fn title_shows_completed_recursive_count() {
        let mut state = AppState::new("/tmp".into());
        state.recursive_file_count = Some(42);

        let rows = rows_with_reversed_flag(&state, Rect::new(0, 0, 60, 6));

        assert!(rows.iter().any(|(text, _)| text.contains("42 files recursively")));
    }

    #[test]
    fn only_the_selected_row_is_reversed() {
        let mut state = AppState::new("/tmp".into());
        state.entries = entries(&["a.txt", "b.txt", "c.txt"]);
        state.selected = 1;

        let rows = rows_with_reversed_flag(&state, Rect::new(0, 0, 30, 6));

        let selected_row = rows.iter().find(|(text, _)| text.contains("b.txt"));
        assert!(matches!(selected_row, Some((_, true))), "selected entry should be reversed: {rows:?}");

        for name in ["a.txt", "c.txt"] {
            let row = rows.iter().find(|(text, _)| text.contains(name));
            assert!(matches!(row, Some((_, false))), "{name} should not be reversed: {rows:?}");
        }
    }

    #[test]
    fn directories_get_a_trailing_slash() {
        let mut state = AppState::new("/tmp".into());
        state.entries = vec![FileEntry::new("/tmp/sub".into(), true, 0)];

        let rows = rows_with_reversed_flag(&state, Rect::new(0, 0, 30, 6));

        assert!(rows.iter().any(|(text, _)| text.contains("sub/")));
    }

    #[test]
    fn does_not_panic_when_empty_or_area_is_degenerate() {
        let state = AppState::new("/tmp".into());
        for area in [Rect::new(0, 0, 0, 0), Rect::new(0, 0, 1, 1), Rect::new(0, 0, 30, 6)] {
            let mut terminal = Terminal::new(TestBackend::new(area.width.max(1), area.height.max(1))).unwrap();
            terminal.draw(|frame| render(frame, &state, area)).unwrap();
        }
    }
}
