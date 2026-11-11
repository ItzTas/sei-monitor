use std::collections::HashSet;

use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252};

use super::media_type;
use super::tokenizer::{Token, Tokenizer};
use crate::gotext::{to_lower, trim};
use crate::sei::json::go_lossy_utf8;

pub(super) const PREVIEW_SIZE: usize = 1024;

#[derive(Clone, Copy)]
pub(super) struct Detected {
    pub(super) encoding: &'static Encoding,
    pub(super) transcode: bool,
}

impl Detected {
    fn decoding(encoding: &'static Encoding) -> Detected {
        Detected {
            encoding,
            transcode: true,
        }
    }

    fn untouched_utf8() -> Detected {
        Detected {
            encoding: UTF_8,
            transcode: false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pragma {
    Unknown,
    Needed,
    NotNeeded,
}

pub(super) fn determine(content_type: Option<&str>, body: &[u8]) -> Detected {
    let preview = &body[..body.len().min(PREVIEW_SIZE)];
    by_byte_order_mark(preview)
        .or_else(|| content_type.and_then(by_header))
        .or_else(|| prescan(preview))
        .unwrap_or_else(|| sniff(preview))
}

fn by_byte_order_mark(preview: &[u8]) -> Option<Detected> {
    let marks: [(&[u8], &'static Encoding); 3] = [
        (b"\xfe\xff", UTF_16BE),
        (b"\xff\xfe", UTF_16LE),
        (b"\xef\xbb\xbf", UTF_8),
    ];
    marks
        .iter()
        .find(|(mark, _)| preview.starts_with(mark))
        .map(|(_, encoding)| Detected::decoding(encoding))
}

fn by_header(content_type: &str) -> Option<Detected> {
    let label = media_type::charset(content_type)?;
    lookup(label.as_bytes()).map(Detected::decoding)
}

fn lookup(label: &[u8]) -> Option<&'static Encoding> {
    let label = go_lossy_utf8(label);
    Encoding::for_label(to_lower(trim(&label)).as_bytes())
}

fn prescan(content: &[u8]) -> Option<Detected> {
    let mut tokenizer = Tokenizer::new(content);
    loop {
        match tokenizer.next() {
            Token::Error => return None,
            Token::StartTag | Token::SelfClosingTag => {
                if let Some(found) = meta_encoding(&tokenizer) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
}

fn meta_encoding(tokenizer: &Tokenizer) -> Option<Detected> {
    if !tokenizer.tag_name_is(b"meta") {
        return None;
    }
    let mut seen = HashSet::new();
    let mut got_pragma = false;
    let mut pragma = Pragma::Unknown;
    let mut encoding = None;
    for (key, mut value) in tokenizer.attributes() {
        if !seen.insert(key.clone()) {
            continue;
        }
        value.make_ascii_lowercase();
        match key.as_slice() {
            b"http-equiv" => got_pragma |= value == b"content-type",
            b"content" if encoding.is_none() => {
                if let Some(found) = from_meta_element(&value).and_then(lookup) {
                    encoding = Some(found);
                    pragma = Pragma::Needed;
                }
            }
            b"charset" => {
                encoding = lookup(&value);
                pragma = Pragma::NotNeeded;
            }
            _ => {}
        }
    }
    if pragma == Pragma::Unknown || (pragma == Pragma::Needed && !got_pragma) {
        return None;
    }
    let encoding = encoding?;
    if encoding == UTF_16BE || encoding == UTF_16LE {
        return Some(Detected::untouched_utf8());
    }
    Some(Detected::decoding(encoding))
}

fn from_meta_element(mut text: &[u8]) -> Option<&[u8]> {
    const SPACES: &[u8] = b" \t\n\x0c\r";
    while !text.is_empty() {
        let at = text.windows(7).position(|window| window == b"charset")?;
        text = trim_start(&text[at + 7..], SPACES);
        let Some(rest) = text.strip_prefix(b"=") else {
            continue;
        };
        let rest = trim_start(rest, SPACES);
        let (&first, quoted) = rest.split_first()?;
        if first == b'"' || first == b'\'' {
            let close = quoted.iter().position(|&c| c == first)?;
            return Some(&quoted[..close]);
        }
        let end = rest
            .iter()
            .position(|c| b"; \t\n\x0c\r".contains(c))
            .unwrap_or(rest.len());
        return Some(&rest[..end]);
    }
    None
}

fn trim_start<'a>(text: &'a [u8], set: &[u8]) -> &'a [u8] {
    let start = text
        .iter()
        .position(|c| !set.contains(c))
        .unwrap_or(text.len());
    &text[start..]
}

fn sniff(preview: &[u8]) -> Detected {
    let complete = without_partial_rune(preview);
    let has_high_bit = complete.iter().any(|&c| c >= 0x80);
    if has_high_bit && std::str::from_utf8(complete).is_ok() {
        return Detected::untouched_utf8();
    }
    Detected::decoding(WINDOWS_1252)
}

fn without_partial_rune(content: &[u8]) -> &[u8] {
    let floor = content.len().saturating_sub(3);
    for index in (floor..content.len()).rev() {
        let c = content[index];
        if c < 0x80 {
            break;
        }
        if c & 0xC0 != 0x80 {
            return &content[..index];
        }
    }
    content
}
