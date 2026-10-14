//! What a resolved command does to `AppState`.
//!
//! Disk work is not done here. This returns a [`Step`] the loop can spawn.
//! The suspicious cases (yank skips hidden rows, delete waits for `y`,
//! `H` does not grow the forward stack, a failed rename stays on disk)
//! are decided in this file so they can be tested without a TTY.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::fs::{sort_entries, validate_entry_name, DiskOp};
use crate::preview::FilePreview;

use super::command::{CdPlace, Command, RenameKind};
use super::console::{self, ConsoleCommand};
use super::editor::{self, EditorLaunch};
use super::mode::InteractionMode;
use super::motion::{self, repeat};
use super::navigation::{self, ScanPlan};
use super::prompt::{self, ConfirmAction, LineAction, LineKind, LinePrompt, OneKey};
use super::rifle::{self, OpenChoice};
use super::search;
use super::state::{index_of_name, AppState, Clipboard, Tab};

pub struct Metrics {
    pub list_page: usize,
    pub list_half: usize,
    pub preview_limit: u16,
}

#[derive(Debug)]
pub enum Step {
    None,
    Scan(ScanPlan),
    OpenEditor(PathBuf),
    Run {
        spec: EditorLaunch,
        report_nonzero: bool,
    },
    Recount,
    CancelJob,
    OpenWith,
    FileOp {
        op: DiskOp,
        refresh: PathBuf,
    },
}

pub fn apply(command: Command, count: Option<u32>, state: &mut AppState, metrics: Metrics) -> Step {
    match command {
        Command::Quit => {
            state.should_quit = true;
            Step::None
        }
        Command::RecountRecursive => Step::Recount,
        Command::Cancel => Step::CancelJob,
        Command::SelectPrevious => move_rows(state, -(signed(count))),
        Command::SelectNext => move_rows(state, signed(count)),
        Command::MoveHalfUp => move_rows(state, -(page_step(metrics.list_half, count))),
        Command::MoveHalfDown => move_rows(state, page_step(metrics.list_half, count)),
        Command::MovePageUp => move_rows(state, -(page_step(metrics.list_page, count))),
        Command::MovePageDown => move_rows(state, page_step(metrics.list_page, count)),
        Command::MoveToTop => {
            motion::move_to_line(state, count, false);
            moved(state)
        }
        Command::MoveToBottom => {
            motion::move_to_line(state, count, true);
            moved(state)
        }
        Command::EnterDirectory => scan_of(navigation::enter_selected(state)),
        Command::Activate => activate_now(state, false),
        Command::OpenInEditor => activate_now(state, true),
        Command::OpenWith => Step::OpenWith,
        Command::OpenParent => scan_of(navigation::open_parent(state)),
        Command::PreviewLineUp => {
            state.scroll_preview(narrow(-signed(count)), metrics.preview_limit);
            Step::None
        }
        Command::PreviewLineDown => {
            state.scroll_preview(narrow(signed(count)), metrics.preview_limit);
            Step::None
        }
        Command::ToggleHidden => {
            state.view.show_hidden = !state.view.show_hidden;
            motion::snap_to_visible(state);
            moved(state)
        }
        Command::ToggleDirsFirst => {
            state.view.sort.dirs_first = !state.view.sort.dirs_first;
            resort(state)
        }
        Command::ToggleSortReverse => {
            state.view.sort.reverse = !state.view.sort.reverse;
            resort(state)
        }
        Command::Sort(key) => {
            state.view.sort.key = key;
            resort(state)
        }
        Command::BeginFilter => open_line(state, LineKind::Filter, "filter", String::new(), 0, false),
        Command::ClearFilter => {
            state.view.filter = None;
            motion::snap_to_visible(state);
            moved(state)
        }
        Command::ToggleImages => {
            state.view.preview_images = !state.view.preview_images;
            state.preview = FilePreview::Idle;
            state.preview_scroll = 0;
            Step::None
        }
        Command::BeginSearch => open_line(state, LineKind::Search, "search", String::new(), 0, false),
        Command::SearchNext => search_again(state, true),
        Command::SearchPrev => search_again(state, false),
        Command::BeginFind => open_line(state, LineKind::Find, "find", String::new(), 0, false),
        Command::Yank => {
            stash(state, count, false);
            Step::None
        }
        Command::Cut => {
            stash(state, count, true);
            Step::None
        }
        Command::Paste => paste(state, false),
        Command::PasteOverwrite => paste(state, true),
        Command::BeginDelete => begin_delete(state, count),
        Command::Rename(kind) => begin_rename(state, kind),
        Command::ToggleMark => {
            toggle_mark(state);
            Step::None
        }
        Command::ToggleMarkMove => {
            if state.visual_anchor.is_none() {
                toggle_mark(state);
            }
            move_rows(state, signed(count))
        }
        Command::ToggleVisual => {
            toggle_visual(state);
            Step::None
        }
        Command::ClearMarks => {
            state.marked.clear();
            state.visual_anchor = None;
            state.visual_base.clear();
            Step::None
        }
        Command::HistoryBack => history(state, true),
        Command::HistoryForward => history(state, false),
        Command::Cd(place) => cd_place(state, place),
        Command::TabNew => tab_new(state),
        Command::TabClose => tab_close(state),
        Command::TabNext => tab_switch(state, TabMove::Next),
        Command::TabPrev => tab_switch(state, TabMove::Prev),
        Command::TabGoto(n) => tab_switch(state, TabMove::Index(n)),
        Command::BeginBookmarkSet => {
            state.mode = InteractionMode::OneKey(OneKey::BookmarkSet);
            state.notice = Some("bookmark: press a key".to_string());
            Step::None
        }
        Command::BeginBookmarkGo => {
            state.mode = InteractionMode::OneKey(OneKey::BookmarkGo);
            state.notice = Some("go to bookmark: press a key".to_string());
            Step::None
        }
        Command::BeginBookmarkUnset => {
            state.mode = InteractionMode::OneKey(OneKey::BookmarkUnset);
            state.notice = Some("unset bookmark: press a key".to_string());
            Step::None
        }
        Command::Help => {
            state.mode = InteractionMode::Help(prompt::HelpView { scroll: 0, count: None });
            Step::None
        }
        Command::BeginConsole => open_line(state, LineKind::Console, "console", String::new(), 0, false),
        Command::BeginShell => open_line(state, LineKind::Shell, "shell", String::new(), 0, false),
        Command::OpenShell => Step::Run {
            spec: editor::shell_launch(std::env::var("SHELL").ok().as_deref(), None, &state.current_dir),
            report_nonzero: false,
        },
        Command::Reload => scan_of(Some(navigation::rescan_current(state))),
        Command::DisplayFile => {
            if state.selected_entry().is_none() {
                state.notice = Some("nothing to display".to_string());
            } else {
                state.mode = InteractionMode::Pager(prompt::PagerView { scroll: 0, count: None });
            }
            Step::None
        }
    }
}

