//! Central application state and the main event/render loop.

pub mod state;

pub use state::AppState;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::event::{AppEvent, EventHandler};
use crate::fs::{self, FileEntry};
use crate::ui;
use crate::util::Tui;

/// Drives the app until `state.should_quit` is set: draws a frame, then
/// waits for the next `AppEvent`, the next batch of scanned directory
/// entries, or a completed recursive file count, and applies it to `state`.
///
/// Takes `tui` by reference (rather than owning it) so the caller keeps
/// control of it and can call `Tui::restore` after `run` returns, whether it
/// returned `Ok` or `Err`.
pub async fn run(mut state: AppState, tui: &mut Tui, mut events: EventHandler) -> anyhow::Result<()> {
    let cancel_token = CancellationToken::new();
    let mut scan_results = fs::scan_directory(state.current_dir.clone(), cancel_token.clone());
    // Once the scan channel closes (the scan finished), we must stop
    // `recv()`-ing it: a closed `mpsc::Receiver` resolves to `None`
    // immediately on every poll, so leaving that branch enabled in
    // `select!` below would spin the loop as fast as possible instead of
    // waiting on the next real event.
    let mut scanning = true;

    // At most one recursive count runs at a time; `count_job` is `None`
    // whenever none is in flight, which also disables its `select!` arm
    // (see `next_count_result`).
    let mut count_job: Option<oneshot::Receiver<u64>> = None;
    let mut count_cancel_token: Option<CancellationToken> = None;

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
                    Ok(AppEvent::Tick) => {}
                    Ok(AppEvent::Input(Event::Key(key))) => {
                        handle_key(&mut state, key);
                        if count_job.is_none() && is_recount_key(key) {
                            let token = CancellationToken::new();
                            count_job = Some(fs::count_files_recursive(
                                state.current_dir.clone(),
                                token.clone(),
                            ));
                            count_cancel_token = Some(token);
                            state.is_counting_recursively = true;
                        }
                    }
                    Ok(AppEvent::Input(_)) => {}
                    Ok(AppEvent::Error(message)) => break Err(anyhow::anyhow!(message)),
                    Err(err) => break Err(err),
                }
            }
            batch = scan_results.recv(), if scanning => {
                apply_scan_result(&mut state, &mut scanning, batch);
            }
            count_result = next_count_result(&mut count_job) => {
                apply_count_result(&mut state, count_result);
                count_job = None;
                count_cancel_token = None;
            }
        }
    };

    cancel_token.cancel();
    if let Some(token) = count_cancel_token {
        token.cancel();
    }
    result
}

/// Applies one receive from the scan-results channel: extends `state`'s
/// entries on a batch, or flips `scanning` off once the channel closes.
/// Pulled out of the `select!` arm so this (in particular, the "stop
/// polling a closed channel" transition) is unit-testable without a real
/// scanner task.
fn apply_scan_result(state: &mut AppState, scanning: &mut bool, batch: Option<Vec<FileEntry>>) {
    match batch {
        Some(batch) => state.entries.extend(batch),
        None => *scanning = false,
    }
}

/// Awaits `job` if one is running, otherwise never resolves.
///
/// The `None` case using `std::future::pending` (rather than, say, an `if`
/// precondition on the `select!` arm) is what lets the same branch handle
/// "no count in flight yet" and "count just finished" without extra
/// bookkeeping in the caller.
async fn next_count_result(
    job: &mut Option<oneshot::Receiver<u64>>,
) -> Result<u64, oneshot::error::RecvError> {
    match job.as_mut() {
        Some(receiver) => receiver.await,
        None => std::future::pending().await,
    }
}

/// Applies a finished (or failed) recursive count to `state`.
///
/// A `RecvError` means the counting task was aborted or panicked before
/// sending a result; in that case we just clear the in-progress flag and
/// leave any previously computed count as-is, rather than treating it as a
/// fatal application error.
fn apply_count_result(state: &mut AppState, result: Result<u64, oneshot::error::RecvError>) {
    state.is_counting_recursively = false;
    if let Ok(count) = result {
        state.recursive_file_count = Some(count);
    }
}

