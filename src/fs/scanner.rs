//! Async, cancellable directory scanning.

use std::path::PathBuf;

use tokio::fs::DirEntry;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::entry::FileEntry;

/// One update from a directory scan. The channel closes after the last
/// update (success or failure).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanUpdate {
    Batch(Vec<FileEntry>),
    Failed(String),
}

/// Entries are streamed to the consumer in batches this large, so a huge
/// directory doesn't block the UI waiting for the whole listing at once.
const BATCH_SIZE: usize = 100;

/// Bounds how many batches can sit in the channel ahead of the consumer,
/// capping memory during a large scan while still letting the scanner run
/// ahead of a slow UI. Unlike the event bus's channel (unbounded, low-rate
/// input events), a directory read can produce an unbounded number of
/// entries, so this one is deliberately bounded.
const CHANNEL_CAPACITY: usize = 8;

/// Spawns a background task that reads `path`'s entries and streams them
/// back in batches. Cancelling `cancel_token` (e.g. because the user
/// navigated to a different directory) stops the scan promptly, even if
/// it's mid-read or blocked sending a full batch, rather than finishing a
/// large, no-longer-wanted directory listing.
pub fn scan_directory(
    path: PathBuf,
    cancel_token: CancellationToken,
) -> mpsc::Receiver<ScanUpdate> {
    let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
    tokio::spawn(run_scan(path, cancel_token, sender, BATCH_SIZE));
    receiver
}

async fn run_scan(
    path: PathBuf,
    cancel_token: CancellationToken,
    sender: mpsc::Sender<ScanUpdate>,
    batch_size: usize,
) {
    if cancel_token.is_cancelled() {
        return;
    }

    let mut read_dir = match tokio::fs::read_dir(&path).await {
        Ok(read_dir) => read_dir,
        Err(err) => {
            send_update(&sender, &cancel_token, ScanUpdate::Failed(err.to_string())).await;
            return;
        }
    };

    let mut batch = Vec::with_capacity(batch_size);

    loop {
        let next = tokio::select! {
            biased;
            _ = cancel_token.cancelled() => return,
            entry = read_dir.next_entry() => entry,
        };

        match next {
            Ok(Some(dir_entry)) => {
                if let Some(file_entry) = to_file_entry(&dir_entry).await {
                    batch.push(file_entry);
                }
                if batch.len() >= batch_size
                    && !send_update(
                        &sender,
                        &cancel_token,
                        ScanUpdate::Batch(std::mem::replace(&mut batch, Vec::with_capacity(batch_size))),
                    )
                    .await
                {
                    return;
                }
            }
            // Directory exhausted.
            Ok(None) => break,
            // A single unreadable entry shouldn't stop the whole scan.
            Err(_) => continue,
        }
    }

    if !batch.is_empty() {
        send_update(&sender, &cancel_token, ScanUpdate::Batch(batch)).await;
    }
}

/// Sends one update, racing against cancellation so a full channel doesn't
/// leave a stale scan blocked after the user has already navigated away.
///
/// Returns whether the scan should continue.
async fn send_update(
    sender: &mpsc::Sender<ScanUpdate>,
    cancel_token: &CancellationToken,
    update: ScanUpdate,
) -> bool {
    tokio::select! {
        biased;
        _ = cancel_token.cancelled() => false,
        result = sender.send(update) => result.is_ok(),
    }
}

async fn to_file_entry(dir_entry: &DirEntry) -> Option<FileEntry> {
    // `file_type` does not follow. `metadata` does, so a symlink to a
    // directory stays enterable, while a dangling symlink is still listed
    // instead of being dropped on the follow error.
    let file_type = dir_entry.file_type().await.ok()?;
    let is_symlink = file_type.is_symlink();
    // `DirEntry::metadata` does not follow. A link to a directory has to be
    // stat'd through `fs::metadata`, or `l` would try to open it as a file.
    let followed = if is_symlink {
        tokio::fs::metadata(dir_entry.path()).await.ok()
    } else {
        dir_entry.metadata().await.ok()
    };
    if !is_symlink && followed.is_none() {
        return None;
    }
    let is_dir = followed.as_ref().is_some_and(|metadata| metadata.is_dir());
    let size = followed.as_ref().map(|metadata| metadata.len()).unwrap_or(0);
    let modified = followed.as_ref().map(modified_secs).unwrap_or(0);
    let mut entry = FileEntry::new(dir_entry.path(), is_dir, size);
    entry.is_symlink = is_symlink;
    entry.modified = modified;
    Some(entry)
}

