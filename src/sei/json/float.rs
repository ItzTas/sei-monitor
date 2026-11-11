struct FloatText {
    mantissa: u64,
    exponent: i64,
    negative: bool,
    truncated: bool,
    hex: bool,
}

pub(super) fn parse_float(text: &str) -> Option<f64> {
    let parsed = read_float(text.as_bytes())?;
    if parsed.hex {
        return hex_float(&parsed);
    }
    let digits: String = text.chars().filter(|c| *c != '_').collect();
    digits.parse::<f64>().ok().filter(|value| value.is_finite())
}

fn read_float(s: &[u8]) -> Option<FloatText> {
    let mut i = 0;
    let mut underscores = false;
    let negative = s.first() == Some(&b'-');
    if matches!(s.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let hex = i + 2 < s.len() && s[i] == b'0' && s[i + 1].eq_ignore_ascii_case(&b'x');
    let (base, max_digits, exponent_char) = if hex { (16, 16, b'p') } else { (10, 19, b'e') };
    if hex {
        i += 2;
    }
    let (mut saw_dot, mut saw_digits) = (false, false);
    let (mut digits, mut mantissa_digits, mut point) = (0_i64, 0_i64, 0_i64);
    let (mut mantissa, mut truncated) = (0_u64, false);
    while i < s.len() {
        let c = s[i];
        if c == b'_' {
            underscores = true;
            i += 1;
            continue;
        }
        if c == b'.' {
            if saw_dot {
                break;
            }
            saw_dot = true;
            point = digits;
            i += 1;
            continue;
        }
        let Some(digit) = char::from(c).to_digit(base) else {
            break;
        };
        i += 1;
        saw_digits = true;
        if c == b'0' && digits == 0 {
            point -= 1;
            continue;
        }
        digits += 1;
        if mantissa_digits < max_digits {
            mantissa = mantissa * u64::from(base) + u64::from(digit);
            mantissa_digits += 1;
        } else if digit != 0 {
            truncated = true;
        }
    }
    if !saw_digits {
        return None;
    }
    if !saw_dot {
        point = digits;
    }
    if hex {
        point *= 4;
        mantissa_digits *= 4;
    }
    if i < s.len() && s[i].to_ascii_lowercase() == exponent_char {
        i += 1;
        let sign = match s.get(i) {
            Some(b'+') => 1,
            Some(b'-') => -1,
            _ => 0,
        };
        if sign != 0 {
            i += 1;
        }
        if !s.get(i).is_some_and(u8::is_ascii_digit) {
            return None;
        }
        let mut exponent = 0_i64;
        while i < s.len() && (s[i].is_ascii_digit() || s[i] == b'_') {
            if s[i] == b'_' {
                underscores = true;
            } else if exponent < 10_000 {
                exponent = exponent * 10 + i64::from(s[i] - b'0');
            }
            i += 1;
        }
        point += exponent * if sign < 0 { -1 } else { 1 };
    } else if hex {
        return None;
    }
    if i != s.len() || underscores && !underscore_ok(s) {
        return None;
    }
    let exponent = if mantissa != 0 {
        point - mantissa_digits
    } else {
        0
    };
    Some(FloatText {
        mantissa,
        exponent,
        negative,
        truncated,
        hex,
    })
}

fn underscore_ok(s: &[u8]) -> bool {
    let s = match s.first() {
        Some(b'+' | b'-') => &s[1..],
        _ => s,
    };
    let prefixed =
        s.len() >= 2 && s[0] == b'0' && matches!(s[1].to_ascii_lowercase(), b'b' | b'o' | b'x');
    let hex = prefixed && s[1].eq_ignore_ascii_case(&b'x');
    let mut saw = if prefixed { b'0' } else { b'^' };
    let start = if prefixed { 2 } else { 0 };
    for &c in &s[start..] {
        if c.is_ascii_digit() || hex && c.is_ascii_hexdigit() {
            saw = b'0';
            continue;
        }
        if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
            continue;
        }
        if saw == b'_' {
            return false;
        }
        saw = b'!';
    }
    saw != b'_'
}

fn hex_float(parsed: &FloatText) -> Option<f64> {
    const MANTISSA_BITS: u32 = 52;
    const EXPONENT_BITS: u32 = 11;
    const BIAS: i64 = -1023;
    const MAX_EXPONENT: i64 = (1 << EXPONENT_BITS) + BIAS - 2;
    const MIN_EXPONENT: i64 = BIAS + 1;
    let mut mantissa = parsed.mantissa;
    let mut exponent = parsed.exponent + i64::from(MANTISSA_BITS);
    while mantissa != 0 && mantissa >> (MANTISSA_BITS + 2) == 0 {
        mantissa <<= 1;
        exponent -= 1;
    }
    if parsed.truncated {
        mantissa |= 1;
    }
    while mantissa >> (1 + MANTISSA_BITS + 2) != 0 {
        mantissa = mantissa >> 1 | mantissa & 1;
        exponent += 1;
    }
    while mantissa > 1 && exponent < MIN_EXPONENT - 2 {
        mantissa = mantissa >> 1 | mantissa & 1;
        exponent += 1;
    }
    let mut round = mantissa & 3;
    mantissa >>= 2;
    round |= mantissa & 1;
    exponent += 2;
    if round == 3 {
        mantissa += 1;
        if mantissa == 1 << (1 + MANTISSA_BITS) {
            mantissa >>= 1;
            exponent += 1;
        }
    }
    if mantissa >> MANTISSA_BITS == 0 {
        exponent = BIAS;
    }
    if exponent > MAX_EXPONENT {
        return None;
    }
    let mut bits = mantissa & ((1 << MANTISSA_BITS) - 1);
    bits |= (((exponent - BIAS) & ((1 << EXPONENT_BITS) - 1)) as u64) << MANTISSA_BITS;
    if parsed.negative {
        bits |= 1 << 63;
    }
    Some(f64::from_bits(bits))
}
