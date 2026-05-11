use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use terminal_cell::TerminalCellSocketClient;

struct DaemonFixture {
    child: Child,
    root: PathBuf,
    socket: PathBuf,
}

impl DaemonFixture {
    fn binary(name: &str) -> String {
        env::var(format!("CARGO_BIN_EXE_{name}")).expect("cargo exposes binary path to test")
    }

    fn spawn_agent(name: &str) -> Self {
        Self::spawn_command(name, &Self::binary("agent-terminal-fixture"), &[])
    }

    fn spawn_shell(name: &str, script: &str) -> Self {
        Self::spawn_command(name, "sh", &["-lc", script])
    }

    fn spawn_command(name: &str, program: &str, arguments: &[&str]) -> Self {
        let root = env::temp_dir().join(format!(
            "terminal-cell-production-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("daemon test root created");
        let socket = root.join("cell.sock");

        let mut command = Command::new(Self::binary("terminal-cell-daemon"));
        command.arg("--socket").arg(&socket).arg("--").arg(program);
        for argument in arguments {
            command.arg(argument);
        }

        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("daemon spawned");
        let stdout = child.stdout.take().expect("daemon stdout is captured");
        let mut stderr = child.stderr.take().expect("daemon stderr is captured");
        let mut reader = BufReader::new(stdout);
        let mut ready = String::new();
        reader
            .read_line(&mut ready)
            .expect("daemon announces readiness");
        let mut error_output = String::new();
        if ready.is_empty() {
            let _ = stderr.read_to_string(&mut error_output);
        }
        assert!(
            ready.contains(socket.to_string_lossy().as_ref()),
            "daemon ready line names socket path: {ready}; stderr: {error_output}"
        );

        Self {
            child,
            root,
            socket,
        }
    }

    fn client(&self) -> TerminalCellSocketClient {
        TerminalCellSocketClient::new(self.socket.clone())
    }

    fn wait_for_text(&self, text: &str) {
        self.client()
            .wait_for_transcript(text.as_bytes())
            .expect("wait request succeeds");
    }

    fn send_line(&self, line: &str) {
        let mut bytes = line.as_bytes().to_vec();
        bytes.push(b'\r');
        self.client()
            .send_programmatic_input(&bytes)
            .expect("programmatic input succeeds");
    }

    fn capture_text(&self) -> String {
        let bytes = self.client().capture().expect("capture succeeds");
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn open_attach_stream(&self) -> UnixStream {
        self.client()
            .open_attach_stream()
            .expect("attach stream opens")
    }
}

impl Drop for DaemonFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct AttachedStream {
    stream: UnixStream,
}

impl AttachedStream {
    fn new(stream: UnixStream) -> Self {
        stream
            .set_read_timeout(Some(Duration::from_millis(50)))
            .expect("read timeout set");
        Self { stream }
    }

    fn write_line(&mut self, line: &str) {
        self.stream
            .write_all(line.as_bytes())
            .expect("attached stream writes line bytes");
        self.stream
            .write_all(b"\r")
            .expect("attached stream writes carriage return");
    }

    fn read_until(&mut self, needle: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        let mut output = Vec::new();
        let mut buffer = [0_u8; 8192];

        while Instant::now() < deadline {
            match self.stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    output.extend_from_slice(&buffer[..count]);
                    if String::from_utf8_lossy(&output).contains(needle) {
                        return String::from_utf8_lossy(&output).into_owned();
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {
                    return String::from_utf8_lossy(&output).into_owned();
                }
                Err(error) => panic!("attached stream read failed: {error}"),
            }
        }

        panic!(
            "attached stream did not produce {needle:?}; saw {} bytes ending with {:?}",
            output.len(),
            String::from_utf8_lossy(&output)
                .chars()
                .rev()
                .take(200)
                .collect::<String>()
        );
    }
}

#[test]
fn detached_viewer_leaves_daemon_alive_and_late_viewer_receives_replay() {
    let daemon = DaemonFixture::spawn_agent("reattach-replay");
    daemon.wait_for_text("agent-ready");

    let mut first = AttachedStream::new(daemon.open_attach_stream());
    first.read_until("agent-ready", Duration::from_secs(2));
    drop(first);

    daemon.send_line("while viewer is detached");
    daemon.wait_for_text("agent-response: while viewer is detached");

    let mut reattached = AttachedStream::new(daemon.open_attach_stream());
    reattached.read_until(
        "agent-response: while viewer is detached",
        Duration::from_secs(2),
    );
    reattached.write_line("after reattach");
    reattached.read_until("agent-response: after reattach", Duration::from_secs(2));
}

#[test]
fn second_attached_viewer_is_rejected_while_first_viewer_is_active() {
    let daemon = DaemonFixture::spawn_agent("single-viewer");
    daemon.wait_for_text("agent-ready");

    let mut first = AttachedStream::new(daemon.open_attach_stream());
    first.read_until("agent-ready", Duration::from_secs(2));

    let error = daemon
        .client()
        .open_attach_stream()
        .expect_err("second attached viewer is explicitly rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::ConnectionRefused);
    assert!(
        error.to_string().contains("attached viewer"),
        "rejection explains the active viewer conflict: {error}"
    );

    first.write_line("first viewer remains active");
    first.read_until(
        "agent-response: first viewer remains active",
        Duration::from_secs(2),
    );
    let transcript = daemon.capture_text();
    assert!(
        !transcript.contains("agent-response: second viewer must not reach child"),
        "rejected viewer input must not reach the child PTY"
    );
}

#[test]
fn headless_resize_cli_resizes_without_attached_viewer() {
    let daemon = DaemonFixture::spawn_shell(
        "headless-resize-cli",
        "stty size; IFS= read -r _; stty size",
    );
    daemon.wait_for_text("24 80");

    let output = Command::new(DaemonFixture::binary("terminal-cell-resize"))
        .arg("--socket")
        .arg(&daemon.socket)
        .args(["--rows", "41", "--columns", "113"])
        .output()
        .expect("resize cli runs");
    assert!(
        output.status.success(),
        "resize cli exits successfully; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    daemon.send_line("");
    daemon.wait_for_text("41 113");
}

#[test]
fn slow_transcript_subscriber_does_not_block_attached_viewer_output() {
    let daemon = DaemonFixture::spawn_shell(
        "slow-subscriber",
        "IFS= read -r line; \
         i=0; \
         while [ \"$i\" -lt 30000 ]; do \
           printf 'bulk-%05d abcdefghijklmnopqrstuvwxyz0123456789\\n' \"$i\"; \
           i=$((i + 1)); \
         done; \
         printf 'after:%s\\n' \"$line\"",
    );

    let _slow_subscriber = daemon
        .client()
        .subscribe_from_beginning()
        .expect("slow transcript subscriber connects");
    let mut viewer = AttachedStream::new(daemon.open_attach_stream());

    viewer.write_line("attached viewer survives slow subscriber");
    let output = viewer.read_until(
        "after:attached viewer survives slow subscriber",
        Duration::from_secs(10),
    );

    assert!(
        output.contains("bulk-00000"),
        "viewer saw high-volume child output before the final marker"
    );
}

#[test]
fn session_selector_skips_newer_stale_sessions() {
    let root = env::temp_dir().join(format!(
        "terminal-cell-session-select-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("session selector root created");

    let live_session = root.join("session-live");
    fs::create_dir_all(&live_session).expect("live session dir created");
    let _live_socket =
        UnixListener::bind(live_session.join("cell.sock")).expect("live session socket bound");
    fs::write(
        live_session.join("daemon.pid"),
        std::process::id().to_string(),
    )
    .expect("live daemon pid written");

    std::thread::sleep(Duration::from_millis(20));

    let stale_session = root.join("session-stale");
    fs::create_dir_all(&stale_session).expect("stale session dir created");
    let _stale_socket =
        UnixListener::bind(stale_session.join("cell.sock")).expect("stale session socket bound");
    fs::write(stale_session.join("daemon.pid"), "99999999").expect("stale daemon pid written");

    let output = Command::new(DaemonFixture::binary("terminal-cell-session-select"))
        .arg("--root")
        .arg(&root)
        .output()
        .expect("session selector runs");
    assert!(
        output.status.success(),
        "session selector exits successfully; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()),
        live_session
    );

    let rejected = Command::new(DaemonFixture::binary("terminal-cell-session-select"))
        .arg("--session")
        .arg(&stale_session)
        .output()
        .expect("session selector validates exact sessions");
    assert!(
        !rejected.status.success(),
        "exact stale session is rejected"
    );

    let _ = fs::remove_dir_all(&root);
}
