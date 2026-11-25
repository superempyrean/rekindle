//! `host.format-time` / `format-duration` / `snowflake-ms` defaults: UTC and
//! en-US. A real client formats in the user's own locale and timezone
//! (`ClientBackend::format_time`); plugins never learn which.

use crate::bindings::wit::host::TimeStyle;

/// Discord's epoch (2015-01-01T00:00:00Z) in Unix ms.
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

pub(super) fn snowflake_ms(id: u64) -> u64 {
    (id >> 22) + DISCORD_EPOCH_MS
}

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = [
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
];

pub(super) fn format(epoch_ms: u64, style: TimeStyle, now: u64) -> String {
    let secs = (epoch_ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, m, d) = civil(days);
    let month = MONTHS[(m - 1) as usize];
    let weekday = WEEKDAYS[days.rem_euclid(7) as usize];
    match style {
        TimeStyle::ShortTime => format!("{h:02}:{mi:02}"),
        TimeStyle::LongTime => format!("{h:02}:{mi:02}:{s:02}"),
        TimeStyle::ShortDate => format!("{m:02}/{d:02}/{y}"),
        TimeStyle::LongDate => format!("{month} {d}, {y}"),
        TimeStyle::ShortDateTime => format!("{month} {d}, {y} {h:02}:{mi:02}"),
        TimeStyle::LongDateTime => format!("{weekday}, {month} {d}, {y} {h:02}:{mi:02}"),
        TimeStyle::Relative => relative(epoch_ms as i64 - now as i64),
    }
}

fn relative(delta_ms: i64) -> String {
    let future = delta_ms > 0;
    let s = delta_ms.unsigned_abs() / 1000;
    let (n, unit) = match s {
        0..=44 => {
            return if future {
                "in a few seconds".into()
            } else {
                "a few seconds ago".into()
            };
        }
        45..=2_699 => ((s + 30) / 60, "minute"),
        2_700..=79_199 => ((s + 1800) / 3600, "hour"),
        79_200..=2_246_399 => ((s + 43_200) / 86_400, "day"),
        2_246_400..=27_647_999 => ((s + 1_296_000) / 2_592_000, "month"),
        _ => ((s + 15_768_000) / 31_536_000, "year"),
    };
    let n = n.max(1);
    let unit = if n == 1 {
        unit.to_string()
    } else {
        format!("{unit}s")
    };
    let amount = if n == 1 {
        if unit.starts_with('h') {
            "an".to_string()
        } else {
            "a".to_string()
        }
    } else {
        n.to_string()
    };
    if future {
        format!("in {amount} {unit}")
    } else {
        format!("{amount} {unit} ago")
    }
}

/// "0:42", "1:02:03", "3 days".
pub(super) fn duration(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 2 * 86_400 {
        return format!("{} days", s / 86_400);
    }
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_utc() {
        // 2026-11-07T14:02:31Z, a Saturday.
        let t = 1_794_060_151_000;
        assert_eq!(format(t, TimeStyle::ShortTime, t), "14:02");
        assert_eq!(format(t, TimeStyle::LongTime, t), "14:02:31");
        assert_eq!(format(t, TimeStyle::ShortDate, t), "11/07/2026");
        assert_eq!(format(t, TimeStyle::LongDate, t), "November 7, 2026");
        assert_eq!(
            format(t, TimeStyle::LongDateTime, t),
            "Saturday, November 7, 2026 14:02"
        );
        assert_eq!(
            format(t - 3 * 365 * 86_400_000, TimeStyle::Relative, t),
            "3 years ago"
        );
        assert_eq!(
            format(t + 5 * 60_000, TimeStyle::Relative, t),
            "in 5 minutes"
        );
        assert_eq!(format(t - 3_600_000, TimeStyle::Relative, t), "an hour ago");
    }

    #[test]
    fn durations_and_snowflakes() {
        assert_eq!(duration(42_000), "0:42");
        assert_eq!(duration(3_723_000), "1:02:03");
        assert_eq!(duration(3 * 86_400_000), "3 days");
        assert_eq!(snowflake_ms(175_928_847_299_117_063), 1_462_015_105_796);
    }
}
