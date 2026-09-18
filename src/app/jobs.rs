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
    let mut results = fs::scan_directory(path, cancel_token);
    tokio::spawn(async move {
        while let Some(batch) = results.recv().await {
            if messages.send(Message::ScanBatch(batch)).is_err() {
                return; // App is shutting down; nothing left to report to.
            }
        }
        let _ = messages.send(Message::ScanFinished);
    });
}

/// Spawns a recursive file count for `path`, forwarding its result (or
/// `None` if the task was aborted/panicked) onto `messages`.
pub fn spawn_recursive_count(
    path: PathBuf,
    cancel_token: CancellationToken,
    messages: UnboundedSender<Message>,
) {
    let receiver = fs::count_files_recursive(path, cancel_token);
    tokio::spawn(async move {
        let count = receiver.await.ok();
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
}
