//! When a photo was taken, as its RAW file says, read from the few parts of
//! the file that hold it, for the containers whose dates are not in a TIFF
//! at the file's start: Canon's CR3 (`CMT1` and `CMT2`) and CRW (CIFF's
//! TimeStamp record), Minolta's MRW (the TIFF in its TTW block) and Sigma's
//! X3F (its preview's EXIF, else its TIME property).
use crate::container::{ciff, cr3, mrw, x3f};
use crate::io::{Pieces, ReadSeek};

/// A date as EXIF writes it, "2018:08:26 10:39:33", and its subsecond
/// digits ("12" for .12 s), each where the file has it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stamp {
    pub date: Option<String>,
    pub subseconds: Option<String>,
}

/// A RAW file's capture dates: EXIF's DateTimeOriginal, DateTimeDigitized
/// and DateTime with their subseconds. A container's own capture time
/// (CIFF's TimeStamp, X3F's TIME) is the original.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaptureDates {
    pub original: Stamp,
    pub digitized: Stamp,
    pub modified: Stamp,
}

/// The capture dates of a CR3, CRW, MRW or X3F; none for another file or
/// one that can't be read.
pub(crate) fn read(file: &mut dyn ReadSeek) -> CaptureDates {
    let Some(mut f) = Pieces::new(file) else {
        return CaptureDates::default();
    };
    let head = f.read_upto(0, 16).unwrap_or_default();
    if cr3::is_cr3(&head) {
        cr3::capture_dates(&mut f)
    } else if ciff::is_ciff(&head) {
        ciff::capture_dates(&mut f)
    } else if mrw::is_mrw(&head) {
        mrw::capture_dates(&mut f)
    } else if x3f::is_x3f(&head) {
        x3f::capture_dates(&mut f)
    } else {
        CaptureDates::default()
    }
}

/// `seconds` since 1970 on a clock with no zone (the camera's) as EXIF
/// writes a date; `None` for 0, which cameras write for no date, and past
/// the year 9999.
pub(crate) fn exif_date(seconds: u64) -> Option<String> {
    if seconds == 0 || seconds >= 253_402_300_800 {
        return None;
    }
    let (days, time) = ((seconds / 86_400) as i64, seconds % 86_400);
    // The proleptic Gregorian calendar's day count from 1 March of year 0
    // (Howard Hinnant's civil_from_days).
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    Some(format!(
        "{year:04}:{month:02}:{day:02} {:02}:{:02}:{:02}",
        time / 3600,
        time / 60 % 60,
        time % 60
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_since_1970_are_dates_on_the_cameras_clock() {
        assert_eq!(exif_date(0), None);
        assert_eq!(exif_date(1).as_deref(), Some("1970:01:01 00:00:01"));
        // A leap day, the turn of a century and of a year.
        assert_eq!(
            exif_date(951_782_400).as_deref(),
            Some("2000:02:29 00:00:00")
        );
        assert_eq!(
            exif_date(1_083_408_238).as_deref(),
            Some("2004:05:01 10:43:58")
        );
        assert_eq!(
            exif_date(1_704_067_199).as_deref(),
            Some("2023:12:31 23:59:59")
        );
        assert_eq!(
            exif_date(u64::from(u32::MAX)).as_deref(),
            Some("2106:02:07 06:28:15")
        );
        assert_eq!(
            exif_date(253_402_300_799).as_deref(),
            Some("9999:12:31 23:59:59")
        );
        assert_eq!(exif_date(253_402_300_800), None);
    }

    #[test]
    fn other_files_have_no_dates() {
        for file in [&b""[..], b"II*\0\x08\0\0\0", &[0xff, 0xd8, 0xff, 0xd9]] {
            assert_eq!(
                read(&mut std::io::Cursor::new(file)),
                CaptureDates::default()
            );
        }
    }
}
