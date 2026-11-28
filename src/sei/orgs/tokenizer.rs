use std::collections::HashSet;

use super::entity;

const RAW_TEXT_TAGS: [&str; 10] = [
    "iframe",
    "noembed",
    "noframes",
    "noscript",
    "plaintext",
    "script",
    "style",
    "textarea",
    "title",
    "xmp",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Token {
    Error,
    Text,
    StartTag,
    EndTag,
    SelfClosingTag,
    Comment,
    Doctype,
}

#[derive(Clone, Copy, Default)]
struct Span {
    start: usize,
    end: usize,
}

#[derive(Clone, Copy)]
enum Script {
    Data,
    LessThanSign,
    EndTagOpen,
    EscapeStart,
    EscapeStartDash,
    Escaped,
    EscapedDash,
    EscapedDashDash,
    EscapedLessThanSign,
    EscapedEndTagOpen,
    DoubleEscapeStart,
    DoubleEscaped,
    DoubleEscapedDash,
    DoubleEscapedDashDash,
    DoubleEscapedLessThanSign,
    DoubleEscapeEnd,
}

pub(super) struct Tokenizer<'a> {
    buf: &'a [u8],
    eof: bool,
    raw: Span,
    data: Span,
    pending: [Span; 2],
    attrs: Vec<[Span; 2]>,
    attr_names: HashSet<Vec<u8>>,
    raw_tag: &'static str,
}

