//! Message-window handlers, split by the kind of message they answer.

pub(crate) mod commands;
pub(crate) mod drag_preview;
pub(crate) mod ipc;
pub(crate) mod keyboard;
pub(crate) mod session;
pub(crate) mod shell;
pub(crate) mod winevents;
