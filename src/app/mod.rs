//! Central application state and the main event/render loop.
//!
//! Every background job (directory scan, recursive count, and anything
//! added later) reports its results through one shared `Message` channel,
//! so `run`'s `select!` never has to grow past two arms as features are
//! added. See `message` for the job -> state "reducer", `command` for the
//! key -> intent lookup, `navigation` for directory changes, and `jobs`
//! for spawning.

pub mod command;
pub mod editor;
pub mod jobs;
pub mod message;
pub mod navigation;
pub mod open_with;
pub mod state;

pub use state::AppState;

use std::path::{Path, PathBuf};

use crossterm::event::Event;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio_util::sync::CancellationToken;

use command::{Command, command_for_key};
use editor::EditorLaunch;
use message::{Message, update};
use navigation::ScanPlan;
use open_with::{InteractionMode, OpenWithAction, OpenWithPrompt};

use crate::event::{AppEvent, EventHandler};
use crate::ui;
use crate::util::Tui;

/// Tokens and the shared mailbox the event loop needs to start or cancel
/// background work. Kept together so `run_command` does not grow a new
/// parameter every time a job is added.
struct LoopCtl {
    app_cancel: CancellationToken,
    scan_cancel: CancellationToken,
    active_job_cancel: Option<CancellationToken>,
    preview_cancel: Option<CancellationToken>,
    open_with_cancel: Option<CancellationToken>,
    messages: UnboundedSender<Message>,
}

impl LoopCtl {
    fn new(app_cancel: CancellationToken, messages: UnboundedSender<Message>) -> Self {
        Self {
            scan_cancel: app_cancel.child_token(),
            app_cancel,
            active_job_cancel: None,
            preview_cancel: None,
            open_with_cancel: None,
            messages,
        }
    }

    fn cancel_preview(&mut self) {
        if let Some(token) = self.preview_cancel.take() {
            token.cancel();
        }
    }

    fn cancel_open_with_listing(&mut self) {
        if let Some(token) = self.open_with_cancel.take() {
            token.cancel();
        }
    }

    fn start_open_with_listing(&mut self, generation: u64) {
        self.cancel_open_with_listing();
        let token = self.app_cancel.child_token();
        jobs::spawn_open_with_listing(generation, token.clone(), self.messages.clone());
        self.open_with_cancel = Some(token);
    }

    /// Cancels in-flight directory scans (and any recursive count of the
    /// old path), then starts whatever `plan` asks for under a fresh child
    /// token of `app_cancel`.
    fn apply_scan_plan(&mut self, state: &mut AppState, plan: ScanPlan) {
        self.scan_cancel.cancel();
        self.scan_cancel = self.app_cancel.child_token();
        if let Some(token) = self.active_job_cancel.take() {
            token.cancel();
        }
        self.cancel_preview();

        if plan.current.is_some() {
            state.entries.clear();
            state.current_listing_complete = false;
            state.current_scan_error = None;
        }
        if plan.parent.is_some() {
            state.parent_entries.clear();
            state.parent_listing_complete = false;
            state.parent_scan_error = None;
        }

        let generation = state.scan_generation;
        if let Some(path) = plan.current {
            jobs::spawn_scan(
                path,
                self.scan_cancel.clone(),
                generation,
                self.messages.clone(),
            );
        }
        if let Some(path) = plan.parent {
            jobs::spawn_parent_scan(
                path,
                self.scan_cancel.clone(),
                generation,
                self.messages.clone(),
            );
        }
    }
}