pub fn activate_now(state: &mut AppState, editor_only: bool) -> Step {
    if !editor_only && let Some(plan) = navigation::enter_selected(state) {
        return Step::Scan(plan);
    }
    let Some(entry) = state.selected_entry() else {
        return Step::None;
    };
    if entry.is_dir {
        return Step::None;
    }
    let path = entry.path.clone();
    let name = entry.name.clone();
    if editor_only {
        return Step::OpenEditor(path);
    }
    match rifle::choose(name.as_str(), rifle::xdg_open_available()) {
        OpenChoice::Editor => Step::OpenEditor(path),
        OpenChoice::External(program) => Step::Run {
            spec: EditorLaunch {
                program: program.into(),
                args: vec![path.as_os_str().to_os_string()],
                cwd: Some(state.current_dir.clone()),
            },
            report_nonzero: true,
        },
    }
}

fn scan_of(plan: Option<ScanPlan>) -> Step {
    match plan {
        Some(plan) => Step::Scan(plan),
        None => Step::None,
    }
}

fn signed(count: Option<u32>) -> isize {
    repeat(count).min(isize::MAX as usize) as isize
}

fn page_step(page: usize, count: Option<u32>) -> isize {
    page.saturating_mul(repeat(count)).min(isize::MAX as usize) as isize
}

fn move_rows(state: &mut AppState, delta: isize) -> Step {
    motion::move_by(state, delta);
    moved(state)
}

fn moved(state: &mut AppState) -> Step {
    refresh_visual(state);
    Step::None
}

fn resort(state: &mut AppState) -> Step {
    let follow = state.selected_entry().map(|entry| entry.name.clone());
    sort_entries(&mut state.entries, &state.view.sort);
    sort_entries(&mut state.parent_entries, &state.view.sort);
    state.sync_selection_after_listing_change(follow.as_deref());
    motion::snap_to_visible(state);
    moved(state)
}

fn open_line(state: &mut AppState, kind: LineKind, title: &'static str, buffer: String, cursor: usize, replace: bool) -> Step {
    let origin = state.selected;
    state.mode = InteractionMode::Line(LinePrompt::new(kind, title, buffer, cursor, replace, origin));
    Step::None
}

fn search_again(state: &mut AppState, forward: bool) -> Step {
    let Some(pattern) = state.search_pattern.clone() else {
        state.notice = Some("no search".to_string());
        return Step::None;
    };
    if !search::search_from_selection(state, &pattern, forward) {
        state.notice = Some(format!("no match for {pattern}"));
    }
    moved(state)
}

fn stash(state: &mut AppState, count: Option<u32>, cut: bool) {
    let paths = targets(state, count);
    if paths.is_empty() {
        state.notice = Some(if cut { "nothing to cut".to_string() } else { "nothing to yank".to_string() });
        return;
    }
    let count = paths.len();
    state.clipboard = Clipboard { paths, cut };
    state.notice = Some(if cut { format!("cut {count}") } else { format!("yanked {count}") });
}

fn targets(state: &AppState, count: Option<u32>) -> Vec<PathBuf> {
    let positions = motion::visible_positions(&state.entries, &state.view, state.pinned.as_deref());
    let marked: Vec<PathBuf> = positions
        .iter()
        .filter_map(|index| {
            let entry = &state.entries[*index];
            state.marked.contains(&entry.path).then(|| entry.path.clone())
        })
        .collect();
    if !marked.is_empty() {
        return marked;
    }
    let Some(current) = motion::highlighted_index(state.selected, &state.entries, &state.view, state.pinned.as_deref()) else {
        return Vec::new();
    };
    let Some(slot) = positions.iter().position(|index| *index == current) else {
        return Vec::new();
    };
    positions
        .iter()
        .skip(slot)
        .take(repeat(count))
        .map(|index| state.entries[*index].path.clone())
        .collect()
}

