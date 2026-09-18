//! Terminal lifecycle: enters/leaves raw mode, the alternate screen, and
//! mouse capture, and guarantees all three are reverted even on panic.

use std::io::{self, Stdout, Write};
use std::ops::{Deref, DerefMut};
use std::sync::Once;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

type Backend = CrosstermBackend<Stdout>;

static PANIC_HOOK: Once = Once::new();

/// Owns the terminal for the app's lifetime. Derefs to the wrapped
/// `ratatui::Terminal` so callers can draw directly through it.
pub struct Tui {
    terminal: Terminal<Backend>,
    restored: bool,
}

impl Tui {
    pub fn init() -> anyhow::Result<Self> {
        CrosstermRawMode.enable()?;
        enter_screen(&mut io::stdout())?;
        install_panic_hook();

        let terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        Ok(Self {
            terminal,
            restored: false,
        })
    }

    /// Reverts raw mode / alternate screen / mouse capture. Safe to call
    /// more than once; only the first call has any effect.
    pub fn restore(&mut self) -> anyhow::Result<()> {
        restore_once(&CrosstermRawMode, &mut io::stdout(), &mut self.restored)
    }
}

impl Deref for Tui {
    type Target = Terminal<Backend>;

    fn deref(&self) -> &Self::Target {
        &self.terminal
    }
}

impl DerefMut for Tui {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.terminal
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// Abstracts raw-mode enable/disable behind a trait so the idempotency logic
/// in [`restore_once`] can be unit tested without a real TTY, which
/// `enable_raw_mode`/`disable_raw_mode` otherwise require.
trait RawMode {
    fn enable(&self) -> io::Result<()>;
    fn disable(&self) -> io::Result<()>;
}

struct CrosstermRawMode;

impl RawMode for CrosstermRawMode {
    fn enable(&self) -> io::Result<()> {
        enable_raw_mode()
    }

    fn disable(&self) -> io::Result<()> {
        disable_raw_mode()
    }
}

/// Disables raw mode exactly once, flipping `restored` so repeat calls (from
/// an explicit `restore()` followed by `Drop`, for example) are no-ops.
///
/// Generic over both the raw-mode disabler and the writer so this can be
/// exercised in tests without touching the real terminal or process stdout.
fn restore_once<R: RawMode, W: Write>(
    raw_mode: &R,
    writer: &mut W,
    restored: &mut bool,
) -> anyhow::Result<()> {
    if *restored {
        return Ok(());
    }
    leave_screen(writer)?;
    raw_mode.disable()?;
    *restored = true;
    Ok(())
}

/// Split from raw-mode toggling (which needs a real TTY) so the exact byte
/// sequence is unit-testable against an in-memory buffer.
fn enter_screen<W: Write>(writer: &mut W) -> io::Result<()> {
    execute!(writer, EnterAlternateScreen, EnableMouseCapture)
}

/// Reverse of [`enter_screen`].
fn leave_screen<W: Write>(writer: &mut W) -> io::Result<()> {
    execute!(writer, DisableMouseCapture, LeaveAlternateScreen)
}

fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            let _ = leave_screen(&mut io::stdout());
            let _ = disable_raw_mode();
            default_hook(panic_info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use crossterm::Command;

    use super::*;

    fn ansi_of(command: impl Command) -> String {
        let mut out = String::new();
        command.write_ansi(&mut out).unwrap();
        out
    }

    #[test]
    fn enter_screen_enters_alt_screen_then_mouse_capture() {
        let mut buf = Vec::new();
        enter_screen(&mut buf).unwrap();

        let expected = format!(
            "{}{}",
            ansi_of(EnterAlternateScreen),
            ansi_of(EnableMouseCapture)
        );
        assert_eq!(String::from_utf8(buf).unwrap(), expected);
    }

    #[test]
    fn leave_screen_reverses_enter_screen_order() {
        let mut buf = Vec::new();
        leave_screen(&mut buf).unwrap();

        let expected = format!(
            "{}{}",
            ansi_of(DisableMouseCapture),
            ansi_of(LeaveAlternateScreen)
        );
        assert_eq!(String::from_utf8(buf).unwrap(), expected);
    }

    #[derive(Default)]
    struct CountingRawMode {
        disables: Cell<u32>,
    }

    impl RawMode for CountingRawMode {
        fn enable(&self) -> io::Result<()> {
            Ok(())
        }

        fn disable(&self) -> io::Result<()> {
            self.disables.set(self.disables.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn restore_once_disables_raw_mode_on_first_call_only() {
        let raw_mode = CountingRawMode::default();
        let mut restored = false;
        let mut buf = Vec::new();

        restore_once(&raw_mode, &mut buf, &mut restored).unwrap();
        restore_once(&raw_mode, &mut buf, &mut restored).unwrap();
        restore_once(&raw_mode, &mut buf, &mut restored).unwrap();

        assert_eq!(raw_mode.disables.get(), 1);
        assert!(restored);
        // The screen-leave sequence itself should also only be written once.
        let expected = format!(
            "{}{}",
            ansi_of(DisableMouseCapture),
            ansi_of(LeaveAlternateScreen)
        );
        assert_eq!(String::from_utf8(buf).unwrap(), expected);
    }
}
