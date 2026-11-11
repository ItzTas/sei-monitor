mod lookpath;
mod reader;
#[cfg(test)]
pub(crate) mod tests;
mod url;

use std::ffi::OsString;
use std::fs;
use std::future::{Future, ready};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono_tz::Tz;

use crate::duration::{format_go_duration, parse_go_duration};
use crate::gotext::{atoi, equal_fold, go_quote, to_lower, to_upper, trim};
use crate::password::{PasswordCommand, PasswordCommandError, PasswordSource};
use crate::secret::Secret;
use crate::timezone::load_zone;
use lookpath::look_path;
use reader::Reader;
pub(crate) use url::path_escape;
use url::{normalize_api_url, normalize_sei_url};

pub const DEFAULT_TIME_ZONE: &str = "America/Sao_Paulo";
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(15 * 60);
pub const MIN_POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);

type Lookup = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;
type ReadFile = Box<dyn Fn(&str) -> std::io::Result<Vec<u8>> + Send + Sync>;
type InvalidUtf8 = Box<dyn Fn(&str) -> bool + Send + Sync>;

pub struct Source {
    pub lookup: Lookup,
    pub read_file: ReadFile,
    pub invalid_utf8: InvalidUtf8,
}

impl Source {
    pub fn environment() -> Source {
        Source {
            lookup: Box::new(|name| lossy(std::env::var_os(name))),
            read_file: Box::new(|path| fs::read(path)),
            invalid_utf8: Box::new(|name| not_utf8(std::env::var_os(name))),
        }
    }
}

fn lossy(value: Option<OsString>) -> Option<String> {
    value.map(|value| value.to_string_lossy().into_owned())
}

fn not_utf8(value: Option<OsString>) -> bool {
    value.is_some_and(|value| value.to_str().is_none())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid configuration:\n  - {}", .problems.join("\n  - "))]
pub struct ConfigError {
    pub problems: Vec<String>,
}

pub enum PasswordSetting {
    Static(Secret),
    Command(PasswordCommand),
}

impl PasswordSource for PasswordSetting {
    fn password(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Secret, PasswordCommandError>> + Send + '_>> {
        match self {
            PasswordSetting::Static(secret) => Box::pin(ready(Ok(secret.clone()))),
            PasswordSetting::Command(command) => command.password(),
        }
    }
}

pub struct SeiConfig {
    pub url: String,
    pub api_url: String,
    pub username: String,
    pub password: Arc<PasswordSetting>,
    pub org: String,
    pub units: Vec<String>,
    pub all_units: bool,
    pub zone: Tz,
    pub concurrency: usize,
}

impl SeiConfig {
    pub fn load(source: &Source) -> Result<SeiConfig, ConfigError> {
        let mut reader = Reader::new(source);
        let sei = read_sei(&mut reader);
        reader.finish(sei)
    }
}

pub struct PluginConfig {
    pub sei: SeiConfig,
    pub database_url: Secret,
    pub poll_interval: Duration,
    pub return_warning_days: i64,
    pub retention_days: i64,
    pub full_detail: bool,
    pub log_level: i64,
}

impl PluginConfig {
    pub fn load(source: &Source) -> Result<PluginConfig, ConfigError> {
        let mut reader = Reader::new(source);
        let full_detail = detail_level(&mut reader, "NOTIFICATION_DETAIL", false);
        let return_warning_days = reader.integer("RETURN_WARNING_DAYS", 2, 0, 60);
        let sei = read_sei(&mut reader);
        let database_url = database_url(&mut reader);
        let poll_interval = poll_interval(&mut reader);
        let retention_days = retention_days(&mut reader, return_warning_days);
        let log_level = log_level(&mut reader);
        reader.finish(PluginConfig {
            sei,
            database_url,
            poll_interval,
            return_warning_days,
            retention_days,
            full_detail,
            log_level,
        })
    }

    pub fn log_filter(&self) -> &'static str {
        const FILTERS: [(i64, &str); 3] = [(-4, "debug"), (0, "info"), (4, "warn")];
        FILTERS
            .iter()
            .find(|(limit, _)| self.log_level <= *limit)
            .map_or("error", |(_, filter)| filter)
    }
}

pub fn load_sei_url(source: &Source) -> Result<String, ConfigError> {
    let mut reader = Reader::new(source);
    let url = sei_url(&mut reader);
    reader.finish(url)
}

fn read_sei(reader: &mut Reader) -> SeiConfig {
    let url = sei_url(reader);
    let api_url = api_url(reader);
    let username = reader.required("SEI_USERNAME", "");
    let password = Arc::new(sei_password(reader));
    let org = reader.required(
        "SEI_ORG",
        " (acronym or numeric id; the orgs subcommand lists them)",
    );
    let zone = zone(reader);
    let concurrency = reader.integer("SEI_CONCURRENCY", 3, 1, 10);
    let (units, all_units) = units(&reader.text("SEI_UNITS"));
    SeiConfig {
        url,
        api_url,
        username,
        password,
        org,
        units,
        all_units,
        zone,
        concurrency: usize::try_from(concurrency).unwrap_or(3),
    }
}

fn sei_url(reader: &mut Reader) -> String {
    let value = reader.required("SEI_URL", " (the SEI address, e.g. https://sei.rr.gov.br)");
    if value.is_empty() {
        return value;
    }
    normalize_sei_url(&value).unwrap_or_else(|problem| {
        reader.problem(problem);
        String::new()
    })
}

fn api_url(reader: &mut Reader) -> String {
    let value = reader.text("SEI_API_URL");
    if value.is_empty() {
        return value;
    }
    normalize_api_url(&value).unwrap_or_else(|problem| {
        reader.problem(problem);
        String::new()
    })
}

