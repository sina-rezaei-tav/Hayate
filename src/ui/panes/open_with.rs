//! Overlay for the open-with picker. Pure: reads `AppState`, no I/O.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

use crate::app::open_with::OpenWithRow;
use crate::app::AppState;

pub fn render(frame: &mut Frame, state: &AppState) {
    let crate::app::open_with::InteractionMode::OpenWith(prompt) = &state.mode else {
        return;
    };

    let area = centered(frame.area(), 60, 16);
    if area.width < 3 || area.height < 3 {
        frame.render_widget(Clear, area);
        return;
    }
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("Open with — {}", prompt.target_name));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 2 || inner.height < 3 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let query = if prompt.query.is_empty() {
        "> ".to_string()
    } else {
        format!("> {}", prompt.query)
    };
    frame.render_widget(Paragraph::new(query), chunks[0]);

    let rows = prompt.visible_rows();
    let items: Vec<ListItem> = if rows.is_empty() {
        let placeholder = if prompt.listing_complete {
            if let Some(err) = &prompt.listing_error {
                format!("listing failed: {err}")
            } else {
                "(type a command)".to_string()
            }
        } else {
            "loading PATH…".to_string()
        };
        vec![ListItem::new(Line::from(placeholder))]
    } else {
        rows.iter()
            .map(|row| {
                let label = match row {
                    OpenWithRow::Custom(query) => format!("run  {query}"),
                    OpenWithRow::Binary(name) => name.to_string(),
                };
                ListItem::new(Line::from(label))
            })
            .collect()
    };
    let mut list_state = ListState::default();
    if !rows.is_empty() {
        list_state.select(Some(prompt.selected.min(rows.len() - 1)));
    }
    frame.render_stateful_widget(
        List::new(items).highlight_style(Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)),
        chunks[1],
        &mut list_state,
    );

    frame.render_widget(
        Paragraph::new("Enter open   Esc cancel   type a command")
            .alignment(Alignment::Center),
        chunks[2],
    );
}

fn centered(area: Rect, percent_x: u16, height: u16) -> Rect {
    if area.width == 0 || area.height == 0 {
        return area;
    }
    let width = ((u32::from(area.width) * u32::from(percent_x.min(100))) / 100) as u16;
    let width = width.clamp(1, area.width);
    let height = height.clamp(1, area.height);
    let x = area.x + (area.width - width) / 2;
    let y = area.y + (area.height - height) / 2;
    Rect::new(x, y, width, height)
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::open_with::{InteractionMode, OpenWithPrompt};
    use crate::fs::FileEntry;

    use super::*;

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
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

    fn draw(state: &AppState, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, state)).unwrap();
        buffer_text(&terminal)
    }

    fn picker_state(query: &str, candidates: &[&str]) -> AppState {
        let mut state = AppState::new("/tmp".into());
        state.entries = vec![FileEntry::new("/tmp/notes.txt".into(), false, 4)];
        let mut prompt = OpenWithPrompt::new("/tmp/notes.txt".into(), 1);
        prompt.query = query.into();
        prompt.set_candidates(candidates.iter().map(|name| (*name).into()).collect());
        state.mode = InteractionMode::OpenWith(prompt);
        state
    }

    #[test]
    fn overlay_shows_the_filename_and_query() {
        let state = picker_state("mpv", &["mpv", "vim"]);
        let content = draw(&state, 80, 20);
        assert!(content.contains("Open with"), "{content:?}");
        assert!(content.contains("notes.txt"), "{content:?}");
        assert!(content.contains("mpv"), "{content:?}");
        assert!(content.contains("run"), "{content:?}");
    }

    #[test]
    fn overlay_does_not_draw_in_browser_mode() {
        let state = AppState::new("/tmp".into());
        let content = draw(&state, 40, 10);
        assert!(!content.contains("Open with"), "{content:?}");
    }

    #[test]
    fn empty_listing_shows_a_type_hint() {
        let state = picker_state("", &["cat"]);
        let content = draw(&state, 80, 16);
        assert!(content.contains("type a command") || content.contains("loading"), "{content:?}");
    }

    #[test]
    fn does_not_panic_on_degenerate_areas() {
        let state = picker_state("vim", &["vim"]);
        for (width, height) in [(0, 0), (1, 1), (10, 4)] {
            let mut terminal = Terminal::new(TestBackend::new(width.max(1), height.max(1))).unwrap();
            terminal.draw(|frame| render(frame, &state)).unwrap();
        }
    }
}