/// Whether `key` should (re)start a recursive file count of the current
/// directory. Only a bare, unmodified `r` press qualifies, same convention
/// as the bare-`q` quit key.
fn is_recount_key(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && key.modifiers == KeyModifiers::NONE
        && matches!(key.code, KeyCode::Char('r'))
}

/// Applies a key event to `state`.
///
/// Ignores everything but `Press`: without this, Windows (and Unix
/// terminals with the Kitty keyboard protocol enabled) also deliver
/// `Release`/`Repeat` for the same physical keystroke, which would
/// double-apply whatever action the key maps to.
fn handle_key(state: &mut AppState, key: KeyEvent) {
    if key.kind != KeyEventKind::Press {
        return;
    }

    let is_quit_key = (key.modifiers == KeyModifiers::NONE && matches!(key.code, KeyCode::Char('q')))
        || (key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')));

    if is_quit_key {
        state.should_quit = true;
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyEventKind;

    use super::*;

    fn state() -> AppState {
        AppState::new("/tmp".into())
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn key_with_kind(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
        KeyEvent::new_with_kind(code, modifiers, kind)
    }

    #[test]
    fn q_quits() {
        let mut state = state();
        handle_key(&mut state, key(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(state.should_quit);
    }

    #[test]
    fn ctrl_c_quits() {
        let mut state = state();
        handle_key(&mut state, key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(state.should_quit);
    }

    #[test]
    fn plain_c_does_not_quit() {
        let mut state = state();
        handle_key(&mut state, key(KeyCode::Char('c'), KeyModifiers::NONE));
        assert!(!state.should_quit);
    }

    #[test]
    fn unrelated_keys_do_not_quit() {
        let mut state = state();

        for bits in 0..=0b0111_1111 {
            let modifiers = KeyModifiers::from_bits_truncate(bits);

            for c in ('a'..='z').chain('A'..='Z') {
                // Exclude exact quit keys safely
                let is_quit_key = (c == 'q' && modifiers == KeyModifiers::NONE)
                    || (c == 'c' && modifiers.contains(KeyModifiers::CONTROL));

                if is_quit_key {
                    continue;
                }

                handle_key(&mut state, key(KeyCode::Char(c), modifiers));
                assert!(
                    !state.should_quit,
                    "Key combination '{c}' with modifiers {modifiers:?} unexpectedly set should_quit to true"
                );
            }
        }
    }

    #[test]
    fn release_and_repeat_kinds_are_ignored_even_for_quit_keys() {
        let mut release = state();
        handle_key(
            &mut release,
            key_with_kind(KeyCode::Char('q'), KeyModifiers::NONE, KeyEventKind::Release),
        );
        assert!(!release.should_quit);

        let mut repeat = state();
        handle_key(
            &mut repeat,
            key_with_kind(KeyCode::Char('q'), KeyModifiers::NONE, KeyEventKind::Repeat),
        );
        assert!(!repeat.should_quit);
    }

    fn some_entry() -> FileEntry {
        FileEntry::new("/tmp/a.txt".into(), false, 0)
    }

    #[test]
    fn scan_batch_extends_entries_and_keeps_scanning() {
        let mut state = state();
        let mut scanning = true;

        apply_scan_result(&mut state, &mut scanning, Some(vec![some_entry()]));

        assert_eq!(state.entries.len(), 1);
        assert!(scanning);
    }

    #[test]
    fn closed_scan_channel_stops_scanning_without_touching_entries() {
        let mut state = state();
        let mut scanning = true;

        apply_scan_result(&mut state, &mut scanning, None);

        assert!(!scanning);
        assert!(state.entries.is_empty());
    }

    #[test]
    fn multiple_batches_accumulate() {
        let mut state = state();
        let mut scanning = true;

        apply_scan_result(&mut state, &mut scanning, Some(vec![some_entry()]));
        apply_scan_result(&mut state, &mut scanning, Some(vec![some_entry(), some_entry()]));

        assert_eq!(state.entries.len(), 3);
        assert!(scanning);
    }
}
