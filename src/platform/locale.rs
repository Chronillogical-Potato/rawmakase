//! The system's number format, for what Lightroom writes in it: the decimal
//! separator of the user's region on macOS and Windows. Elsewhere the
//! environment's locale names are not separators, and reading them would
//! need the process-wide `setlocale`, so names keep Lightroom's default `.`.

/// The decimal separator of the user's region; `.` when it can't be read.
pub(crate) fn decimal_separator() -> char {
    single(system()).unwrap_or('.')
}

/// The separator when it is one character.
fn single(separator: Option<String>) -> Option<char> {
    let separator = separator?;
    let mut chars = separator.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

#[cfg(target_os = "macos")]
fn system() -> Option<String> {
    let locale = objc2_foundation::NSLocale::currentLocale();
    Some(locale.decimalSeparator().to_string())
}

#[cfg(windows)]
fn system() -> Option<String> {
    use windows_sys::Win32::Globalization::{GetLocaleInfoEx, LOCALE_SDECIMAL};
    let mut buffer = [0u16; 8];
    // SAFETY: a null name is the user's default locale; the buffer's length
    // is given in characters, as the call counts them.
    let written = unsafe {
        GetLocaleInfoEx(
            std::ptr::null(),
            LOCALE_SDECIMAL,
            buffer.as_mut_ptr(),
            buffer.len() as i32,
        )
    };
    // The count includes the terminating NUL.
    let length = usize::try_from(written).ok()?.checked_sub(1)?;
    String::from_utf16(buffer.get(..length)?).ok()
}

#[cfg(not(any(target_os = "macos", windows)))]
fn system() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_separator_is_one_character_or_the_default() {
        assert_eq!(single(Some(",".into())), Some(','));
        assert_eq!(single(Some("".into())), None);
        assert_eq!(single(Some("٫x".into())), None);
        assert_eq!(single(None), None);
        // Whatever this system's region says, it is one character.
        let _ = decimal_separator();
    }
}
