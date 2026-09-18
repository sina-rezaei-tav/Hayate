//! Resolves `$EDITOR` / `$VISUAL` and builds the argv used to open a file.
//!
//! Spawning the process is a blocking TTY hand-off (the file manager leaves
//! raw mode first). This module stays free of Ratatui so the lookup and argv
//! can be unit-tested without a terminal.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;
use std::process::{Command, ExitStatus};

/// Tried in order when neither `EDITOR` nor `VISUAL` is set. Same idea as
/// ranger defaulting to vim: most machines have one of these even when the
/// env vars were never exported.
const PATH_FALLBACKS: &[&str] = &["nvim", "vim", "vi", "nano"];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EditorError {
    #[error("no editor found (set EDITOR, or install nvim/vim/vi/nano)")]
    NotFound,
    #[error("command is empty")]
    Empty,
}

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error(transparent)]
    Resolve(#[from] EditorError),
    #[error("failed to run {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: io::Error,
    },
}

/// Program + args ready to `Command::new(program).args(args)`.
/// The file path is always the last argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorLaunch {
    pub program: OsString,
    pub args: Vec<OsString>,
}

/// `EDITOR` wins over `VISUAL`, then a PATH fallback (`nvim`/`vim`/`vi`/`nano`).
/// Env values are trimmed; all-whitespace is treated as unset for that variable.
pub fn resolve_editor(
    editor: Option<&str>,
    visual: Option<&str>,
    fallback: Option<&str>,
) -> Result<String, EditorError> {
    pick_var(editor)
        .or_else(|| pick_var(visual))
        .or_else(|| pick_var(fallback))
        .ok_or(EditorError::NotFound)
}

fn pick_var(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// First of `names` that is an executable file somewhere on `path_var`.
/// Searches names in order (so `nvim` beats `vim` even if `vim` appears in
/// an earlier PATH directory).
pub fn first_on_path<'a>(names: &[&'a str], path_var: Option<&OsStr>) -> Option<&'a str> {
    let path_var = path_var?;
    let dirs: Vec<_> = std::env::split_paths(path_var).collect();
    for name in names {
        for dir in &dirs {
            if is_runnable(&dir.join(name)) {
                return Some(*name);
            }
        }
    }
    None
}

pub(crate) fn is_runnable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_file() => meta.permissions().mode() & 0o111 != 0,
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        path.is_file() || path.with_extension("exe").is_file()
    }
}

/// Splits `editor` on whitespace so values like `code --wait` work, then
/// appends `path`. Does not invoke a shell, so the path is never interpolated.
pub fn launch_spec(editor: &str, path: &Path) -> Result<EditorLaunch, EditorError> {
    let mut parts = editor.split_whitespace();
    let program = parts.next().ok_or(EditorError::Empty)?;
    let mut args: Vec<OsString> = parts.map(OsString::from).collect();
    args.push(path.as_os_str().to_os_string());
    Ok(EditorLaunch {
        program: OsString::from(program),
        args,
    })
}

pub fn launch_spec_from_env(path: &Path) -> Result<EditorLaunch, EditorError> {
    let editor = std::env::var("EDITOR").ok();
    let visual = std::env::var("VISUAL").ok();
    let fallback = first_on_path(PATH_FALLBACKS, std::env::var_os("PATH").as_deref());
    let command = resolve_editor(editor.as_deref(), visual.as_deref(), fallback)?;
    launch_spec(&command, path)
}

/// Blocking spawn. Caller must have already left raw mode and released the
/// input task so the child owns the TTY.
pub fn run_spec(spec: &EditorLaunch) -> Result<ExitStatus, LaunchError> {
    Command::new(&spec.program)
        .args(&spec.args)
        .status()
        .map_err(|source| LaunchError::Spawn {
            program: spec.program.to_string_lossy().into_owned(),
            source,
        })
}

