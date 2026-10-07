//! Calendar dates from Unix time, without a date library.

/// `[year, month, day, hour, minute, second]` in UTC of `seconds` since 1970.
pub fn utc(seconds: i64) -> [i64; 6] {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    [year, month, day, rest / 3600, rest % 3600 / 60, rest % 60]
}

/// Days since 1970 of a UTC calendar date (days-from-civil, Howard Hinnant).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The ISO 8601 week of `seconds` since 1970, in UTC, e.g. "2026-W40".
pub fn iso_week(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    // 1970-01-01 was a Thursday; Monday is 0.
    let weekday = (days + 3).rem_euclid(7);
    // The Thursday of this week decides its year.
    let thursday = days - weekday + 3;
    let year = utc(thursday * 86_400)[0];
    let week = (thursday - days_from_civil(year, 1, 1)) / 7 + 1;
    format!("{year}-W{week:02}")
}

/// Seconds since 1970 now.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Now in UTC as XMP writes it, e.g. "2026-09-27T06:12:22Z".
pub fn now_xmp() -> String {
    let [y, mo, d, h, mi, s] = utc(now());
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// `seconds` since 1970 in UTC as catalogs store times, e.g.
/// "2026-09-27 06:12:22" (SQL's `CURRENT_TIMESTAMP` form).
pub fn utc_text(seconds: i64) -> String {
    let [y, mo, d, h, mi, s] = utc(seconds);
    // Years before 1 as SQL writes them: "-0001".
    let sign = if y < 0 { "-" } else { "" };
    let y = y.abs();
    format!("{sign}{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// Now in UTC as catalogs store times (see [`utc_text`]).
pub fn now_text() -> String {
    utc_text(now())
}

#[cfg(test)]
mod tests {
    #[test]
    fn converts_unix_seconds_to_utc() {
        assert_eq!(super::utc(1_469_686_444), [2016, 7, 28, 6, 14, 4]);
        assert_eq!(super::utc(0), [1970, 1, 1, 0, 0, 0]);
        let now = super::now_xmp();
        assert!(now.len() == 20 && now.ends_with('Z'));
        assert_eq!(super::utc_text(1_469_686_444), "2016-07-28 06:14:04");
        assert_eq!(super::now_text().len(), 19);
    }

    #[test]
    fn finds_iso_weeks() {
        let week = |date: [i64; 3]| {
            super::iso_week(super::days_from_civil(date[0], date[1], date[2]) * 86_400)
        };
        assert_eq!(week([2026, 10, 1]), "2026-W40");
        // Monday starts the week; Sunday ends it.
        assert_eq!(week([2026, 9, 28]), "2026-W40");
        assert_eq!(week([2026, 10, 4]), "2026-W40");
        assert_eq!(week([2026, 10, 5]), "2026-W41");
        // First and last days belonging to a neighbouring year's week.
        assert_eq!(week([2024, 12, 30]), "2025-W01");
        assert_eq!(week([2027, 1, 1]), "2026-W53");
        assert_eq!(week([2021, 1, 3]), "2020-W53");
        assert_eq!(week([2026, 1, 1]), "2026-W01");
        assert_eq!(super::iso_week(0), "1970-W01");
        assert_eq!(super::days_from_civil(1970, 1, 1), 0);
    }
}