fn paste(state: &mut AppState, overwrite: bool) -> Step {
    if state.clipboard.paths.is_empty() {
        state.notice = Some("nothing to paste".to_string());
        return Step::None;
    }
    let refresh = state.current_dir.clone();
    Step::FileOp {
        op: DiskOp::Paste {
            sources: state.clipboard.paths.clone(),
            dest: refresh.clone(),
            cut: state.clipboard.cut,
            overwrite,
        },
        refresh,
    }
}

fn begin_delete(state: &mut AppState, count: Option<u32>) -> Step {
    let paths = targets(state, count);
    if paths.is_empty() {
        state.notice = Some("nothing to delete".to_string());
        return Step::None;
    }
    state.mode = InteractionMode::Confirm(prompt::ConfirmPrompt { paths });
    Step::None
}

fn begin_rename(state: &mut AppState, kind: RenameKind) -> Step {
    let Some(entry) = state.selected_entry() else {
        state.notice = Some("nothing to rename".to_string());
        return Step::None;
    };
    let name = entry.name.to_string();
    let path = entry.path.clone();
    let (cursor, replace) = match kind {
        RenameKind::Change => (0, true),
        RenameKind::AppendStem => (prompt::stem_cursor(&name), false),
        RenameKind::AppendEnd => (prompt::char_len(&name), false),
        RenameKind::Insert => (0, false),
    };
    open_line(state, LineKind::Rename(path), "rename", name, cursor, replace)
}

fn toggle_mark(state: &mut AppState) {
    let Some(path) = state.selected_entry().map(|entry| entry.path.clone()) else {
        return;
    };
    if state.visual_anchor.is_some() {
        if !state.visual_base.remove(&path) {
            state.visual_base.insert(path);
        }
        refresh_visual(state);
    } else if !state.marked.remove(&path) {
        state.marked.insert(path);
    }
}

fn toggle_visual(state: &mut AppState) {
    if state.visual_anchor.take().is_some() {
        state.visual_base.clear();
        return;
    }
    let Some(path) = state.selected_entry().map(|entry| entry.path.clone()) else {
        return;
    };
    state.visual_base = state.marked.clone();
    state.visual_anchor = Some(path.clone());
    state.marked.insert(path);
}

fn refresh_visual(state: &mut AppState) {
    let Some(anchor) = state.visual_anchor.clone() else {
        return;
    };
    let positions = motion::visible_positions(&state.entries, &state.view, state.pinned.as_deref());
    let Some(current) = motion::highlighted_index(state.selected, &state.entries, &state.view, state.pinned.as_deref()) else {
        return;
    };
    let Some(start) = positions.iter().position(|index| state.entries[*index].path == anchor) else {
        return;
    };
    let Some(end) = positions.iter().position(|index| *index == current) else {
        return;
    };
    let (lo, hi) = if start <= end { (start, end) } else { (end, start) };
    let mut marks = state.visual_base.clone();
    for index in &positions[lo..=hi] {
        marks.insert(state.entries[*index].path.clone());
    }
    state.marked = marks;
}

fn history(state: &mut AppState, back: bool) -> Step {
    let plan = if back { navigation::history_back(state) } else { navigation::history_forward(state) };
    match plan {
        Some(plan) => Step::Scan(plan),
        None => {
            state.notice = Some(if back { "no earlier directory".to_string() } else { "no later directory".to_string() });
            Step::None
        }
    }
}

fn cd_place(state: &mut AppState, place: CdPlace) -> Step {
    let home = std::env::var("HOME").ok();
    let path = match place_path(place, home.as_deref()) {
        Ok(path) => path,
        Err(err) => {
            state.notice = Some(err);
            return Step::None;
        }
    };
    scan_of(navigation::go_to(state, path, true))
}

pub fn place_path(place: CdPlace, home: Option<&str>) -> Result<PathBuf, String> {
    let path = match place {
        CdPlace::Home => {
            let home = home.map(str::trim).filter(|value| !value.is_empty()).ok_or_else(|| "HOME is not set".to_string())?;
            return Ok(PathBuf::from(home));
        }
        CdPlace::Root => "/",
        CdPlace::Etc => "/etc",
        CdPlace::Usr => "/usr",
        CdPlace::Dev => "/dev",
        CdPlace::Media => "/media",
        CdPlace::Mnt => "/mnt",
        CdPlace::Tmp => "/tmp",
        CdPlace::Srv => "/srv",
        CdPlace::Opt => "/opt",
        CdPlace::Var => "/var",
    };
    Ok(PathBuf::from(path))
}

enum TabMove {
    Next,
    Prev,
    Index(u8),
}

fn tab_new(state: &mut AppState) -> Step {
    ensure_tabs(state);
    let tab = snapshot(state);
    state.tab_index += 1;
    state.tabs.insert(state.tab_index, tab);
    Step::None
}

fn tab_close(state: &mut AppState) -> Step {
    ensure_tabs(state);
    if state.tabs.len() <= 1 {
        state.notice = Some("last tab stays open".to_string());
        return Step::None;
    }
    state.tabs.remove(state.tab_index);
    if state.tab_index >= state.tabs.len() {
        state.tab_index = state.tabs.len() - 1;
    }
    scan_of(load_tab(state))
}