fn modified_secs(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::time::Duration;

    use tokio::time::timeout;

    use super::*;

    fn write_file(dir: &std::path::Path, name: &str) {
        std::fs::write(dir.join(name), b"x").unwrap();
    }

    async fn collect(path: &std::path::Path) -> Vec<FileEntry> {
        let mut receiver = scan_directory(path.to_path_buf(), CancellationToken::new());
        let mut listed = Vec::new();
        while let Some(update) = receiver.recv().await {
            match update {
                ScanUpdate::Batch(batch) => listed.extend(batch),
                ScanUpdate::Failed(err) => panic!("scan failed: {err}"),
            }
        }
        listed
    }

    #[tokio::test]
    async fn streams_all_entries_in_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "a.txt");
        write_file(dir.path(), "b.txt");
        std::fs::create_dir(dir.path().join("sub")).unwrap();

        let mut receiver = scan_directory(dir.path().to_path_buf(), CancellationToken::new());

        let mut names = HashSet::new();
        while let Some(update) = receiver.recv().await {
            match update {
                ScanUpdate::Batch(batch) => {
                    for entry in batch {
                        names.insert(entry.name.to_string());
                    }
                }
                ScanUpdate::Failed(err) => panic!("scan failed: {err}"),
            }
        }

        assert_eq!(
            names,
            HashSet::from(["a.txt".to_string(), "b.txt".to_string(), "sub".to_string()])
        );
    }

    #[tokio::test]
    async fn listing_includes_hidden_files() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "visible.txt");
        write_file(dir.path(), ".viminfo");
        write_file(dir.path(), ".gitignore");

        let mut receiver = scan_directory(dir.path().to_path_buf(), CancellationToken::new());
        let mut names = HashSet::new();
        while let Some(update) = receiver.recv().await {
            match update {
                ScanUpdate::Batch(batch) => {
                    for entry in batch {
                        names.insert(entry.name.to_string());
                    }
                }
                ScanUpdate::Failed(err) => panic!("scan failed: {err}"),
            }
        }

        assert!(names.contains(".viminfo"), "listing skipped a hidden file: {names:?}");
        assert!(names.contains(".gitignore"), "listing skipped a hidden file: {names:?}");
        assert!(names.contains("visible.txt"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_to_a_file_is_listed_as_a_file() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "real.txt");
        std::os::unix::fs::symlink(dir.path().join("real.txt"), dir.path().join("alias.txt")).unwrap();

        let mut receiver = scan_directory(dir.path().to_path_buf(), CancellationToken::new());
        let mut listed = Vec::new();
        while let Some(update) = receiver.recv().await {
            match update {
                ScanUpdate::Batch(batch) => listed.extend(batch),
                ScanUpdate::Failed(err) => panic!("scan failed: {err}"),
            }
        }

        let alias = listed.iter().find(|entry| entry.name == "alias.txt").expect("alias missing");
        assert!(!alias.is_dir, "symlink-to-file must list as a file: {alias:?}");
        assert!(alias.is_symlink, "symlink-to-file must be marked as a link: {alias:?}");
        let real = listed.iter().find(|entry| entry.name == "real.txt").expect("real missing");
        assert!(!real.is_symlink);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_to_a_directory_stays_enterable_and_marked() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("linked")).unwrap();

        let path = dir.path().to_path_buf();
        let listed = collect(&path).await;
        let linked = listed.iter().find(|entry| entry.name == "linked").expect("linked missing");
        assert!(linked.is_dir, "l must still enter a link to a directory: {linked:?}");
        assert!(linked.is_symlink);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_dangling_symlink_is_listed() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(dir.path().join("missing"), dir.path().join("gone")).unwrap();

        let path = dir.path().to_path_buf();
        let listed = collect(&path).await;
        let gone = listed.iter().find(|entry| entry.name == "gone").expect("dangling link was dropped");
        assert!(gone.is_symlink);
        assert!(!gone.is_dir, "a dangling link must not be entered as a directory");
    }

    #[tokio::test]
    async fn nonexistent_directory_reports_failure_and_does_not_panic() {
        let mut receiver = scan_directory(
            PathBuf::from("/definitely/does/not/exist"),
            CancellationToken::new(),
        );

        match receiver.recv().await {
            Some(ScanUpdate::Failed(message)) => assert!(!message.is_empty()),
            other => panic!("expected a failure, got {other:?}"),
        }
        assert!(receiver.recv().await.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unreadable_directory_reports_failure_and_does_not_panic() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "secret.txt");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o000)).unwrap();

        let mut receiver = scan_directory(dir.path().to_path_buf(), CancellationToken::new());
        let result = timeout(Duration::from_millis(500), receiver.recv()).await;

        // Restore permissions so the tempdir can clean itself up on drop.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();

        match result {
            Ok(Some(ScanUpdate::Failed(message))) => assert!(!message.is_empty()),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn batches_are_capped_at_the_configured_size() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..7 {
            write_file(dir.path(), &format!("file-{i}"));
        }

        let (sender, mut receiver) = mpsc::channel(CHANNEL_CAPACITY);
        tokio::spawn(run_scan(
            dir.path().to_path_buf(),
            CancellationToken::new(),
            sender,
            3,
        ));

        let mut total = 0;
        while let Some(update) = receiver.recv().await {
            match update {
                ScanUpdate::Batch(batch) => {
                    assert!(batch.len() <= 3, "batch exceeded configured size: {batch:?}");
                    total += batch.len();
                }
                ScanUpdate::Failed(err) => panic!("scan failed: {err}"),
            }
        }
        assert_eq!(total, 7);
    }

    #[tokio::test]
    async fn already_cancelled_token_stops_before_the_first_batch() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "a.txt");

        let cancel_token = CancellationToken::new();
        cancel_token.cancel();

        let mut receiver = scan_directory(dir.path().to_path_buf(), cancel_token);

        assert!(receiver.recv().await.is_none());
    }

    #[tokio::test]
    async fn cancellation_unblocks_a_scan_stuck_on_a_full_channel() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..20 {
            write_file(dir.path(), &format!("file-{i}"));
        }

        let cancel_token = CancellationToken::new();
        let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
        // batch_size = 1 fills the capacity-8 channel after 8 entries,
        // guaranteeing the task is blocked on the 9th send when we cancel.
        let handle = tokio::spawn(run_scan(
            dir.path().to_path_buf(),
            cancel_token.clone(),
            sender,
            1,
        ));
        // Keep the receiver alive (undrained) so the send blocks on a full
        // channel rather than erroring out because it was dropped.
        let _receiver = receiver;

        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel_token.cancel();

        timeout(Duration::from_millis(500), handle)
            .await
            .expect("scan task did not stop promptly after cancellation")
            .expect("scan task panicked");
    }
}
