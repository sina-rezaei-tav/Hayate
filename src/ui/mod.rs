//! Pure Ratatui rendering: reads `AppState`, writes to `Frame`. No I/O, no
//! state mutation.

use ratatui::Frame;
use ratatui::layout::Alignment;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::AppState;

pub fn render(frame: &mut Frame, state: &AppState) {
    let block = Block::default().borders(Borders::ALL).title("Hayate");
    let recount_line = recount_status_line(state);
    let text = format!(
        "Hayate File Manager - Press 'q' to quit\n{}\n{} entries scanned\n{recount_line}",
        state.current_dir.display(),
        state.entries.len()
    );
    let paragraph = Paragraph::new(text)
        .alignment(Alignment::Center)
        .block(block);
    frame.render_widget(paragraph, frame.area());
}

fn recount_status_line(state: &AppState) -> String {
    if state.is_counting_recursively {
        "Counting files recursively...".to_string()
    } else if let Some(count) = state.recursive_file_count {
        format!("{count} files found recursively (press 'r' to recount)")
    } else {
        "Press 'r' to count files recursively".to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::fs::FileEntry;

    use super::*;

    fn rendered_content(state: &AppState, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, state)).unwrap();
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
    fn renders_quit_hint_current_dir_and_entry_count() {
        let mut state = AppState::new("/tmp".into());
        state.entries.push(FileEntry::new(PathBuf::from("/tmp/a.txt"), false, 0));

        let content = rendered_content(&state, 60, 6);

        assert!(content.contains("Press 'q' to quit"));
        assert!(content.contains("/tmp"));
        assert!(content.contains("1 entries scanned"));
        assert!(content.contains("Press 'r' to count files recursively"));
    }

    #[test]
    fn renders_counting_in_progress() {
        let mut state = AppState::new("/tmp".into());
        state.is_counting_recursively = true;

        let content = rendered_content(&state, 60, 6);

        assert!(content.contains("Counting files recursively"));
    }

    #[test]
    fn renders_completed_recursive_count() {
        let mut state = AppState::new("/tmp".into());
        state.recursive_file_count = Some(42);

        let content = rendered_content(&state, 60, 6);

        assert!(content.contains("42 files found recursively"));
    }

    #[test]
    fn does_not_panic_on_degenerate_terminal_sizes() {
        let state = AppState::default();

        for (width, height) in [(0, 0), (1, 1), (2, 2)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| render(frame, &state)).unwrap();
        }
    }
}
