//! What `l` / Enter does with a file until a real rifle config exists.
//!
//! Known text extensions open in `$EDITOR` even when `xdg-open` is on
//! PATH, so a Rust file is not handed to a browser. Everything else uses
//! `xdg-open` when it exists, and the editor otherwise. `E` does not call
//! this; it always edits.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenChoice {
    Editor,
    External(&'static str),
}

const TEXT_EXT: &[&str] = &[
    "txt", "md", "rs", "toml", "json", "csv", "log", "conf", "yml", "yaml", "sh", "py", "c", "h", "cpp", "hpp", "html", "css", "js", "ts", "ini", "xml", "lock",
];

pub fn choose(name: &str, xdg_open: bool) -> OpenChoice {
    let ext = extension(name);
    if TEXT_EXT.iter().any(|candidate| candidate.eq_ignore_ascii_case(ext)) {
        return OpenChoice::Editor;
    }
    if xdg_open {
        OpenChoice::External("xdg-open")
    } else {
        OpenChoice::Editor
    }
}

pub fn extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(0) | None => "",
        Some(index) => &name[index + 1..],
    }
}

pub fn xdg_open_available() -> bool {
    super::editor::first_on_path(&["xdg-open"], std::env::var_os("PATH").as_deref()).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_files_stay_in_the_editor_even_when_xdg_open_exists() {
        assert_eq!(choose("main.rs", true), OpenChoice::Editor);
        assert_eq!(choose("README.MD", true), OpenChoice::Editor);
        assert_eq!(choose("notes.txt", false), OpenChoice::Editor);
    }

    #[test]
    fn other_files_use_xdg_open_only_when_it_exists() {
        assert_eq!(choose("photo.png", true), OpenChoice::External("xdg-open"));
        assert_eq!(choose("photo.png", false), OpenChoice::Editor);
        assert_eq!(choose("Makefile", true), OpenChoice::External("xdg-open"));
        assert_eq!(choose("Makefile", false), OpenChoice::Editor);
    }

    #[test]
    fn a_leading_dotfile_is_not_treated_as_an_extension() {
        assert_eq!(extension(".gitignore"), "");
        assert_eq!(choose(".gitignore", true), OpenChoice::External("xdg-open"));
    }
}
