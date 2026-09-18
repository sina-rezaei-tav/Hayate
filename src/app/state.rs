use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    pub should_quit: bool,
    pub current_dir: PathBuf,
}

impl AppState {
    pub fn new(current_dir: PathBuf) -> Self {
        Self {
            should_quit: false,
            current_dir,
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        // Falls back to "." rather than unwrapping, since the cwd can be
        // unreadable (e.g. removed out from under the process).
        Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_not_quitting_with_the_given_dir() {
        let state = AppState::new(PathBuf::from("/tmp"));
        assert!(!state.should_quit);
        assert_eq!(state.current_dir, PathBuf::from("/tmp"));
    }

    #[test]
    fn default_starts_not_quitting() {
        assert!(!AppState::default().should_quit);
    }
}
