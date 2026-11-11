use std::ffi::OsString;
use std::future::{Future, ready};
use std::io;
use std::mem;
use std::pin::{Pin, pin};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::time::{Instant, sleep_until, timeout};
use zeroize::Zeroizing;

use crate::duration::format_go_duration;
use crate::gotext::to_upper;
use crate::secret::Secret;

pub const PASSWORD_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

const WAIT_DELAY: Duration = Duration::from_secs(1);
const WAIT_DELAY_EXPIRED: &str = "exec: WaitDelay expired before I/O complete";
const READ_CHUNK: usize = 4096;

pub trait PasswordSource: Send + Sync {
    fn password(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Secret, PasswordCommandError>> + Send + '_>>;
}

pub struct StaticPassword(pub Secret);

impl PasswordSource for StaticPassword {
    fn password(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Secret, PasswordCommandError>> + Send + '_>> {
        Box::pin(ready(Ok(self.0.clone())))
    }
}

pub struct PasswordCommand {
    args: Vec<String>,
    timeout: Duration,
    environ: fn() -> Vec<(OsString, OsString)>,
}

impl PasswordCommand {
    pub fn new(line: &str) -> Result<PasswordCommand, String> {
        Ok(PasswordCommand {
            args: split_command(line)?,
            timeout: PASSWORD_COMMAND_TIMEOUT,
            environ: process_environ,
        })
    }

    pub fn with_timeout(self, timeout: Duration) -> PasswordCommand {
        PasswordCommand { timeout, ..self }
    }

    pub fn program(&self) -> &str {
        &self.args[0]
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }

    async fn run(&self) -> Result<Secret, PasswordCommandError> {
        let deadline = Instant::now() + self.timeout;
        let mut child = self
            .command()
            .spawn()
            .map_err(|err| self.error(CommandFailure::Start(err.to_string()), -1, 0))?;
        let mut stdout = Zeroizing::new(Vec::new());
        let mut stderr_bytes = 0;
        let pipes = read_pipes(
            child.stdout.take(),
            child.stderr.take(),
            &mut stdout,
            &mut stderr_bytes,
        );
        let ending = supervise(&mut child, pipes, deadline).await;
        self.conclude(ending, deadline, &stdout, stderr_bytes)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(self.program());
        command
            .args(&self.args[1..])
            .env_clear()
            .envs(without_secrets((self.environ)()))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }

    fn conclude(
        &self,
        ending: Ending,
        deadline: Instant,
        stdout: &[u8],
        stderr_bytes: usize,
    ) -> Result<Secret, PasswordCommandError> {
        let fail = |failure, exit_code| Err(self.error(failure, exit_code, stderr_bytes));
        let Some(status) = ending.status.filter(|_| Instant::now() < deadline) else {
            return fail(CommandFailure::TimedOut(self.timeout), -1);
        };
        let status = match status {
            Ok(status) => status,
            Err(err) => return fail(CommandFailure::Start(err.to_string()), -1),
        };
        if !status.success() {
            return fail(CommandFailure::Exit, status.code().unwrap_or(-1));
        }
        match ending.pipes {
            None => return fail(CommandFailure::Start(WAIT_DELAY_EXPIRED.to_owned()), -1),
            Some(Err(err)) => return fail(CommandFailure::Start(err.to_string()), -1),
            Some(Ok(())) => {}
        }
        let Ok(text) = std::str::from_utf8(stdout) else {
            return fail(CommandFailure::NotUtf8, 0);
        };
        let password = Secret::new(trim_final_newline(text).to_owned());
        if password.is_empty() {
            return fail(CommandFailure::Empty, 0);
        }
        Ok(password)
    }

    fn error(
        &self,
        failure: CommandFailure,
        exit_code: i32,
        stderr_bytes: usize,
    ) -> PasswordCommandError {
        PasswordCommandError {
            program: self.program().to_owned(),
            exit_code,
            stderr_bytes,
            failure,
        }
    }
}

impl PasswordSource for PasswordCommand {
    fn password(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Secret, PasswordCommandError>> + Send + '_>> {
        Box::pin(self.run())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("SEI_PASSWORD_CMD: {}{}", self.reason(), self.stderr_note())]
pub struct PasswordCommandError {
    pub program: String,
    pub exit_code: i32,
    pub stderr_bytes: usize,
    pub failure: CommandFailure,
}

impl PasswordCommandError {
    fn reason(&self) -> String {
        let program = &self.program;
        match &self.failure {
            CommandFailure::TimedOut(timeout) => {
                let nanos = i64::try_from(timeout.as_nanos()).unwrap_or(i64::MAX);
                format!(
                    "{program} did not finish within {}",
                    format_go_duration(nanos)
                )
            }
            CommandFailure::Start(err) => format!("could not run {program}: {err}"),
            CommandFailure::Empty => format!("{program} printed nothing"),
            CommandFailure::NotUtf8 => {
                format!("{program} printed bytes that are not valid UTF-8")
            }
            CommandFailure::Exit => format!("{program} exited with code {}", self.exit_code),
        }
    }

