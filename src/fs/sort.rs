//! Listing order. Directories stay grouped when `dirs_first` is set, and
//! reversing flips the key only, not that grouping.

use std::cmp::Ordering;

use super::FileEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Mtime,
    Extension,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortSpec {
    pub key: SortKey,
    pub reverse: bool,
    pub dirs_first: bool,
}

impl Default for SortSpec {
    fn default() -> Self {
        Self {
            key: SortKey::Name,
            reverse: false,
            dirs_first: true,
        }
    }
}

pub fn sort_entries(entries: &mut [FileEntry], spec: &SortSpec) {
    entries.sort_by(|left, right| cmp_entries(left, right, spec));
}

fn cmp_entries(left: &FileEntry, right: &FileEntry, spec: &SortSpec) -> Ordering {
    let group = if spec.dirs_first {
        right.is_dir.cmp(&left.is_dir)
    } else {
        Ordering::Equal
    };
    let mut key = match spec.key {
        SortKey::Name => natural_cmp(left.name.as_str(), right.name.as_str()),
        SortKey::Size => left.size.cmp(&right.size).then_with(|| natural_cmp(left.name.as_str(), right.name.as_str())),
        SortKey::Mtime => left
            .modified
            .cmp(&right.modified)
            .then_with(|| natural_cmp(left.name.as_str(), right.name.as_str())),
        SortKey::Extension => extension(left.name.as_str())
            .cmp(extension(right.name.as_str()))
            .then_with(|| natural_cmp(left.name.as_str(), right.name.as_str())),
    };
    if spec.reverse {
        key = key.reverse();
    }
    group.then(key)
}

