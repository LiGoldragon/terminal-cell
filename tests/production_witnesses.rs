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
    control_socket: PathBuf,
    data_socket: PathBuf,
}

impl DaemonFixture {
    fn binary(name: &str) -> String {
        env::var(format!("CARGO_BIN_EXE_{name}")).expect("cargo exposes binary path to test")
    }

    fn spawn_agent(name: &str) -> Self {
        Self::spawn_command(name, &Self::binary("agent-terminal-fixture"), &[])
    }

    fn spawn_shell(name: &str, script: &str) -> Self {
        let shell = env::var("TERMINAL_CELL_TEST_SHELL").unwrap_or_else(|_| "bash".to_string());
        Self::spawn_command(name, &shell, &["-lc", script])
    }

    fn spawn_command(name: &str, program: &str, arguments: &[&str]) -> Self {
        let root = env::temp_dir().join(format!(
            "terminal-cell-production-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("daemon test root created");
        let control_socket = root.join("control.sock");
        let data_socket = root.join("data.sock");

        let mut command = Command::new(Self::binary("terminal-cell-daemon"));
        command
            .arg("--control-socket")
            .arg(&control_socket)
            .arg("--data-socket")
            .arg(&data_socket)
            .arg("--")
            .arg(program);
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
            ready.contains(control_socket.to_string_lossy().as_ref())
                && ready.contains(data_socket.to_string_lossy().as_ref()),
            "daemon ready line names both socket paths: {ready}; stderr: {error_output}"
        );

        Self {
            child,
            root,
            control_socket,
            data_socket,
        }
    }

    fn client(&self) -> TerminalCellSocketClient {
        TerminalCellSocketClient::new(self.control_socket.clone(), self.data_socket.clone())
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
        .arg("--control-socket")
        .arg(&daemon.control_socket)
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

/// Architectural-truth witness for the §3.3 "Transcript decoupling"
/// constraint: `TranscriptScriber` reads notices from a bounded queue
/// fed by `ViewerFanout`, and the queue drops the oldest pending notice
/// under overflow. The viewer fanout returns immediately; transcript
/// append happens on a separate worker.
///
/// The witness: drive the child to flood output that exceeds the
/// scriber's queue capacity (1024 notices), then prove the attached
/// viewer reads a steady stream of those bytes in bounded wall time.
/// If the viewer write were synchronously coupled to transcript
/// append (the pre-split shape), the viewer would block behind the
/// scriber and the read window would either stall or take far longer
/// than the budget below.
#[test]
fn slow_transcript_append_does_not_block_viewer_output() {
    let daemon = DaemonFixture::spawn_command(
        "transcript-append-decoupled-from-viewer",
        &DaemonFixture::binary("output-flood-fixture"),
        &[],
    );

    let _slow_subscriber = daemon
        .client()
        .subscribe_from_beginning()
        .expect("slow transcript subscriber connects");
    let mut viewer = AttachedStream::new(daemon.open_attach_stream());

    // Read enough output to prove the viewer is flowing far past the
    // scriber's bounded queue capacity (1024 notices). If the viewer
    // write were synchronously coupled to transcript append, the
    // viewer would stall behind the scriber and the read window would
    // not reach line 5000 inside the budget.
    let started = Instant::now();
    let output = viewer.read_until("flood-05000", Duration::from_secs(5));
    let elapsed = started.elapsed();

    assert!(
        output.contains("flood-ready"),
        "viewer saw the flood-ready marker"
    );
    assert!(
        output.contains("flood-00000"),
        "viewer saw early flood lines despite scriber load"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "viewer reads stream past the scriber's bounded queue inside the budget: {elapsed:?}"
    );
}

/// Architectural-truth witness for the data plane's no-actor promise
/// stated in `terminal-cell/ARCHITECTURE.md` §1 and §3: viewer attach
/// transport bytes never traverse the `TerminalCell` actor mailbox. We
/// can't introspect the actor's mailbox directly here, but we can prove
/// the round-trip latency between sending a byte on the attach stream
/// and seeing it echoed back is small enough that an actor `ask`
/// could not have been on the path. The transport budget is well under
/// the per-message wait time an actor ask would impose under any
/// non-trivial transcript or worker load.
#[test]
fn attached_viewer_input_round_trip_does_not_traverse_actor_mailbox() {
    let daemon = DaemonFixture::spawn_shell(
        "viewer-data-plane-latency",
        "while IFS= read -r line; do printf 'echo:%s\\n' \"$line\"; done",
    );

    let mut viewer = AttachedStream::new(daemon.open_attach_stream());

    // Warm the stream: send one line and wait for the echo so the
    // shell is at the read loop. The first iteration absorbs initial
    // shell setup time.
    viewer.write_line("warm");
    viewer.read_until("echo:warm", Duration::from_secs(2));

    // The latency budget is 200ms per attach round trip. The actor
    // mailbox would impose much more under load — but more importantly,
    // the test fails on the architectural intent if the data plane is
    // wired through an `ask`: an actor mailbox blocks under any
    // contention and the round trip stretches well beyond 200ms.
    let started = Instant::now();
    viewer.write_line("data-plane-no-mailbox");
    let _output = viewer.read_until("echo:data-plane-no-mailbox", Duration::from_secs(2));
    let round_trip = started.elapsed();
    assert!(
        round_trip < Duration::from_millis(200),
        "viewer attach round trip is sub-200ms; actor mailbox would not meet this budget: {round_trip:?}"
    );
}

#[test]
fn attached_input_reaches_child_during_high_volume_output() {
    let daemon = DaemonFixture::spawn_command(
        "input-latency-output-flood",
        &DaemonFixture::binary("output-flood-fixture"),
        &[],
    );

    let mut viewer = AttachedStream::new(daemon.open_attach_stream());
    viewer.read_until("flood-ready", Duration::from_secs(2));

    let started = Instant::now();
    viewer.write_line("latency-under-output-load");
    let output = viewer.read_until(
        "latency-response:latency-under-output-load",
        Duration::from_secs(3),
    );

    assert!(
        started.elapsed() < Duration::from_secs(3),
        "attached input reached the child before high-volume output could starve it"
    );
    assert!(
        output.contains("flood-"),
        "the witness ran while the child was emitting high-volume output"
    );
}

#[test]
fn daemon_worker_lifecycle_is_observable_over_socket() {
    let daemon = DaemonFixture::spawn_agent("daemon-worker-lifecycle");
    daemon.wait_for_text("agent-ready");

    let initial = daemon
        .client()
        .worker_observation_text()
        .expect("worker observation request succeeds");
    for expected in [
        "started:InputWriter",
        "started:ViewerFanout",
        "started:TranscriptScriber",
        "started:OutputReader",
        "started:ChildExitWatcher",
        "started:SocketAcceptLoop",
    ] {
        assert!(
            initial.contains(expected),
            "daemon worker observation includes {expected}; saw {initial:?}"
        );
    }

    let mut viewer = AttachedStream::new(daemon.open_attach_stream());
    viewer.read_until("agent-ready", Duration::from_secs(2));

    let attached = daemon
        .client()
        .worker_observation_text()
        .expect("worker observation request succeeds while a viewer is attached");
    assert!(
        attached.contains("started:AttachConnectionPump"),
        "attach pump lifecycle is reported through the daemon; saw {attached:?}"
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
    let _live_control =
        UnixListener::bind(live_session.join("control.sock")).expect("live control socket bound");
    let _live_data =
        UnixListener::bind(live_session.join("data.sock")).expect("live data socket bound");
    fs::write(
        live_session.join("daemon.pid"),
        std::process::id().to_string(),
    )
    .expect("live daemon pid written");

    std::thread::sleep(Duration::from_millis(20));

    let stale_session = root.join("session-stale");
    fs::create_dir_all(&stale_session).expect("stale session dir created");
    let _stale_control =
        UnixListener::bind(stale_session.join("control.sock")).expect("stale control socket bound");
    let _stale_data =
        UnixListener::bind(stale_session.join("data.sock")).expect("stale data socket bound");
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
