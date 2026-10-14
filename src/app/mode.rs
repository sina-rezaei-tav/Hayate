//! Which keymap is live. Browser keys never run while a prompt is up,
//! so `q` can type a `q` instead of quitting.

use super::open_with::OpenWithPrompt;
use super::prompt::{ConfirmPrompt, HelpView, LinePrompt, OneKey, PagerView};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionMode {
    Browser,
    OpenWith(OpenWithPrompt),
    Line(LinePrompt),
    Confirm(ConfirmPrompt),
    Help(HelpView),
    Pager(PagerView),
    OneKey(OneKey),
}
