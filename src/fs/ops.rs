//! Disk mutations. Callers run these on a blocking thread.
//!
//! Copies never follow a directory symlink, never replace a symlink by
//! writing through it, and refuse to copy a directory into itself.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskOp {
    Paste {
        sources: Vec<PathBuf>,
        dest: PathBuf,
        cut: bool,
        overwrite: bool,
    },
    Delete {
        paths: Vec<PathBuf>,
    },
    Rename {
        from: PathBuf,
        name: String,
    },
    Mkdir {
        dir: PathBuf,
        name: String,
    },
    Touch {
        dir: PathBuf,
        name: String,
    },
}

pub fn execute(op: &DiskOp, cancel: &CancellationToken) -> OpReport {
    match op {
        DiskOp::Paste {
            sources,
            dest,
            cut,
            overwrite,
        } => paste(sources, dest, *cut, *overwrite, cancel),
        DiskOp::Delete { paths } => delete_paths(paths, cancel),
        DiskOp::Rename { from, name } => outcome(rename_entry(from, name).map(|_| "renamed".to_string())),
        DiskOp::Mkdir { dir, name } => outcome(mkdir_entry(dir, name).map(|_| "created directory".to_string())),
        DiskOp::Touch { dir, name } => outcome(touch_entry(dir, name).map(|_| "created file".to_string())),
    }
}

fn outcome(result: Result<String, String>) -> OpReport {
    match result {
        Ok(notice) => report(notice, true, false),
        Err(notice) => report(notice, false, false),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpReport {
    pub notice: String,
    pub ok: bool,
    /// True only when every cut source was moved. A later yank must not be
    /// cleared just because an older cut happened to finish.
    pub clear_sources: bool,
}

pub fn validate_entry_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("name is empty".to_string());
    }
    if name == "." || name == ".." {
        return Err("name is not allowed".to_string());
    }
    if name.chars().any(|ch| ch == '/' || ch == '\\' || ch.is_control()) {
        return Err("name contains a separator or a control character".to_string());
    }
    Ok(())
}

pub fn rename_entry(from: &Path, new_name: &str) -> Result<PathBuf, String> {
    validate_entry_name(new_name)?;
    let parent = from.parent().filter(|parent| !parent.as_os_str().is_empty()).ok_or_else(|| "file has no parent".to_string())?;
    let dest = parent.join(new_name);
    if dest == from {
        return Ok(dest);
    }
    if name_taken(&dest)? {
        return Err(format!("{new_name} already exists"));
    }
    fs::rename(from, &dest).map_err(|err| err.to_string())?;
    Ok(dest)
}

pub fn mkdir_entry(dir: &Path, name: &str) -> Result<PathBuf, String> {
    validate_entry_name(name)?;
    let dest = dir.join(name);
    if name_taken(&dest)? {
        return Err(format!("{name} already exists"));
    }
    fs::create_dir(&dest).map_err(|err| err.to_string())?;
    Ok(dest)
}

pub fn touch_entry(dir: &Path, name: &str) -> Result<PathBuf, String> {
    validate_entry_name(name)?;
    let dest = dir.join(name);
    if name_taken(&dest)? {
        return Err(format!("{name} already exists"));
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&dest)
        .map_err(|err| err.to_string())?;
    Ok(dest)
}

pub fn delete_paths(paths: &[PathBuf], cancel: &CancellationToken) -> OpReport {
    if paths.is_empty() {
        return report("nothing to delete", false, false);
    }
    let mut failed = Vec::new();
    let mut removed = 0usize;
    for path in paths {
        if cancel.is_cancelled() {
            failed.push(format!("{}: cancelled", display(path)));
            continue;
        }
        if is_root(path) {
            failed.push(format!("{}: refusing to delete the filesystem root", display(path)));
            continue;
        }
        match delete_path(path) {
            Ok(()) => removed += 1,
            Err(err) => failed.push(format!("{}: {err}", display(path))),
        }
    }
    finish(removed, "deleted", &failed, false)
}

pub fn paste(sources: &[PathBuf], dest_dir: &Path, cut: bool, overwrite: bool, cancel: &CancellationToken) -> OpReport {
    if sources.is_empty() {
        return report("nothing to paste", false, false);
    }
    let mut failed = Vec::new();
    let mut done = 0usize;
    for src in sources {
        if cancel.is_cancelled() {
            failed.push(format!("{}: cancelled", display(src)));
            continue;
        }
        match paste_one(src, dest_dir, cut, overwrite) {
            Ok(()) => done += 1,
            Err(err) => failed.push(format!("{}: {err}", display(src))),
        }
    }
    let clear = cut && failed.is_empty() && done == sources.len();
    finish(done, "pasted", &failed, clear)
}

