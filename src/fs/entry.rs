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

/// Directories first, then by name. Re-applied after every scan batch so a
/// listing is ordered even while it is still streaming in.
pub fn sort_listing(entries: &mut [FileEntry]) {
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.as_str().cmp(right.name.as_str()))
    });
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
    fn sort_listing_puts_directories_first_then_names() {
        let mut entries = vec![
            FileEntry::new("/tmp/z.txt".into(), false, 0),
            FileEntry::new("/tmp/m".into(), true, 0),
            FileEntry::new("/tmp/a.txt".into(), false, 0),
            FileEntry::new("/tmp/b".into(), true, 0),
        ];
        sort_listing(&mut entries);
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["b", "m", "a.txt", "z.txt"]);
    }

    #[test]
    fn falls_back_to_full_path_when_there_is_no_file_name() {
        // `/` has no file-name component; this must not panic.
        let entry = FileEntry::new(PathBuf::from("/"), true, 0);
        assert_eq!(entry.name, "/");
    }
}
