use std::collections::BTreeMap;
use std::io;
use std::sync::LazyLock;

use zeroize::Zeroizing;

use super::{ConfigError, ConfigProblem, FIELDS, Source};
use crate::gotext::{atoi, go_quote, trim};
use crate::password::trim_final_newline;
use crate::secret::Secret;

static DEFAULTS: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
    FIELDS
        .iter()
        .filter_map(|field| {
            let default = field.default.as_ref()?.as_str()?;
            Some((field.env.clone()?, default.to_owned()))
        })
        .collect()
});

pub(crate) struct Reader<'a> {
    source: &'a Source,
    problems: Vec<ConfigProblem>,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(source: &'a Source) -> Reader<'a> {
        Reader {
            source,
            problems: Vec::new(),
        }
    }

    pub(crate) fn problem(&mut self, path: &str, text: String) {
        self.problems.push(ConfigProblem {
            path: path.to_owned(),
            text,
        });
    }

    pub(crate) fn finish<T>(self, value: T) -> Result<T, ConfigError> {
        if self.problems.is_empty() {
            return Ok(value);
        }
        Err(ConfigError {
            problems: self.problems,
        })
    }

    pub(crate) fn source(&self) -> &'a Source {
        self.source
    }

    pub(crate) fn lookup(&self, name: &str) -> Option<String> {
        (self.source.lookup)(name)
    }

    pub(crate) fn text(&self, name: &str) -> String {
        let value = self
            .lookup(name)
            .map(|value| trim(&value).to_owned())
            .unwrap_or_default();
        if !value.is_empty() {
            return value;
        }
        DEFAULTS.get(name).cloned().unwrap_or_default()
    }

    pub(crate) fn required(&mut self, name: &str, hint: &str) -> String {
        let value = self.text(name);
        if value.is_empty() {
            self.problem(
                &name.to_ascii_lowercase(),
                format!("{name} is required{hint}"),
            );
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
            self.problem(
                &name.to_ascii_lowercase(),
                format!("set either {name} or {name}_FILE, not both"),
            );
            return Secret::default();
        }
        if path.is_empty() {
            return direct;
        }
        self.secret_file(name, &path)
    }

    fn direct_secret(&self, name: &str) -> Secret {
        let value = Secret::new(self.lookup(name).unwrap_or_default());
        if trim(value.reveal()).is_empty() {
            return Secret::default();
        }
        value
    }

    fn secret_file(&mut self, name: &str, path: &str) -> Secret {
        let field = format!("{name}_file").to_ascii_lowercase();
        let content = match (self.source.read_file)(path) {
            Ok(content) => Zeroizing::new(content),
            Err(err) => {
                let text = format!("{name}_FILE: {}", describe_file_error(path, &err));
                self.problem(&field, text);
                return Secret::default();
            }
        };
        let Ok(text) = std::str::from_utf8(&content) else {
            self.problem(&field, format!("{name}_FILE: {path} is not valid UTF-8"));
            return Secret::default();
        };
        let value = trim_final_newline(text);
        if value.is_empty() {
            self.problem(&field, format!("{name}_FILE: {path} is empty"));
        }
        Secret::new(value.to_owned())
    }

    /// With a problem it gives the declared default, which `retention_days` still reads.
    pub(crate) fn integer(&mut self, name: &str, low: i64, high: i64) -> i64 {
        let value = self.text(name);
        if let Some(number) = atoi(&value).filter(|number| (low..=high).contains(number)) {
            return number;
        }
        self.problem(
            &name.to_ascii_lowercase(),
            format!(
                "{name}: use a whole number from {low} to {high} (got {})",
                go_quote(&value)
            ),
        );
        DEFAULTS
            .get(name)
            .and_then(|default| atoi(default))
            .unwrap_or_default()
    }
}

fn describe_file_error(path: &str, err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::NotFound => format!("{path} does not exist"),
        io::ErrorKind::PermissionDenied => format!("no permission to read {path}"),
        _ => format!("could not read {path}: {err}"),
    }
}
