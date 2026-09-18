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
                            run_command(command, &mut state, &cancel_token, &message_tx);
                        }
                    }
                    Ok(_) => {}
                    Err(err) => break Err(err),
                }
            }
            Some(message) = messages.recv() => update(&mut state, message),
        }
    };

    cancel_token.cancel();
    result
}

/// Carries out `command`: either mutates `state` directly, or spawns a
/// background job that will report back through `messages`.
fn run_command(
    command: Command,
    state: &mut AppState,
    cancel_token: &CancellationToken,
    messages: &mpsc::UnboundedSender<Message>,
) {
    match command {
        Command::Quit => state.should_quit = true,
        Command::RecountRecursive => {
            if state.is_counting_recursively {
                return; // Already running; let it finish.
            }
            state.is_counting_recursively = true;
            jobs::spawn_recursive_count(
                state.current_dir.clone(),
                cancel_token.clone(),
                messages.clone(),
            );
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
        let (tx, _rx) = mpsc::unbounded_channel();

        run_command(Command::Quit, &mut state, &cancel_token, &tx);

        assert!(state.should_quit);
    }

    #[test]
    fn recount_command_is_a_noop_while_already_counting() {
        let mut state = state();
        state.is_counting_recursively = true;
        let cancel_token = CancellationToken::new();
        let (tx, mut rx) = mpsc::unbounded_channel();

        run_command(Command::RecountRecursive, &mut state, &cancel_token, &tx);

        // No job was spawned, so nothing should ever arrive on the channel.
        assert!(matches!(rx.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
    }

    #[tokio::test]
    async fn recount_command_starts_a_job_and_sets_the_flag() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = AppState::new(dir.path().to_path_buf());
        let cancel_token = CancellationToken::new();
        let (tx, mut rx) = mpsc::unbounded_channel();

        run_command(Command::RecountRecursive, &mut state, &cancel_token, &tx);

        assert!(state.is_counting_recursively);
        let message = rx.recv().await.expect("job should report back");
        assert!(matches!(message, Message::RecursiveCountFinished(_)));
    }
}
