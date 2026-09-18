//! Central application state and the main event/render loop.
//!
//! Every background job (directory scan, recursive count, and anything
//! added later) reports its results through one shared `Message` channel,
//! so `run`'s `select!` never has to grow past two arms as features are
//! added. See `message` for the job -> state "reducer", `command` for the
//! key -> intent lookup, `navigation` for directory changes, and `jobs`
//! for spawning.

pub mod command;
pub mod jobs;
pub mod message;
pub mod navigation;
pub mod state;

pub use state::AppState;

use crossterm::event::Event;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio_util::sync::CancellationToken;

use command::{Command, command_for_key};
use message::{Message, update};
use navigation::ScanPlan;

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
    messages: UnboundedSender<Message>,
}

impl LoopCtl {
    fn new(app_cancel: CancellationToken, messages: UnboundedSender<Message>) -> Self {
        Self {
            scan_cancel: app_cancel.child_token(),
            app_cancel,
            active_job_cancel: None,
            messages,
        }
    }

    /// Cancels in-flight directory scans (and any recursive count of the
    /// old path), then starts whatever `plan` asks for under a fresh child
    /// token of `app_cancel`.
    fn apply_scan_plan(&mut self, state: &AppState, plan: ScanPlan) {
        self.scan_cancel.cancel();
        self.scan_cancel = self.app_cancel.child_token();
        if let Some(token) = self.active_job_cancel.take() {
            token.cancel();
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

    ctl.apply_scan_plan(&state, navigation::initial_scan_plan(&state));

    let result = loop {
        if state.should_quit {
            break Ok(());
        }

        if let Err(err) = tui.draw(|frame| ui::render(frame, &state)) {
            break Err(err.into());
        }

        tokio::select! {
            event = events.next() => {
                match event {
                    Ok(AppEvent::Input(Event::Key(key))) => {
                        if let Some(command) = command_for_key(key) {
                            run_command(command, &mut state, &mut ctl);
                        }
                    }
                    Ok(_) => {}
                    Err(err) => break Err(err),
                }
            }
            Some(message) = messages.recv() => {
                if matches!(message, Message::RecursiveCountFinished(_)) {
                    ctl.active_job_cancel = None;
                }
                update(&mut state, message);
            }
        }
    };

    app_cancel.cancel();
    result
}

fn run_command(command: Command, state: &mut AppState, ctl: &mut LoopCtl) {
    match command {
        Command::Quit => state.should_quit = true,
        Command::RecountRecursive => {
            if state.is_counting_recursively {
                return;
            }
            let job_token = ctl.app_cancel.child_token();
            state.is_counting_recursively = true;
            jobs::spawn_recursive_count(
                state.current_dir.clone(),
                job_token.clone(),
                ctl.messages.clone(),
            );
            ctl.active_job_cancel = Some(job_token);
        }
        Command::Cancel => {
            if let Some(token) = ctl.active_job_cancel.take() {
                token.cancel();
            }
            state.is_counting_recursively = false;
        }
        Command::SelectPrevious => {
            state.selected = state.selected.saturating_sub(1);
        }
        Command::SelectNext => {
            if state.selected + 1 < state.entries.len() {
                state.selected += 1;
            }
        }
        Command::EnterDirectory => {
            if let Some(plan) = navigation::enter_selected(state) {
                ctl.apply_scan_plan(state, plan);
            }
        }
        Command::OpenParent => {
            if let Some(plan) = navigation::open_parent(state) {
                ctl.apply_scan_plan(state, plan);
            }
        }
    }
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
        assert!(matches!(message, Message::RecursiveCountFinished(_)));
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

    #[test]
    fn quitting_cancels_a_still_running_child_job_via_the_parent_token() {
        let cancel_token = CancellationToken::new();
        let job_token = cancel_token.child_token();

        cancel_token.cancel();

        assert!(job_token.is_cancelled());
    }
}
