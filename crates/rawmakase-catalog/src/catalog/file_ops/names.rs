//! Which names a file or folder may be given: the ones every system the
//! catalog may be used on can hold, so a catalog stays portable. One rule
//! for folders made and files and folders renamed.

/// Why a name can't be a file's or folder's on some system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NameProblem {
    /// It is empty, or only spaces.
    Empty,
    /// `.` or `..`.
    Dots,
    /// It has `/` or `\` in it.
    Separator,
    /// It has a control character in it.
    Control,
    /// It has a character Windows refuses: `: * ? " < > |`.
    Windows,
    /// It ends in a dot or a space, which Windows drops.
    Trailing,
    /// It is a name Windows keeps for a device (CON, NUL, COM1…), whatever
    /// follows a dot.
    Device,
    /// It is over 255 bytes.
    Long,
}

/// Why `name` can't be a file's or folder's name; `None` when it can.
pub(super) fn problem(name: &str) -> Option<NameProblem> {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.split('.').next().unwrap_or_default().trim_end();
    if name.trim().is_empty() {
        Some(NameProblem::Empty)
    } else if name == "." || name == ".." {
        Some(NameProblem::Dots)
    } else if name.contains(['/', '\\']) {
        Some(NameProblem::Separator)
    } else if name.chars().any(char::is_control) {
        Some(NameProblem::Control)
    } else if name.contains([':', '*', '?', '"', '<', '>', '|']) {
        Some(NameProblem::Windows)
    } else if name.ends_with(['.', ' ']) {
        Some(NameProblem::Trailing)
    } else if RESERVED.iter().any(|r| stem.eq_ignore_ascii_case(r)) {
        Some(NameProblem::Device)
    } else if name.len() > 255 {
        Some(NameProblem::Long)
    } else {
        None
    }
}
