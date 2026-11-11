use std::io;

use zeroize::Zeroizing;

use super::{ConfigError, Source};
use crate::gotext::{atoi, go_quote, trim};
use crate::password::trim_final_newline;
use crate::secret::Secret;

pub(crate) struct Reader<'a> {
    source: &'a Source,
    problems: Vec<String>,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(source: &'a Source) -> Reader<'a> {
        Reader {
            source,
            problems: Vec::new(),
        }
    }

    pub(crate) fn problem(&mut self, problem: String) {
        self.problems.push(problem);
    }

    pub(crate) fn finish<T>(self, value: T) -> Result<T, ConfigError> {
        if self.problems.is_empty() {
            return Ok(value);
        }
        Err(ConfigError {
            problems: self.problems,
        })
    }

    pub(crate) fn lookup(&self, name: &str) -> Option<String> {
        (self.source.lookup)(name)
    }

    pub(crate) fn text(&self, name: &str) -> String {
        self.lookup(name)
            .map(|value| trim(&value).to_owned())
            .unwrap_or_default()
    }

    pub(crate) fn required(&mut self, name: &str, hint: &str) -> String {
        let value = self.text(name);
        if value.is_empty() {
            self.problem(format!("{name} is required{hint}"));
        }
        value
    }

    pub(crate) fn has_secret(&self, name: &str) -> bool {
        self.lookup(name)
            .map(Zeroizing::new)
            .is_some_and(|value| !trim(&value).is_empty())
    }

    pub(crate) fn secret(&mut self, name: &str) -> Secret {
        let direct = self.direct_secret(name);
        let path = self.text(&format!("{name}_FILE"));
        if !direct.is_empty() && !path.is_empty() {
            self.problem(format!("set either {name} or {name}_FILE, not both"));
            return Secret::default();
        }
        if path.is_empty() {
            return self.utf8_direct(name, direct);
        }
        self.secret_file(name, &path)
    }

    fn utf8_direct(&mut self, name: &str, direct: Secret) -> Secret {
        if direct.is_empty() || !(self.source.invalid_utf8)(name) {
            return direct;
        }
        self.problem(format!("{name}: the value is not valid UTF-8"));
        Secret::default()
    }

    fn direct_secret(&self, name: &str) -> Secret {
        let value = Secret::new(self.lookup(name).unwrap_or_default());
        if trim(value.reveal()).is_empty() {
            return Secret::default();
        }
        value
    }

    fn secret_file(&mut self, name: &str, path: &str) -> Secret {
        let content = match (self.source.read_file)(path) {
            Ok(content) => Zeroizing::new(content),
            Err(err) => {
                self.problem(format!("{name}_FILE: {}", describe_file_error(path, &err)));
                return Secret::default();
            }
        };
        let Ok(text) = std::str::from_utf8(&content) else {
            self.problem(format!("{name}_FILE: {path} is not valid UTF-8"));
            return Secret::default();
        };
        let value = trim_final_newline(text);
        if value.is_empty() {
            self.problem(format!("{name}_FILE: {path} is empty"));
        }
        Secret::new(value.to_owned())
    }

    pub(crate) fn integer(&mut self, name: &str, fallback: i64, low: i64, high: i64) -> i64 {
        let value = self.text(name);
        if value.is_empty() {
            return fallback;
        }
        if let Some(number) = atoi(&value).filter(|number| (low..=high).contains(number)) {
            return number;
        }
        self.problem(format!(
            "{name}: use a whole number from {low} to {high} (got {})",
            go_quote(&value)
        ));
        fallback
    }
}

fn describe_file_error(path: &str, err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::NotFound => format!("{path} does not exist"),
        io::ErrorKind::PermissionDenied => format!("no permission to read {path}"),
        _ => format!("could not read {path}: {err}"),
    }
}