fn paste_one(src: &Path, dest_dir: &Path, cut: bool, overwrite: bool) -> Result<(), String> {
    let name = src
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| "missing file name".to_string())?;
    validate_entry_name(&name)?;
    if would_nest(src, dest_dir)? {
        return Err("refusing to copy a directory into itself".to_string());
    }
    let same = dest_dir.join(&name) == src;
    if same && cut {
        return Ok(());
    }
    let dest = if overwrite {
        let dest = dest_dir.join(&name);
        if dest != src && name_taken(&dest)? {
            replace_dest(&dest)?;
        }
        dest
    } else {
        unique_dest(dest_dir, &name)?
    };
    if cut { move_path(src, &dest) } else { copy_path(src, &dest) }
}

fn would_nest(src: &Path, dest_dir: &Path) -> Result<bool, String> {
    let meta = fs::symlink_metadata(src).map_err(|err| err.to_string())?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Ok(false);
    }
    let src = src.canonicalize().map_err(|err| err.to_string())?;
    let dest = dest_dir.canonicalize().map_err(|err| err.to_string())?;
    Ok(dest.starts_with(&src))
}

fn unique_dest(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let direct = dir.join(name);
    if !name_taken(&direct)? {
        return Ok(direct);
    }
    let (stem, ext) = split_name(name);
    for index in 1..10_000u32 {
        let candidate = match ext {
            Some(ext) => format!("{stem}_{index}.{ext}"),
            None => format!("{stem}_{index}"),
        };
        let path = dir.join(candidate);
        if !name_taken(&path)? {
            return Ok(path);
        }
    }
    Err("no free name left".to_string())
}

fn split_name(name: &str) -> (&str, Option<&str>) {
    match name.rfind('.') {
        Some(0) | None => (name, None),
        Some(index) => (&name[..index], Some(&name[index + 1..])),
    }
}

fn copy_path(src: &Path, dest: &Path) -> Result<(), String> {
    if name_taken(dest)? {
        return Err("destination exists".to_string());
    }
    let meta = fs::symlink_metadata(src).map_err(|err| err.to_string())?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(src).map_err(|err| err.to_string())?;
        symlink(&target, dest)?;
        return Ok(());
    }
    if meta.is_dir() {
        fs::create_dir(dest).map_err(|err| err.to_string())?;
        let entries = fs::read_dir(src).map_err(|err| err.to_string())?;
        for entry in entries {
            let entry = entry.map_err(|err| err.to_string())?;
            copy_path(&entry.path(), &dest.join(entry.file_name()))?;
        }
        return Ok(());
    }
    fs::copy(src, dest).map(|_| ()).map_err(|err| err.to_string())
}

fn move_path(src: &Path, dest: &Path) -> Result<(), String> {
    if src == dest {
        return Ok(());
    }
    if name_taken(dest)? {
        return Err("destination exists".to_string());
    }
    match fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(err) if is_cross_device(&err) => {
            copy_path(src, dest)?;
            delete_path(src)
        }
        Err(err) => Err(err.to_string()),
    }
}

fn delete_path(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|err| err.to_string())?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        fs::remove_file(path).map_err(|err| err.to_string())
    } else {
        fs::remove_dir_all(path).map_err(|err| err.to_string())
    }
}

fn replace_dest(path: &Path) -> Result<(), String> {
    delete_path(path)
}

fn name_taken(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.to_string()),
    }
}

fn is_root(path: &Path) -> bool {
    if path.as_os_str().is_empty() || path.parent().is_none() || path == Path::new("/") {
        return true;
    }
    // A symlink is the link itself. Resolving it would refuse to delete a
    // link that merely points at `/`, and would also hide a real directory
    // whose path is `/tmp/..`.
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => false,
        Ok(meta) if meta.is_dir() => match path.canonicalize() {
            Ok(canon) => canon.parent().is_none() || canon == Path::new("/"),
            Err(_) => false,
        },
        _ => false,
    }
}

fn is_cross_device(err: &io::Error) -> bool {
    err.raw_os_error() == Some(18)
}

fn symlink(target: &Path, dest: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, dest).map_err(|err| err.to_string())
    }
    #[cfg(not(unix))]
    {
        let _ = (target, dest);
        Err("symlinks are not supported on this platform".to_string())
    }
}