pub fn run(path: &Path) -> Result<ExitStatus, LaunchError> {
    let spec = launch_spec_from_env(path)?;
    run_spec(&spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_wins_over_visual_and_fallback() {
        assert_eq!(
            resolve_editor(Some("vim"), Some("nano"), Some("vi")).unwrap(),
            "vim"
        );
    }

    #[test]
    fn visual_is_used_when_editor_is_unset() {
        assert_eq!(
            resolve_editor(None, Some("nano"), Some("vim")).unwrap(),
            "nano"
        );
    }

    #[test]
    fn blank_editor_falls_back_to_visual() {
        assert_eq!(
            resolve_editor(Some("  "), Some("nano"), Some("vim")).unwrap(),
            "nano"
        );
    }

    #[test]
    fn path_fallback_is_used_when_env_is_unset() {
        assert_eq!(resolve_editor(None, None, Some("vim")).unwrap(), "vim");
    }

    #[test]
    fn path_fallback_is_used_when_env_is_blank() {
        assert_eq!(
            resolve_editor(Some("  "), Some("\t"), Some("nvim")).unwrap(),
            "nvim"
        );
    }

    #[test]
    fn missing_env_and_fallback_is_not_found() {
        assert_eq!(
            resolve_editor(None, None, None),
            Err(EditorError::NotFound)
        );
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(
            resolve_editor(Some("  nvim  "), None, None).unwrap(),
            "nvim"
        );
    }

    #[test]
    fn launch_spec_appends_the_path_as_the_last_arg() {
        let spec = launch_spec("vim", Path::new("/tmp/notes.txt")).unwrap();
        assert_eq!(spec.program, OsString::from("vim"));
        assert_eq!(spec.args, vec![OsString::from("/tmp/notes.txt")]);
    }

    #[test]
    fn launch_spec_keeps_editor_flags() {
        let spec = launch_spec("code --wait", Path::new("/tmp/a.rs")).unwrap();
        assert_eq!(spec.program, OsString::from("code"));
        assert_eq!(
            spec.args,
            vec![OsString::from("--wait"), OsString::from("/tmp/a.rs")]
        );
    }

    #[test]
    fn launch_spec_does_not_split_the_file_path() {
        let spec = launch_spec("vim", Path::new("/tmp/my notes.txt")).unwrap();
        assert_eq!(spec.args, vec![OsString::from("/tmp/my notes.txt")]);
    }

    #[test]
    fn launch_spec_rejects_an_empty_program() {
        assert_eq!(launch_spec("", Path::new("/tmp/a.txt")), Err(EditorError::Empty));
        assert_eq!(launch_spec("   ", Path::new("/tmp/a.txt")), Err(EditorError::Empty));
    }

    #[cfg(unix)]
    fn make_executable(dir: &Path, name: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn first_on_path_prefers_nvim_over_vim_regardless_of_directory_order() {
        let early = tempfile::tempdir().unwrap();
        let late = tempfile::tempdir().unwrap();
        make_executable(early.path(), "vim");
        make_executable(late.path(), "nvim");

        let path = std::env::join_paths([early.path(), late.path()]).unwrap();
        assert_eq!(
            first_on_path(&["nvim", "vim", "vi", "nano"], Some(path.as_os_str())),
            Some("nvim"),
            "nvim must win even when vim sits in an earlier PATH directory"
        );
    }

    #[cfg(unix)]
    #[test]
    fn first_on_path_skips_a_non_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("vim"), b"not executable").unwrap();
        make_executable(dir.path(), "vi");

        let path = std::env::join_paths([dir.path()]).unwrap();
        assert_eq!(
            first_on_path(&["nvim", "vim", "vi"], Some(path.as_os_str())),
            Some("vi")
        );
    }

    #[cfg(unix)]
    #[test]
    fn first_on_path_returns_none_when_nothing_matches() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::env::join_paths([dir.path()]).unwrap();
        assert_eq!(
            first_on_path(&["nvim", "vim", "vi", "nano"], Some(path.as_os_str())),
            None
        );
    }

    #[test]
    fn first_on_path_returns_none_without_a_path() {
        assert_eq!(first_on_path(&["vim"], None), None);
    }
}
