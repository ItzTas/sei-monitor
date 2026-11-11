use super::escape::{Mode, unescape};
use super::ip::parse_addr;

pub(super) struct Authority {
    pub(super) has_user: bool,
    pub(super) host: Vec<u8>,
}

pub(super) fn parse_authority(scheme: &str, authority: &str) -> Result<Authority, ()> {
    let Some(at) = authority.rfind('@') else {
        let host = parse_host(scheme, authority)?;
        return Ok(Authority {
            has_user: false,
            host,
        });
    };
    let host = parse_host(scheme, &authority[at + 1..])?;
    check_userinfo(&authority[..at])?;
    Ok(Authority {
        has_user: true,
        host,
    })
}

fn check_userinfo(userinfo: &str) -> Result<(), ()> {
    if !userinfo.chars().all(valid_userinfo_char) {
        return Err(());
    }
    let (username, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
    unescape(username, Mode::UserPassword)?;
    unescape(password, Mode::UserPassword)?;
    Ok(())
}

fn valid_userinfo_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-._:~!$&'()*+,;=%@".contains(c)
}

fn parse_host(scheme: &str, host: &str) -> Result<Vec<u8>, ()> {
    match host.rfind('[') {
        Some(0) => parse_ip_literal(host),
        Some(_) => Err(()),
        None => {
            check_port(scheme, host)?;
            unescape(host, Mode::Host)
        }
    }
}

fn check_port(scheme: &str, host: &str) -> Result<(), ()> {
    let Some(first) = host.find(':') else {
        return Ok(());
    };
    let last = host.rfind(':').unwrap_or(first);
    let colon = if scheme == "http" || scheme == "https" {
        first
    } else {
        last
    };
    if !valid_optional_port(&host[colon..]) {
        return Err(());
    }
    Ok(())
}

fn parse_ip_literal(host: &str) -> Result<Vec<u8>, ()> {
    let close = host.rfind(']').ok_or(())?;
    let colon_port = &host[close + 1..];
    if !valid_optional_port(colon_port) {
        return Err(());
    }
    let port = unescape(colon_port, Mode::Host)?;
    let hostname = &host[1..close];
    let address = match hostname.find("%25") {
        Some(zone) => [
            unescape(&hostname[..zone], Mode::Host)?,
            unescape(&hostname[zone..], Mode::Zone)?,
        ]
        .concat(),
        None => unescape(hostname, Mode::Host)?,
    };
    if parse_addr(&address)?.is_ipv4() {
        return Err(());
    }
    Ok([b"[".as_slice(), &address, b"]", &port].concat())
}

pub(super) fn valid_optional_port(port: &str) -> bool {
    let Some(digits) = port.strip_prefix(':') else {
        return port.is_empty();
    };
    digits.bytes().all(|c| c.is_ascii_digit())
}

pub(super) fn hostname(host: &[u8]) -> &[u8] {
    let host = match host.iter().rposition(|&c| c == b':') {
        Some(colon) if valid_port_bytes(&host[colon + 1..]) => &host[..colon],
        _ => host,
    };
    match host {
        [b'[', inner @ .., b']'] => inner,
        _ => host,
    }
}

fn valid_port_bytes(digits: &[u8]) -> bool {
    digits.iter().all(u8::is_ascii_digit)
}
