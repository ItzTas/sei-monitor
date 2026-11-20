use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use monitor_hub_sdk::proto::{
    CheckRequest, CheckResponse, DescribeRequest, DescribeResponse, Event, EventsRequest,
    EventsResponse, StatusRequest, StatusResponse, WatchRequest, WatchResponse,
};
use monitor_hub_sdk::{MAGIC_COOKIE_KEY, MAGIC_COOKIE_VALUE};
use serde_json::Value;
use tonic::client::Grpc;
use tonic::codegen::http::uri::PathAndQuery;
use tonic::transport::{Channel, Endpoint};
use tonic::{Request, Status, Streaming};
use tonic_prost::ProstCodec;
use tonic_prost::prost::Message;

pub const COOKIE: (&str, &str) = (MAGIC_COOKIE_KEY, MAGIC_COOKIE_VALUE);

const HANDSHAKE_LIMIT: Duration = Duration::from_secs(30);
const PIPE_LIMIT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(10);
const CONNECT_LIMIT: Duration = Duration::from_secs(10);
const CALL_LIMIT: Duration = Duration::from_secs(30);

static DIRECTORIES: AtomicUsize = AtomicUsize::new(0);

pub struct ShortTempDir(PathBuf);

impl ShortTempDir {
    pub fn new() -> ShortTempDir {
        let number = DIRECTORIES.fetch_add(1, Ordering::SeqCst);
        let path = env::temp_dir().join(format!("sei-{}-{number}", std::process::id()));
        fs::create_dir_all(&path).expect("create a short temporary directory");
        ShortTempDir(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn text(&self) -> &str {
        self.0.to_str().expect("a UTF-8 temporary directory")
    }
}

impl Drop for ShortTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub struct RunningMonitor {
    pub child: Child,
    pub socket: PathBuf,
    pub stderr: Arc<Mutex<String>>,
    _temp: ShortTempDir,
}

pub struct Exited {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

pub fn spawn_monitor(env: &[(&str, &str)]) -> RunningMonitor {
    let temp = ShortTempDir::new();
    let mut child = command(&["monitor"], &[COOKIE, ("TMPDIR", temp.text())], env)
        .spawn()
        .expect("start the monitor");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = Arc::new(Mutex::new(String::new()));
    collect_lines(
        child.stderr.take().expect("piped stderr"),
        Arc::clone(&stderr),
    );
    let first_line = first_line(stdout);
    let mut spawned = RunningMonitor {
        child,
        socket: PathBuf::new(),
        stderr,
        _temp: temp,
    };
    let Ok(line) = first_line.recv_timeout(HANDSHAKE_LIMIT) else {
        spawned.fail("the monitor printed no handshake line within 30 seconds");
    };
    let Some(socket) = handshake_socket(&line) else {
        spawned.fail(&format!(
            "the first stdout line is not a handshake: {line:?}"
        ));
    };
    spawned.socket = socket;
    spawned
}

pub fn run_to_exit(args: &[&str], env: &[(&str, &str)], within: Duration) -> Exited {
    let temp = ShortTempDir::new();
    let mut child = command(args, &[("TMPDIR", temp.text())], env)
        .spawn()
        .expect("start the program");
    let stdout = read_all(child.stdout.take().expect("piped stdout"));
    let stderr = read_all(child.stderr.take().expect("piped stderr"));
    let Some(status) = wait_exit(&mut child, within) else {
        let _ = child.kill();
        let _ = child.wait();
        panic!(
            "the program did not exit within {within:?}\nstderr:\n{}",
            collected(&stderr)
        );
    };
    Exited {
        status,
        stdout: collected(&stdout),
        stderr: collected(&stderr),
    }
}

pub fn terminate(child: &mut Child) {
    let pid = libc::pid_t::try_from(child.id()).expect("a process id");
    let sent = unsafe { libc::kill(pid, libc::SIGTERM) };
    assert_eq!(sent, 0, "{}", std::io::Error::last_os_error());
}

pub struct StartedCommand {
    child: Child,
    stdout: Receiver<String>,
    stderr: Receiver<String>,
    _temp: ShortTempDir,
}

pub fn start_command(args: &[&str], env: &[(&str, &str)]) -> StartedCommand {
    let temp = ShortTempDir::new();
    let mut child = command(args, &[("TMPDIR", temp.text())], env)
        .spawn()
        .expect("start the program");
    let stdout = read_all(child.stdout.take().expect("piped stdout"));
    let stderr = read_all(child.stderr.take().expect("piped stderr"));
    StartedCommand {
        child,
        stdout,
        stderr,
        _temp: temp,
    }
}

impl StartedCommand {
    pub fn signal(&mut self, signal: libc::c_int) {
        let pid = libc::pid_t::try_from(self.child.id()).expect("a process id");
        let sent = unsafe { libc::kill(pid, signal) };
        assert_eq!(sent, 0, "{}", std::io::Error::last_os_error());
    }

    pub fn finish(&mut self, within: Duration) -> Exited {
        let Some(status) = wait_exit(&mut self.child, within) else {
            let _ = self.child.kill();
            let _ = self.child.wait();
            panic!(
                "the program did not exit within {within:?}\nstderr:\n{}",
                collected(&self.stderr)
            );
        };
        Exited {
            status,
            stdout: collected(&self.stdout),
            stderr: collected(&self.stderr),
        }
    }
}

impl Drop for StartedCommand {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn wait_exit(child: &mut Child, within: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + within;
    loop {
        if let Some(status) = child.try_wait().expect("poll the child") {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(POLL);
    }
}

pub fn error_lines(stderr: &str) -> Vec<Value> {
    stderr
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| entry["@level"] == "error")
        .collect()
}

impl RunningMonitor {
    pub fn stderr_text(&self) -> String {
        self.stderr
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn still_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn exit_within(&mut self, within: Duration) -> ExitStatus {
        let Some(status) = wait_exit(&mut self.child, within) else {
            self.fail(&format!("the monitor did not exit within {within:?}"));
        };
        status
    }

    pub fn fail(&mut self, problem: &str) -> ! {
        let _ = self.child.kill();
        let _ = self.child.wait();
        thread::sleep(Duration::from_millis(50));
        panic!("{problem}\nmonitor stderr:\n{}", self.stderr_text());
    }
}

impl Drop for RunningMonitor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct MonitorClient {
    grpc: Grpc<Channel>,
}

pub async fn monitor_client(socket: &Path) -> MonitorClient {
    let endpoint = Endpoint::from_shared(format!("unix:{}", socket.display()))
        .expect("a socket endpoint")
        .connect_timeout(CONNECT_LIMIT)
        .timeout(CALL_LIMIT);
    let channel = tokio::time::timeout(CONNECT_LIMIT, endpoint.connect())
        .await
        .expect("the monitor socket answered in time")
        .expect("connect to the monitor");
    MonitorClient {
        grpc: Grpc::new(channel),
    }
}

impl MonitorClient {
    pub async fn describe(&mut self) -> Result<DescribeResponse, Status> {
        self.call("Describe", DescribeRequest {}).await
    }

    pub async fn status(&mut self) -> Result<StatusResponse, Status> {
        self.call("Status", StatusRequest {}).await
    }

    pub async fn check(&mut self) -> Result<CheckResponse, Status> {
        self.call("Check", CheckRequest {}).await
    }

    pub async fn events(&mut self, after: i64, limit: i32) -> Result<Vec<Event>, Status> {
        let page: EventsResponse = self.call("Events", EventsRequest { after, limit }).await?;
        Ok(page.events)
    }

    pub async fn watch(&mut self) -> Result<Streaming<WatchResponse>, Status> {
        self.grpc
            .ready()
            .await
            .map_err(|error| Status::unavailable(error.to_string()))?;
        self.grpc
            .server_streaming(
                Request::new(WatchRequest {}),
                PathAndQuery::from_static("/monitorhub.monitor.v1.Monitor/Watch"),
                ProstCodec::<WatchRequest, WatchResponse>::default(),
            )
            .await
            .map(tonic::Response::into_inner)
    }

    async fn call<Q, A>(&mut self, method: &'static str, request: Q) -> Result<A, Status>
    where
        Q: Message + Send + Sync + 'static,
        A: Message + Default + Send + Sync + 'static,
    {
        self.grpc
            .ready()
            .await
            .map_err(|error| Status::unavailable(error.to_string()))?;
        let path = PathAndQuery::try_from(format!("/monitorhub.monitor.v1.Monitor/{method}"))
            .expect("a method path");
        self.grpc
            .unary(Request::new(request), path, ProstCodec::<Q, A>::default())
            .await
            .map(tonic::Response::into_inner)
    }
}

fn command(args: &[&str], fixed: &[(&str, &str)], env: &[(&str, &str)]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sei-monitor"));
    command
        .args(args)
        .env_clear()
        .env("PATH", env::var_os("PATH").unwrap_or_default())
        .envs(fixed.iter().copied())
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn first_line(stdout: impl Read + Send + 'static) -> Receiver<String> {
    let (sender, line) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut text = String::new();
        let _ = reader.read_line(&mut text);
        let _ = sender.send(text);
        let mut rest = String::new();
        let _ = reader.read_to_string(&mut rest);
    });
    line
}

pub fn collect_lines(pipe: impl Read + Send + 'static, into: Arc<Mutex<String>>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut line = String::new();
        while reader.read_line(&mut line).is_ok_and(|read| read > 0) {
            into.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_str(&line);
            line.clear();
        }
    });
}

fn read_all(pipe: impl Read + Send + 'static) -> Receiver<String> {
    let (sender, text) = mpsc::channel();
    thread::spawn(move || {
        let mut all = String::new();
        let _ = BufReader::new(pipe).read_to_string(&mut all);
        let _ = sender.send(all);
    });
    text
}

fn collected(text: &Receiver<String>) -> String {
    text.recv_timeout(PIPE_LIMIT)
        .unwrap_or_else(|_| "<the pipe stayed open>".to_owned())
}

fn handshake_socket(line: &str) -> Option<PathBuf> {
    let fields: Vec<&str> = line.trim().split('|').collect();
    let ["1", "2", "unix", socket, "grpc"] = fields[..] else {
        return None;
    };
    Some(PathBuf::from(socket))
}