fn tab_switch(state: &mut AppState, movement: TabMove) -> Step {
    ensure_tabs(state);
    let len = state.tabs.len();
    let next = match movement {
        TabMove::Next => (state.tab_index + 1) % len,
        TabMove::Prev => (state.tab_index + len - 1) % len,
        TabMove::Index(n) => {
            let index = (n as usize).saturating_sub(1);
            if index >= len {
                state.notice = Some(format!("no tab {n}"));
                return Step::None;
            }
            index
        }
    };
    if next == state.tab_index {
        return Step::None;
    }
    state.tab_index = next;
    scan_of(load_tab(state))
}

fn ensure_tabs(state: &mut AppState) {
    if state.tabs.is_empty() {
        state.tabs.push(snapshot(state));
        state.tab_index = 0;
    } else {
        let tab = snapshot(state);
        if let Some(slot) = state.tabs.get_mut(state.tab_index) {
            *slot = tab;
        }
    }
}

fn snapshot(state: &AppState) -> Tab {
    Tab {
        dir: state.current_dir.clone(),
        selected_name: state.selected_entry().map(|entry| entry.name.clone()),
    }
}

fn load_tab(state: &mut AppState) -> Option<ScanPlan> {
    let tab = state.tabs.get(state.tab_index).cloned()?;
    if tab.dir == state.current_dir {
        if let Some(name) = tab.selected_name.as_deref()
            && let Some(index) = index_of_name(&state.entries, name)
        {
            state.selected = index;
        }
        motion::snap_to_visible(state);
        return None;
    }
    if let Some(name) = &tab.selected_name {
        state.history.insert(tab.dir.clone(), name.clone());
    }
    navigation::go_to(state, tab.dir, false)
}

pub fn on_line(state: &mut AppState, key: KeyEvent) -> Step {
    let InteractionMode::Line(mut prompt) = std::mem::replace(&mut state.mode, InteractionMode::Browser) else {
        return Step::None;
    };
    match prompt::on_key(&mut prompt, key) {
        LineAction::Close => {
            if matches!(prompt.kind, LineKind::Find) {
                state.selected = prompt.origin;
            }
            Step::None
        }
        LineAction::Changed => {
            let kind = prompt.kind.clone();
            let buffer = prompt.buffer.clone();
            let origin = prompt.origin;
            state.mode = InteractionMode::Line(prompt);
            if matches!(kind, LineKind::Find) {
                apply_find(state, &buffer, origin);
            }
            Step::None
        }
        LineAction::Submit => submit_line(state, prompt),
        LineAction::None => {
            state.mode = InteractionMode::Line(prompt);
            Step::None
        }
    }
}

fn apply_find(state: &mut AppState, prefix: &str, origin: usize) {
    let positions = motion::visible_positions(&state.entries, &state.view, state.pinned.as_deref());
    state.selected = search::find_prefix(&state.entries, &positions, origin, prefix).unwrap_or(origin);
}

fn submit_line(state: &mut AppState, prompt: LinePrompt) -> Step {
    match prompt.kind {
        LineKind::Search => {
            if prompt.buffer.is_empty() {
                state.notice = Some("empty search".to_string());
                return Step::None;
            }
            if !search::search_from_selection(state, &prompt.buffer, true) {
                state.notice = Some(format!("no match for {}", prompt.buffer));
            }
            moved(state)
        }
        LineKind::Find => Step::None,
        LineKind::Filter => {
            state.view.filter = if prompt.buffer.is_empty() { None } else { Some(prompt.buffer) };
            motion::snap_to_visible(state);
            moved(state)
        }
        LineKind::Rename(ref from) => {
            let from = from.clone();
            if let Err(err) = validate_entry_name(&prompt.buffer) {
                state.notice = Some(err);
                state.mode = InteractionMode::Line(prompt);
                return Step::None;
            }
            let current_name = from.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            if prompt.buffer == current_name {
                return Step::None;
            }
            let refresh = from.parent().unwrap_or(Path::new(".")).to_path_buf();
            Step::FileOp {
                op: DiskOp::Rename {
                    from,
                    name: prompt.buffer,
                },
                refresh,
            }
        }
        LineKind::Console => match console::parse(&prompt.buffer) {
            Ok(command) => run_console(state, command),
            Err(err) => {
                state.notice = Some(err);
                state.mode = InteractionMode::Line(prompt);
                Step::None
            }
        },
        LineKind::Shell => {
            if prompt.buffer.trim().is_empty() {
                state.notice = Some("empty shell command".to_string());
                state.mode = InteractionMode::Line(prompt);
                return Step::None;
            }
            Step::Run {
                spec: editor::shell_launch(std::env::var("SHELL").ok().as_deref(), Some(&prompt.buffer), &state.current_dir),
                report_nonzero: true,
            }
        }
    }
}