/// Byte-wise natural order: digit runs compare as integers, without
/// overflowing, and the result is a total order (different strings never
/// compare equal).
pub fn natural_cmp(left: &str, right: &str) -> Ordering {
    let mut left_chars = left.chars().peekable();
    let mut right_chars = right.chars().peekable();
    loop {
        match (left_chars.peek().copied(), right_chars.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(a), Some(b)) if a.is_ascii_digit() && b.is_ascii_digit() => {
                let left_digits = take_digits(&mut left_chars);
                let right_digits = take_digits(&mut right_chars);
                let ordering = cmp_digit_runs(&left_digits, &right_digits);
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (Some(a), Some(b)) => {
                left_chars.next();
                right_chars.next();
                let ordering = a.cmp(&b);
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
        }
    }
}

fn take_digits(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut digits = String::new();
    while let Some(ch) = chars.peek().copied() {
        if !ch.is_ascii_digit() {
            break;
        }
        digits.push(ch);
        chars.next();
    }
    digits
}

/// Compare two digit runs as integers. Leading zeros do not change the
/// value; if the values match, the shorter run (fewer zeros) comes first
/// so `file2` and `file02` still have a stable order.
fn cmp_digit_runs(left: &str, right: &str) -> Ordering {
    let left_trimmed = left.trim_start_matches('0');
    let right_trimmed = right.trim_start_matches('0');
    let left_value = if left_trimmed.is_empty() { "0" } else { left_trimmed };
    let right_value = if right_trimmed.is_empty() { "0" } else { right_trimmed };
    left_value
        .len()
        .cmp(&right_value.len())
        .then_with(|| left_value.cmp(right_value))
        .then_with(|| left.len().cmp(&right.len()))
}

pub fn extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(0) | None => "",
        Some(index) => &name[index + 1..],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, dir: bool, size: u64, modified: u64) -> FileEntry {
        let mut entry = FileEntry::new(format!("/tmp/{name}").into(), dir, size);
        entry.modified = modified;
        entry
    }

    fn names(entries: &[FileEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.name.as_str()).collect()
    }

    #[test]
    fn natural_order_sorts_file2_before_file10() {
        assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("file10", "file2"), Ordering::Greater);
    }

    #[test]
    fn natural_order_is_total_for_different_zero_padding() {
        assert_ne!(natural_cmp("file2", "file02"), Ordering::Equal);
        assert_eq!(natural_cmp("file2", "file2"), Ordering::Equal);
        assert_eq!(natural_cmp("a", "a"), Ordering::Equal);
        assert_eq!(natural_cmp("a", "b"), Ordering::Less);
    }

    #[test]
    fn natural_order_handles_a_huge_digit_run_without_overflowing() {
        let big = format!("f{}", "9".repeat(80));
        let small = format!("f{}", "8".repeat(80));
        assert_eq!(natural_cmp(&small, &big), Ordering::Less);
    }

    #[test]
    fn an_empty_name_sorts_before_a_non_empty_one() {
        assert_eq!(natural_cmp("", "a"), Ordering::Less);
        assert_eq!(natural_cmp("", ""), Ordering::Equal);
    }

    #[test]
    fn default_spec_keeps_directories_first_then_natural_name() {
        let mut entries = vec![
            entry("z.txt", false, 0, 0),
            entry("m", true, 0, 0),
            entry("file10.txt", false, 0, 0),
            entry("file2.txt", false, 0, 0),
            entry("b", true, 0, 0),
        ];
        sort_entries(&mut entries, &SortSpec::default());
        assert_eq!(names(&entries), ["b", "m", "file2.txt", "file10.txt", "z.txt"]);
    }

    #[test]
    fn reverse_flips_names_but_not_the_directory_group() {
        let mut entries = vec![
            entry("a.txt", false, 0, 0),
            entry("b", true, 0, 0),
            entry("c.txt", false, 0, 0),
            entry("a", true, 0, 0),
        ];
        let spec = SortSpec {
            key: SortKey::Name,
            reverse: true,
            dirs_first: true,
        };
        sort_entries(&mut entries, &spec);
        assert_eq!(names(&entries), ["b", "a", "c.txt", "a.txt"]);
    }

    #[test]
    fn dirs_first_off_interleaves_files_and_directories() {
        let mut entries = vec![
            entry("m", true, 0, 0),
            entry("a.txt", false, 0, 0),
            entry("b", true, 0, 0),
        ];
        let spec = SortSpec {
            key: SortKey::Name,
            reverse: false,
            dirs_first: false,
        };
        sort_entries(&mut entries, &spec);
        assert_eq!(names(&entries), ["a.txt", "b", "m"]);
    }

    #[test]
    fn size_sort_breaks_ties_by_name() {
        let mut entries = vec![
            entry("b.txt", false, 5, 0),
            entry("a.txt", false, 5, 0),
            entry("c.txt", false, 1, 0),
        ];
        let spec = SortSpec {
            key: SortKey::Size,
            reverse: false,
            dirs_first: false,
        };
        sort_entries(&mut entries, &spec);
        assert_eq!(names(&entries), ["c.txt", "a.txt", "b.txt"]);
    }

    #[test]
    fn mtime_sort_uses_the_modified_field() {
        let mut entries = vec![
            entry("new.txt", false, 0, 30),
            entry("old.txt", false, 0, 10),
        ];
        let spec = SortSpec {
            key: SortKey::Mtime,
            reverse: false,
            dirs_first: false,
        };
        sort_entries(&mut entries, &spec);
        assert_eq!(names(&entries), ["old.txt", "new.txt"]);
    }

    #[test]
    fn extension_sort_treats_a_leading_dotfile_as_having_no_extension() {
        assert_eq!(extension(".gitignore"), "");
        assert_eq!(extension("a.tar.gz"), "gz");
        assert_eq!(extension("README"), "");
        let mut entries = vec![
            entry("b.txt", false, 0, 0),
            entry(".gitignore", false, 0, 0),
            entry("a.rs", false, 0, 0),
            entry("c.gz", false, 0, 0),
        ];
        let spec = SortSpec {
            key: SortKey::Extension,
            reverse: false,
            dirs_first: false,
        };
        sort_entries(&mut entries, &spec);
        assert_eq!(names(&entries), [".gitignore", "c.gz", "a.rs", "b.txt"]);
    }
}
