//! Help, pager, rename/search/console prompts, and the delete confirm.
//! Pure: no I/O, no mutation.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::command;
use crate::app::mode::InteractionMode;
use crate::app::AppState;
use crate::preview::FilePreview;

pub fn render(frame: &mut Frame, state: &AppState) {
    match &state.mode {
        InteractionMode::Line(prompt) => prompt_box(frame, prompt.title, &prompt.buffer, "enter accepts, esc cancels"),
        InteractionMode::Confirm(prompt) => {
            let body = format!("delete {} path(s)?  y confirms, n or esc cancels", prompt.paths.len());
            prompt_box(frame, "confirm", &body, "q does not quit");
        }
        InteractionMode::Help(view) => full_screen(frame, "help", &help_text(), view.scroll),
        InteractionMode::Pager(view) => full_screen(frame, "pager", &pager_text(state), view.scroll),
        InteractionMode::OneKey(_) | InteractionMode::Browser | InteractionMode::OpenWith(_) => {}
    }
}

pub fn help_text() -> String {
    let mut lines = vec![
        "counts: 5j, 12G, 3gg. 0 means no count.".to_string(),
        "search and filter are literal, not regular expressions.".to_string(),
        String::new(),
    ];
    for (label, blurb) in command::help_lines() {
        lines.push(format!("{label:<12} {blurb}"));
    }
    lines.join("\n")
}

pub fn pager_text(state: &AppState) -> String {
    let Some(entry) = state.selected_entry() else {
        return "(nothing selected)".to_string();
    };
    if entry.is_dir {
        return format!("{}/\n<directory>\n{} bytes", entry.name, entry.size);
    }
    match &state.preview {
        FilePreview::Text { content, truncated, path } if path == &entry.path => {
            let marker = if *truncated { "\n\n[truncated]" } else { "" };
            format!("{}\n\n{content}{marker}", entry.name)
        }
        FilePreview::Image { width, height, path } if path == &entry.path => {
            format!("{}\n\nimage {width}x{height}", entry.name)
        }
        FilePreview::Binary { path } if path == &entry.path => format!("{}\n\n<binary file>", entry.name),
        FilePreview::Error { message, path } if path == &entry.path => format!("{}\n\n{message}", entry.name),
        _ => format!("{}\n\nLoading...", entry.name),
    }
}

fn full_screen(frame: &mut Frame, title: &str, text: &str, scroll: u16) {
    let area = frame.area();
    if area.width < 3 || area.height < 3 {
        return;
    }
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }).scroll((scroll, 0)).block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

fn prompt_box(frame: &mut Frame, title: &str, body: &str, hint: &str) {
    let area = centered(frame.area(), 60, 8);
    if area.width < 3 || area.height < 3 {
        return;
    }
    frame.render_widget(Clear, area);
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 2 || inner.height < 2 {
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    frame.render_widget(Paragraph::new(body.to_string()), chunks[0]);
    frame.render_widget(Paragraph::new(hint.to_string()), chunks[1]);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    if area.width == 0 || area.height == 0 {
        return area;
    }
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(area.x + (area.width - width) / 2, area.y + (area.height - height) / 2, width, height)
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::mode::InteractionMode;
    use crate::app::prompt::{LineKind, LinePrompt};
    use crate::fs::FileEntry;
    use crate::preview::FilePreview;

    use super::*;

    fn blob(state: &AppState) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| crate::ui::layout::render(frame, state)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..24 {
            for x in 0..80 {
                text.push_str(buffer.cell((x, y)).unwrap().symbol());
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn help_lists_dc_and_does_not_claim_a_blank_binding() {
        let text = help_text();
        assert!(text.contains("dc"));
        assert!(text.contains("yy"));
        assert!(text.contains("literal"));
        let mut state = AppState::new("/tmp".into());
        let line = text.lines().position(|line| line.contains("dc")).expect("dc binding") as u16;
        state.mode = InteractionMode::Help(crate::app::prompt::HelpView { scroll: line, count: None });
        let rendered = blob(&state);
        assert!(rendered.contains("help"), "{rendered}");
        assert!(rendered.contains("dc"), "{rendered}");
        assert!(!rendered.contains("unbound"), "{rendered}");
    }

    #[test]
    fn the_rename_prompt_shows_what_was_typed() {
        let mut state = AppState::new("/tmp".into());
        state.mode = InteractionMode::Line(LinePrompt::new(LineKind::Rename("/tmp/a.txt".into()), "rename", "notes", 5, false, 0));
        let rendered = blob(&state);
        assert!(rendered.contains("notes"), "{rendered}");
        assert!(rendered.contains("rename"), "{rendered}");
    }

    #[test]
    fn the_pager_shows_the_cached_text_and_not_the_file_list_keys() {
        let mut state = AppState::new("/tmp".into());
        let path = std::path::PathBuf::from("/tmp/a.txt");
        state.entries = vec![FileEntry::new(path.clone(), false, 4)];
        state.preview = FilePreview::Text {
            path,
            content: "hello pager".into(),
            truncated: false,
        };
        state.mode = InteractionMode::Pager(crate::app::prompt::PagerView { scroll: 0, count: None });
        let rendered = blob(&state);
        assert!(rendered.contains("hello pager"), "{rendered}");
    }

    #[test]
    fn an_image_preview_names_its_dimensions() {
        let mut state = AppState::new("/tmp".into());
        let path = std::path::PathBuf::from("/tmp/a.png");
        state.entries = vec![FileEntry::new(path.clone(), false, 8)];
        state.preview = FilePreview::Image {
            path,
            width: 2,
            height: 3,
        };
        state.view.preview_images = true;
        let rendered = blob(&state);
        assert!(rendered.contains("image 2x3"), "{rendered}");
    }
}