fn run_console(state: &mut AppState, command: ConsoleCommand) -> Step {
    match command {
        ConsoleCommand::Cd(arg) => scan_of(navigation::go_to(state, navigation::lexical_cd(&state.current_dir, &arg), true)),
        ConsoleCommand::Mkdir(name) => file_name_op(state, name, NameOp::Mkdir),
        ConsoleCommand::Touch(name) => file_name_op(state, name, NameOp::Touch),
        ConsoleCommand::Delete => begin_delete(state, None),
        ConsoleCommand::Rename(name) => {
            let Some(from) = state.selected_entry().map(|entry| entry.path.clone()) else {
                state.notice = Some("nothing to rename".to_string());
                return Step::None;
            };
            if let Err(err) = validate_entry_name(&name) {
                state.notice = Some(err);
                return Step::None;
            }
            Step::FileOp {
                op: DiskOp::Rename { from, name },
                refresh: state.current_dir.clone(),
            }
        }
        ConsoleCommand::Search(pattern) => {
            if !search::search_from_selection(state, &pattern, true) {
                state.notice = Some(format!("no match for {pattern}"));
            }
            moved(state)
        }
        ConsoleCommand::Filter(pattern) => {
            state.view.filter = Some(pattern);
            motion::snap_to_visible(state);
            moved(state)
        }
        ConsoleCommand::ClearFilter => {
            state.view.filter = None;
            motion::snap_to_visible(state);
            moved(state)
        }
        ConsoleCommand::Sort(key) => {
            state.view.sort.key = key;
            resort(state)
        }
        ConsoleCommand::ToggleReverse => {
            state.view.sort.reverse = !state.view.sort.reverse;
            resort(state)
        }
        ConsoleCommand::ToggleHidden => {
            state.view.show_hidden = !state.view.show_hidden;
            motion::snap_to_visible(state);
            moved(state)
        }
        ConsoleCommand::Shell(command) => Step::Run {
            spec: editor::shell_launch(std::env::var("SHELL").ok().as_deref(), Some(&command), &state.current_dir),
            report_nonzero: true,
        },
        ConsoleCommand::Quit => {
            state.should_quit = true;
            Step::None
        }
        ConsoleCommand::TabNew => tab_new(state),
        ConsoleCommand::TabClose => tab_close(state),
        ConsoleCommand::Reload => scan_of(Some(navigation::rescan_current(state))),
        ConsoleCommand::Help => {
            state.mode = InteractionMode::Help(prompt::HelpView { scroll: 0, count: None });
            Step::None
        }
    }
}

enum NameOp {
    Mkdir,
    Touch,
}

fn file_name_op(state: &mut AppState, name: String, kind: NameOp) -> Step {
    if let Err(err) = validate_entry_name(&name) {
        state.notice = Some(err);
        return Step::None;
    }
    let dir = state.current_dir.clone();
    let op = match kind {
        NameOp::Mkdir => DiskOp::Mkdir { dir: dir.clone(), name },
        NameOp::Touch => DiskOp::Touch { dir: dir.clone(), name },
    };
    Step::FileOp { op, refresh: dir }
}

pub fn on_confirm(state: &mut AppState, key: KeyEvent) -> Step {
    match prompt::on_confirm_key(key) {
        ConfirmAction::None => Step::None,
        ConfirmAction::Cancel => {
            state.mode = InteractionMode::Browser;
            Step::None
        }
        ConfirmAction::Confirm => {
            let InteractionMode::Confirm(prompt) = std::mem::replace(&mut state.mode, InteractionMode::Browser) else {
                return Step::None;
            };
            Step::FileOp {
                op: DiskOp::Delete { paths: prompt.paths },
                refresh: state.current_dir.clone(),
            }
        }
    }
}

pub fn on_one_key(state: &mut AppState, key: KeyEvent) -> Step {
    let InteractionMode::OneKey(action) = &state.mode else {
        return Step::None;
    };
    let action = *action;
    if key.kind != KeyEventKind::Press {
        return Step::None;
    }
    if key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE {
        state.mode = InteractionMode::Browser;
        state.notice = None;
        return Step::None;
    }
    let Some(mark) = bookmark_char(key) else {
        state.mode = InteractionMode::Browser;
        state.notice = Some("not a bookmark key".to_string());
        return Step::None;
    };
    state.mode = InteractionMode::Browser;
    match action {
        OneKey::BookmarkSet => {
            state.bookmarks.insert(mark, state.current_dir.clone());
            state.notice = Some(format!("bookmark {mark}"));
            Step::None
        }
        OneKey::BookmarkUnset => {
            if state.bookmarks.remove(&mark).is_some() {
                state.notice = Some(format!("unset {mark}"));
            } else {
                state.notice = Some(format!("no bookmark {mark}"));
            }
            Step::None
        }
        OneKey::BookmarkGo => {
            let Some(path) = state.bookmarks.get(&mark).cloned() else {
                state.notice = Some(format!("no bookmark {mark}"));
                return Step::None;
            };
            scan_of(navigation::go_to(state, path, true))
        }
    }
}

fn bookmark_char(key: KeyEvent) -> Option<char> {
    if key.modifiers != KeyModifiers::NONE && key.modifiers != KeyModifiers::SHIFT {
        return None;
    }
    match key.code {
        KeyCode::Char(ch) if ch.is_ascii_alphanumeric() => Some(ch),
        _ => None,
    }
}

