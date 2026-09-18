//! `[3] Preview` pane: metadata plus cached file contents from `AppState`.
//! Never reads the disk.

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::AppState;
use crate::preview::FilePreview;

pub fn render(frame: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title("[3] Preview");
    let text = preview_text(state);
    let inner = inner_area(area);
    let limit = scroll_limit_for_text(&text, inner);
    let scroll = state.preview_scroll.min(limit);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0))
            .block(block),
        area,
    );
}

pub fn preview_text(state: &AppState) -> String {
    match state.selected_entry() {
        None => "(nothing selected)".to_string(),
        Some(entry) if entry.is_dir => format!("{}/\n<directory>", entry.name),
        Some(entry) => match &state.preview {
            FilePreview::Idle | FilePreview::Loading(_) => {
                format!("{}\n{} bytes\n\nLoading preview...", entry.name, entry.size)
            }
            FilePreview::Text {
                content,
                truncated,
                path,
            } if path == &entry.path => {
                let cap = if *truncated {
                    "\n\n[truncated]"
                } else {
                    ""
                };
                format!("{}\n{} bytes\n\n{content}{cap}", entry.name, entry.size)
            }
            FilePreview::Binary { path } if path == &entry.path => {
                format!("{}\n{} bytes\n\n<binary file>", entry.name, entry.size)
            }
            FilePreview::Error { message, path } if path == &entry.path => {
                format!("{}\n{} bytes\n\npreview failed: {message}", entry.name, entry.size)
            }
            _ => format!("{}\n{} bytes\n\nLoading preview...", entry.name, entry.size),
        },
    }
}

/// How far the preview can scroll in `area` (the full pane, including borders).
pub fn scroll_limit(state: &AppState, area: Rect) -> u16 {
    scroll_limit_for_text(&preview_text(state), inner_area(area))
}

/// Lines to jump on PageUp/PageDown: one inner pane, minus a sticky line.
pub fn page_size(area: Rect) -> u16 {
    inner_area(area).height.saturating_sub(1).max(1)
}

fn inner_area(area: Rect) -> Rect {
    area.inner(Margin::new(1, 1))
}

fn scroll_limit_for_text(text: &str, inner: Rect) -> u16 {
    let lines = wrapped_line_count(text, inner.width);
    lines.saturating_sub(inner.height as usize) as u16
}

fn wrapped_line_count(text: &str, width: u16) -> usize {
    if width == 0 {
        return text.lines().count().max(1);
    }
    let width = width as usize;
    text.lines()
        .map(|line| {
            let chars = line.chars().count();
            if chars == 0 {
                1
            } else {
                chars.div_ceil(width)
            }
        })
        .sum::<usize>()
        .max(1)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::fs::FileEntry;
    use crate::preview::FilePreview;

    use super::*;

    fn text_state(content: &str) -> AppState {
        let mut state = AppState::new("/tmp".into());
        state.entries = vec![FileEntry::new("/tmp/a.txt".into(), false, content.len() as u64)];
        state.preview = FilePreview::Text {
            path: PathBuf::from("/tmp/a.txt"),
            content: content.into(),
            truncated: false,
        };
        state
    }

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
    fn shows_directory_marker_for_a_dir_without_reading_contents() {
        let mut state = AppState::new("/tmp".into());
        state.entries = vec![FileEntry::new("/tmp/sub".into(), true, 0)];

        let content = rendered_content(&state, 30, 6);

        assert!(content.contains("sub/"));
        assert!(content.contains("<directory>"));
        assert!(!content.contains("Loading preview"));
    }

    #[test]
    fn shows_cached_text_contents() {
        let state = text_state("hello");
        let content = rendered_content(&state, 40, 8);

        assert!(content.contains("a.txt"));
        assert!(content.contains("hello"));
        assert!(!content.contains("[truncated]"));
    }

    #[test]
    fn shows_truncated_marker() {
        let mut state = text_state("hello");
        if let FilePreview::Text { truncated, .. } = &mut state.preview {
            *truncated = true;
        }
        state.entries[0].size = 99;

        assert!(rendered_content(&state, 40, 10).contains("[truncated]"));
    }

    #[test]
    fn shows_binary_placeholder() {
        let mut state = AppState::new("/tmp".into());
        state.entries = vec![FileEntry::new("/tmp/a.bin".into(), false, 8)];
        state.preview = FilePreview::Binary {
            path: PathBuf::from("/tmp/a.bin"),
        };

        assert!(rendered_content(&state, 40, 8).contains("<binary file>"));
    }

    #[test]
    fn scrolling_hides_the_top_and_reveals_later_lines() {
        let mut lines = vec!["HEADER".to_string()];
        lines.extend((0..20).map(|i| format!("LINE-{i}")));
        let mut state = text_state(&lines.join("\n"));
        let area = Rect::new(0, 0, 24, 8);

        assert!(rendered_content(&state, area.width, area.height).contains("HEADER"));

        state.preview_scroll = scroll_limit(&state, area).max(1);
        let scrolled = rendered_content(&state, area.width, area.height);

        assert!(!scrolled.contains("HEADER"), "scrolled view still showed the top: {scrolled:?}");
        assert!(scrolled.contains("LINE-"), "scrolled view showed no later lines: {scrolled:?}");
    }

    #[test]
    fn scroll_limit_is_zero_when_content_fits() {
        let state = text_state("short");
        assert_eq!(scroll_limit(&state, Rect::new(0, 0, 40, 20)), 0);
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