    fn stderr_note(&self) -> String {
        if self.stderr_bytes == 0 {
            return String::new();
        }
        format!(" ({} bytes of stderr, not shown)", self.stderr_bytes)
    }
}

#[derive(Debug)]
pub enum CommandFailure {
    TimedOut(Duration),
    Start(String),
    Empty,
    NotUtf8,
    Exit,
}

struct Ending {
    status: Option<io::Result<ExitStatus>>,
    pipes: Option<io::Result<()>>,
}

async fn supervise(
    child: &mut Child,
    pipes: impl Future<Output = io::Result<()>>,
    deadline: Instant,
) -> Ending {
    let mut pipes = pin!(pipes);
    let mut drained = None;
    let status = loop {
        tokio::select! {
            result = &mut pipes, if drained.is_none() => drained = Some(result),
            status = child.wait() => break Some(status),
            () = sleep_until(deadline) => break None,
        }
    };
    if status.is_none() {
        let _ = child.start_kill();
    }
    if drained.is_none() {
        drained = timeout(WAIT_DELAY, pipes).await.ok();
    }
    Ending {
        status,
        pipes: drained,
    }
}

async fn read_pipes(
    stdout: Option<impl AsyncRead + Unpin>,
    stderr: Option<impl AsyncRead + Unpin>,
    password: &mut Zeroizing<Vec<u8>>,
    stderr_bytes: &mut usize,
) -> io::Result<()> {
    let (stdout, stderr) = tokio::join!(
        drain(stdout, |bytes| append(password, bytes)),
        drain(stderr, |bytes| *stderr_bytes += bytes.len()),
    );
    stdout.and(stderr)
}

async fn drain(
    pipe: Option<impl AsyncRead + Unpin>,
    mut sink: impl FnMut(&[u8]),
) -> io::Result<()> {
    let Some(mut pipe) = pipe else {
        return Ok(());
    };
    let mut chunk = Zeroizing::new([0; READ_CHUNK]);
    loop {
        let read = pipe.read(&mut chunk[..]).await?;
        if read == 0 {
            return Ok(());
        }
        sink(&chunk[..read]);
    }
}

fn append(buffer: &mut Zeroizing<Vec<u8>>, bytes: &[u8]) {
    let needed = buffer.len() + bytes.len();
    if needed > buffer.capacity() {
        let mut grown = Zeroizing::new(Vec::with_capacity(needed.max(buffer.capacity() * 2)));
        grown.extend_from_slice(buffer);
        *buffer = grown;
    }
    buffer.extend_from_slice(bytes);
}

fn process_environ() -> Vec<(OsString, OsString)> {
    std::env::vars_os().collect()
}

fn without_secrets(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
) -> impl Iterator<Item = (OsString, OsString)> {
    vars.into_iter()
        .filter(|(name, _)| !name.to_str().is_some_and(is_secret_name))
}

pub(crate) fn is_secret_name(name: &str) -> bool {
    const SECRETS: [&str; 9] = [
        "SEI_PASSWORD",
        "FEED_TOKEN",
        "API_TOKEN",
        "SMTP_PASSWORD",
        "TELEGRAM_TOKEN",
        "NTFY_TOKEN",
        "WEBHOOK_SECRET",
        "POSTGRES_PASSWORD",
        "DATABASE_URL",
    ];
    let upper = to_upper(name);
    let base = upper.strip_suffix("_FILE").unwrap_or(&upper);
    let base = base.strip_suffix("_CMD").unwrap_or(base);
    SECRETS.contains(&base)
}

pub(crate) fn trim_final_newline(text: &str) -> &str {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text)
}

pub(crate) fn split_command(line: &str) -> Result<Vec<String>, String> {
    let mut splitter = Splitter::default();
    for c in line.chars() {
        splitter.feed(c);
    }
    splitter.finish()
}

#[derive(Default)]
struct Splitter {
    args: Vec<String>,
    current: String,
    started: bool,
    quote: Option<char>,
    escaped: bool,
}

impl Splitter {
    fn feed(&mut self, c: char) {
        match c {
            _ if self.escaped => {
                self.current.push(c);
                self.escaped = false;
            }
            '\'' if self.quote == Some('\'') => self.quote = None,
            _ if self.quote == Some('\'') => self.current.push(c),
            '"' if self.quote == Some('"') => self.quote = None,
            '\\' => {
                self.escaped = true;
                self.started = true;
            }
            _ if self.quote == Some('"') => self.current.push(c),
            '\'' | '"' => {
                self.quote = Some(c);
                self.started = true;
            }
            _ if c.is_whitespace() => self.end_word(),
            _ => {
                self.current.push(c);
                self.started = true;
            }
        }
    }

