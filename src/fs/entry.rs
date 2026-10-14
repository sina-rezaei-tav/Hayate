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
    /// The entry itself is a symlink. `is_dir` is then the target: a link
    /// to a directory can still be entered, and a dangling link is listed
    /// with both flags false except `is_symlink`.
    pub is_symlink: bool,
    /// Unix seconds. `0` when the scanner could not read mtime.
    pub modified: u64,
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
            is_symlink: false,
            modified: 0,
        }
    }

    /// Suffix drawn after the name. A symlink is marked even when its
    /// target is a directory, so the row is not indistinguishable from a
    /// real directory.
    pub fn kind_suffix(&self) -> &'static str {
        match (self.is_dir, self.is_symlink) {
            (true, true) => "/@",
            (true, false) => "/",
            (false, true) => "@",
            (false, false) => "",
        }
    }
}

/// Directories first, then by name. Re-applied after every scan batch so a
/// listing is ordered even while it is still streaming in.
pub fn sort_listing(entries: &mut [FileEntry]) {
    super::sort::sort_entries(entries, &super::sort::SortSpec::default());
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
    fn kind_suffix_marks_links_without_hiding_directories() {
        let file = FileEntry::new("/tmp/a.txt".into(), false, 0);
        assert_eq!(file.kind_suffix(), "");
        let mut link = FileEntry::new("/tmp/a.txt".into(), false, 0);
        link.is_symlink = true;
        assert_eq!(link.kind_suffix(), "@");
        let dir = FileEntry::new("/tmp/sub".into(), true, 0);
        assert_eq!(dir.kind_suffix(), "/");
        let mut linked_dir = FileEntry::new("/tmp/sub".into(), true, 0);
        linked_dir.is_symlink = true;
        assert_eq!(linked_dir.kind_suffix(), "/@");
    }

    #[test]
    fn falls_back_to_full_path_when_there_is_no_file_name() {
        // `/` has no file-name component; this must not panic.
        let entry = FileEntry::new(PathBuf::from("/"), true, 0);
        assert_eq!(entry.name, "/");
    }
}
