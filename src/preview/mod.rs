//! File content preview. I/O lives here; the UI only renders cached results.

use std::path::{Path, PathBuf};

use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

/// Read at most this many bytes for a text preview. Larger files are shown
/// truncated rather than pulled into memory in full.
pub const PREVIEW_BYTE_LIMIT: usize = 64 * 1024;

/// Cached preview for the currently selected file. Directories use `Idle`
/// and are rendered from entry metadata instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilePreview {
    Idle,
    Loading(PathBuf),
    Text {
        path: PathBuf,
        content: String,
        truncated: bool,
    },
    Binary {
        path: PathBuf,
    },
    Error {
        path: PathBuf,
        message: String,
    },
}

impl FilePreview {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Idle => None,
            Self::Loading(path)
            | Self::Text { path, .. }
            | Self::Binary { path }
            | Self::Error { path, .. } => Some(path),
        }
    }
}

/// Outcome of a background preview read. `None` from [`load`] means the
/// work was cancelled before a meaningful result existed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewPayload {
    Text { content: String, truncated: bool },
    Binary,
    Error(String),
}

/// Reads up to [`PREVIEW_BYTE_LIMIT`] bytes from `path`. Treats a NUL byte
/// as binary so we never dump garbage into the terminal.
pub async fn load(path: &Path, cancel: &CancellationToken) -> Option<PreviewPayload> {
    if cancel.is_cancelled() {
        return None;
    }

    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(err) => return Some(PreviewPayload::Error(err.to_string())),
    };

    let mut limited = file.take(PREVIEW_BYTE_LIMIT as u64 + 1);
    let mut buf = Vec::new();
    if let Err(err) = limited.read_to_end(&mut buf).await {
        return Some(PreviewPayload::Error(err.to_string()));
    }

    if cancel.is_cancelled() {
        return None;
    }

    let truncated = buf.len() > PREVIEW_BYTE_LIMIT;
    if truncated {
        buf.truncate(PREVIEW_BYTE_LIMIT);
    }

    if buf.contains(&0) {
        return Some(PreviewPayload::Binary);
    }

    Some(PreviewPayload::Text {
        content: sanitize_for_preview(&String::from_utf8_lossy(&buf)),
        truncated,
    })
}

/// Makes file bytes safe to paint in a terminal. Tabs have display width 0
/// in Ratatui, so they skip cells and leave the previous file's characters
/// showing through (`.viminfo` is full of them). Other control characters
/// are replaced with spaces; newlines are kept.
pub(crate) fn sanitize_for_preview(text: &str) -> String {
    const TAB_WIDTH: usize = 8;
    let mut out = String::with_capacity(text.len());
    let mut column = 0usize;
    for ch in text.chars() {
        match ch {
            '\n' => {
                out.push('\n');
                column = 0;
            }
            '\r' => {}
            '\t' => {
                let spaces = TAB_WIDTH - (column % TAB_WIDTH);
                for _ in 0..spaces {
                    out.push(' ');
                }
                column += spaces;
            }
            ch if ch.is_control() => {
                out.push(' ');
                column += 1;
            }
            ch => {
                out.push(ch);
                column += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn loads_utf8_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "hello preview").unwrap();

        let result = load(&path, &CancellationToken::new()).await.unwrap();
        assert_eq!(
            result,
            PreviewPayload::Text {
                content: "hello preview".into(),
                truncated: false,
            }
        );
    }

    #[tokio::test]
    async fn truncates_over_the_byte_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        let contents = "a".repeat(PREVIEW_BYTE_LIMIT + 32);
        std::fs::write(&path, &contents).unwrap();

        match load(&path, &CancellationToken::new()).await.unwrap() {
            PreviewPayload::Text { content, truncated } => {
                assert!(truncated);
                assert_eq!(content.len(), PREVIEW_BYTE_LIMIT);
            }
            other => panic!("expected truncated text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn nul_bytes_are_treated_as_binary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob.bin");
        std::fs::write(&path, b"abc\0def").unwrap();

        assert_eq!(
            load(&path, &CancellationToken::new()).await.unwrap(),
            PreviewPayload::Binary
        );
    }

    #[tokio::test]
    async fn missing_file_is_an_error_not_a_panic() {
        match load(Path::new("/definitely/missing.txt"), &CancellationToken::new()).await {
            Some(PreviewPayload::Error(message)) => assert!(!message.is_empty()),
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancelled_token_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "hello").unwrap();

        let cancel = CancellationToken::new();
        cancel.cancel();

        assert!(load(&path, &cancel).await.is_none());
    }

    #[test]
    fn idle_has_no_path() {
        assert!(FilePreview::Idle.path().is_none());
    }

    #[test]
    fn tabs_expand_to_spaces_so_they_cannot_punch_holes_in_the_preview() {
        assert_eq!(sanitize_for_preview("\thello"), "        hello");
        assert_eq!(sanitize_for_preview("ab\tc"), "ab      c");
    }

    #[tokio::test]
    async fn viminfo_style_tabs_are_expanded_when_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".viminfo");
        std::fs::write(&path, "\"\tCHAR\t0\n\tGRUB\n|1,4\n").unwrap();

        match load(&path, &CancellationToken::new()).await.unwrap() {
            PreviewPayload::Text { content, .. } => {
                assert!(!content.contains('\t'), "raw tab survived: {content:?}");
                assert!(content.contains("CHAR"));
                assert!(content.contains("GRUB"));
            }
            other => panic!("expected text, got {other:?}"),
        }
    }
}