fn display(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

fn report(notice: impl Into<String>, ok: bool, clear_sources: bool) -> OpReport {
    OpReport {
        notice: notice.into(),
        ok,
        clear_sources,
    }
}

fn finish(done: usize, verb: &str, failed: &[String], clear_sources: bool) -> OpReport {
    if failed.is_empty() {
        report(format!("{verb} {done}"), true, clear_sources)
    } else {
        report(
            format!("{verb} {done}, failed {}: {}", failed.len(), failed[0]),
            false,
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token() -> CancellationToken {
        CancellationToken::new()
    }

    #[test]
    fn rejects_empty_dot_dot_slash_and_control_names() {
        assert!(validate_entry_name("").is_err());
        assert!(validate_entry_name(".").is_err());
        assert!(validate_entry_name("..").is_err());
        assert!(validate_entry_name("a/b").is_err());
        assert!(validate_entry_name("a\\b").is_err());
        assert!(validate_entry_name("a\nb").is_err());
        assert!(validate_entry_name("a\0b").is_err());
        assert!(validate_entry_name("hello world").is_ok());
        assert!(validate_entry_name("héllo").is_ok());
    }

    #[test]
    fn paste_without_overwrite_keeps_the_existing_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("src");
        let dest = dir.path().join("dest");
        fs::create_dir(&src_dir).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(src_dir.join("a.txt"), b"new").unwrap();
        fs::write(dest.join("a.txt"), b"old").unwrap();

        let report = paste(&[src_dir.join("a.txt")], &dest, false, false, &token());

        assert!(report.ok, "{}", report.notice);
        assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"old");
        assert_eq!(fs::read(dest.join("a_1.txt")).unwrap(), b"new");
        assert!(!report.clear_sources);
    }

    #[test]
    fn a_dangling_symlink_still_occupies_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("src");
        let dest = dir.path().join("dest");
        fs::create_dir(&src_dir).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(src_dir.join("a.txt"), b"new").unwrap();
        symlink(Path::new("/no/such/target"), &dest.join("a.txt")).unwrap();

        let report = paste(&[src_dir.join("a.txt")], &dest, false, false, &token());

        assert!(report.ok, "{}", report.notice);
        assert!(fs::symlink_metadata(dest.join("a.txt")).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(dest.join("a_1.txt")).unwrap(), b"new");
    }

    #[test]
    fn overwrite_replaces_a_symlink_and_leaves_the_target_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from");
        let to = dir.path().join("to");
        fs::create_dir(&from).unwrap();
        fs::create_dir(&to).unwrap();
        let outside = dir.path().join("outside.txt");
        fs::write(&outside, b"safe").unwrap();
        fs::write(from.join("link"), b"new").unwrap();
        symlink(&outside, &to.join("link")).unwrap();

        let report = paste(&[from.join("link")], &to, false, true, &token());

        assert!(report.ok, "{}", report.notice);
        assert_eq!(fs::read(&outside).unwrap(), b"safe");
        assert_eq!(fs::read(to.join("link")).unwrap(), b"new");
        assert!(!fs::symlink_metadata(to.join("link")).unwrap().file_type().is_symlink());
    }

    #[test]
    fn copying_a_directory_into_itself_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("a.txt"), b"a").unwrap();
        let nested = src.join("nested");
        fs::create_dir(&nested).unwrap();

        let report = paste(std::slice::from_ref(&src), &nested, false, false, &token());

        assert!(!report.ok, "{}", report.notice);
        assert!(report.notice.contains("itself"));
        assert_eq!(fs::read(src.join("a.txt")).unwrap(), b"a");
        assert!(!nested.join("src").exists());
    }

    #[test]
    fn copy_duplicates_a_directory_without_following_an_inner_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        fs::create_dir(&src).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(src.join("a.txt"), b"a").unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.txt"), b"keep").unwrap();
        symlink(&outside, &src.join("link")).unwrap();

        let report = paste(std::slice::from_ref(&src), &dest, false, false, &token());

        assert!(report.ok, "{}", report.notice);
        assert_eq!(fs::read(dest.join("src").join("a.txt")).unwrap(), b"a");
        assert!(fs::symlink_metadata(dest.join("src").join("link")).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(outside.join("keep.txt")).unwrap(), b"keep");
    }

    #[test]
    fn cut_moves_the_file_and_asks_to_clear_only_on_full_success() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        fs::create_dir(&src).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(src.join("a.txt"), b"a").unwrap();

        let report = paste(&[src.join("a.txt")], &dest, true, false, &token());

        assert!(report.ok && report.clear_sources, "{}", report.notice);
        assert!(!src.join("a.txt").exists());
        assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"a");
    }

    #[test]
    fn a_missing_source_does_not_fail_the_files_that_copied() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        fs::create_dir(&src).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(src.join("a.txt"), b"a").unwrap();

        let report = paste(&[src.join("a.txt"), src.join("missing.txt")], &dest, true, false, &token());

        assert!(!report.ok);
        assert!(!report.clear_sources);
        assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"a");
        assert!(report.notice.contains("failed"));
    }

    #[test]
    fn cutting_a_file_onto_itself_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, b"same").unwrap();

        let report = paste(std::slice::from_ref(&file), dir.path(), true, true, &token());

        assert!(report.ok, "{}", report.notice);
        assert_eq!(fs::read(&file).unwrap(), b"same");
    }

    #[test]
    fn copying_a_file_in_place_makes_a_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, b"same").unwrap();

        let report = paste(std::slice::from_ref(&file), dir.path(), false, false, &token());

        assert!(report.ok, "{}", report.notice);
        assert_eq!(fs::read(&file).unwrap(), b"same");
        assert_eq!(fs::read(dir.path().join("a_1.txt")).unwrap(), b"same");
    }

    #[test]
    fn delete_removes_a_symlink_and_not_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep.txt"), b"keep").unwrap();
        let link = dir.path().join("linkdir");
        symlink(&target, &link).unwrap();

        let report = delete_paths(std::slice::from_ref(&link), &token());

        assert!(report.ok, "{}", report.notice);
        assert!(fs::symlink_metadata(&link).is_err());
        assert_eq!(fs::read(target.join("keep.txt")).unwrap(), b"keep");
    }

    #[test]
    fn delete_removes_a_directory_without_following_an_inner_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("folder");
        fs::create_dir(&folder).unwrap();
        let outside = dir.path().join("outside.txt");
        fs::write(&outside, b"keep").unwrap();
        symlink(&outside, &folder.join("link")).unwrap();

        let report = delete_paths(std::slice::from_ref(&folder), &token());

        assert!(report.ok, "{}", report.notice);
        assert!(!folder.exists());
        assert_eq!(fs::read(&outside).unwrap(), b"keep");
    }

    #[test]
    fn delete_refuses_the_filesystem_root() {
        let report = delete_paths(&[PathBuf::from("/")], &token());
        assert!(!report.ok);
        assert!(report.notice.contains("root"));
        assert!(Path::new("/tmp").is_dir());
    }

    #[test]
    fn a_directory_whose_path_resolves_to_root_is_root() {
        assert!(is_root(Path::new("/")));
        assert!(is_root(Path::new("/.")));
        assert!(is_root(Path::new("/tmp/..")));
        assert!(!is_root(Path::new("/tmp")));
    }

    #[test]
    fn delete_removes_a_symlink_that_points_at_root() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("rootlink");
        std::os::unix::fs::symlink("/", &link).unwrap();

        let report = delete_paths(std::slice::from_ref(&link), &token());

        assert!(report.ok, "{}", report.notice);
        assert!(link.symlink_metadata().is_err());
        assert!(Path::new("/usr").is_dir());
    }

    #[test]
    fn an_already_cancelled_paste_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        let dest = dir.path().join("dest");
        fs::create_dir(&dest).unwrap();
        fs::write(&src, b"a").unwrap();
        let cancel = token();
        cancel.cancel();

        let report = paste(&[src], &dest, false, false, &cancel);

        assert!(!report.ok);
        assert!(fs::read_dir(&dest).unwrap().next().is_none());
    }

    #[test]
    fn rename_refuses_to_overwrite_and_renames_a_symlink_not_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, b"a").unwrap();
        fs::write(dir.path().join("taken"), b"taken").unwrap();

        assert!(rename_entry(&file, "taken").is_err());
        assert_eq!(fs::read(&file).unwrap(), b"a");
        assert_eq!(fs::read(dir.path().join("taken")).unwrap(), b"taken");

        let target = dir.path().join("target.txt");
        fs::write(&target, b"body").unwrap();
        let link = dir.path().join("link");
        symlink(&target, &link).unwrap();
        rename_entry(&link, "renamed").unwrap();
        assert!(!link.exists());
        assert!(fs::symlink_metadata(dir.path().join("renamed")).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&target).unwrap(), b"body");
    }

    #[test]
    fn rename_to_the_same_name_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, b"a").unwrap();
        assert!(rename_entry(&file, "a.txt").is_ok());
        assert_eq!(fs::read(&file).unwrap(), b"a");
    }

    #[test]
    fn mkdir_and_touch_do_not_clobber_or_create_parents() {
        let dir = tempfile::tempdir().unwrap();
        let created = mkdir_entry(dir.path(), "sub").unwrap();
        assert!(created.is_dir());
        assert!(mkdir_entry(dir.path(), "sub").is_err());
        assert!(mkdir_entry(dir.path(), "a/b").is_err());

        let file = touch_entry(dir.path(), "note").unwrap();
        fs::write(&file, b"keep").unwrap();
        assert!(touch_entry(dir.path(), "note").is_err());
        assert_eq!(fs::read(&file).unwrap(), b"keep");
    }
}