impl<'a> Tokenizer<'a> {
    pub(super) fn new(buf: &'a [u8]) -> Tokenizer<'a> {
        Tokenizer {
            buf,
            eof: false,
            raw: Span::default(),
            data: Span::default(),
            pending: [Span::default(); 2],
            attrs: Vec::new(),
            attr_names: HashSet::new(),
            raw_tag: "",
        }
    }

    pub(super) fn tag_name_is(&self, name: &[u8]) -> bool {
        self.buf[self.data.start..self.data.end].eq_ignore_ascii_case(name)
    }

    pub(super) fn attributes(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.attrs
            .iter()
            .map(|[key, value]| {
                let mut key = without_nul(&self.buf[key.start..key.end]);
                key.make_ascii_lowercase();
                let value = without_nul(&self.buf[value.start..value.end]);
                (key, unescape_attribute(&convert_newlines(&value)))
            })
            .collect()
    }

    pub(super) fn next(&mut self) -> Token {
        self.raw.start = self.raw.end;
        self.data = Span {
            start: self.raw.end,
            end: self.raw.end,
        };
        if self.eof {
            return Token::Error;
        }
        if !self.raw_tag.is_empty() {
            self.read_raw_text();
            if self.data.end > self.data.start {
                return Token::Text;
            }
        }
        self.scan()
    }

    fn read_raw_text(&mut self) {
        if self.raw_tag != "plaintext" {
            self.read_raw_or_rcdata();
            return;
        }
        while !self.eof {
            self.read_byte();
        }
        self.data.end = self.raw.end;
    }

    fn scan(&mut self) -> Token {
        loop {
            let c = self.read_byte();
            if self.eof {
                break;
            }
            if c != b'<' {
                continue;
            }
            let c = self.read_byte();
            if self.eof {
                break;
            }
            if !(c.is_ascii_alphabetic() || matches!(c, b'/' | b'!' | b'?')) {
                self.raw.end -= 1;
                continue;
            }
            let tag_start = self.raw.end - 2;
            if self.raw.start < tag_start {
                self.raw.end = tag_start;
                self.data.end = tag_start;
                return Token::Text;
            }
            let token = match c {
                b'/' => self.end_tag(),
                b'!' => Some(self.read_markup_declaration()),
                b'?' => Some(self.bogus_comment()),
                _ => Some(self.read_start_tag()),
            };
            let Some(token) = token else {
                break;
            };
            return token;
        }
        if self.raw.start < self.raw.end {
            self.data.end = self.raw.end;
            return Token::Text;
        }
        Token::Error
    }

    fn end_tag(&mut self) -> Option<Token> {
        let c = self.read_byte();
        if self.eof {
            return None;
        }
        if c == b'>' {
            return Some(Token::Comment);
        }
        if c.is_ascii_alphabetic() {
            self.read_tag(false);
            return Some(if self.eof {
                Token::Error
            } else {
                Token::EndTag
            });
        }
        Some(self.bogus_comment())
    }

    fn bogus_comment(&mut self) -> Token {
        self.raw.end -= 1;
        self.read_until_close_angle();
        Token::Comment
    }

    fn read_byte(&mut self) -> u8 {
        let Some(&c) = self.buf.get(self.raw.end) else {
            self.eof = true;
            return 0;
        };
        self.raw.end += 1;
        c
    }

    fn skip_white_space(&mut self) {
        if self.eof {
            return;
        }
        loop {
            let c = self.read_byte();
            if self.eof {
                return;
            }
            if !is_space(c) {
                self.raw.end -= 1;
                return;
            }
        }
    }

    fn read_raw_or_rcdata(&mut self) {
        if self.raw_tag == "script" {
            self.read_script();
            self.raw_tag = "";
            return;
        }
        loop {
            let c = self.read_byte();
            if self.eof {
                break;
            }
            if c != b'<' {
                continue;
            }
            let c = self.read_byte();
            if self.eof {
                break;
            }
            if c != b'/' {
                self.raw.end -= 1;
                continue;
            }
            if self.read_raw_end_tag() || self.eof {
                break;
            }
        }
        self.data.end = self.raw.end;
        self.raw_tag = "";
    }

    fn read_raw_end_tag(&mut self) -> bool {
        let tag = self.raw_tag.as_bytes();
        for &expected in tag {
            let c = self.read_byte();
            if self.eof {
                return false;
            }
            if c != expected && c != expected.to_ascii_uppercase() {
                self.raw.end -= 1;
                return false;
            }
        }
        let c = self.read_byte();
        if self.eof {
            return false;
        }
        if is_space(c) || c == b'/' || c == b'>' {
            self.raw.end -= 3 + tag.len();
            return true;
        }
        self.raw.end -= 1;
        false
    }

    fn read_script(&mut self) {
        let mut state = Script::Data;
        while let Some(next) = self.script_step(state) {
            state = next;
        }
        self.data.end = self.raw.end;
    }

    fn script_step(&mut self, state: Script) -> Option<Script> {
        match state {
            Script::EndTagOpen => self.script_end_tag(Script::Data),
            Script::EscapedEndTagOpen => self.script_end_tag(Script::Escaped),
            Script::DoubleEscapeStart => self.double_escape_start(),
            Script::DoubleEscapeEnd => self.double_escape_end(),
            _ => {
                let c = self.read_byte();
                if self.eof {
                    return None;
                }
                Some(self.script_transition(state, c))
            }
        }
    }

    fn script_transition(&mut self, state: Script, c: u8) -> Script {
        match (state, c) {
            (Script::Data, b'<') => Script::LessThanSign,
            (Script::Data, _) => Script::Data,
            (Script::LessThanSign, b'/') => Script::EndTagOpen,
            (Script::LessThanSign, b'!') => Script::EscapeStart,
            (Script::EscapeStart, b'-') => Script::EscapeStartDash,
            (Script::EscapeStartDash, b'-') => Script::EscapedDashDash,
            (Script::LessThanSign | Script::EscapeStart | Script::EscapeStartDash, _) => {
                self.unread(Script::Data)
            }
            (Script::Escaped | Script::EscapedDash | Script::EscapedDashDash, b'<') => {
                Script::EscapedLessThanSign
            }
            (Script::Escaped, b'-') => Script::EscapedDash,
            (Script::EscapedDash | Script::EscapedDashDash, b'-') => Script::EscapedDashDash,
            (Script::EscapedDashDash, b'>') => Script::Data,
            (Script::EscapedLessThanSign, b'/') => Script::EscapedEndTagOpen,
            (Script::EscapedLessThanSign, _) if c.is_ascii_alphabetic() => {
                Script::DoubleEscapeStart
            }
            (Script::EscapedLessThanSign, _) => self.unread(Script::Data),
            (
                Script::DoubleEscaped | Script::DoubleEscapedDash | Script::DoubleEscapedDashDash,
                b'<',
            ) => Script::DoubleEscapedLessThanSign,
            (Script::DoubleEscaped, b'-') => Script::DoubleEscapedDash,
            (Script::DoubleEscapedDash | Script::DoubleEscapedDashDash, b'-') => {
                Script::DoubleEscapedDashDash
            }
            (Script::DoubleEscapedDashDash, b'>') => Script::Data,
            (Script::DoubleEscapedLessThanSign, b'/') => Script::DoubleEscapeEnd,
            (Script::DoubleEscapedLessThanSign, _) => self.unread(Script::DoubleEscaped),
            (
                Script::DoubleEscaped | Script::DoubleEscapedDash | Script::DoubleEscapedDashDash,
                _,
            ) => Script::DoubleEscaped,
            _ => Script::Escaped,
        }
    }

    fn unread(&mut self, next: Script) -> Script {
        self.raw.end -= 1;
        next
    }

    fn script_end_tag(&mut self, otherwise: Script) -> Option<Script> {
        if self.read_raw_end_tag() || self.eof {
            return None;
        }
        Some(otherwise)
    }

    fn double_escape_start(&mut self) -> Option<Script> {
        self.raw.end -= 1;
        for expected in *b"script" {
            let c = self.read_byte();
            if self.eof {
                return None;
            }
            if c != expected && c != expected.to_ascii_uppercase() {
                return Some(self.unread(Script::Escaped));
            }
        }
        let c = self.read_byte();
        if self.eof {
            return None;
        }
        if is_space(c) || c == b'/' || c == b'>' {
            return Some(Script::DoubleEscaped);
        }
        Some(self.unread(Script::Escaped))
    }

    fn double_escape_end(&mut self) -> Option<Script> {
        if self.read_raw_end_tag() {
            self.raw.end += "</script>".len();
            return Some(Script::Escaped);
        }
        if self.eof {
            return None;
        }
        Some(Script::DoubleEscaped)
    }

    fn read_comment(&mut self) {
        let mut dashes = 0;
        let mut beginning = true;
        loop {
            let c = self.read_byte();
            if self.eof {
                return;
            }
            match c {
                b'-' => {
                    dashes += 1;
                    continue;
                }
                b'>' if dashes >= 2 || beginning => return,
                b'!' if dashes >= 2 => {
                    let c = self.read_byte();
                    if self.eof || c == b'>' {
                        return;
                    }
                    if c == b'-' {
                        dashes = 1;
                        beginning = false;
                        continue;
                    }
                }
                _ => {}
            }
            dashes = 0;
            beginning = false;
        }
    }

    fn read_until_close_angle(&mut self) {
        loop {
            let c = self.read_byte();
            if self.eof || c == b'>' {
                return;
            }
        }
    }

    fn read_markup_declaration(&mut self) -> Token {
        let mark = self.raw.end;
        let first = self.read_byte();
        if self.eof {
            return Token::Comment;
        }
        let second = self.read_byte();
        if self.eof {
            return Token::Comment;
        }
        if first == b'-' && second == b'-' {
            self.read_comment();
            return Token::Comment;
        }
        self.raw.end -= 2;
        if self.read_doctype(mark) {
            return Token::Doctype;
        }
        self.read_until_close_angle();
        Token::Comment
    }

    fn read_doctype(&mut self, mark: usize) -> bool {
        for expected in *b"DOCTYPE" {
            let c = self.read_byte();
            if self.eof {
                self.raw.end = mark;
                self.eof = false;
                return false;
            }
            if c != expected && c != expected.to_ascii_lowercase() {
                self.raw.end = mark;
                return false;
            }
        }
        self.skip_white_space();
        if self.eof {
            return true;
        }
        self.read_until_close_angle();
        true
    }

    fn read_start_tag(&mut self) -> Token {
        self.read_tag(true);
        if self.eof {
            return Token::Error;
        }
        let name = &self.buf[self.data.start..self.data.end];
        if let Some(tag) = RAW_TEXT_TAGS
            .iter()
            .find(|tag| name.eq_ignore_ascii_case(tag.as_bytes()))
        {
            self.raw_tag = tag;
        }
        let closes_itself = self.buf[self.raw.end - 2] == b'/'
            && self
                .attrs
                .last()
                .is_none_or(|[_, value]| self.raw.end - 1 != value.end);
        if closes_itself {
            return Token::SelfClosingTag;
        }
        Token::StartTag
    }

    fn read_tag(&mut self, save_attributes: bool) {
        self.attrs.clear();
        self.attr_names.clear();
        self.read_tag_name();
        self.skip_white_space();
        if self.eof {
            return;
        }
        loop {
            let c = self.read_byte();
            if self.eof || c == b'>' {
                break;
            }
            self.raw.end -= 1;
            self.read_tag_attr_key();
            self.read_tag_attr_val();
            let [key, _] = self.pending;
            let name = self.buf[key.start..key.end].to_ascii_lowercase();
            if save_attributes && key.start != key.end && self.attr_names.insert(name) {
                self.attrs.push(self.pending);
            }
            self.skip_white_space();
            if self.eof {
                break;
            }
        }
    }

    fn read_tag_name(&mut self) {
        self.data.start = self.raw.end - 1;
        loop {
            let c = self.read_byte();
            if self.eof {
                self.data.end = self.raw.end;
                return;
            }
            if is_space(c) {
                self.data.end = self.raw.end - 1;
                return;
            }
            if c == b'/' || c == b'>' {
                self.raw.end -= 1;
                self.data.end = self.raw.end;
                return;
            }
        }
    }

    fn read_tag_attr_key(&mut self) {
        self.pending[0].start = self.raw.end;
        loop {
            let c = self.read_byte();
            if self.eof {
                self.pending[0].end = self.raw.end;
                return;
            }
            if c == b'=' && self.pending[0].start + 1 == self.raw.end {
                continue;
            }
            if c == b'=' || is_space(c) || c == b'/' || c == b'>' {
                self.raw.end -= 1;
                self.pending[0].end = self.raw.end;
                return;
            }
        }
    }

    fn read_tag_attr_val(&mut self) {
        self.pending[1] = Span {
            start: self.raw.end,
            end: self.raw.end,
        };
        self.skip_white_space();
        if self.eof {
            return;
        }
        let c = self.read_byte();
        if self.eof || c == b'/' {
            return;
        }
        if c != b'=' {
            self.raw.end -= 1;
            return;
        }
        self.skip_white_space();
        if self.eof {
            return;
        }
        let quote = self.read_byte();
        if self.eof {
            return;
        }
        match quote {
            b'>' => self.raw.end -= 1,
            b'\'' | b'"' => self.read_quoted_value(quote),
            _ => self.read_unquoted_value(),
        }
    }

    fn read_quoted_value(&mut self, quote: u8) {
        self.pending[1].start = self.raw.end;
        loop {
            let c = self.read_byte();
            if self.eof {
                self.pending[1].end = self.raw.end;
                return;
            }
            if c == quote {
                self.pending[1].end = self.raw.end - 1;
                return;
            }
        }
    }

    fn read_unquoted_value(&mut self) {
        self.pending[1].start = self.raw.end - 1;
        loop {
            let c = self.read_byte();
            if self.eof {
                self.pending[1].end = self.raw.end;
                return;
            }
            if is_space(c) {
                self.pending[1].end = self.raw.end - 1;
                return;
            }
            if c == b'>' {
                self.raw.end -= 1;
                self.pending[1].end = self.raw.end;
                return;
            }
        }
    }
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\n' | b'\r' | b'\t' | b'\x0c')
}

fn without_nul(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &c in bytes {
        match c {
            0 => out.extend_from_slice("\u{FFFD}".as_bytes()),
            _ => out.push(c),
        }
    }
    out
}

fn convert_newlines(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut rest = bytes.iter().peekable();
    while let Some(&c) = rest.next() {
        if c != b'\r' {
            out.push(c);
            continue;
        }
        rest.next_if_eq(&&b'\n');
        out.push(b'\n');
    }
    out
}

fn unescape_attribute(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'&' {
            out.push(bytes[index]);
            index += 1;
            continue;
        }
        let Some((text, consumed)) = entity::reference(&bytes[index..]) else {
            out.push(b'&');
            index += 1;
            continue;
        };
        out.extend_from_slice(text.as_bytes());
        index += consumed;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(input: &str) -> Vec<(Token, String)> {
        let mut tokenizer = Tokenizer::new(input.as_bytes());
        let mut found = Vec::new();
        loop {
            let token = tokenizer.next();
            if token == Token::Error {
                return found;
            }
            let raw = &input.as_bytes()[tokenizer.raw.start..tokenizer.raw.end];
            found.push((token, String::from_utf8_lossy(raw).into_owned()));
        }
    }

    #[test]
    fn tokens_split_like_go() {
        let input = "a<!--c--><!DOCTYPE html><p class=x>t</p><br/><script>if (a<b) {}</script><?x>";
        let kinds: Vec<Token> = tokens(input).into_iter().map(|(token, _)| token).collect();
        assert_eq!(
            kinds,
            [
                Token::Text,
                Token::Comment,
                Token::Doctype,
                Token::StartTag,
                Token::Text,
                Token::EndTag,
                Token::SelfClosingTag,
                Token::StartTag,
                Token::Text,
                Token::EndTag,
                Token::Comment,
            ]
        );
    }

    #[test]
    fn attributes_are_lowered_deduplicated_and_unescaped() {
        let mut tokenizer = Tokenizer::new(b"<meta A=1 a=2 B='x&amp;y\r\nz' c=&quot d=&quot=>");
        assert_eq!(tokenizer.next(), Token::StartTag);
        let attributes: Vec<(String, String)> = tokenizer
            .attributes()
            .into_iter()
            .map(|(key, value)| {
                (
                    String::from_utf8_lossy(&key).into_owned(),
                    String::from_utf8_lossy(&value).into_owned(),
                )
            })
            .collect();
        let pairs = |list: &[(&str, &str)]| {
            list.iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            attributes,
            pairs(&[("a", "1"), ("b", "x&y\nz"), ("c", "\""), ("d", "&quot=")])
        );
    }
}
