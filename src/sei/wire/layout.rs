use chrono::{NaiveDate, NaiveDateTime};

pub(super) fn parse_date(text: &str) -> Option<NaiveDate> {
    let mut cursor = Cursor(text.as_bytes());
    let date = cursor.date()?;
    cursor.finished()?;
    date.date()
}

pub(super) fn parse_date_time(text: &str) -> Option<NaiveDateTime> {
    let mut cursor = Cursor(text.as_bytes());
    let date = cursor.date()?;
    cursor.literal(b" ")?;
    let hour = cursor.number(false).filter(|hour| *hour < 24)?;
    cursor.literal(b":")?;
    let minute = cursor.number(true).filter(|minute| *minute < 60)?;
    cursor.literal(b":")?;
    let second = cursor.number(true).filter(|second| *second < 60)?;
    let nanos = cursor.fraction();
    cursor.finished()?;
    date.date()?.and_hms_nano_opt(hour, minute, second, nanos)
}

struct Date {
    year: i32,
    month: u32,
    day: u32,
}

impl Date {
    fn date(&self) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(self.year, self.month, self.day)
    }
}

struct Cursor<'a>(&'a [u8]);

impl Cursor<'_> {
    fn date(&mut self) -> Option<Date> {
        let day = self.number(true)?;
        self.literal(b"/")?;
        let month = self.number(true).filter(|month| (1..=12).contains(month))?;
        self.literal(b"/")?;
        let year = self.year()?;
        Some(Date { year, month, day })
    }

    fn digit(&self, index: usize) -> Option<u32> {
        self.0
            .get(index)
            .filter(|c| c.is_ascii_digit())
            .map(|c| u32::from(c - b'0'))
    }

    fn number(&mut self, fixed: bool) -> Option<u32> {
        let first = self.digit(0)?;
        let Some(second) = self.digit(1) else {
            if fixed {
                return None;
            }
            self.0 = &self.0[1..];
            return Some(first);
        };
        self.0 = &self.0[2..];
        Some(first * 10 + second)
    }

    fn year(&mut self) -> Option<i32> {
        let digits = self.0.get(..4)?;
        if !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        self.0 = &self.0[4..];
        Some(
            digits
                .iter()
                .fold(0, |year, c| year * 10 + i32::from(c - b'0')),
        )
    }

    fn literal(&mut self, mut prefix: &[u8]) -> Option<()> {
        while let Some((&expected, rest)) = prefix.split_first() {
            if expected == b' ' {
                if self.0.first().is_some_and(|c| *c != b' ') {
                    return None;
                }
                prefix = cut_spaces(prefix);
                self.0 = cut_spaces(self.0);
                continue;
            }
            let (&found, value) = self.0.split_first()?;
            if found != expected {
                return None;
            }
            prefix = rest;
            self.0 = value;
        }
        Some(())
    }

    fn fraction(&mut self) -> u32 {
        let starts = matches!(self.0.first(), Some(b'.' | b',')) && self.digit(1).is_some();
        if !starts {
            return 0;
        }
        let length = 1 + self.0[1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        let kept = &self.0[1..length.min(10)];
        self.0 = &self.0[length..];
        let nanos = kept
            .iter()
            .fold(0, |nanos, c| nanos * 10 + u32::from(c - b'0'));
        nanos * 10_u32.pow(9 - kept.len() as u32)
    }

    fn finished(&self) -> Option<()> {
        self.0.is_empty().then_some(())
    }
}

fn cut_spaces(text: &[u8]) -> &[u8] {
    let spaces = text.iter().take_while(|c| **c == b' ').count();
    &text[spaces..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_follow_the_go_layout() {
        let date = |y, m, d| NaiveDate::from_ymd_opt(y, m, d);
        let cases = [
            ("06/11/2026", date(2026, 11, 6)),
            ("29/02/2024", date(2024, 2, 29)),
            ("06/11/0000", date(0, 11, 6)),
            ("00/11/2026", None),
            ("06/13/2026", None),
            ("06/11/26", None),
            ("06/11/+202", None),
            ("06/11/20266", None),
            ("06-11-2026", None),
        ];
        for (text, want) in cases {
            assert_eq!(parse_date(text), want, "{text:?}");
        }
    }

    #[test]
    fn times_follow_the_go_layout() {
        let at = |h, m, s, n| {
            NaiveDate::from_ymd_opt(2026, 11, 4).and_then(|date| date.and_hms_nano_opt(h, m, s, n))
        };
        let cases = [
            ("04/11/2026 13:13:05", at(13, 13, 5, 0)),
            ("04/11/2026 9:00:00", at(9, 0, 0, 0)),
            ("04/11/2026   09:00:00", at(9, 0, 0, 0)),
            ("04/11/2026 09:00:00,5", at(9, 0, 0, 500_000_000)),
            ("04/11/2026 09:00:00.1234567891", at(9, 0, 0, 123_456_789)),
            ("04/11/2026 09:00:00.", None),
            ("04/11/2026 09:00:00.5x", None),
            ("04/11/202609:00:00", None),
            ("04/11/2026 23:60:00", None),
            ("04/11/2026 23:59:60", None),
            ("04/11/2026 9:5:00", None),
        ];
        for (text, want) in cases {
            assert_eq!(parse_date_time(text), want, "{text:?}");
        }
    }
}
