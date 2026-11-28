mod escape;
mod host;
mod ip;

use std::fmt;

use escape::{Mode, escape, unescape, valid_encoded};
use host::{hostname, parse_authority};
use ip::is_loopback_ip;

use crate::gotext::equal_fold;

pub(crate) struct GoUrl {
    scheme: String,
    has_user: bool,
    host: Vec<u8>,
    path: Vec<u8>,
    raw_path: String,
    raw_query: String,
    force_query: bool,
    fragment: Vec<u8>,
}

pub(crate) fn path_escape(text: &str) -> String {
    escape(text.as_bytes(), Mode::PathSegment)
}

pub(crate) fn parse_url(value: &str) -> Result<GoUrl, ()> {
    let (rest, fragment) = value.split_once('#').unwrap_or((value, ""));
    let mut url = parse(rest)?;
    url.fragment = unescape(fragment, Mode::Fragment)?;
    Ok(url)
}

fn parse(raw: &str) -> Result<GoUrl, ()> {
    if raw.bytes().any(|c| c < b' ' || c == 0x7f) {
        return Err(());
    }
    let (scheme, rest) = scheme(raw)?;
    let (rest, raw_query, force_query) = split_query(rest);
    let authority = rest.strip_prefix("//").ok_or(())?;
    let (authority, path) = authority
        .find('/')
        .map_or((authority, ""), |slash| authority.split_at(slash));
    let scheme = scheme.to_ascii_lowercase();
    let authority = parse_authority(&scheme, authority)?;
    let (path, raw_path) = set_path(path)?;
    Ok(GoUrl {
        scheme,
        has_user: authority.has_user,
        host: authority.host,
        path,
        raw_path,
        raw_query: raw_query.to_owned(),
        force_query,
        fragment: Vec::new(),
    })
}

fn scheme(raw: &str) -> Result<(&str, &str), ()> {
    for (index, c) in raw.bytes().enumerate() {
        match c {
            b'a'..=b'z' | b'A'..=b'Z' => {}
            b'0'..=b'9' | b'+' | b'-' | b'.' if index > 0 => {}
            b':' if index > 0 => return Ok((&raw[..index], &raw[index + 1..])),
            _ => return Err(()),
        }
    }
    Err(())
}

fn split_query(rest: &str) -> (&str, &str, bool) {
    if let Some(stripped) = rest.strip_suffix('?')
        && !stripped.contains('?')
    {
        return (stripped, "", true);
    }
    let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
    (rest, query, false)
}

fn set_path(escaped: &str) -> Result<(Vec<u8>, String), ()> {
    let path = unescape(escaped, Mode::Path)?;
    if escape(&path, Mode::Path) == escaped {
        return Ok((path, String::new()));
    }
    Ok((path, escaped.to_owned()))
}

impl GoUrl {
    fn escaped_path(&self) -> String {
        let raw_is_valid = !self.raw_path.is_empty()
            && valid_encoded(&self.raw_path, Mode::Path)
            && unescape(&self.raw_path, Mode::Path).is_ok_and(|path| path == self.path);
        if raw_is_valid {
            return self.raw_path.clone();
        }
        if self.path == b"*" {
            return "*".to_owned();
        }
        escape(&self.path, Mode::Path)
    }

    fn hostname(&self) -> &[u8] {
        hostname(&self.host)
    }
}

impl fmt::Display for GoUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:", self.scheme)?;
        if !self.host.is_empty() || !self.path.is_empty() || self.has_user {
            f.write_str("//")?;
        }
        f.write_str(&escape(&self.host, Mode::Host))?;
        let path = self.escaped_path();
        if !path.is_empty() && !path.starts_with('/') && !self.host.is_empty() {
            f.write_str("/")?;
        }
        f.write_str(&path)?;
        if self.force_query || !self.raw_query.is_empty() {
            write!(f, "?{}", self.raw_query)?;
        }
        Ok(())
    }
}

