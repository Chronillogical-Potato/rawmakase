//! Native menu transport. The editor owns command availability and execution;
//! AppKit only sends typed choices and wakes the event loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Preferences,
    NewCatalog,
    OpenCatalog,
    AddFolder,
    Export,
    Close,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Library,
    Develop,
    Sidebar,
    Inspector,
    Filmstrip,
    Fullscreen,
    Shortcuts,
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::{clear, install, take_actions, update};
