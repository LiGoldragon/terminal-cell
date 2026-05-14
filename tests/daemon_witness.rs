use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use signal_core::SignalVerb;
use signal_persona_terminal as terminal_signal;
use terminal_cell::{
    SocketReplyReader, SocketRequestWriter, TerminalCellSocketClient, TerminalSize,
};

struct DaemonFixture {
    child: Child,
    root: PathBuf,
    socket: PathBuf,
}

impl DaemonFixture {
    fn binary(name: &str) -> String {
        env::var(format!("CARGO_BIN_EXE_{name}")).expect("cargo exposes binary path to test")
    }

    fn spawn(name: &str) -> Self {
        Self::spawn_command(name, &Self::binary("agent-terminal-fixture"), &[])
    }

    fn spawn_shell(name: &str, script: &str) -> Self {
        let shell = env::var("TERMINAL_CELL_TEST_SHELL").unwrap_or_else(|_| "bash".to_string());
        Self::spawn_command(name, &shell, &["-lc", script])
    }

    fn spawn_command(name: &str, program: &str, arguments: &[&str]) -> Self {
        let root = env::temp_dir().join(format!("terminal-cell-{name}-{}", std::process::id()));
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

    fn wait_for_text(&self, text: &str) {
        let status = Command::new(Self::binary("terminal-cell-wait"))
            .arg("--socket")
            .arg(&self.socket)
            .arg("--text")
            .arg(text)
            .status()
            .expect("wait command runs");
        assert!(status.success(), "wait command succeeded");
    }

    fn send_line(&self, line: &str) {
        let status = Command::new(Self::binary("terminal-cell-send"))
            .arg("--socket")
            .arg(&self.socket)
            .arg("--line")
            .arg(line)
            .status()
            .expect("send command runs");
        assert!(status.success(), "send command succeeded");
    }

    fn resize(&self, rows: u16, columns: u16) {
        TerminalCellSocketClient::new(&self.socket)
            .resize(TerminalSize::new(rows, columns))
            .expect("resize request accepted");
    }

    fn client(&self) -> TerminalCellSocketClient {
        TerminalCellSocketClient::new(self.socket.clone())
    }

    fn open_attach_stream(&self) -> std::os::unix::net::UnixStream {
        TerminalCellSocketClient::new(&self.socket)
            .open_attach_stream()
            .expect("attach stream opened")
    }

    fn capture_text(&self) -> String {
        let output = Command::new(Self::binary("terminal-cell-capture"))
            .arg("--socket")
            .arg(&self.socket)
            .output()
            .expect("capture command runs");
        assert!(output.status.success(), "capture command succeeded");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn view_once_text(&self) -> String {
        let output = Command::new(Self::binary("terminal-cell-view"))
            .arg("--socket")
            .arg(&self.socket)
            .arg("--once")
            .output()
            .expect("view command runs");
        assert!(output.status.success(), "view --once command succeeded");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn wait_for_exit_status(&self) -> String {
        let output = Command::new(Self::binary("terminal-cell-exit"))
            .arg("--socket")
            .arg(&self.socket)
            .output()
            .expect("exit command runs");
        assert!(output.status.success(), "exit command succeeded");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

impl Drop for DaemonFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn daemon_accepts_programmatic_prompt_and_capture_reads_transcript() {
    let daemon = DaemonFixture::spawn("capture");

    daemon.wait_for_text("agent-ready");
    daemon.send_line("hello attached daemon");
    daemon.wait_for_text("agent-response: hello attached daemon");
    daemon.send_line("/usage");
    daemon.wait_for_text("usage-window: five-hour=73 weekly=41");

    let transcript = daemon.capture_text();
    assert!(transcript.contains("agent-response: hello attached daemon"));
    assert!(transcript.contains("usage-window: five-hour=73 weekly=41"));
}

#[test]
fn attach_view_replays_transcript_without_owning_the_child() {
    let daemon = DaemonFixture::spawn("view");

    daemon.wait_for_text("agent-ready");
    daemon.send_line("hello replayed viewer");
    daemon.wait_for_text("agent-response: hello replayed viewer");

    let replay = daemon.view_once_text();
    assert!(replay.contains("agent-ready"));
    assert!(replay.contains("agent-response: hello replayed viewer"));
}

#[test]
fn daemon_exposes_terminal_exit_status() {
    let daemon = DaemonFixture::spawn_shell("exit", "printf 'done-exiting\\n'; exit 7");

    daemon.wait_for_text("done-exiting");

    let status = daemon.wait_for_exit_status();
    assert!(
        !status.trim().is_empty(),
        "terminal-cell-exit prints the child status"
    );
}

#[test]
fn daemon_resizes_the_owned_pty() {
    let daemon = DaemonFixture::spawn_shell("resize", "stty size; IFS= read -r _; stty size");

    daemon.wait_for_text("24 80");
    daemon.resize(31, 97);
    daemon.send_line("");
    daemon.wait_for_text("31 97");

    let transcript = daemon.capture_text();
    assert!(transcript.contains("24 80"));
    assert!(transcript.contains("31 97"));
}

#[test]
fn attach_stream_is_raw_bidirectional_byte_path() {
    let daemon = DaemonFixture::spawn("attach-stream");

    daemon.wait_for_text("agent-ready");
    let mut stream = daemon.open_attach_stream();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("attach stream read timeout set");

    let mut output = Vec::new();
    let mut buffer = [0_u8; 4096];
    for _ in 0..8 {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => output.extend_from_slice(&buffer[..count]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(error) => panic!("attach stream read failed: {error}"),
        }
        if String::from_utf8_lossy(&output).contains("agent-ready") {
            break;
        }
    }
    assert!(
        String::from_utf8_lossy(&output).contains("agent-ready"),
        "attached stream emits transcript replay before live bytes"
    );

    stream
        .write_all(b"hello raw attach stream\r")
        .expect("attach stream accepts input bytes");
    daemon.wait_for_text("agent-response: hello raw attach stream");

    let transcript = daemon.capture_text();
    assert!(transcript.contains("agent-response: hello raw attach stream"));
}

#[test]
fn input_gate_holds_human_bytes_during_programmatic_injection() {
    let daemon = DaemonFixture::spawn_shell(
        "input_gate",
        "IFS= read -r first; printf 'first:%s\\n' \"$first\"; \
         IFS= read -r second; printf 'second:%s\\n' \"$second\"",
    );
    let client = TerminalCellSocketClient::new(daemon.socket.clone());
    let mut viewer = daemon.open_attach_stream();

    let lease = client.close_human_input().expect("human input gate closes");
    viewer
        .write_all(b"human-held-behind-gate\r")
        .expect("viewer bytes are accepted while gate is closed");

    client
        .send_programmatic_input(b"persona-injection\r")
        .expect("programmatic bytes write while gate is closed");
    daemon.wait_for_text("first:persona-injection");

    let release = client
        .open_human_input(lease)
        .expect("human input gate reopens");
    assert_eq!(release.lease(), lease);
    assert_eq!(release.held_byte_count(), "human-held-behind-gate\r".len());
    daemon.wait_for_text("second:human-held-behind-gate");

    let transcript = daemon.capture_text();
    let programmatic = transcript
        .find("first:persona-injection")
        .expect("programmatic line appears");
    let human = transcript
        .find("second:human-held-behind-gate")
        .expect("held human line appears");
    assert!(
        programmatic < human,
        "programmatic input is written before held human input"
    );
}

#[test]
fn signal_control_plane_acquires_gate_injects_releases_and_replays_human_bytes() {
    let daemon = DaemonFixture::spawn("signal-control-plane");
    let client = daemon.client();

    daemon.wait_for_text("agent-ready");
    let registered = client
        .send_signal_request(
            SignalVerb::Assert,
            terminal_signal::RegisterPromptPattern {
                terminal: terminal_signal::TerminalName::new("operator"),
                pattern: terminal_signal::PromptPattern::RegexSuffix {
                    pattern: terminal_signal::PromptPatternBytes::new(
                        br"agent-ready\r+\n$".to_vec(),
                    ),
                },
            }
            .into(),
        )
        .expect("prompt pattern registration succeeds");
    let pattern_id = match registered {
        terminal_signal::TerminalEvent::PromptPatternRegistered(registered) => {
            registered.pattern_id
        }
        other => panic!("expected prompt-pattern registration, got {other:?}"),
    };

    let acquired = client
        .send_signal_request(
            SignalVerb::Assert,
            terminal_signal::AcquireInputGate {
                terminal: terminal_signal::TerminalName::new("operator"),
                reason: terminal_signal::InputGateReason::new("witness injection"),
                prompt_pattern_id: Some(pattern_id),
            }
            .into(),
        )
        .expect("input gate acquisition succeeds");
    let lease = match acquired {
        terminal_signal::TerminalEvent::GateAcquired(acquired) => {
            assert_eq!(acquired.prompt_state, terminal_signal::PromptState::Clean);
            acquired.lease
        }
        other => panic!("expected gate acquisition, got {other:?}"),
    };

    let mut viewer = daemon.open_attach_stream();
    viewer
        .write_all(b"human-held-behind-signal-gate\r")
        .expect("viewer bytes are accepted while Signal gate is closed");

    let ack = client
        .send_signal_request(
            SignalVerb::Assert,
            terminal_signal::WriteInjection {
                terminal: terminal_signal::TerminalName::new("operator"),
                lease: lease.clone(),
                bytes: terminal_signal::TerminalInputBytes::new(b"persona-signal\r".to_vec()),
            }
            .into(),
        )
        .expect("Signal write injection succeeds");
    assert!(
        matches!(ack, terminal_signal::TerminalEvent::InjectionAck(_)),
        "Signal write injection is acknowledged: {ack:?}"
    );
    daemon.wait_for_text("agent-response: persona-signal");

    let released = client
        .send_signal_request(
            SignalVerb::Assert,
            terminal_signal::ReleaseInputGate {
                terminal: terminal_signal::TerminalName::new("operator"),
                lease,
            }
            .into(),
        )
        .expect("Signal input gate release succeeds");
    match released {
        terminal_signal::TerminalEvent::GateReleased(released) => {
            assert_eq!(
                released.cached_human_bytes.into_u64(),
                "human-held-behind-signal-gate\r".len() as u64
            );
        }
        other => panic!("expected gate release, got {other:?}"),
    }
    daemon.wait_for_text("agent-response: human-held-behind-signal-gate");
}

#[test]
fn signal_dirty_prompt_rejects_write_injection_by_default() {
    let daemon = DaemonFixture::spawn_shell(
        "signal-dirty-prompt",
        "printf 'ready> dirty'; IFS= read -r line; printf 'seen:%s\\n' \"$line\"",
    );
    let client = daemon.client();

    daemon.wait_for_text("ready> dirty");
    let registered = client
        .send_signal_request(
            SignalVerb::Assert,
            terminal_signal::RegisterPromptPattern {
                terminal: terminal_signal::TerminalName::new("operator"),
                pattern: terminal_signal::PromptPattern::RegexSuffix {
                    pattern: terminal_signal::PromptPatternBytes::new(br"ready> ".to_vec()),
                },
            }
            .into(),
        )
        .expect("prompt pattern registration succeeds");
    let pattern_id = match registered {
        terminal_signal::TerminalEvent::PromptPatternRegistered(registered) => {
            registered.pattern_id
        }
        other => panic!("expected prompt-pattern registration, got {other:?}"),
    };

    let acquired = client
        .send_signal_request(
            SignalVerb::Assert,
            terminal_signal::AcquireInputGate {
                terminal: terminal_signal::TerminalName::new("operator"),
                reason: terminal_signal::InputGateReason::new("dirty prompt witness"),
                prompt_pattern_id: Some(pattern_id),
            }
            .into(),
        )
        .expect("dirty prompt still returns a gate acquisition with state");
    let lease = match acquired {
        terminal_signal::TerminalEvent::GateAcquired(acquired) => {
            assert_eq!(
                acquired.prompt_state,
                terminal_signal::PromptState::Dirty {
                    trailing_count: terminal_signal::TerminalByteCount::new(5),
                },
                "regex suffix prompt reports trailing dirty bytes"
            );
            acquired.lease
        }
        other => panic!("expected gate acquisition, got {other:?}"),
    };

    let rejected = client
        .send_signal_request(
            SignalVerb::Assert,
            terminal_signal::WriteInjection {
                terminal: terminal_signal::TerminalName::new("operator"),
                lease: lease.clone(),
                bytes: terminal_signal::TerminalInputBytes::new(b"must-not-inject\r".to_vec()),
            }
            .into(),
        )
        .expect("dirty injection produces a typed rejection");
    assert!(
        matches!(
            rejected,
            terminal_signal::TerminalEvent::InjectionRejected(terminal_signal::InjectionRejected {
                reason: terminal_signal::InjectionRejectionReason::DirtyPrompt,
                ..
            })
        ),
        "dirty prompt rejects write injection: {rejected:?}"
    );

    let _ = client
        .send_signal_request(
            SignalVerb::Assert,
            terminal_signal::ReleaseInputGate {
                terminal: terminal_signal::TerminalName::new("operator"),
                lease,
            }
            .into(),
        )
        .expect("dirty prompt gate release succeeds");
}

#[test]
fn signal_worker_lifecycle_subscription_streams_snapshot_then_deltas() {
    let daemon = DaemonFixture::spawn("signal-worker-lifecycle");
    daemon.wait_for_text("agent-ready");

    let mut subscription =
        std::os::unix::net::UnixStream::connect(&daemon.socket).expect("subscription socket opens");
    subscription
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("subscription read timeout set");
    SocketRequestWriter::new(&mut subscription)
        .write_signal_request(
            SignalVerb::Subscribe,
            terminal_signal::SubscribeTerminalWorkerLifecycle {
                terminal: terminal_signal::TerminalName::new("operator"),
            }
            .into(),
        )
        .expect("worker lifecycle subscription request writes");

    let snapshot = SocketReplyReader::new(&mut subscription)
        .read_signal_event()
        .expect("initial worker lifecycle snapshot arrives");
    match snapshot {
        terminal_signal::TerminalEvent::TerminalWorkerLifecycleSnapshot(snapshot) => {
            assert!(
                snapshot.observations.iter().any(|observation| matches!(
                    observation,
                    terminal_signal::TerminalWorkerLifecycle::Started(
                        terminal_signal::TerminalWorkerKind::InputWriter
                    )
                )),
                "worker lifecycle snapshot includes already-started workers"
            );
        }
        other => panic!("expected worker lifecycle snapshot, got {other:?}"),
    }

    let mut viewer = daemon.open_attach_stream();
    let event = SocketReplyReader::new(&mut subscription)
        .read_signal_event()
        .expect("worker lifecycle delta arrives");
    assert!(
        matches!(
            event,
            terminal_signal::TerminalEvent::TerminalWorkerLifecycleEvent(
                terminal_signal::TerminalWorkerLifecycleEvent {
                    observation: terminal_signal::TerminalWorkerLifecycle::Started(
                        terminal_signal::TerminalWorkerKind::AttachConnectionPump
                    ),
                    ..
                }
            )
        ),
        "attach pump start is streamed as a Signal lifecycle delta: {event:?}"
    );
    viewer
        .write_all(b"signal lifecycle stream remains separate from raw input\r")
        .expect("raw viewer stream is still writable after Signal subscription");
}
