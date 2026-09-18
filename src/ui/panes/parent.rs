//! `[1] Parent` pane: the parent directory's entries, with the entry that
//! matches `current_dir` highlighted so it's clear which child you're in.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

use crate::app::AppState;
use crate::fs::FileEntry;

pub fn render(frame: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title("[1] Parent");

    if state.current_dir.parent().is_none() {
        let items = vec![ListItem::new(Line::from("(filesystem root, no parent)"))];
        frame.render_widget(List::new(items).block(block), area);
        return;
    }

    let (items, selected) = if let Some(err) = &state.parent_scan_error {
        (
            vec![ListItem::new(Line::from(format!("scan failed: {err}")))],
            None,
        )
    } else {
        let current_name = state.current_dir.file_name().map(|name| name.to_string_lossy());
        let mut selected = None;
        let items: Vec<ListItem> = state
            .parent_entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let label = entry_label(entry);
                let is_current_dir = current_name.as_deref() == Some(entry.name.as_str());
                if is_current_dir {
                    selected = Some(index);
                }
                let style = if is_current_dir {
                    Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(Line::from(label)).style(style)
            })
            .collect();
        (items, selected)
    };

    let mut list_state = ListState::default();
    list_state.select(selected);
    frame.render_stateful_widget(List::new(items).block(block), area, &mut list_state);
}

fn entry_label(entry: &FileEntry) -> String {
    if entry.is_dir {
        format!("{}/", entry.name)
    } else {
        entry.name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use super::*;

    fn rendered_rows(state: &AppState, area: Rect) -> Vec<(String, bool)> {
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
    fn root_directory_shows_a_no_parent_placeholder() {
        let state = AppState::new("/".into());

        let rows = rendered_rows(&state, Rect::new(0, 0, 30, 6));

        assert!(rows.iter().any(|(text, _)| text.contains("no parent")));
    }

    #[test]
    fn current_dir_is_highlighted_among_its_siblings() {
        let mut state = AppState::new("/tmp/project".into());
        state.parent_entries = vec![
            FileEntry::new("/tmp/project".into(), true, 0),
            FileEntry::new("/tmp/other".into(), true, 0),
        ];

        let rows = rendered_rows(&state, Rect::new(0, 0, 30, 6));

        let current_row = rows.iter().find(|(text, _)| text.contains("project"));
        assert!(matches!(current_row, Some((_, true))), "current dir should be reversed: {rows:?}");

        let sibling_row = rows.iter().find(|(text, _)| text.contains("other"));
        assert!(matches!(sibling_row, Some((_, false))), "sibling should not be reversed: {rows:?}");
    }

    #[test]
    fn does_not_panic_on_degenerate_areas() {
        let state = AppState::new("/tmp".into());
        for area in [Rect::new(0, 0, 0, 0), Rect::new(0, 0, 1, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(area.width.max(1), area.height.max(1))).unwrap();
            terminal.draw(|frame| render(frame, &state, area)).unwrap();
        }
    }
}
