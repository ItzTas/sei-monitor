const LIMIT: u64 = 1 << 63;
const SECOND: u64 = 1_000_000_000;

pub fn parse_go_duration(text: &str) -> Option<i64> {
    let (negative, mut rest) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if rest == "0" {
        return Some(0);
    }
    if rest.is_empty() {
        return None;
    }
    let mut total: u64 = 0;
    while !rest.is_empty() {
        let (value, remaining) = component(rest)?;
        total = total.wrapping_add(value);
        if total > LIMIT {
            return None;
        }
        rest = remaining;
    }
    if negative {
        return Some((total as i64).wrapping_neg());
    }
    i64::try_from(total).ok()
}

fn component(text: &str) -> Option<(u64, &str)> {
    let first = text.as_bytes()[0];
    if first != b'.' && !first.is_ascii_digit() {
        return None;
    }
    let (mut value, rest) = leading_int(text)?;
    let whole = rest.len() != text.len();
    let (fraction, scale, decimals, rest) = match rest.strip_prefix('.') {
        Some(digits) => leading_fraction(digits),
        None => (0, 1.0, false, rest),
    };
    if !whole && !decimals {
        return None;
    }
    let length = rest
        .bytes()
        .position(|c| c == b'.' || c.is_ascii_digit())
        .unwrap_or(rest.len());
    if length == 0 {
        return None;
    }
    let (unit, rest) = rest.split_at(length);
    let unit = unit_nanos(unit)?;
    if value > LIMIT / unit {
        return None;
    }
    value *= unit;
    if fraction > 0 {
        value += (fraction as f64 * (unit as f64 / scale)) as u64;
        if value > LIMIT {
            return None;
        }
    }
    Some((value, rest))
}

fn unit_nanos(unit: &str) -> Option<u64> {
    const UNITS: [(&str, u64); 8] = [
        ("ns", 1),
        ("us", 1_000),
        ("µs", 1_000),
        ("μs", 1_000),
        ("ms", 1_000_000),
        ("s", SECOND),
        ("m", 60 * SECOND),
        ("h", 3_600 * SECOND),
    ];
    UNITS
        .iter()
        .find(|(name, _)| *name == unit)
        .map(|(_, nanos)| *nanos)
}

fn leading_int(text: &str) -> Option<(u64, &str)> {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    let mut value: u64 = 0;
    for c in text[..digits].bytes() {
        if value > LIMIT / 10 {
            return None;
        }
        value = value * 10 + u64::from(c - b'0');
        if value > LIMIT {
            return None;
        }
    }
    Some((value, &text[digits..]))
}

fn leading_fraction(text: &str) -> (u64, f64, bool, &str) {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    let mut value: u64 = 0;
    let mut scale = 1.0;
    for c in text[..digits].bytes() {
        if value > (LIMIT - 1) / 10 {
            break;
        }
        let next = value * 10 + u64::from(c - b'0');
        if next > LIMIT {
            break;
        }
        value = next;
        scale *= 10.0;
    }
    (value, scale, digits > 0, &text[digits..])
}

pub fn format_go_duration(nanos: i64) -> String {
    let magnitude = nanos.unsigned_abs();
    let text = if magnitude < SECOND {
        subsecond(magnitude)
    } else {
        seconds(magnitude)
    };
    if nanos < 0 {
        return format!("-{text}");
    }
    text
}

fn subsecond(nanos: u64) -> String {
    if nanos == 0 {
        return "0s".to_owned();
    }
    let (digits, unit) = match nanos {
        ..1_000 => (0, "ns"),
        1_000..1_000_000 => (3, "µs"),
        _ => (6, "ms"),
    };
    let (whole, fraction) = split_fraction(nanos, digits);
    format!("{whole}{fraction}{unit}")
}

fn seconds(nanos: u64) -> String {
    let (seconds, fraction) = split_fraction(nanos, 9);
    let text = format!("{}{fraction}s", seconds % 60);
    let minutes = seconds / 60;
    if minutes == 0 {
        return text;
    }
    let text = format!("{}m{text}", minutes % 60);
    let hours = minutes / 60;
    if hours == 0 {
        return text;
    }
    format!("{hours}h{text}")
}

fn split_fraction(value: u64, digits: u32) -> (u64, String) {
    let scale = 10u64.pow(digits);
    let rest = value % scale;
    if rest == 0 {
        return (value / scale, String::new());
    }
    let width = digits as usize;
    let fraction = format!(".{rest:0width$}");
    (value / scale, fraction.trim_end_matches('0').to_owned())
}

pub fn round_go_duration(nanos: i64, unit: i64) -> i64 {
    if unit <= 0 {
        return nanos;
    }
    let rest = nanos % unit;
    if nanos < 0 {
        return round_negative(nanos, unit, -rest);
    }
    if less_than_half(rest, unit) {
        return nanos - rest;
    }
    let rounded = nanos.wrapping_add(unit).wrapping_sub(rest);
    if rounded > nanos {
        return rounded;
    }
    i64::MAX
}

fn round_negative(nanos: i64, unit: i64, rest: i64) -> i64 {
    if less_than_half(rest, unit) {
        return nanos + rest;
    }
    let rounded = nanos.wrapping_sub(unit).wrapping_add(rest);
    if rounded < nanos {
        return rounded;
    }
    i64::MIN
}

fn less_than_half(x: i64, y: i64) -> bool {
    (x as u64).wrapping_add(x as u64) < y as u64
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::golden;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParseCase {
        input: String,
        nanoseconds: Option<i64>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FormatCase {
        nanoseconds: i64,
        text: String,
    }

    #[test]
    fn duration_matches_go() {
        for case in golden::cases::<ParseCase>("duration", "/parse") {
            assert_eq!(
                parse_go_duration(&case.input),
                case.nanoseconds,
                "parse {:?}",
                case.input
            );
        }
        for case in golden::cases::<FormatCase>("duration", "/format") {
            assert_eq!(
                format_go_duration(case.nanoseconds),
                case.text,
                "format {}",
                case.nanoseconds
            );
        }
    }

    #[test]
    fn parse_wraps_the_sum_like_go() {
        assert_eq!(
            parse_go_duration("9223372036854775808ns9223372036854775808ns"),
            Some(0)
        );
    }

    #[test]
    fn round_halves_away_from_zero() {
        let cases = [
            (1500, 1000, 2000),
            (-1500, 1000, -2000),
            (1499, 1000, 1000),
            (-1499, 1000, -1000),
            (2500, 1000, 3000),
            (7, 2, 8),
            (-7, 2, -8),
            (42, 0, 42),
            (42, -5, 42),
            (i64::MAX, 3_600_000_000_000, i64::MAX),
            (i64::MIN, 3_600_000_000_000, i64::MIN),
            (299_999_999_999, 1_000_000_000, 300_000_000_000),
            (90_000_000_000, 60_000_000_000, 120_000_000_000),
        ];
        for (nanos, unit, want) in cases {
            assert_eq!(round_go_duration(nanos, unit), want, "{nanos} by {unit}");
        }
    }
}