/// Drives the app until `state.should_quit` is set: draws a frame, then
/// waits for either the next terminal `AppEvent` or the next `Message`
/// from a background job.
///
/// Takes `tui` by reference (rather than owning it) so the caller keeps
/// control of it and can call `Tui::restore` after `run` returns, whether it
/// returned `Ok` or `Err`.
pub async fn run(mut state: AppState, tui: &mut Tui, mut events: EventHandler) -> anyhow::Result<()> {
    let app_cancel = CancellationToken::new();
    let (message_tx, mut messages) = mpsc::unbounded_channel();
    let mut ctl = LoopCtl::new(app_cancel.clone(), message_tx);

    let plan = navigation::initial_scan_plan(&state);
    ctl.apply_scan_plan(&mut state, plan);

    let result = loop {
        if state.should_quit {
            break Ok(());
        }

        if let Ok(size) = tui.size() {
            state.frame_width = size.width;
            state.frame_height = size.height;
        }

        if let Err(err) = tui.draw(|frame| ui::render(frame, &state)) {
            break Err(err.into());
        }

        tokio::select! {
            event = events.next() => {
                match event {
                    Ok(AppEvent::Input(Event::Key(key))) => {
                        match dispatch_key(key, &mut state, &mut ctl) {
                            LoopEffect::OpenEditor(path) => {
                                if let Some(spec) = resolve_editor_spec(&mut state, &path)
                                    && run_external(tui, &mut events, &mut state, spec, false).await
                                {
                                    invalidate_cached_preview(&mut state, &mut ctl);
                                }
                                request_preview_if_needed(&mut state, &mut ctl);
                            }
                            LoopEffect::RunExternal { spec, report_nonzero } => {
                                if run_external(tui, &mut events, &mut state, spec, report_nonzero)
                                    .await
                                {
                                    invalidate_cached_preview(&mut state, &mut ctl);
                                }
                                request_preview_if_needed(&mut state, &mut ctl);
                            }
                            LoopEffect::None => {
                                request_preview_if_needed(&mut state, &mut ctl);
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(err) => break Err(err),
                }
            }
            Some(message) = messages.recv() => {
                handle_message(&mut state, &mut ctl, message);
            }
        }
    };

    app_cancel.cancel();
    result
}

/// Applies one background `Message`. Extracted from `run` so the stale-job
/// claims can be tested against the same control-token cleanup the loop uses.
fn handle_message(state: &mut AppState, ctl: &mut LoopCtl, message: Message) {
    match &message {
        Message::RecursiveCountFinished(generation, _) if *generation == state.count_generation => {
            ctl.active_job_cancel = None;
        }
        Message::PreviewReady(path, _) if state.preview.path() == Some(path.as_path()) => {
            ctl.preview_cancel = None;
        }
        _ => {}
    }
    update(state, message);
    request_preview_if_needed(state, ctl);
}

fn run_command(command: Command, state: &mut AppState, ctl: &mut LoopCtl) -> LoopEffect {
    state.notice = None;
    match command {
        Command::Quit => {
            state.should_quit = true;
            LoopEffect::None
        }
        Command::RecountRecursive => {
            if state.is_counting_recursively {
                return LoopEffect::None;
            }
            let job_token = ctl.app_cancel.child_token();
            state.is_counting_recursively = true;
            state.count_generation = state.count_generation.wrapping_add(1);
            jobs::spawn_recursive_count(
                state.current_dir.clone(),
                job_token.clone(),
                state.count_generation,
                ctl.messages.clone(),
            );
            ctl.active_job_cancel = Some(job_token);
            LoopEffect::None
        }
        Command::Cancel => {
            if let Some(token) = ctl.active_job_cancel.take() {
                token.cancel();
                state.count_generation = state.count_generation.wrapping_add(1);
            }
            state.is_counting_recursively = false;
            LoopEffect::None
        }
        Command::SelectPrevious => {
            let current = state.selected_index().unwrap_or(0);
            state.selected = current.saturating_sub(1);
            LoopEffect::None
        }
        Command::SelectNext => {
            if let Some(index) = state.selected_index()
                && index + 1 < state.entries.len()
            {
                state.selected = index + 1;
            }
            LoopEffect::None
        }
        Command::EnterDirectory => {
            if let Some(plan) = navigation::enter_selected(state) {
                ctl.apply_scan_plan(state, plan);
            }
            LoopEffect::None
        }
        Command::Activate => {
            if let Some(plan) = navigation::enter_selected(state) {
                ctl.apply_scan_plan(state, plan);
                LoopEffect::None
            } else {
                open_selected_file(state)
            }
        }
        Command::OpenInEditor => open_selected_file(state),
        Command::OpenWith => {
            begin_open_with(state, ctl);
            LoopEffect::None
        }
        Command::OpenParent => {
            if let Some(plan) = navigation::open_parent(state) {
                ctl.apply_scan_plan(state, plan);
            }
            LoopEffect::None
        }
        Command::PreviewPageUp => {
            let (page, limit) = preview_scroll_metrics(state);
            state.scroll_preview(-page, limit);
            LoopEffect::None
        }
        Command::PreviewPageDown => {
            let (page, limit) = preview_scroll_metrics(state);
            state.scroll_preview(page, limit);
            LoopEffect::None
        }
    }
}

/// Side effects that own the TTY. Kept out of `update` so later "shell out"
/// features (pager, `$SHELL`) use the same hand-off without growing `select!`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LoopEffect {
    None,
    OpenEditor(PathBuf),
    RunExternal {
        spec: EditorLaunch,
        report_nonzero: bool,
    },
}

fn dispatch_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
    ctl: &mut LoopCtl,
) -> LoopEffect {
    // Ctrl+C is the emergency exit even inside a modal. Bare `q` types into
    // the picker; it only quits from the browser.
    if matches!(command_for_key(key), Some(Command::Quit))
        && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
    {
        return run_command(Command::Quit, state, ctl);
    }
    if matches!(state.mode, InteractionMode::OpenWith(_)) {
        return apply_open_with_key(key, state, ctl);
    }
    match command_for_key(key) {
        Some(command) => run_command(command, state, ctl),
        None => LoopEffect::None,
    }
}

fn apply_open_with_key(
    key: crossterm::event::KeyEvent,
    state: &mut AppState,
    ctl: &mut LoopCtl,
) -> LoopEffect {
    let InteractionMode::OpenWith(prompt) = &mut state.mode else {
        return LoopEffect::None;
    };
    match open_with::handle_key(prompt, key) {
        OpenWithAction::None => {
            state.notice = None;
            LoopEffect::None
        }
        OpenWithAction::Close => {
            ctl.cancel_open_with_listing();
            state.mode = InteractionMode::Browser;
            state.notice = None;
            LoopEffect::None
        }
        OpenWithAction::Notice(message) => {
            state.notice = Some(message);
            LoopEffect::None
        }
        OpenWithAction::Launch(spec) => {
            ctl.cancel_open_with_listing();
            state.mode = InteractionMode::Browser;
            state.notice = None;
            LoopEffect::RunExternal {
                spec,
                report_nonzero: true,
            }
        }
    }
}

fn begin_open_with(state: &mut AppState, ctl: &mut LoopCtl) {
    let Some(entry) = state.selected_entry() else {
        return;
    };
    if entry.is_dir {
        return;
    }
    let path = entry.path.clone();
    state.open_with_generation = state.open_with_generation.wrapping_add(1);
    let generation = state.open_with_generation;
    state.mode = InteractionMode::OpenWith(OpenWithPrompt::new(path, generation));
    ctl.start_open_with_listing(generation);
}

fn open_selected_file(state: &AppState) -> LoopEffect {
    match state.selected_entry() {
        Some(entry) if !entry.is_dir => LoopEffect::OpenEditor(entry.path.clone()),
        _ => LoopEffect::None,
    }
}

fn resolve_editor_spec(state: &mut AppState, path: &Path) -> Option<EditorLaunch> {
    match editor::launch_spec_from_env(path) {
        Ok(spec) => Some(spec),
        Err(err) => {
            state.notice = Some(err.to_string());
            None
        }
    }
}

/// Hands the TTY to an external program and takes it back. Returns whether
/// the process was actually started (so the caller can reload a possibly
/// edited preview).
async fn run_external(
    tui: &mut Tui,
    events: &mut EventHandler,
    state: &mut AppState,
    spec: EditorLaunch,
    report_nonzero: bool,
) -> bool {
    let program = spec.program.to_string_lossy().into_owned();
    events.release_tty();
    if let Err(err) = tui.suspend() {
        events.recapture_tty();
        state.notice = Some(err.to_string());
        return false;
    }

    let outcome = tokio::task::spawn_blocking(move || editor::run_spec(&spec)).await;
    let resume_result = tui.resume();
    events.recapture_tty();

    if let Err(err) = resume_result {
        state.notice = Some(err.to_string());
    } else {
        match outcome {
            Ok(Ok(status)) if report_nonzero && !status.success() => {
                state.notice = Some(match status.code() {
                    Some(code) => format!("{program} exited with status {code}"),
                    None => format!("{program} was terminated by a signal"),
                });
            }
            Ok(Ok(_)) => {}
            Ok(Err(err)) => state.notice = Some(err.to_string()),
            Err(err) => state.notice = Some(format!("editor task failed: {err}")),
        }
    }
    true
}

fn invalidate_cached_preview(state: &mut AppState, ctl: &mut LoopCtl) {
    ctl.cancel_preview();
    state.preview = crate::preview::FilePreview::Idle;
    state.preview_scroll = 0;
}

/// Starts a preview read when the highlighted file is not already cached.
/// Directories and empty selections clear any in-flight read.
fn request_preview_if_needed(state: &mut AppState, ctl: &mut LoopCtl) {
    let wanted = state
        .selected_entry()
        .filter(|entry| !entry.is_dir)
        .map(|entry| entry.path.clone());

    match wanted {
        None => {
            ctl.cancel_preview();
            state.preview = crate::preview::FilePreview::Idle;
            state.preview_scroll = 0;
        }
        Some(path) if state.preview.path() == Some(path.as_path()) => {}
        Some(path) => {
            ctl.cancel_preview();
            let token = ctl.app_cancel.child_token();
            state.preview = crate::preview::FilePreview::Loading(path.clone());
            state.preview_scroll = 0;
            jobs::spawn_preview(path, token.clone(), ctl.messages.clone());
            ctl.preview_cancel = Some(token);
        }
    }
}

fn preview_scroll_metrics(state: &AppState) -> (i32, u16) {
    let area = ui::layout::split_panes(ratatui::layout::Rect::new(
        0,
        0,
        state.frame_width,
        state.frame_height,
    ))[2];
    let limit = ui::panes::preview::scroll_limit(state, area);
    let page = i32::from(ui::panes::preview::page_size(area));
    (page, limit)
}

#[cfg(test)]
mod tests {
    use crate::fs::FileEntry;

    use super::*;

    fn state() -> AppState {
        AppState::new("/tmp".into())
    }

    fn ctl() -> (LoopCtl, mpsc::UnboundedReceiver<Message>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (LoopCtl::new(CancellationToken::new(), tx), rx)
    }

    fn state_with_entries(count: usize) -> AppState {
        let mut state = state();
        state.entries = (0..count)
            .map(|i| FileEntry::new(format!("/tmp/{i}.txt").into(), false, 0))
            .collect();
        state
    }

    #[test]
    fn quit_command_sets_should_quit() {
        let mut state = state();
        let (mut ctl, _rx) = ctl();

        run_command(Command::Quit, &mut state, &mut ctl);

        assert!(state.should_quit);
    }

    #[test]
    fn recount_command_is_a_noop_while_already_counting() {
        let mut state = state();
        state.is_counting_recursively = true;
        let (mut ctl, mut rx) = ctl();

        run_command(Command::RecountRecursive, &mut state, &mut ctl);

        assert!(matches!(rx.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
        assert!(ctl.active_job_cancel.is_none());
    }

    #[tokio::test]
    async fn recount_command_starts_a_job_and_sets_the_flag() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = AppState::new(dir.path().to_path_buf());
        let (mut ctl, mut rx) = ctl();

        run_command(Command::RecountRecursive, &mut state, &mut ctl);

        assert!(state.is_counting_recursively);
        assert!(ctl.active_job_cancel.is_some());
        let message = rx.recv().await.expect("job should report back");
        assert!(matches!(message, Message::RecursiveCountFinished(_, _)));
    }

    #[test]
    fn cancel_command_cancels_the_active_job_and_clears_the_flag() {
        let mut state = state();
        state.is_counting_recursively = true;
        let (mut ctl, _rx) = ctl();
        let job_token = ctl.app_cancel.child_token();
        ctl.active_job_cancel = Some(job_token.clone());

        run_command(Command::Cancel, &mut state, &mut ctl);

        assert!(job_token.is_cancelled());
        assert!(ctl.active_job_cancel.is_none());
        assert!(!state.is_counting_recursively);
        assert!(!ctl.app_cancel.is_cancelled());
    }

    #[test]
    fn cancel_command_is_a_harmless_noop_when_nothing_is_running() {
        let mut state = state();
        let (mut ctl, _rx) = ctl();

        run_command(Command::Cancel, &mut state, &mut ctl);

        assert!(!state.should_quit);
        assert!(ctl.active_job_cancel.is_none());
    }

    #[test]
    fn select_previous_stops_at_zero() {
        let mut state = state_with_entries(3);
        let (mut ctl, _rx) = ctl();

        run_command(Command::SelectPrevious, &mut state, &mut ctl);
        assert_eq!(state.selected, 0);

        state.selected = 2;
        run_command(Command::SelectPrevious, &mut state, &mut ctl);
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn select_next_stops_at_the_last_entry() {
        let mut state = state_with_entries(3);
        let (mut ctl, _rx) = ctl();

        run_command(Command::SelectNext, &mut state, &mut ctl);
        assert_eq!(state.selected, 1);

        state.selected = 2;
        run_command(Command::SelectNext, &mut state, &mut ctl);
        assert_eq!(state.selected, 2, "must not move past the last entry");
    }

    #[test]
    fn select_next_on_empty_entries_does_not_panic_or_move() {
        let mut state = state();
        let (mut ctl, _rx) = ctl();

        run_command(Command::SelectNext, &mut state, &mut ctl);

        assert_eq!(state.selected, 0);
    }

    #[test]
    fn select_previous_moves_the_visible_highlight_when_selected_is_past_the_end() {
        let mut state = state_with_entries(3);
        state.selected = 10;
        let (mut ctl, _rx) = ctl();

        run_command(Command::SelectPrevious, &mut state, &mut ctl);

        assert_eq!(
            state.selected_index(),
            Some(1),
            "k should move off the last visible row, not decrement a stored index that is already off the end (selected={})",
            state.selected
        );
    }

    #[test]
    fn select_next_when_selected_is_past_the_end_stays_on_the_last_visible_row() {
        let mut state = state_with_entries(3);
        state.selected = 10;
        let (mut ctl, _rx) = ctl();

        run_command(Command::SelectNext, &mut state, &mut ctl);

        assert_eq!(
            state.selected_index(),
            Some(2),
            "j must not walk off the listing; the visible highlight is already the last row (selected={})",
            state.selected
        );
    }

    #[test]
    fn a_stale_recount_finished_does_not_abort_a_newer_count() {
        let mut state = state();
        state.is_counting_recursively = true;
        state.count_generation = 1;
        let (mut ctl, _rx) = ctl();
        let live = ctl.app_cancel.child_token();
        ctl.active_job_cancel = Some(live.clone());

        handle_message(
            &mut state,
            &mut ctl,
            Message::RecursiveCountFinished(0, None),
        );

        assert!(
            state.is_counting_recursively,
            "a cancelled older count cleared the in-progress flag"
        );
        assert!(
            ctl.active_job_cancel.is_some(),
            "a cancelled older count dropped the live cancel token"
        );
        assert!(!live.is_cancelled());
    }

    #[test]
    fn cancelling_a_recount_drops_a_late_successful_result() {
        let mut state = state();
        state.is_counting_recursively = true;
        state.count_generation = 1;
        let (mut ctl, _rx) = ctl();
        ctl.active_job_cancel = Some(ctl.app_cancel.child_token());

        run_command(Command::Cancel, &mut state, &mut ctl);
        handle_message(
            &mut state,
            &mut ctl,
            Message::RecursiveCountFinished(1, Some(99)),
        );

        assert!(!state.is_counting_recursively);
        assert!(state.recursive_file_count.is_none());
    }

    #[tokio::test]
    async fn a_stale_preview_ready_does_not_drop_the_live_preview_token() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("a.txt");
        let second = dir.path().join("b.txt");
        std::fs::write(&first, "aaa").unwrap();
        std::fs::write(&second, "bbb").unwrap();

        let mut state = AppState::new(dir.path().to_path_buf());
        state.entries = vec![
            FileEntry::new(first.clone(), false, 3),
            FileEntry::new(second.clone(), false, 3),
        ];
        let (mut ctl, _rx) = ctl();

        request_preview_if_needed(&mut state, &mut ctl);
        run_command(Command::SelectNext, &mut state, &mut ctl);
        request_preview_if_needed(&mut state, &mut ctl);
        let live = ctl.preview_cancel.clone().expect("b.txt preview should be in flight");

        handle_message(
            &mut state,
            &mut ctl,
            Message::PreviewReady(
                first,
                crate::preview::PreviewPayload::Text {
                    content: "stale".into(),
                    truncated: false,
                },
            ),
        );

        assert!(
            ctl.preview_cancel.is_some(),
            "a late preview for a.txt dropped the in-flight token for b.txt"
        );
        assert!(!live.is_cancelled());
        assert!(
            matches!(state.preview, crate::preview::FilePreview::Loading(ref path) if path == &second)
        );
    }

    #[tokio::test]
    async fn enter_directory_cancels_the_previous_scan_token() {
        let mut state = AppState::new("/tmp/project".into());
        state.entries = vec![FileEntry::new("/tmp/project/src".into(), true, 0)];
        let (mut ctl, _rx) = ctl();
        let previous_scan = ctl.scan_cancel.clone();

        run_command(Command::EnterDirectory, &mut state, &mut ctl);

        assert!(previous_scan.is_cancelled());
        assert!(!ctl.scan_cancel.is_cancelled());
        assert!(!ctl.app_cancel.is_cancelled());
        assert_eq!(state.current_dir, std::path::PathBuf::from("/tmp/project/src"));
    }

    #[test]
    fn open_parent_at_root_does_not_cancel_scans() {
        let mut state = AppState::new("/".into());
        let (mut ctl, _rx) = ctl();
        let previous_scan = ctl.scan_cancel.clone();

        run_command(Command::OpenParent, &mut state, &mut ctl);

        assert!(!previous_scan.is_cancelled());
        assert_eq!(state.current_dir, std::path::PathBuf::from("/"));
    }

    #[tokio::test]
    async fn applying_a_scan_plan_also_cancels_an_in_flight_recount() {
        let mut state = AppState::new("/tmp/project".into());
        state.entries = vec![FileEntry::new("/tmp/project/src".into(), true, 0)];
        state.is_counting_recursively = true;
        let (mut ctl, _rx) = ctl();
        let recount = ctl.app_cancel.child_token();
        ctl.active_job_cancel = Some(recount.clone());

        run_command(Command::EnterDirectory, &mut state, &mut ctl);

        assert!(recount.is_cancelled());
        assert!(ctl.active_job_cancel.is_none());
        assert!(!state.is_counting_recursively);
    }

    #[tokio::test]
    async fn applying_a_current_scan_plan_drops_stale_entries_and_errors() {
        let mut state = AppState::new("/tmp/project".into());
        state.entries = vec![FileEntry::new("/tmp/project/stale.txt".into(), false, 0)];
        state.current_listing_complete = true;
        state.current_scan_error = Some("old error".into());
        let (mut ctl, _rx) = ctl();

        ctl.apply_scan_plan(
            &mut state,
            ScanPlan {
                current: Some("/tmp/project/src".into()),
                parent: None,
            },
        );

        assert!(state.entries.is_empty());
        assert!(!state.current_listing_complete);
        assert!(state.current_scan_error.is_none());
    }

    #[test]
    fn quitting_cancels_a_still_running_child_job_via_the_parent_token() {
        let cancel_token = CancellationToken::new();
        let job_token = cancel_token.child_token();

        cancel_token.cancel();

        assert!(job_token.is_cancelled());
    }

    #[tokio::test]
    async fn requesting_preview_loads_the_selected_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "hello").unwrap();

        let mut state = AppState::new(dir.path().to_path_buf());
        state.entries = vec![FileEntry::new(path.clone(), false, 5)];
        let (mut ctl, mut rx) = ctl();

        request_preview_if_needed(&mut state, &mut ctl);
        assert!(matches!(state.preview, crate::preview::FilePreview::Loading(_)));

        let message = rx.recv().await.expect("preview job should report back");
        update(&mut state, message);

        assert!(matches!(
            state.preview,
            crate::preview::FilePreview::Text { ref content, .. } if content == "hello"
        ));
    }

    #[tokio::test]
    async fn requesting_preview_again_for_the_same_file_does_not_respawn() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "hello").unwrap();

        let mut state = AppState::new(dir.path().to_path_buf());
        state.entries = vec![FileEntry::new(path, false, 5)];
        let (mut ctl, _rx) = ctl();

        request_preview_if_needed(&mut state, &mut ctl);
        let first = ctl.preview_cancel.clone().expect("first request should spawn");
        request_preview_if_needed(&mut state, &mut ctl);

        // A respawn would cancel the previous token and replace it.
        assert!(!first.is_cancelled());
        assert!(ctl.preview_cancel.is_some());
    }

    #[test]
    fn requesting_preview_for_a_directory_clears_cached_file_text() {
        let mut state = state();
        state.entries = vec![FileEntry::new("/tmp/sub".into(), true, 0)];
        state.preview_scroll = 12;
        state.preview = crate::preview::FilePreview::Text {
            path: "/tmp/old.txt".into(),
            content: "stale".into(),
            truncated: false,
        };
        let (mut ctl, _rx) = ctl();
        let previous = ctl.app_cancel.child_token();
        ctl.preview_cancel = Some(previous.clone());

        request_preview_if_needed(&mut state, &mut ctl);

        assert!(previous.is_cancelled());
        assert!(matches!(state.preview, crate::preview::FilePreview::Idle));
        assert_eq!(state.preview_scroll, 0);
        assert!(ctl.preview_cancel.is_none());
    }

    fn tall_preview_state() -> AppState {
        let mut state = state();
        state.frame_width = 80;
        state.frame_height = 24;
        state.entries = vec![FileEntry::new("/tmp/a.txt".into(), false, 0)];
        state.preview = crate::preview::FilePreview::Text {
            path: "/tmp/a.txt".into(),
            content: (0..80).map(|i| format!("LINE-{i}")).collect::<Vec<_>>().join("\n"),
            truncated: false,
        };
        state
    }

    #[test]
    fn preview_page_down_scrolls_without_moving_the_file_list() {
        let mut state = tall_preview_state();
        let (mut ctl, _rx) = ctl();

        run_command(Command::PreviewPageDown, &mut state, &mut ctl);

        assert!(state.preview_scroll > 0);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn preview_page_down_stops_at_the_last_visible_page() {
        let mut state = tall_preview_state();
        let (mut ctl, _rx) = ctl();

        for _ in 0..20 {
            run_command(Command::PreviewPageDown, &mut state, &mut ctl);
        }

        let (_, limit) = preview_scroll_metrics(&state);
        assert_eq!(state.preview_scroll, limit);
        assert!(limit > 0);
    }

    #[test]
    fn preview_page_up_from_the_top_stays_at_zero() {
        let mut state = tall_preview_state();
        let (mut ctl, _rx) = ctl();

        run_command(Command::PreviewPageUp, &mut state, &mut ctl);

        assert_eq!(state.preview_scroll, 0);
    }

    #[tokio::test]
    async fn highlighting_a_different_file_resets_preview_scroll() {
        let mut state = tall_preview_state();
        state.entries.push(FileEntry::new("/tmp/b.txt".into(), false, 0));
        state.preview_scroll = 10;
        let (mut ctl, _rx) = ctl();

        run_command(Command::SelectNext, &mut state, &mut ctl);
        request_preview_if_needed(&mut state, &mut ctl);

        assert_eq!(state.preview_scroll, 0);
    }

    #[test]
    fn open_in_editor_on_a_file_does_not_cancel_scans() {
        let mut state = state_with_entries(2);
        let (mut ctl, _rx) = ctl();
        let scan = ctl.scan_cancel.clone();

        let effect = run_command(Command::OpenInEditor, &mut state, &mut ctl);

        assert!(!scan.is_cancelled());
        assert_eq!(
            effect,
            LoopEffect::OpenEditor(PathBuf::from("/tmp/0.txt"))
        );
    }

    #[test]
    fn open_in_editor_on_a_directory_is_a_noop() {
        let mut state = state();
        state.entries = vec![FileEntry::new("/tmp/sub".into(), true, 0)];
        let (mut ctl, _rx) = ctl();

        let effect = run_command(Command::OpenInEditor, &mut state, &mut ctl);

        assert_eq!(effect, LoopEffect::None);
        assert_eq!(state.current_dir, PathBuf::from("/tmp"));
    }

    #[test]
    fn open_in_editor_on_an_empty_listing_is_a_noop() {
        let mut state = state();
        let (mut ctl, _rx) = ctl();

        assert_eq!(
            run_command(Command::OpenInEditor, &mut state, &mut ctl),
            LoopEffect::None
        );
    }

    #[test]
    fn enter_directory_on_a_file_does_not_open_the_editor() {
        let mut state = state_with_entries(1);
        let (mut ctl, _rx) = ctl();

        let effect = run_command(Command::EnterDirectory, &mut state, &mut ctl);

        assert_eq!(effect, LoopEffect::None, "l/Right must not launch $EDITOR");
    }

    #[test]
    fn activate_on_a_file_opens_the_editor() {
        let mut state = state_with_entries(1);
        let (mut ctl, _rx) = ctl();

        let effect = run_command(Command::Activate, &mut state, &mut ctl);

        assert_eq!(effect, LoopEffect::OpenEditor(PathBuf::from("/tmp/0.txt")));
        assert_eq!(state.current_dir, PathBuf::from("/tmp"));
    }

    #[tokio::test]
    async fn activate_on_a_directory_enters_it() {
        let mut state = AppState::new("/tmp/project".into());
        state.entries = vec![FileEntry::new("/tmp/project/src".into(), true, 0)];
        let (mut ctl, _rx) = ctl();

        let effect = run_command(Command::Activate, &mut state, &mut ctl);

        assert_eq!(effect, LoopEffect::None);
        assert_eq!(state.current_dir, PathBuf::from("/tmp/project/src"));
    }

    #[test]
    fn a_command_clears_a_previous_notice() {
        let mut state = state();
        state.notice = Some("no editor found (set EDITOR, or install nvim/vim/vi/nano)".into());
        let (mut ctl, _rx) = ctl();

        run_command(Command::SelectNext, &mut state, &mut ctl);

        assert!(state.notice.is_none());
    }

    #[test]
    fn invalidate_cached_preview_drops_the_in_flight_token() {
        let mut state = state_with_entries(1);
        state.preview = crate::preview::FilePreview::Text {
            path: "/tmp/0.txt".into(),
            content: "old".into(),
            truncated: false,
        };
        state.preview_scroll = 4;
        let (mut ctl, _rx) = ctl();
        let previous = ctl.app_cancel.child_token();
        ctl.preview_cancel = Some(previous.clone());

        invalidate_cached_preview(&mut state, &mut ctl);

        assert!(previous.is_cancelled());
        assert!(ctl.preview_cancel.is_none());
        assert!(matches!(state.preview, crate::preview::FilePreview::Idle));
        assert_eq!(state.preview_scroll, 0);
    }

    fn key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    #[tokio::test]
    async fn open_with_on_a_file_opens_the_picker_without_cancelling_scans() {
        let mut state = state_with_entries(1);
        let (mut ctl, _rx) = ctl();
        let scan = ctl.scan_cancel.clone();

        let effect = run_command(Command::OpenWith, &mut state, &mut ctl);

        assert_eq!(effect, LoopEffect::None);
        assert!(!scan.is_cancelled());
        assert!(matches!(state.mode, InteractionMode::OpenWith(_)));
        assert_eq!(state.open_with_generation, 1);
    }

    #[test]
    fn open_with_on_a_directory_is_a_noop() {
        let mut state = state();
        state.entries = vec![FileEntry::new("/tmp/sub".into(), true, 0)];
        let (mut ctl, _rx) = ctl();

        run_command(Command::OpenWith, &mut state, &mut ctl);

        assert!(matches!(state.mode, InteractionMode::Browser));
    }

    #[tokio::test]
    async fn j_in_the_open_with_picker_does_not_move_the_file_list() {
        let mut state = state_with_entries(3);
        state.selected = 0;
        let (mut ctl, _rx) = ctl();
        run_command(Command::OpenWith, &mut state, &mut ctl);
        if let InteractionMode::OpenWith(prompt) = &mut state.mode {
            prompt.set_candidates(vec!["nvim".into(), "vim".into()]);
        }

        let effect = dispatch_key(key(crossterm::event::KeyCode::Char('j')), &mut state, &mut ctl);

        assert_eq!(effect, LoopEffect::None);
        assert_eq!(state.selected, 0, "j must not move the file list while the picker is open");
        match &state.mode {
            InteractionMode::OpenWith(prompt) => assert_eq!(prompt.selected, 1),
            InteractionMode::Browser => panic!("picker closed"),
        }
    }

    #[tokio::test]
    async fn q_in_the_open_with_picker_does_not_quit() {
        let mut state = state_with_entries(1);
        let (mut ctl, _rx) = ctl();
        run_command(Command::OpenWith, &mut state, &mut ctl);

        dispatch_key(key(crossterm::event::KeyCode::Char('q')), &mut state, &mut ctl);

        assert!(!state.should_quit);
        match &state.mode {
            InteractionMode::OpenWith(prompt) => assert_eq!(prompt.query, "q"),
            InteractionMode::Browser => panic!("picker closed"),
        }
    }

    #[tokio::test]
    async fn ctrl_c_quits_even_while_the_picker_is_open() {
        let mut state = state_with_entries(1);
        let (mut ctl, _rx) = ctl();
        run_command(Command::OpenWith, &mut state, &mut ctl);

        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('c'),
            crossterm::event::KeyModifiers::CONTROL,
        );
        dispatch_key(key, &mut state, &mut ctl);

        assert!(state.should_quit);
    }

    #[tokio::test]
    async fn esc_closes_the_open_with_picker() {
        let mut state = state_with_entries(1);
        let (mut ctl, _rx) = ctl();
        run_command(Command::OpenWith, &mut state, &mut ctl);
        assert!(ctl.open_with_cancel.is_some());

        dispatch_key(key(crossterm::event::KeyCode::Esc), &mut state, &mut ctl);

        assert!(matches!(state.mode, InteractionMode::Browser));
        assert!(ctl.open_with_cancel.is_none());
    }

    #[tokio::test]
    async fn enter_in_the_picker_launches_the_typed_command() {
        let mut state = state_with_entries(1);
        let (mut ctl, _rx) = ctl();
        run_command(Command::OpenWith, &mut state, &mut ctl);
        if let InteractionMode::OpenWith(prompt) = &mut state.mode {
            prompt.query = "hexdump -C".into();
        }

        let effect = dispatch_key(key(crossterm::event::KeyCode::Enter), &mut state, &mut ctl);

        match effect {
            LoopEffect::RunExternal { spec, report_nonzero } => {
                assert!(report_nonzero);
                assert_eq!(spec.program, std::ffi::OsString::from("hexdump"));
                assert_eq!(
                    spec.args,
                    vec![
                        std::ffi::OsString::from("-C"),
                        std::ffi::OsString::from("/tmp/0.txt"),
                    ]
                );
            }
            other => panic!("expected RunExternal, got {other:?}"),
        }
        assert!(matches!(state.mode, InteractionMode::Browser));
    }
}