fn sei_password(reader: &mut Reader) -> PasswordSetting {
    let command_set = !reader.text("SEI_PASSWORD_CMD").is_empty();
    let options = [
        reader.has_secret("SEI_PASSWORD"),
        !reader.text("SEI_PASSWORD_FILE").is_empty(),
        command_set,
    ];
    let problem = match options.iter().filter(|set| **set).count() {
        0 => "SEI_PASSWORD is required (or SEI_PASSWORD_FILE, or SEI_PASSWORD_CMD)",
        1 => return chosen_password(reader, command_set),
        _ => "set only one of SEI_PASSWORD, SEI_PASSWORD_FILE and SEI_PASSWORD_CMD",
    };
    reader.problem(problem.to_owned());
    PasswordSetting::Static(Secret::default())
}

fn chosen_password(reader: &mut Reader, command_set: bool) -> PasswordSetting {
    if !command_set {
        return PasswordSetting::Static(reader.secret("SEI_PASSWORD"));
    }
    let command = match PasswordCommand::new(&reader.text("SEI_PASSWORD_CMD")) {
        Ok(command) => command,
        Err(problem) => {
            reader.problem(problem);
            return PasswordSetting::Static(Secret::default());
        }
    };
    if !look_path(command.program(), reader.lookup("PATH").as_deref()) {
        reader.problem(format!(
            "SEI_PASSWORD_CMD: program not found: {}",
            command.program()
        ));
    }
    PasswordSetting::Command(command)
}

fn zone(reader: &mut Reader) -> Tz {
    let text = reader.text("SEI_TIMEZONE");
    let name = if text.is_empty() {
        DEFAULT_TIME_ZONE
    } else {
        &text
    };
    load_zone(name).unwrap_or_else(|| {
        reader.problem(format!(
            "SEI_TIMEZONE: unknown time zone {} (e.g. America/Boa_Vista)",
            go_quote(name)
        ));
        Tz::UTC
    })
}

fn units(value: &str) -> (Vec<String>, bool) {
    if equal_fold(value, "all") {
        return (Vec::new(), true);
    }
    let mut units: Vec<String> = Vec::new();
    for part in value.split(',').map(trim) {
        if part.is_empty() || units.iter().any(|unit| equal_fold(unit, part)) {
            continue;
        }
        units.push(part.to_owned());
    }
    (units, false)
}

fn database_url(reader: &mut Reader) -> Secret {
    let value = reader.secret("DATABASE_URL");
    if value.is_empty() {
        reader.problem(
            "DATABASE_URL: set the Postgres address, postgres://user:password@host/database"
                .to_owned(),
        );
        return value;
    }
    let valid = ["postgres://", "postgresql://"].iter().any(|scheme| {
        value
            .reveal()
            .strip_prefix(scheme)
            .is_some_and(|rest| !rest.is_empty())
    });
    if !valid {
        reader.problem("DATABASE_URL: use postgres://user:password@host/database".to_owned());
    }
    value
}

fn poll_interval(reader: &mut Reader) -> Duration {
    let value = reader.text("POLL_INTERVAL");
    if value.is_empty() {
        return DEFAULT_POLL_INTERVAL;
    }
    let Some(nanos) = parse_go_duration(&value) else {
        reader.problem(format!(
            "POLL_INTERVAL: use a duration such as 15m or 1h (got {})",
            go_quote(&value)
        ));
        return DEFAULT_POLL_INTERVAL;
    };
    let minimum = nanos_of(MIN_POLL_INTERVAL);
    if nanos < minimum {
        reader.problem(format!(
            "POLL_INTERVAL: the minimum is {}, to keep the load on the agency's SEI low (got {value})",
            format_go_duration(minimum)
        ));
        return MIN_POLL_INTERVAL;
    }
    Duration::from_nanos(nanos.unsigned_abs())
}

fn nanos_of(duration: Duration) -> i64 {
    i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX)
}

fn retention_days(reader: &mut Reader, return_warning_days: i64) -> i64 {
    let days = reader.integer("RETENTION_DAYS", 90, 0, 3650);
    let minimum = return_warning_days + 2;
    if days == 0 || days >= minimum {
        return days;
    }
    reader.problem(format!(
        "RETENTION_DAYS: keep events for at least {minimum} days, RETURN_WARNING_DAYS plus 2, or use 0 to keep them forever (got {days})"
    ));
    days
}

fn detail_level(reader: &mut Reader, name: &str, fallback: bool) -> bool {
    let value = to_lower(&reader.text(name));
    if value.is_empty() {
        return fallback;
    }
    const LEVELS: [(&str, bool); 2] = [("minimal", false), ("full", true)];
    if let Some((_, full)) = LEVELS.iter().find(|(level, _)| *level == value) {
        return *full;
    }
    reader.problem(format!(
        "{name}: use minimal or full (got {})",
        go_quote(&value)
    ));
    fallback
}

fn log_level(reader: &mut Reader) -> i64 {
    let text = reader.text("LOG_LEVEL");
    let value = if text.is_empty() { "info" } else { &text };
    parse_level(value).unwrap_or_else(|| {
        reader.problem(format!(
            "LOG_LEVEL: use debug, info, warn or error (got {})",
            go_quote(value)
        ));
        0
    })
}

fn parse_level(value: &str) -> Option<i64> {
    const NAMES: [(&str, i64); 4] = [("DEBUG", -4), ("INFO", 0), ("WARN", 4), ("ERROR", 8)];
    let (name, offset) = match value.find(['+', '-']) {
        Some(sign) => (&value[..sign], atoi(&value[sign..])?),
        None => (value, 0),
    };
    let name = to_upper(name);
    NAMES
        .iter()
        .find(|(level, _)| *level == name)
        .map(|(_, level)| level.wrapping_add(offset))
}
