//! Spawns background jobs and forwards their results onto the shared
//! `Message` channel. `fs::` / `preview::` stay independently tested; these
//! adapters only translate their results into the app's unified `Message`.

use std::path::PathBuf;

use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::fs;
use crate::preview;

use super::message::Message;

/// Spawns the directory scanner for `path`, forwarding each batch (and a
/// final `ScanFinished`) onto `messages`, tagged with `generation` so the
/// reducer can drop results from a scan the user has already navigated away
/// from.
pub fn spawn_scan(
    path: PathBuf,
    cancel_token: CancellationToken,
    generation: u64,
    messages: UnboundedSender<Message>,
) {
    spawn_scan_forwarding(
        path,
        cancel_token,
        messages,
        move |batch| Message::ScanBatch(generation, batch),
        move |err| Message::ScanFailed(generation, err),
        Message::ScanFinished(generation),
    );
}

/// Spawns the directory scanner for `path` (intended to be `current_dir`'s
/// parent), forwarding each batch (and a final `ParentScanFinished`) onto
/// `messages`.
pub fn spawn_parent_scan(
    path: PathBuf,
    cancel_token: CancellationToken,
    generation: u64,
    messages: UnboundedSender<Message>,
) {
    spawn_scan_forwarding(
        path,
        cancel_token,
        messages,
        move |batch| Message::ParentScanBatch(generation, batch),
        move |err| Message::ParentScanFailed(generation, err),
        Message::ParentScanFinished(generation),
    );
}

/// Shared plumbing behind `spawn_scan`/`spawn_parent_scan`: only the
/// `Message` variants they wrap results in differ.
fn spawn_scan_forwarding(
    path: PathBuf,
    cancel_token: CancellationToken,
    messages: UnboundedSender<Message>,
    make_batch: impl Fn(Vec<crate::fs::FileEntry>) -> Message + Send + 'static,
    make_failed: impl Fn(String) -> Message + Send + 'static,
    finished: Message,
) {
    let mut results = fs::scan_directory(path, cancel_token);
    tokio::spawn(async move {
        while let Some(update) = results.recv().await {
            let message = match update {
                fs::ScanUpdate::Batch(batch) => make_batch(batch),
                fs::ScanUpdate::Failed(err) => make_failed(err),
            };
            if messages.send(message).is_err() {
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
    generation: u64,
    messages: UnboundedSender<Message>,
) {
    let receiver = fs::count_files_recursive(path, cancel_token.clone());
    tokio::spawn(async move {
        let count = receiver.await.ok().filter(|_| !cancel_token.is_cancelled());
        let _ = messages.send(Message::RecursiveCountFinished(generation, count));
    });
}

/// Reads a text preview of `path` off the render thread. Sends nothing if
/// the load is cancelled (the selection moved on).
pub fn spawn_preview(
    path: PathBuf,
    cancel_token: CancellationToken,
    messages: UnboundedSender<Message>,
) {
    tokio::spawn(async move {
        if let Some(payload) = preview::load(&path, &cancel_token).await {
            let _ = messages.send(Message::PreviewReady(path, payload));
        }
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
        spawn_scan(dir.path().to_path_buf(), CancellationToken::new(), 0, tx);

        let mut saw_batch = false;
        loop {
            match rx.recv().await.expect("channel closed before ScanFinished") {
                Message::ScanBatch(0, batch) => {
                    saw_batch = true;
                    assert!(!batch.is_empty());
                }
                Message::ScanFinished(0) => break,
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
        spawn_parent_scan(dir.path().to_path_buf(), CancellationToken::new(), 0, tx);

        let mut saw_batch = false;
        loop {
            match rx.recv().await.expect("channel closed before ParentScanFinished") {
                Message::ParentScanBatch(0, batch) => {
                    saw_batch = true;
                    assert!(!batch.is_empty());
                }
                Message::ParentScanFinished(0) => break,
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
        spawn_recursive_count(dir.path().to_path_buf(), CancellationToken::new(), 0, tx);

        match rx.recv().await.expect("channel closed without a result") {
            Message::RecursiveCountFinished(0, Some(count)) => assert_eq!(count, 2),
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
        spawn_recursive_count(dir.path().to_path_buf(), cancel_token, 0, tx);

        match rx.recv().await.expect("channel closed without a result") {
            Message::RecursiveCountFinished(0, None) => {}
            other => panic!("expected a cancelled (None) result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn spawn_preview_forwards_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "preview me").unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        spawn_preview(path.clone(), CancellationToken::new(), tx);

        match rx.recv().await.expect("channel closed without a result") {
            Message::PreviewReady(ready_path, crate::preview::PreviewPayload::Text { content, truncated }) => {
                assert_eq!(ready_path, path);
                assert_eq!(content, "preview me");
                assert!(!truncated);
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn spawn_preview_sends_nothing_when_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "preview me").unwrap();

        let cancel = CancellationToken::new();
        cancel.cancel();
        let (tx, mut rx) = mpsc::unbounded_channel();
        spawn_preview(path, cancel, tx);

        assert!(rx.recv().await.is_none());
    }
}
