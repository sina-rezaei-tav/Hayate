//! Spawns background jobs and forwards their results onto the shared
//! `Message` channel. `fs::scan_directory` / `fs::count_files_recursive`
//! stay untouched and independently tested; these are thin adapters that
//! translate their existing channel types into the app's unified `Message`.

use std::path::PathBuf;

use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::fs;

use super::message::Message;

/// Spawns the directory scanner for `path`, forwarding each batch (and a
/// final `ScanFinished`) onto `messages`.
pub fn spawn_scan(path: PathBuf, cancel_token: CancellationToken, messages: UnboundedSender<Message>) {
    spawn_scan_forwarding(path, cancel_token, messages, Message::ScanBatch, Message::ScanFinished);
}

/// Spawns the directory scanner for `path` (intended to be `current_dir`'s
/// parent), forwarding each batch (and a final `ParentScanFinished`) onto
/// `messages`.
pub fn spawn_parent_scan(
    path: PathBuf,
    cancel_token: CancellationToken,
    messages: UnboundedSender<Message>,
) {
    spawn_scan_forwarding(
        path,
        cancel_token,
        messages,
        Message::ParentScanBatch,
        Message::ParentScanFinished,
    );
}

/// Shared plumbing behind `spawn_scan`/`spawn_parent_scan`: only the
/// `Message` variants they wrap results in differ.
fn spawn_scan_forwarding(
    path: PathBuf,
    cancel_token: CancellationToken,
    messages: UnboundedSender<Message>,
    make_batch: impl Fn(Vec<crate::fs::FileEntry>) -> Message + Send + 'static,
    finished: Message,
) {
    let mut results = fs::scan_directory(path, cancel_token);
    tokio::spawn(async move {
        while let Some(batch) = results.recv().await {
            if messages.send(make_batch(batch)).is_err() {
                return; // App is shutting down; nothing left to report to.
            }
        }
        let _ = messages.send(finished);
    });
}

/// Spawns a recursive file count for `path`, forwarding its result onto
/// `messages`.
///
/// Reports `None` if the task was aborted/panicked, *or* if `cancel_token`
/// was cancelled before it finished: `count_files_recursive` still returns
/// whatever partial count it had accumulated at the moment it stopped, but
/// a partial count is not a meaningful answer to "how many files are
/// there", so it's discarded rather than shown as if it were complete.
pub fn spawn_recursive_count(
    path: PathBuf,
    cancel_token: CancellationToken,
    messages: UnboundedSender<Message>,
) {
    let receiver = fs::count_files_recursive(path, cancel_token.clone());
    tokio::spawn(async move {
        let count = receiver.await.ok().filter(|_| !cancel_token.is_cancelled());
        let _ = messages.send(Message::RecursiveCountFinished(count));
    });
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::*;

    #[tokio::test]
    async fn spawn_scan_forwards_batches_then_finished() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        spawn_scan(dir.path().to_path_buf(), CancellationToken::new(), tx);

        let mut saw_batch = false;
        loop {
            match rx.recv().await.expect("channel closed before ScanFinished") {
                Message::ScanBatch(batch) => {
                    saw_batch = true;
                    assert!(!batch.is_empty());
                }
                Message::ScanFinished => break,
                other => panic!("unexpected message: {other:?}"),
            }
        }
        assert!(saw_batch);
    }

    #[tokio::test]
    async fn spawn_parent_scan_forwards_batches_then_finished() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        spawn_parent_scan(dir.path().to_path_buf(), CancellationToken::new(), tx);

        let mut saw_batch = false;
        loop {
            match rx.recv().await.expect("channel closed before ParentScanFinished") {
                Message::ParentScanBatch(batch) => {
                    saw_batch = true;
                    assert!(!batch.is_empty());
                }
                Message::ParentScanFinished => break,
                other => panic!("unexpected message: {other:?}"),
            }
        }
        assert!(saw_batch);
    }

    #[tokio::test]
    async fn spawn_recursive_count_forwards_result() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b.txt"), b"x").unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        spawn_recursive_count(dir.path().to_path_buf(), CancellationToken::new(), tx);

        match rx.recv().await.expect("channel closed without a result") {
            Message::RecursiveCountFinished(Some(count)) => assert_eq!(count, 2),
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn spawn_recursive_count_discards_a_partial_count_when_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();

        let cancel_token = CancellationToken::new();
        cancel_token.cancel(); // Already cancelled before the walk even starts.

        let (tx, mut rx) = mpsc::unbounded_channel();
        spawn_recursive_count(dir.path().to_path_buf(), cancel_token, tx);

        match rx.recv().await.expect("channel closed without a result") {
            Message::RecursiveCountFinished(None) => {}
            other => panic!("expected a cancelled (None) result, got {other:?}"),
        }
    }
}