pub(crate) fn normalize_sei_url(value: &str) -> Result<String, String> {
    let mut url = accepted("SEI_URL", value)?;
    let end = url
        .path
        .iter()
        .rposition(|&c| c != b'/')
        .map_or(0, |last| last + 1);
    let trimmed = &url.path[..end];
    url.path = trimmed.strip_suffix(b"/sei").unwrap_or(trimmed).to_vec();
    url.raw_path.clear();
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

pub(crate) fn normalize_api_url(value: &str) -> Result<String, String> {
    let url = accepted("SEI_API_URL", value)?;
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

fn accepted(name: &str, value: &str) -> Result<GoUrl, String> {
    let problem = |text: &str| Err(format!("{name}: {text}"));
    let Some(url) = parse_url(value)
        .ok()
        .filter(|url| !url.host.is_empty() && matches!(url.scheme.as_str(), "http" | "https"))
    else {
        return problem("use a full address starting with https://");
    };
    if url.has_user {
        return problem("remove the username and password from the address");
    }
    if !url.fragment.is_empty() || !url.raw_query.is_empty() {
        return problem("remove everything after ? or #");
    }
    if url.scheme != "https" && !is_loopback(url.hostname()) {
        return problem("use https (plain http is only accepted for localhost)");
    }
    Ok(url)
}

fn is_loopback(host: &[u8]) -> bool {
    let named = std::str::from_utf8(host).is_ok_and(|host| equal_fold(host, "localhost"));
    named || is_loopback_ip(host)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::config::tests::{minimal, source};
    use crate::config::{SeiConfig, load_sei_url};
    use crate::golden;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct UrlCase {
        input: String,
        normalized: Option<String>,
        problems: Option<Vec<String>>,
    }

    fn outcome(
        result: Result<String, crate::config::ConfigError>,
    ) -> (Option<String>, Option<Vec<String>>) {
        result.map_or_else(|err| (None, Some(err.texts())), |url| (Some(url), None))
    }

    #[test]
    fn sei_url_matches_go() {
        for case in golden::cases::<UrlCase>("sei_url", "") {
            let got = outcome(load_sei_url(&source(&[("SEI_URL", &case.input)])));
            assert_eq!(
                got,
                (case.normalized, case.problems),
                "SEI_URL {:?}",
                case.input
            );
        }
    }

    #[test]
    fn api_url_matches_go() {
        for case in golden::cases::<UrlCase>("api_url", "") {
            let env = minimal(&[("SEI_API_URL", &case.input)]);
            let got = outcome(SeiConfig::load(&source(&env)).map(|sei| sei.api_url));
            assert_eq!(
                got,
                (case.normalized, case.problems),
                "SEI_API_URL {:?}",
                case.input
            );
        }
    }

    #[test]
    fn brackets_in_the_path_follow_go() {
        let cases = [
            ("https://h/a[b]", "https://h/a%5Bb%5D", "https://h/a[b]"),
            (
                "https://h/a[b]/sei/",
                "https://h/a%5Bb%5D",
                "https://h/a[b]/sei",
            ),
            (
                "https://h/%5Bx%5D",
                "https://h/%5Bx%5D",
                "https://h/%5Bx%5D",
            ),
        ];
        for (input, sei, api) in cases {
            assert_eq!(normalize_sei_url(input).as_deref(), Ok(sei), "{input}");
            assert_eq!(normalize_api_url(input).as_deref(), Ok(api), "{input}");
        }
    }

    #[test]
    fn path_escape_matches_go() {
        let cases = [
            ("9001", "9001"),
            ("a b/c;d,e?f", "a%20b%2Fc%3Bd%2Ce%3Ff"),
            ("$&+=:@-_.~", "$&+=:@-_.~"),
            ("!*'()[]#%\"", "%21%2A%27%28%29%5B%5D%23%25%22"),
            ("é", "%C3%A9"),
        ];
        for (input, escaped) in cases {
            assert_eq!(path_escape(input), escaped, "{input}");
        }
    }
}