pub fn on_help(state: &mut AppState, key: KeyEvent, max_scroll: u16, page: u16) -> Step {
    let InteractionMode::Help(view) = &state.mode else {
        return Step::None;
    };
    let mut view = *view;
    if key.kind != KeyEventKind::Press {
        return Step::None;
    }
    if let Some(digit) = digit(key) {
        view.count = Some(view.count.unwrap_or(0).saturating_mul(10).saturating_add(digit));
        state.mode = InteractionMode::Help(view);
        return Step::None;
    }
    let count = view.count.take();
    match (key.code, key.modifiers) {
        (KeyCode::Esc | KeyCode::Char('q' | '?'), KeyModifiers::NONE) if count.is_none() => {
            state.mode = InteractionMode::Browser;
        }
        (KeyCode::Esc, KeyModifiers::NONE) => {
            state.mode = InteractionMode::Help(view);
        }
        (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
            view.scroll = add_scroll(view.scroll, signed(count), max_scroll);
            state.mode = InteractionMode::Help(view);
        }
        (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
            view.scroll = add_scroll(view.scroll, -signed(count), max_scroll);
            state.mode = InteractionMode::Help(view);
        }
        (KeyCode::Char('d'), KeyModifiers::CONTROL) | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
            view.scroll = add_scroll(view.scroll, scaled(page, count), max_scroll);
            state.mode = InteractionMode::Help(view);
        }
        (KeyCode::Char('u'), KeyModifiers::CONTROL) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
            view.scroll = add_scroll(view.scroll, -scaled(page, count), max_scroll);
            state.mode = InteractionMode::Help(view);
        }
        _ => state.mode = InteractionMode::Help(view),
    }
    Step::None
}

pub fn on_pager(state: &mut AppState, key: KeyEvent, max_scroll: u16, page: u16) -> Step {
    let InteractionMode::Pager(view) = &state.mode else {
        return Step::None;
    };
    let mut view = *view;
    if key.kind != KeyEventKind::Press {
        return Step::None;
    }
    if let Some(digit) = digit(key) {
        view.count = Some(view.count.unwrap_or(0).saturating_mul(10).saturating_add(digit));
        state.mode = InteractionMode::Pager(view);
        return Step::None;
    }
    let count = view.count.take();
    match (key.code, key.modifiers) {
        (KeyCode::Esc | KeyCode::Char('q' | 'i'), KeyModifiers::NONE) if count.is_none() => {
            state.mode = InteractionMode::Browser;
            Step::None
        }
        (KeyCode::Esc, KeyModifiers::NONE) => {
            state.mode = InteractionMode::Pager(view);
            Step::None
        }
        (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
            view.scroll = add_scroll(view.scroll, signed(count), max_scroll);
            state.mode = InteractionMode::Pager(view);
            Step::None
        }
        (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
            view.scroll = add_scroll(view.scroll, -signed(count), max_scroll);
            state.mode = InteractionMode::Pager(view);
            Step::None
        }
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
            view.scroll = add_scroll(view.scroll, scaled(page.max(1) / 2, count), max_scroll);
            state.mode = InteractionMode::Pager(view);
            Step::None
        }
        (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
            view.scroll = add_scroll(view.scroll, scaled(page, count), max_scroll);
            state.mode = InteractionMode::Pager(view);
            Step::None
        }
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
            view.scroll = add_scroll(view.scroll, -scaled(page.max(1) / 2, count), max_scroll);
            state.mode = InteractionMode::Pager(view);
            Step::None
        }
        (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
            view.scroll = add_scroll(view.scroll, -scaled(page, count), max_scroll);
            state.mode = InteractionMode::Pager(view);
            Step::None
        }
        (KeyCode::Char('g'), KeyModifiers::NONE) => {
            view.scroll = match count {
                Some(n) if n > 0 => (n as u16).saturating_sub(1).min(max_scroll),
                _ => 0,
            };
            state.mode = InteractionMode::Pager(view);
            Step::None
        }
        (KeyCode::Char('G') | KeyCode::End, KeyModifiers::NONE | KeyModifiers::SHIFT) => {
            view.scroll = match count {
                Some(n) if n > 0 => (n as u16).saturating_sub(1).min(max_scroll),
                _ => max_scroll,
            };
            state.mode = InteractionMode::Pager(view);
            Step::None
        }
        (KeyCode::Char('E'), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
            state.mode = InteractionMode::Browser;
            activate_now(state, true)
        }
        _ => {
            state.mode = InteractionMode::Pager(view);
            Step::None
        }
    }
}

fn digit(key: KeyEvent) -> Option<u32> {
    if key.modifiers != KeyModifiers::NONE {
        return None;
    }
    match key.code {
        KeyCode::Char(ch) if ch.is_ascii_digit() => ch.to_digit(10),
        _ => None,
    }
}

fn scaled(page: u16, count: Option<u32>) -> isize {
    (page as isize).saturating_mul(signed(count))
}

fn narrow(delta: isize) -> i32 {
    delta.clamp(i32::MIN as isize, i32::MAX as isize) as i32
}

