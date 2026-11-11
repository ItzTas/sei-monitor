const UPPER_HEX: &[u8; 16] = b"0123456789ABCDEF";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Path,
    Host,
    Zone,
    UserPassword,
    Fragment,
}

pub(super) fn should_escape(c: u8, mode: Mode) -> bool {
    if c.is_ascii_alphanumeric() {
        return false;
    }
    if matches!(mode, Mode::Host | Mode::Zone) && b"!$&'()*+,;=:[]<>\"".contains(&c) {
        return false;
    }
    if b"-_.~".contains(&c) {
        return false;
    }
    if b"$&+,/:;=?@".contains(&c) {
        return reserved_escapes(c, mode);
    }
    !(mode == Mode::Fragment && b"!()*".contains(&c))
}

fn reserved_escapes(c: u8, mode: Mode) -> bool {
    match mode {
        Mode::Path => c == b'?',
        Mode::UserPassword => matches!(c, b'@' | b'/' | b'?' | b':'),
        Mode::Fragment => false,
        Mode::Host | Mode::Zone => true,
    }
}

pub(super) fn escape(bytes: &[u8], mode: Mode) -> String {
    let mut escaped = String::with_capacity(bytes.len());
    for &c in bytes {
        if !should_escape(c, mode) {
            escaped.push(char::from(c));
            continue;
        }
        escaped.push('%');
        escaped.push(char::from(UPPER_HEX[usize::from(c >> 4)]));
        escaped.push(char::from(UPPER_HEX[usize::from(c & 15)]));
    }
    escaped
}

pub(super) fn unescape(text: &str, mode: Mode) -> Result<Vec<u8>, ()> {
    let bytes = text.as_bytes();
    let mut unescaped = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let c = bytes[index];
        if c == b'%' {
            unescaped.push(percent(bytes, index, mode)?);
            index += 3;
            continue;
        }
        if matches!(mode, Mode::Host | Mode::Zone) && c < 0x80 && should_escape(c, mode) {
            return Err(());
        }
        unescaped.push(c);
        index += 1;
    }
    Ok(unescaped)
}

fn percent(bytes: &[u8], index: usize, mode: Mode) -> Result<u8, ()> {
    if index + 2 >= bytes.len() {
        return Err(());
    }
    let (high, low) = (hex(bytes[index + 1])?, hex(bytes[index + 2])?);
    let value = high << 4 | low;
    let percent_sign = &bytes[index..index + 3] == b"%25";
    if mode == Mode::Host && high < 8 && !percent_sign {
        return Err(());
    }
    if mode == Mode::Zone && !percent_sign && value != b' ' && should_escape(value, Mode::Host) {
        return Err(());
    }
    Ok(value)
}

fn hex(c: u8) -> Result<u8, ()> {
    char::from(c)
        .to_digit(16)
        .and_then(|digit| u8::try_from(digit).ok())
        .ok_or(())
}

pub(super) fn valid_encoded(text: &str, mode: Mode) -> bool {
    text.bytes()
        .all(|c| b"!$&'()*+,;=:@[]%".contains(&c) || !should_escape(c, mode))
}