    fn end_word(&mut self) {
        if !self.started {
            return;
        }
        self.args.push(mem::take(&mut self.current));
        self.started = false;
    }

    fn finish(mut self) -> Result<Vec<String>, String> {
        if self.quote.is_some() || self.escaped {
            return Err("SEI_PASSWORD_CMD: unterminated quote or trailing backslash".to_owned());
        }
        self.end_word();
        if self.args.is_empty() {
            return Err("SEI_PASSWORD_CMD is empty".to_owned());
        }
        Ok(self.args)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;
    use std::process::Stdio;

    use serde::Deserialize;
    use tokio::time::Instant;

    use super::*;
    use crate::golden;

    fn require_sh() -> bool {
        let found = std::env::var_os("PATH")
            .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("sh").is_file()));
        if found {
            return true;
        }
        if std::env::var_os("CI").is_some_and(|ci| !ci.is_empty()) {
            panic!("sh is not on PATH, and CI is set");
        }
        eprintln!("skipping: sh is not on PATH");
        false
    }

    fn command(line: &str) -> PasswordCommand {
        PasswordCommand::new(line).unwrap()
    }

    async fn failure(command: PasswordCommand) -> PasswordCommandError {
        command.password().await.unwrap_err()
    }

    fn scratch_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("sei-monitor-test-{}-{name}", std::process::id()))
    }

    fn read_pid(path: &PathBuf) -> String {
        let pid = fs::read_to_string(path).unwrap().trim().to_owned();
        fs::remove_file(path).unwrap();
        pid
    }

    async fn process_exists(pid: &str) -> bool {
        tokio::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(Stdio::null())
            .status()
            .await
            .unwrap()
            .success()
    }

    async fn kill(pid: &str) {
        tokio::process::Command::new("kill")
            .arg(pid)
            .stderr(Stdio::null())
            .status()
            .await
            .unwrap();
    }

    async fn gone_within(pid: &str, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if !process_exists(pid).await {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        !process_exists(pid).await
    }

    fn environ_with_secrets() -> Vec<(OsString, OsString)> {
        let mut vars: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        vars.extend(
            [
                ("SEI_PASSWORD", "hunter2"),
                ("DATABASE_URL_FILE", "/run/secrets/database-url"),
                ("API_TOKEN_CMD", "pass show api-token"),
                ("KEEP_ME", "kept"),
            ]
            .map(|(name, value)| (name.into(), value.into())),
        );
        vars
    }

    #[tokio::test]
    async fn static_password_returns_its_secret() {
        let source = StaticPassword(Secret::new("hunter2".to_owned()));

        assert_eq!(source.password().await.unwrap().reveal(), "hunter2");
        assert_eq!(source.password().await.unwrap().reveal(), "hunter2");
    }

    #[test]
    fn password_command_keeps_the_whole_command_line() {
        let command = command("pass show \"sei/my pass\"");

        assert_eq!(command.program(), "pass");
        assert_eq!(command.args(), ["pass", "show", "sei/my pass"]);
        assert_eq!(command.timeout, PASSWORD_COMMAND_TIMEOUT);
        assert_eq!(
            PasswordCommand::new("cmd 'open").err().unwrap(),
            "SEI_PASSWORD_CMD: unterminated quote or trailing backslash"
        );
    }

    #[tokio::test]
    async fn password_command_uses_the_whole_stdout_minus_the_final_newline() {
        if !require_sh() {
            return;
        }
        let cases = [
            (r"printf 'a b\n\n'", "a b\n"),
            (r"printf 'x\r\n'", "x"),
            ("printf p4ss", "p4ss"),
            ("printf 'pass\u{e9}'", "pass\u{e9}"),
        ];
        for (line, want) in cases {
            let secret = command(line).password().await.unwrap();
            assert_eq!(secret.reveal(), want, "{line}");
        }
    }

    #[tokio::test]
    async fn password_command_failures() {
        if !require_sh() {
            return;
        }
        let cases = [
            ("sh -c 'exit 3'", "SEI_PASSWORD_CMD: sh exited with code 3"),
            (
                "sh -c 'echo oops >&2; exit 1'",
                "SEI_PASSWORD_CMD: sh exited with code 1 (5 bytes of stderr, not shown)",
            ),
            (
                "sh -c 'printf stdout-secret; printf stderr-hint >&2; exit 3'",
                "SEI_PASSWORD_CMD: sh exited with code 3 (11 bytes of stderr, not shown)",
            ),
            (
                "sh -c 'kill -9 $$'",
                "SEI_PASSWORD_CMD: sh exited with code -1",
            ),
            ("true", "SEI_PASSWORD_CMD: true printed nothing"),
            (
                r"printf 'out-secret\377\n'",
                "SEI_PASSWORD_CMD: printf printed bytes that are not valid UTF-8",
            ),
        ];
        for (line, want) in cases {
            let err = failure(command(line)).await;
            let debug = format!("{err:?}");
            assert_eq!(err.to_string(), want, "{line}");
            for leaked in ["oops", "stdout-secret", "stderr-hint", "out-secret"] {
                assert!(!debug.contains(leaked), "{debug}");
            }
        }

        let err = failure(command("true")).await;
        assert!(matches!(err.failure, CommandFailure::Empty));
        assert_eq!((err.exit_code, err.stderr_bytes), (0, 0));

        let err = failure(command(r"printf '\377'")).await;
        assert!(matches!(err.failure, CommandFailure::NotUtf8));
        assert_eq!((err.exit_code, err.stderr_bytes), (0, 0));

        let err = failure(command("sh -c 'echo oops >&2; exit 1'")).await;
        assert!(matches!(err.failure, CommandFailure::Exit));
        assert_eq!((err.exit_code, err.stderr_bytes), (1, 5));

        let started = Instant::now();
        let err = failure(command("sleep 5").with_timeout(Duration::from_millis(200))).await;
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(
            err.to_string(),
            "SEI_PASSWORD_CMD: sleep did not finish within 200ms"
        );
        assert!(
            matches!(err.failure, CommandFailure::TimedOut(timeout) if timeout == Duration::from_millis(200))
        );
        assert_eq!(err.exit_code, -1);

        let err = failure(
            command("sh -c 'echo oops >&2; exec sleep 5'").with_timeout(Duration::from_millis(200)),
        )
        .await;
        assert_eq!(
            err.to_string(),
            "SEI_PASSWORD_CMD: sh did not finish within 200ms (5 bytes of stderr, not shown)"
        );

        let err = failure(command("no-such-program-sei-monitor")).await;
        assert!(
            err.to_string()
                .starts_with("SEI_PASSWORD_CMD: could not run no-such-program-sei-monitor: "),
            "{err}"
        );
        assert!(matches!(err.failure, CommandFailure::Start(_)));
        assert_eq!((err.exit_code, err.stderr_bytes), (-1, 0));
    }

    #[test]
    fn timeouts_are_written_like_go() {
        let timed_out = |timeout| PasswordCommandError {
            program: "pass".to_owned(),
            exit_code: -1,
            stderr_bytes: 0,
            failure: CommandFailure::TimedOut(timeout),
        };
        let cases = [
            (Duration::from_secs(30), "30s"),
            (Duration::from_millis(200), "200ms"),
            (Duration::from_secs(90), "1m30s"),
            (Duration::from_millis(1500), "1.5s"),
            (Duration::from_secs(7200), "2h0m0s"),
            (Duration::from_nanos(1500), "1.5µs"),
            (Duration::from_nanos(1), "1ns"),
            (Duration::from_millis(1001), "1.001s"),
            (Duration::from_micros(3_660_000_500), "1h1m0.0005s"),
            (Duration::ZERO, "0s"),
        ];
        for (duration, want) in cases {
            assert_eq!(
                timed_out(duration).to_string(),
                format!("SEI_PASSWORD_CMD: pass did not finish within {want}"),
                "{duration:?}"
            );
        }
        assert_eq!(
            timed_out(PASSWORD_COMMAND_TIMEOUT).to_string(),
            "SEI_PASSWORD_CMD: pass did not finish within 30s"
        );
    }

    #[tokio::test]
    async fn password_command_is_killed_when_the_caller_gives_up() {
        if !require_sh() {
            return;
        }
        let pid_file = scratch_file("caller-gives-up");
        let line = format!("sh -c 'echo $$ > {}; exec sleep 30'", pid_file.display());
        let command = command(&line);

        let outcome = tokio::time::timeout(Duration::from_millis(300), command.password()).await;

        assert!(outcome.is_err());
        let pid = read_pid(&pid_file);
        assert!(
            gone_within(&pid, Duration::from_secs(1)).await,
            "pid {pid} still exists"
        );
    }

    #[tokio::test]
    async fn password_command_gives_a_grandchild_holding_stdout_one_second() {
        if !require_sh() {
            return;
        }
        let pid_file = scratch_file("grandchild-after-exit");
        let line = format!(
            "sh -c 'sleep 30 & echo $! > {}; echo x'",
            pid_file.display()
        );

        let started = Instant::now();
        let err = failure(command(&line)).await;
        let elapsed = started.elapsed();

        kill(&read_pid(&pid_file)).await;
        assert_eq!(
            err.to_string(),
            "SEI_PASSWORD_CMD: could not run sh: exec: WaitDelay expired before I/O complete"
        );
        assert!(
            elapsed >= WAIT_DELAY && elapsed < Duration::from_secs(3),
            "{elapsed:?}"
        );
    }

    #[tokio::test]
    async fn password_command_times_out_while_a_grandchild_holds_stdout() {
        if !require_sh() {
            return;
        }
        let pid_file = scratch_file("grandchild-timeout");
        let line = format!(
            "sh -c 'sleep 30 & echo $! > {}; exec sleep 30'",
            pid_file.display()
        );
        let command = command(&line).with_timeout(Duration::from_millis(200));

        let started = Instant::now();
        let err = failure(command).await;
        let elapsed = started.elapsed();

        kill(&read_pid(&pid_file)).await;
        assert_eq!(
            err.to_string(),
            "SEI_PASSWORD_CMD: sh did not finish within 200ms"
        );
        assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    }

    #[tokio::test]
    async fn password_command_environment_has_no_secrets() {
        if !require_sh() {
            return;
        }
        let command = PasswordCommand {
            environ: environ_with_secrets,
            ..command("sh -c 'env'")
        };

        let listing = command.password().await.unwrap();

        let names: Vec<&str> = listing
            .reveal()
            .lines()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name))
            .collect();
        assert!(names.contains(&"KEEP_ME"), "{names:?}");
        assert!(names.contains(&"PATH"), "{names:?}");
        for secret in ["SEI_PASSWORD", "DATABASE_URL_FILE", "API_TOKEN_CMD"] {
            assert!(!names.contains(&secret), "{secret} reached the command");
        }
        assert!(!listing.reveal().contains("hunter2"));
    }

    #[test]
    fn environment_filter_keeps_names_that_are_not_utf8() {
        let vars = vec![
            (
                OsString::from_vec(b"SEI_PASSWORD\xff".to_vec()),
                OsString::from("a"),
            ),
            (
                OsString::from_vec(b"\xfe".to_vec()),
                OsString::from_vec(b"\xfd".to_vec()),
            ),
            (
                OsString::from("KEEP_ME"),
                OsString::from_vec(b"\xfc".to_vec()),
            ),
            (
                OsString::from("SEI_PASSWORD"),
                OsString::from_vec(b"\xfb".to_vec()),
            ),
            (OsString::from("database_url_file"), OsString::from("/x")),
        ];

        let kept: Vec<_> = without_secrets(vars.clone()).collect();

        assert_eq!(kept, vars[..3]);
    }

    #[tokio::test]
    async fn password_command_splits_arguments_without_a_shell() {
        if !require_sh() {
            return;
        }
        let secret = command(r#"printf '%s|' "a b" c"#).password().await.unwrap();

        assert_eq!(secret.reveal(), "a b|c|");
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct SplitCase {
        input: String,
        args: Option<Vec<String>>,
        error: Option<String>,
    }

    #[test]
    fn split_command_matches_go() {
        for case in golden::cases::<SplitCase>("split_command", "") {
            let got = split_command(&case.input)
                .map_or_else(|err| (None, Some(err)), |args| (Some(args), None));
            assert_eq!(got, (case.args, case.error), "input {:?}", case.input);
        }
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct EnvironCase {
        environ: Vec<String>,
        kept: Vec<String>,
    }

    #[test]
    fn secret_names_match_go() {
        for case in golden::cases::<EnvironCase>("without_secrets", "") {
            let vars = case.environ.iter().map(|entry| {
                let name = entry
                    .split_once('=')
                    .map_or(entry.as_str(), |(name, _)| name);
                (OsString::from(name), OsString::from(entry))
            });
            let kept: Vec<String> = without_secrets(vars)
                .map(|(_, entry)| entry.into_string().unwrap())
                .collect();
            assert_eq!(kept, case.kept, "environ {:?}", case.environ);
        }
    }
}
