//! Efficient recursive file counting.

use std::path::{Path, PathBuf};

use jwalk::WalkDir;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// Spawns a blocking task that recursively counts regular files under
/// `path` using `jwalk`'s parallel (rayon-backed) directory walker, so a
/// large tree is walked with multiple OS threads instead of one, and never
/// blocks the async runtime or the render loop.
///
/// Cancelling `cancel_token` stops the walk early rather than completing a
/// count the caller no longer needs (e.g. the user pressed the key again,
/// or quit while a count was still running).
pub fn count_files_recursive(
    path: PathBuf,
    cancel_token: CancellationToken,
) -> oneshot::Receiver<u64> {
    let (sender, receiver) = oneshot::channel();

    tokio::task::spawn_blocking(move || {
        let count = walk_and_count(&path, &cancel_token);
        // Ignored: a send error only means the receiver (and so the
        // caller) was already dropped, e.g. the app quit mid-walk.
        let _ = sender.send(count);
    });

    receiver
}

fn walk_and_count(path: &Path, cancel_token: &CancellationToken) -> u64 {
    let mut count = 0u64;

    // skip_hidden defaults to true in jwalk, which would omit `.viminfo`,
    // `.gitignore`, and everything under `.git` / `.config`. The listing
    // shows those entries, so the count has to include them.
    for entry in WalkDir::new(path).skip_hidden(false) {
        if cancel_token.is_cancelled() {
            break;
        }

        match entry {
            Ok(entry) if is_counted_file(&entry) => count += 1,
            // Directories aren't counted themselves; unreadable entries
            // (permission errors, races with concurrent deletion) are
            // skipped rather than aborting the whole count.
            _ => {}
        }
    }

    count
}

fn is_counted_file(entry: &jwalk::DirEntry<((), ())>) -> bool {
    let file_type = entry.file_type();
    if file_type.is_file() {
        return true;
    }
    // The listing treats a symlink-to-file as a file (`metadata` follows).
    // The walk itself does not follow links, so we still count the link.
    file_type.is_symlink() && entry.path().is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(dir: &Path, relative: &str) {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn counts_files_across_nested_directories() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "a.txt");
        write_file(dir.path(), "sub/b.txt");
        write_file(dir.path(), "sub/nested/c.txt");
        write_file(dir.path(), "sub/nested/d.txt");

        let count = walk_and_count(dir.path(), &CancellationToken::new());

        assert_eq!(count, 4);
    }

    #[test]
    fn does_not_count_directories_themselves() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("empty/nested")).unwrap();

        let count = walk_and_count(dir.path(), &CancellationToken::new());

        assert_eq!(count, 0);
    }

    #[test]
    fn counts_hidden_files_and_files_inside_hidden_directories() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "visible.txt");
        write_file(dir.path(), ".viminfo");
        write_file(dir.path(), ".gitignore");
        write_file(dir.path(), ".config/settings.json");
        write_file(dir.path(), ".git/HEAD");
        write_file(dir.path(), ".git/objects/ab");

        let count = walk_and_count(dir.path(), &CancellationToken::new());

        assert_eq!(
            count, 6,
            "jwalk skips hidden entries by default; the listing shows them, so the count must too"
        );
    }

    #[cfg(unix)]
    #[test]
    fn counts_a_symlink_to_a_file_as_one_file() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "real.txt");
        std::os::unix::fs::symlink(dir.path().join("real.txt"), dir.path().join("alias.txt")).unwrap();

        let count = walk_and_count(dir.path(), &CancellationToken::new());

        assert_eq!(count, 2, "the listing shows a symlink-to-file as a file");
    }

    #[cfg(unix)]
    #[test]
    fn does_not_count_a_dangling_symlink() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "real.txt");
        std::os::unix::fs::symlink(dir.path().join("missing.txt"), dir.path().join("broken")).unwrap();

        let count = walk_and_count(dir.path(), &CancellationToken::new());

        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlinked_directories() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "outside.txt");
        write_file(dir.path(), "realdir/a.txt");
        write_file(dir.path(), "realdir/b.txt");
        std::os::unix::fs::symlink(dir.path().join("realdir"), dir.path().join("linkdir")).unwrap();

        let count = walk_and_count(dir.path(), &CancellationToken::new());

        assert_eq!(
            count, 3,
            "following symlink dirs would double-count and can loop; count the real files only"
        );
    }

    #[test]
    fn a_single_file_counts_as_one() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "only.txt");

        assert_eq!(walk_and_count(dir.path(), &CancellationToken::new()), 1);
    }

    #[test]
    fn already_cancelled_token_stops_before_counting_anything() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "a.txt");
        write_file(dir.path(), "sub/b.txt");

        let cancel_token = CancellationToken::new();
        cancel_token.cancel();

        assert_eq!(walk_and_count(dir.path(), &cancel_token), 0);
    }

    #[cfg(unix)]
    #[test]
    fn skips_unreadable_subdirectories_without_panicking() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "visible.txt");
        std::fs::create_dir(dir.path().join("locked")).unwrap();
        write_file(dir.path(), "locked/hidden.txt");
        std::fs::set_permissions(
            dir.path().join("locked"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();

        let count = walk_and_count(dir.path(), &CancellationToken::new());

        // Restore permissions so the tempdir can clean itself up on drop.
        std::fs::set_permissions(
            dir.path().join("locked"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        // Only the visible file is reachable; the walk must not panic just
        // because one subdirectory couldn't be read.
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn async_wrapper_delivers_the_count() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "a.txt");
        write_file(dir.path(), "sub/b.txt");

        let receiver = count_files_recursive(dir.path().to_path_buf(), CancellationToken::new());

        assert_eq!(receiver.await.unwrap(), 2);
    }
}
