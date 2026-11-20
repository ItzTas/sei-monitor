use chrono::{
    DateTime, Datelike, NaiveDate, NaiveDateTime, Offset, TimeDelta, TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;

const MAX_DURATION_DAYS: i64 = 106_751;

pub fn load_zone(name: &str) -> Option<Tz> {
    if name.is_empty() || name == "UTC" {
        return Some(Tz::UTC);
    }
    if name == "Local" || name.contains("..") || name.starts_with(['/', '\\']) {
        return None;
    }
    name.parse().ok()
}

pub fn local_to_utc(local: NaiveDateTime, zone: Tz) -> DateTime<Utc> {
    let guess = local.and_utc();
    let first = offset_at(zone, guess);
    guess - offset_at(zone, guess - first)
}

fn offset_at(zone: Tz, at: DateTime<Utc>) -> TimeDelta {
    let offset = zone.offset_from_utc_datetime(&at.naive_utc()).fix();
    TimeDelta::seconds(i64::from(offset.local_minus_utc()))
}

pub fn days_until(now: DateTime<Utc>, date: NaiveDate, zone: Tz) -> i64 {
    let today = now.with_timezone(&zone).date_naive();
    date.signed_duration_since(today)
        .num_days()
        .clamp(-MAX_DURATION_DAYS, MAX_DURATION_DAYS)
}

pub fn format_local(at: DateTime<Utc>, zone: Tz) -> String {
    let local = at.with_timezone(&zone);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        local.year(),
        local.month(),
        local.day(),
        local.hour(),
        local.minute()
    )
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::golden;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LocalCase {
        zone: String,
        local: String,
        utc: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct DaysCase {
        now: String,
        date: String,
        zone: String,
        days: i64,
    }

    fn instant(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn zone(name: &str) -> Tz {
        load_zone(name).unwrap_or_else(|| panic!("zone {name} is missing"))
    }

    #[test]
    fn local_time_matches_go() {
        for case in golden::cases::<LocalCase>("local_time", "") {
            let local = NaiveDateTime::parse_from_str(&case.local, "%Y-%m-%dT%H:%M:%S").unwrap();
            assert_eq!(
                local_to_utc(local, zone(&case.zone)),
                instant(&case.utc),
                "{} in {}",
                case.local,
                case.zone
            );
        }
    }

    #[test]
    fn days_until_matches_go() {
        for case in golden::cases::<DaysCase>("days_until", "") {
            let date = NaiveDate::parse_from_str(&case.date, "%Y-%m-%d").unwrap();
            assert_eq!(
                days_until(instant(&case.now), date, zone(&case.zone)),
                case.days,
                "from {} to {} in {}",
                case.now,
                case.date,
                case.zone
            );
        }
    }

    #[test]
    fn days_until_saturates_like_go_durations() {
        let now = instant("2026-11-10T12:00:00Z");
        let cases = [
            ("2319-03-16", 106_751),
            ("2319-12-31", 106_751),
            ("9999-12-31", 106_751),
            ("1734-07-06", -106_751),
            ("1600-01-01", -106_751),
        ];
        for (date, want) in cases {
            let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
            assert_eq!(days_until(now, date, Tz::UTC), want, "{date}");
        }
    }

    #[test]
    fn zones_load_like_go() {
        let loaded = [
            ("", Some("UTC")),
            ("UTC", Some("UTC")),
            ("Etc/UTC", Some("Etc/UTC")),
            ("America/Boa_Vista", Some("America/Boa_Vista")),
            ("Brazil/East", Some("Brazil/East")),
            ("Local", None),
            ("america/boa_vista", None),
            ("US/Pacific-New", None),
            ("../x", None),
            ("/etc/localtime", None),
        ];
        for (name, want) in loaded {
            assert_eq!(load_zone(name).map(Tz::name), want, "{name:?}");
        }
    }

    #[test]
    fn local_times_are_written_to_the_minute() {
        assert_eq!(
            format_local(instant("2026-11-10T23:59:59Z"), zone("America/Sao_Paulo")),
            "2026-11-10 20:59"
        );
    }
}
