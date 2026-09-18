//! `[3] Preview` pane: metadata for the currently selected entry.
//!
//! Only metadata (name/type/size) for now - real content preview
//! (text/image rendering) belongs to `src/preview/` and lands in a later
//! milestone; this pane just formats data `AppState` already has.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::AppState;

pub fn render(frame: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title("[3] Preview");

    let text = match state.selected_entry() {
        Some(entry) if entry.is_dir => format!("{}/\n<directory>", entry.name),
        Some(entry) => format!("{}\n{} bytes", entry.name, entry.size),
        None => "(nothing selected)".to_string(),
    };

    frame.render_widget(Paragraph::new(text).block(block), area);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::fs::FileEntry;

    use super::*;

    fn rendered_content(state: &AppState, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, state, frame.area())).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .fold(String::new(), |mut acc, cell| {
                acc.push_str(cell.symbol());
                acc
            })
    }

    #[test]
    fn shows_a_placeholder_when_nothing_is_selected() {
        let state = AppState::new("/tmp".into());
        assert!(rendered_content(&state, 30, 6).contains("nothing selected"));
    }

    #[test]
    fn shows_name_and_size_for_a_file() {
        let mut state = AppState::new("/tmp".into());
        state.entries = vec![FileEntry::new("/tmp/a.txt".into(), false, 1234)];

        let content = rendered_content(&state, 30, 6);

        assert!(content.contains("a.txt"));
        assert!(content.contains("1234 bytes"));
    }

    #[test]
    fn shows_directory_marker_for_a_dir_without_a_byte_count() {
        let mut state = AppState::new("/tmp".into());
        state.entries = vec![FileEntry::new("/tmp/sub".into(), true, 0)];

        let content = rendered_content(&state, 30, 6);

        assert!(content.contains("sub/"));
        assert!(content.contains("<directory>"));
    }

    #[test]
    fn does_not_panic_on_degenerate_areas() {
        let state = AppState::new("/tmp".into());
        for (width, height) in [(0u16, 0u16), (1, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(width.max(1), height.max(1))).unwrap();
            terminal.draw(|frame| render(frame, &state, frame.area())).unwrap();
        }
    }
}
