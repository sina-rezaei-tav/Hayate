//! Splits the frame into the 3-pane Miller-column layout and delegates
//! rendering to each pane. Pure: reads `AppState`, writes to `Frame` only.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::app::AppState;

use super::panes::{current, parent, preview};

pub fn render(frame: &mut Frame, state: &AppState) {
    let columns = split_panes(frame.area());
    parent::render(frame, state, columns[0]);
    current::render(frame, state, columns[1]);
    preview::render(frame, state, columns[2]);
}

/// Same 20/40/40 split `render` uses, so preview scrolling can page by the
/// actual pane size without the UI mutating state.
pub fn split_panes(area: Rect) -> [Rect; 3] {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(20),
            Constraint::Percentage(40),
            Constraint::Percentage(40),
        ])
        .split(area);
    [columns[0], columns[1], columns[2]]
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    #[test]
    fn splits_into_three_titled_panes() {
        let state = AppState::new("/tmp".into());
        let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();

        terminal.draw(|frame| render(frame, &state)).unwrap();

        let content = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .fold(String::new(), |mut acc, cell| {
                acc.push_str(cell.symbol());
                acc
            });

        assert!(content.contains("[1] Parent"));
        assert!(content.contains("[2] Current"));
        assert!(content.contains("[3] Preview"));
    }

    #[test]
    fn does_not_panic_on_degenerate_terminal_sizes() {
        let state = AppState::default();

        for (width, height) in [(0, 0), (1, 1), (2, 2), (3, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| render(frame, &state)).unwrap();
        }
    }
}
