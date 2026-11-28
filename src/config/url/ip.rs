pub(super) struct Addr {
    octets: [u8; 16],
    ipv4: bool,
    zoned: bool,
}

impl Addr {
    pub(super) fn is_ipv4(&self) -> bool {
        self.ipv4
    }
}

pub(super) fn parse_addr(text: &[u8]) -> Result<Addr, ()> {
    match text.iter().find(|c| matches!(c, b'.' | b':' | b'%')) {
        Some(b'.') => parse_ipv4(text),
        Some(b':') => parse_ipv6(text),
        _ => Err(()),
    }
}

pub(super) fn is_loopback_ip(text: &[u8]) -> bool {
    let Ok(addr) = parse_addr(text) else {
        return false;
    };
    if addr.zoned {
        return false;
    }
    let octets = addr.octets;
    let mapped = octets[..10].iter().all(|&b| b == 0) && octets[10..12] == [0xff, 0xff];
    if mapped {
        return octets[12] == 127;
    }
    octets == [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]
}

fn parse_ipv4(text: &[u8]) -> Result<Addr, ()> {
    let mut octets = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 0, 0];
    parse_ipv4_fields(text, &mut octets[12..])?;
    Ok(Addr {
        octets,
        ipv4: true,
        zoned: false,
    })
}

fn parse_ipv4_fields(text: &[u8], fields: &mut [u8]) -> Result<(), ()> {
    let (mut value, mut position, mut digits) = (0u32, 0, 0);
    for (index, &c) in text.iter().enumerate() {
        if c.is_ascii_digit() {
            if digits == 1 && value == 0 {
                return Err(());
            }
            value = value * 10 + u32::from(c - b'0');
            digits += 1;
            if value > 255 {
                return Err(());
            }
            continue;
        }
        let separator = c == b'.' && index != 0 && index != text.len() - 1;
        if !separator || text[index - 1] == b'.' || position == 3 {
            return Err(());
        }
        fields[position] = u8::try_from(value).map_err(|_| ())?;
        position += 1;
        value = 0;
        digits = 0;
    }
    if position < 3 {
        return Err(());
    }
    fields[3] = u8::try_from(value).map_err(|_| ())?;
    Ok(())
}

fn parse_ipv6(text: &[u8]) -> Result<Addr, ()> {
    let (address, zoned) = match text.iter().position(|&c| c == b'%') {
        Some(index) if index + 1 == text.len() => return Err(()),
        Some(index) => (&text[..index], true),
        None => (text, false),
    };
    let octets = Ipv6Parser::default().parse(address)?;
    Ok(Addr {
        octets,
        ipv4: false,
        zoned,
    })
}

#[derive(Default)]
struct Ipv6Parser {
    octets: [u8; 16],
    filled: usize,
    ellipsis: Option<usize>,
}

impl Ipv6Parser {
    fn parse(mut self, address: &[u8]) -> Result<[u8; 16], ()> {
        let mut rest = address;
        if let Some(after) = rest.strip_prefix(b"::") {
            self.ellipsis = Some(0);
            rest = after;
            if rest.is_empty() {
                return Ok(self.octets);
            }
        }
        while self.filled < 16 {
            let Some(next) = self.group(rest)? else {
                rest = &[];
                break;
            };
            rest = next;
            if rest.is_empty() {
                break;
            }
        }
        if !rest.is_empty() {
            return Err(());
        }
        self.expand()
    }

    fn group<'a>(&mut self, rest: &'a [u8]) -> Result<Option<&'a [u8]>, ()> {
        let (value, digits) = hex_group(rest)?;
        if rest.get(digits) == Some(&b'.') {
            self.embedded_ipv4(rest)?;
            return Ok(None);
        }
        self.octets[self.filled..self.filled + 2].copy_from_slice(&value.to_be_bytes());
        self.filled += 2;
        let rest = &rest[digits..];
        if rest.is_empty() {
            return Ok(Some(rest));
        }
        if rest[0] != b':' || rest.len() == 1 {
            return Err(());
        }
        let rest = &rest[1..];
        let Some(after) = rest.strip_prefix(b":") else {
            return Ok(Some(rest));
        };
        if self.ellipsis.is_some() {
            return Err(());
        }
        self.ellipsis = Some(self.filled);
        if after.is_empty() {
            return Ok(None);
        }
        Ok(Some(after))
    }

    fn embedded_ipv4(&mut self, rest: &[u8]) -> Result<(), ()> {
        if self.ellipsis.is_none() && self.filled != 12 {
            return Err(());
        }
        if self.filled + 4 > 16 {
            return Err(());
        }
        parse_ipv4_fields(rest, &mut self.octets[self.filled..self.filled + 4])?;
        self.filled += 4;
        Ok(())
    }

    fn expand(mut self) -> Result<[u8; 16], ()> {
        let Some(ellipsis) = self.ellipsis else {
            return if self.filled < 16 {
                Err(())
            } else {
                Ok(self.octets)
            };
        };
        if self.filled == 16 {
            return Err(());
        }
        let shift = 16 - self.filled;
        self.octets
            .copy_within(ellipsis..self.filled, ellipsis + shift);
        self.octets[ellipsis..ellipsis + shift].fill(0);
        Ok(self.octets)
    }
}

fn hex_group(text: &[u8]) -> Result<(u16, usize), ()> {
    let mut value: u32 = 0;
    let mut digits = 0;
    for &c in text {
        let Some(digit) = char::from(c).to_digit(16) else {
            break;
        };
        value = (value << 4) + digit;
        if digits > 3 || value > u32::from(u16::MAX) {
            return Err(());
        }
        digits += 1;
    }
    if digits == 0 {
        return Err(());
    }
    Ok((u16::try_from(value).map_err(|_| ())?, digits))
}
