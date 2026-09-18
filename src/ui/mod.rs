//! Pure Ratatui rendering: reads `AppState`, writes to `Frame`. No I/O, no
//! state mutation.

use ratatui::Frame;
use ratatui::layout::Alignment;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::AppState;

pub fn render(frame: &mut Frame, state: &AppState) {
    let block = Block::default().borders(Borders::ALL).title("Hayate");
    let text = format!(
        "Hayate File Manager - Press 'q' to quit\n{}",
        state.current_dir.display()
    );
    let paragraph = Paragraph::new(text)
        .alignment(Alignment::Center)
        .block(block);
    frame.render_widget(paragraph, frame.area());
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    #[test]
    fn renders_quit_hint_and_current_dir() {
        let state = AppState::new("/tmp".into());
        let mut terminal = Terminal::new(TestBackend::new(60, 4)).unwrap();

        terminal.draw(|frame| render(frame, &state)).unwrap();

        let content = terminal.backend().buffer().content.iter().fold(
            String::new(),
            |mut acc, cell| {
                acc.push_str(cell.symbol());
                acc
            },
        );
        assert!(content.contains("Press 'q' to quit"));
        assert!(content.contains("/tmp"));
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
