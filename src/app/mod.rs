//! Central application state and the main event/render loop.
//!
//! Every background job (directory scan, recursive count, and anything
//! added later) reports its results through one shared `Message` channel,
//! so `run`'s `select!` never has to grow past two arms as features are
//! added. See `message` for the job -> state "reducer", `command` for the
//! key -> intent lookup, and `jobs` for spawning.

pub mod command;
pub mod jobs;
pub mod message;
pub mod state;

pub use state::AppState;

use crossterm::event::Event;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use command::{Command, command_for_key};
use message::{Message, update};

use crate::event::{AppEvent, EventHandler};
use crate::ui;
use crate::util::Tui;

/// Drives the app until `state.should_quit` is set: draws a frame, then
/// waits for either the next terminal `AppEvent` or the next `Message`
/// from a background job.
///
/// Takes `tui` by reference (rather than owning it) so the caller keeps
/// control of it and can call `Tui::restore` after `run` returns, whether it
/// returned `Ok` or `Err`.
pub async fn run(mut state: AppState, tui: &mut Tui, mut events: EventHandler) -> anyhow::Result<()> {
    let cancel_token = CancellationToken::new();
    let (message_tx, mut messages) = mpsc::unbounded_channel();

    jobs::spawn_scan(state.current_dir.clone(), cancel_token.clone(), message_tx.clone());

    // Cancellation token for whichever on-demand job (currently just the
    // recursive count) is running, if any. Kept separate from
    // `cancel_token` so `Command::Cancel` can stop just that job without
    // touching the always-running directory scan; cancelling `cancel_token`
    // itself (on quit, below) still cascades to cancel this too, since it's
    // always created as one of its child tokens.
    let mut active_job_cancel: Option<CancellationToken> = None;

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
                            run_command(
                                command,
                                &mut state,
                                &cancel_token,
                                &mut active_job_cancel,
                                &message_tx,
                            );
                        }
                    }
                    Ok(_) => {}
                    Err(err) => break Err(err),
                }
            }
            Some(message) = messages.recv() => {
                if matches!(message, Message::RecursiveCountFinished(_)) {
                    active_job_cancel = None; // Job's done; nothing left to cancel.
                }
                update(&mut state, message);
            }
        }
    };

    // Cascades to cancel `active_job_cancel`, if any, since it's a child of
    // this token.
    cancel_token.cancel();
    result
}

/// Carries out `command`: either mutates `state` directly, or spawns a
/// background job that will report back through `messages`.
fn run_command(
    command: Command,
    state: &mut AppState,
    cancel_token: &CancellationToken,
    active_job_cancel: &mut Option<CancellationToken>,
    messages: &mpsc::UnboundedSender<Message>,
) {
    match command {
        Command::Quit => state.should_quit = true,
        Command::RecountRecursive => {
            if state.is_counting_recursively {
                return; // Already running; let it finish.
            }
            let job_token = cancel_token.child_token();
            state.is_counting_recursively = true;
            jobs::spawn_recursive_count(state.current_dir.clone(), job_token.clone(), messages.clone());
            *active_job_cancel = Some(job_token);
        }
        Command::Cancel => {
            if let Some(token) = active_job_cancel.take() {
                token.cancel();
            }
            state.is_counting_recursively = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        AppState::new("/tmp".into())
    }

    #[test]
    fn quit_command_sets_should_quit() {
        let mut state = state();
        let cancel_token = CancellationToken::new();
        let mut active_job_cancel = None;
        let (tx, _rx) = mpsc::unbounded_channel();

        run_command(Command::Quit, &mut state, &cancel_token, &mut active_job_cancel, &tx);

        assert!(state.should_quit);
    }

    #[test]
    fn recount_command_is_a_noop_while_already_counting() {
        let mut state = state();
        state.is_counting_recursively = true;
        let cancel_token = CancellationToken::new();
        let mut active_job_cancel = None;
        let (tx, mut rx) = mpsc::unbounded_channel();

        run_command(
            Command::RecountRecursive,
            &mut state,
            &cancel_token,
            &mut active_job_cancel,
            &tx,
        );

        // No job was spawned, so nothing should ever arrive on the channel.
        assert!(matches!(rx.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
        assert!(active_job_cancel.is_none());
    }

    #[tokio::test]
    async fn recount_command_starts_a_job_and_sets_the_flag() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = AppState::new(dir.path().to_path_buf());
        let cancel_token = CancellationToken::new();
        let mut active_job_cancel = None;
        let (tx, mut rx) = mpsc::unbounded_channel();

        run_command(
            Command::RecountRecursive,
            &mut state,
            &cancel_token,
            &mut active_job_cancel,
            &tx,
        );

        assert!(state.is_counting_recursively);
        assert!(active_job_cancel.is_some());
        let message = rx.recv().await.expect("job should report back");
        assert!(matches!(message, Message::RecursiveCountFinished(_)));
    }

    #[test]
    fn cancel_command_cancels_the_active_job_and_clears_the_flag() {
        let mut state = state();
        state.is_counting_recursively = true;
        let cancel_token = CancellationToken::new();
        let job_token = cancel_token.child_token();
        let mut active_job_cancel = Some(job_token.clone());
        let (tx, _rx) = mpsc::unbounded_channel();

        run_command(Command::Cancel, &mut state, &cancel_token, &mut active_job_cancel, &tx);

        assert!(job_token.is_cancelled());
        assert!(active_job_cancel.is_none());
        assert!(!state.is_counting_recursively);
        // The always-running scan's token must be untouched.
        assert!(!cancel_token.is_cancelled());
    }

    #[test]
    fn cancel_command_is_a_harmless_noop_when_nothing_is_running() {
        let mut state = state();
        let cancel_token = CancellationToken::new();
        let mut active_job_cancel = None;
        let (tx, _rx) = mpsc::unbounded_channel();

        run_command(Command::Cancel, &mut state, &cancel_token, &mut active_job_cancel, &tx);

        assert!(!state.should_quit);
        assert!(active_job_cancel.is_none());
    }

    #[test]
    fn quitting_cancels_a_still_running_child_job_via_the_parent_token() {
        // Documents the hierarchical relationship `run` relies on: it only
        // ever calls `.cancel()` on the top-level token when exiting, and
        // trusts that any job token derived via `child_token()` is
        // cancelled automatically as a result.
        let cancel_token = CancellationToken::new();
        let job_token = cancel_token.child_token();

        cancel_token.cancel();

        assert!(job_token.is_cancelled());
    }
}
