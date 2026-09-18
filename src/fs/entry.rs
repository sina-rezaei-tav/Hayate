use std::path::PathBuf;

use compact_str::CompactString;

/// A single scanned directory entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub name: CompactString,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub is_hidden: bool,
}

impl FileEntry {
    pub fn new(path: PathBuf, is_dir: bool, size: u64) -> Self {
        // Falls back to the full path when there's no file-name component
        // (e.g. `/`), rather than panicking or producing an empty name.
        let name = path
            .file_name()
            .map(|name| CompactString::from(name.to_string_lossy()))
            .unwrap_or_else(|| CompactString::from(path.to_string_lossy()));
        let is_hidden = name.starts_with('.');

        Self {
            name,
            path,
            is_dir,
            size,
            is_hidden,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_name_from_path() {
        let entry = FileEntry::new(PathBuf::from("/home/user/notes.txt"), false, 42);
        assert_eq!(entry.name, "notes.txt");
        assert_eq!(entry.size, 42);
        assert!(!entry.is_dir);
    }

    #[test]
    fn detects_hidden_entries_by_leading_dot() {
        let entry = FileEntry::new(PathBuf::from("/home/user/.config"), true, 0);
        assert!(entry.is_hidden);
        assert!(entry.is_dir);
    }

    #[test]
    fn non_dotfile_is_not_hidden() {
        let entry = FileEntry::new(PathBuf::from("/home/user/config"), true, 0);
        assert!(!entry.is_hidden);
    }

    #[test]
    fn falls_back_to_full_path_when_there_is_no_file_name() {
        // `/` has no file-name component; this must not panic.
        let entry = FileEntry::new(PathBuf::from("/"), true, 0);
        assert_eq!(entry.name, "/");
    }
}