fn add_scroll(scroll: u16, delta: isize, max: u16) -> u16 {
    let next = (scroll as isize).saturating_add(delta);
    next.clamp(0, max as isize) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::FileEntry;

    fn metrics() -> Metrics {
        Metrics {
            list_page: 10,
            list_half: 5,
            preview_limit: 20,
        }
    }

    fn files(names: &[&str]) -> AppState {
        let mut state = AppState::new("/tmp/work".into());
        state.entries = names
            .iter()
            .map(|name| FileEntry::new(format!("/tmp/work/{name}").into(), false, 1))
            .collect();
        state
    }

    fn name(state: &AppState) -> Option<&str> {
        state.selected_entry().map(|entry| entry.name.as_str())
    }

    #[test]
    fn five_j_moves_five_visible_rows_and_skips_hidden_ones() {
        let mut state = files(&[".a", "b", ".c", "d", "e", "f", "g"]);
        apply(Command::SelectNext, Some(5), &mut state, metrics());
        assert_eq!(name(&state), Some("g"));
    }

    #[test]
    fn zero_j_moves_one_and_zero_g_stays_at_the_bottom() {
        let mut state = files(&["a", "b", "c"]);
        apply(Command::SelectNext, Some(0), &mut state, metrics());
        assert_eq!(name(&state), Some("b"));
        apply(Command::MoveToBottom, Some(0), &mut state, metrics());
        assert_eq!(name(&state), Some("c"));
        apply(Command::MoveToTop, Some(2), &mut state, metrics());
        assert_eq!(name(&state), Some("b"));
        apply(Command::MoveToBottom, None, &mut state, metrics());
        assert_eq!(name(&state), Some("c"));
    }

    #[test]
    fn a_huge_count_clamps_instead_of_walking_off_the_list() {
        let mut state = files(&["a", "b"]);
        apply(Command::SelectNext, Some(u32::MAX), &mut state, metrics());
        assert_eq!(name(&state), Some("b"));
        apply(Command::MoveHalfDown, Some(50), &mut state, metrics());
        assert_eq!(name(&state), Some("b"));
    }

    #[test]
    fn hiding_a_dotfile_moves_the_cursor_off_it() {
        let mut state = files(&[".secret", "a"]);
        state.view.show_hidden = true;
        state.selected = 0;
        apply(Command::ToggleHidden, None, &mut state, metrics());
        assert_eq!(name(&state), Some("a"));
        assert!(!state.view.show_hidden);
    }

    #[test]
    fn yank_prefers_marks_and_otherwise_takes_a_count_downward() {
        let mut state = files(&["a", "b", "c", "d"]);
        apply(Command::Yank, Some(2), &mut state, metrics());
        assert_eq!(state.clipboard.paths.len(), 2);
        assert!(!state.clipboard.cut);
        assert_eq!(state.clipboard.paths[0].file_name().unwrap(), "a");

        state.marked.insert(state.entries[2].path.clone());
        apply(Command::Yank, Some(3), &mut state, metrics());
        assert_eq!(state.clipboard.paths.len(), 1, "marks win over the count");
        assert_eq!(state.clipboard.paths[0].file_name().unwrap(), "c");
    }

    #[test]
    fn yank_does_not_include_a_hidden_file_the_user_cannot_see() {
        let mut state = files(&[".secret", "a", "b"]);
        apply(Command::Yank, Some(3), &mut state, metrics());
        let names: Vec<_> = state
            .clipboard
            .paths
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn delete_opens_a_confirm_and_does_not_touch_the_clipboard() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"keep").unwrap();
        let mut state = AppState::new(dir.path().into());
        state.entries = vec![FileEntry::new(file.clone(), false, 4)];
        apply(Command::BeginDelete, None, &mut state, metrics());
        assert!(matches!(state.mode, InteractionMode::Confirm(_)));
        assert_eq!(std::fs::read(&file).unwrap(), b"keep");
        assert!(!state.should_quit);
    }

    #[test]
    fn visual_mode_keeps_marks_outside_the_moving_range() {
        let mut state = files(&["a", "b", "c", "d", "e"]);
        state.marked.insert(state.entries[0].path.clone());
        state.selected = 2;
        apply(Command::ToggleVisual, None, &mut state, metrics());
        apply(Command::SelectNext, Some(2), &mut state, metrics());
        let names: Vec<_> = state
            .marked
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        for expected in ["a", "c", "d", "e"] {
            assert!(names.iter().any(|name| name == expected), "{names:?} missing {expected}");
        }
        apply(Command::SelectPrevious, Some(2), &mut state, metrics());
        assert!(state.marked.contains(&state.entries[0].path));
        assert!(state.marked.contains(&state.entries[2].path));
        assert!(!state.marked.contains(&state.entries[4].path));
    }

    #[test]
    fn closing_the_only_tab_does_not_quit() {
        let mut state = files(&["a"]);
        apply(Command::TabClose, None, &mut state, metrics());
        assert!(!state.should_quit);
        assert_eq!(state.notice.as_deref(), Some("last tab stays open"));
        assert_eq!(state.current_dir, PathBuf::from("/tmp/work"));
    }

    #[test]
    fn a_new_tab_then_the_next_tab_does_not_drop_the_first_directory() {
        let mut state = files(&["a"]);
        apply(Command::TabNew, None, &mut state, metrics());
        assert_eq!(state.tabs.len(), 2);
        assert_eq!(state.tab_index, 1);
        state.current_dir = PathBuf::from("/tmp/other");
        apply(Command::TabPrev, None, &mut state, metrics());
        assert_eq!(state.current_dir, PathBuf::from("/tmp/work"));
    }

    #[test]
    fn history_back_then_a_new_cd_forgets_the_abandoned_forward_entry() {
        let mut state = AppState::new("/tmp/a".into());
        navigation::go_to(&mut state, "/tmp/b".into(), true);
        navigation::go_to(&mut state, "/tmp/c".into(), true);
        apply(Command::HistoryBack, None, &mut state, metrics());
        assert_eq!(state.current_dir, PathBuf::from("/tmp/b"));
        navigation::go_to(&mut state, "/tmp/d".into(), true);
        apply(Command::HistoryForward, None, &mut state, metrics());
        assert_eq!(state.current_dir, PathBuf::from("/tmp/d"));
        assert_eq!(state.notice.as_deref(), Some("no later directory"));
    }

    #[test]
    fn search_wraps_and_a_missing_pattern_does_not_move() {
        let mut state = files(&["aa", "bb", "ab"]);
        state.selected = 1;
        assert!(search::search_from_selection(&mut state, "a", true));
        assert_eq!(name(&state), Some("ab"));
        assert!(search::search_from_selection(&mut state, "a", true));
        assert_eq!(name(&state), Some("aa"));
        apply(Command::SearchNext, None, &mut state, metrics());
        assert_eq!(name(&state), Some("ab"));
        let mut fresh = files(&["aa"]);
        apply(Command::SearchNext, None, &mut fresh, metrics());
        assert_eq!(name(&fresh), Some("aa"));
        assert_eq!(fresh.notice.as_deref(), Some("no search"));
    }

    #[test]
    fn find_cancel_restores_the_origin_and_backspace_follows() {
        let mut state = files(&["aa", "ab", "ba"]);
        state.selected = 2;
        apply(Command::BeginFind, None, &mut state, metrics());
        let key = |ch| KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE);
        on_line(&mut state, key('a'));
        assert_eq!(name(&state), Some("aa"));
        on_line(&mut state, key('b'));
        assert_eq!(name(&state), Some("ab"));
        on_line(&mut state, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(name(&state), Some("aa"));
        on_line(&mut state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(name(&state), Some("ba"));
        assert!(matches!(state.mode, InteractionMode::Browser));
    }

    #[test]
    fn bookmark_j_does_not_move_the_cursor() {
        let mut state = files(&["a", "b"]);
        apply(Command::BeginBookmarkSet, None, &mut state, metrics());
        on_one_key(&mut state, KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(name(&state), Some("a"));
        assert_eq!(state.bookmarks.get(&'j').map(|path| path.as_path()), Some(Path::new("/tmp/work")));
    }

    #[test]
    fn q_inside_confirm_and_the_console_does_not_quit() {
        let mut state = files(&["a"]);
        apply(Command::BeginDelete, None, &mut state, metrics());
        on_confirm(&mut state, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!state.should_quit);
        assert!(matches!(state.mode, InteractionMode::Confirm(_)));
        on_confirm(&mut state, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(state.mode, InteractionMode::Confirm(_)));

        state.mode = InteractionMode::Browser;
        apply(Command::BeginConsole, None, &mut state, metrics());
        on_line(&mut state, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!state.should_quit);
        match &state.mode {
            InteractionMode::Line(prompt) => assert_eq!(prompt.buffer, "q"),
            other => panic!("console closed: {other:?}"),
        }
    }

    #[test]
    fn mkdir_rejects_a_path_and_cd_dotdot_does_not() {
        let mut state = AppState::new("/tmp/project".into());
        apply(Command::BeginConsole, None, &mut state, metrics());
        type_line(&mut state, "mkdir ../x");
        on_line(&mut state, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(state.mode, InteractionMode::Browser));
        assert!(state.notice.as_deref().unwrap_or("").contains("separator") || state.notice.as_deref().unwrap_or("").contains("name"));
        assert_eq!(state.current_dir, PathBuf::from("/tmp/project"));

        apply(Command::BeginConsole, None, &mut state, metrics());
        type_line(&mut state, "cd ..");
        let step = on_line(&mut state, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match step {
            Step::Scan(plan) => assert_eq!(plan.current.unwrap(), PathBuf::from("/tmp")),
            other => panic!("cd did not scan: {other:?}"),
        }
    }

    #[test]
    fn an_invalid_rename_stays_in_the_prompt() {
        let mut state = files(&["a.txt"]);
        apply(Command::Rename(RenameKind::Change), None, &mut state, metrics());
        on_line(&mut state, KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        on_line(&mut state, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(state.mode, InteractionMode::Line(_)));
        assert!(state.notice.is_some());
    }

    #[test]
    fn pager_j_does_not_move_the_file_and_q_does_not_quit() {
        let mut state = files(&["a", "b"]);
        state.preview = FilePreview::Text {
            path: state.entries[0].path.clone(),
            content: "one\ntwo\nthree\nfour\n".into(),
            truncated: false,
        };
        apply(Command::DisplayFile, None, &mut state, metrics());
        on_pager(&mut state, KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE), 10, 2);
        assert_eq!(name(&state), Some("a"));
        assert!(matches!(state.mode, InteractionMode::Pager(view) if view.scroll == 1));
        on_pager(&mut state, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE), 10, 2);
        assert!(!state.should_quit);
        assert!(matches!(state.mode, InteractionMode::Browser));
    }

    #[test]
    fn help_q_closes_the_overlay() {
        let mut state = files(&["a"]);
        apply(Command::Help, None, &mut state, metrics());
        on_help(&mut state, KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE), 30, 5);
        assert_eq!(name(&state), Some("a"));
        on_help(&mut state, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE), 30, 5);
        assert!(matches!(state.mode, InteractionMode::Browser));
        assert!(!state.should_quit);
    }

    #[test]
    fn text_opens_in_the_editor_and_shift_e_ignores_rifle() {
        let mut state = files(&["main.rs", "photo.png"]);
        let step = activate_now(&mut state, false);
        assert!(matches!(step, Step::OpenEditor(_)));
        state.selected = 1;
        let step = activate_now(&mut state, true);
        assert!(matches!(step, Step::OpenEditor(path) if path.ends_with("photo.png")));
    }

    fn type_line(state: &mut AppState, text: &str) {
        for ch in text.chars() {
            on_line(state, KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
    }
}
