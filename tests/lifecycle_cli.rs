use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use dotos::{DotosEncode, DotosSource};
use terminal_cell::{CellRequest, CellResponse, CloseCell, LaunchCell, ObserveCell, SendLine};

struct CliFixture {
    runtime: PathBuf,
}

impl CliFixture {
    fn new(name: &str) -> Self {
        let runtime =
            env::temp_dir().join(format!("terminal-cell-cli-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&runtime);
        fs::create_dir_all(runtime.as_path()).expect("runtime directory created");
        Self { runtime }
    }

    fn command(&self, request: &str) -> Output {
        let mut command = Command::new(Self::binary("terminal-cell"));
        command.env("TERMINAL_CELL_RUNTIME_DIR", &self.runtime);
        command.env(
            "TERMINAL_CELL_DAEMON_BIN",
            Self::binary("terminal-cell-daemon"),
        );
        command.env(
            "TERMINAL_CELL_VIEWER_BIN",
            Self::binary("terminal-cell-view"),
        );
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().expect("terminal-cell CLI spawned");
        std::io::Write::write_all(
            child.stdin.as_mut().expect("stdin is piped"),
            request.as_bytes(),
        )
        .expect("request written");
        child
            .wait_with_output()
            .expect("terminal-cell CLI finished")
    }

    fn successful(&self, request: &str) -> CellResponse {
        let output = self.command(request);
        assert!(
            output.status.success(),
            "terminal-cell request failed\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        DotosSource::new(&String::from_utf8_lossy(&output.stdout))
            .parse::<CellResponse>()
            .expect("response decodes")
    }

    fn binary(name: &str) -> String {
        fs::canonicalize(
            env::var(format!("CARGO_BIN_EXE_{name}")).expect("cargo exposes binary path"),
        )
        .expect("cargo binary path canonicalizes")
        .to_string_lossy()
        .into_owned()
    }
}

impl Drop for CliFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.runtime.as_path());
    }
}

#[test]
fn dotos_cli_launches_sends_observes_and_closes_arbitrary_command() {
    let fixture = CliFixture::new("lifecycle");
    let workspace = fixture.runtime.join("workspace");
    fs::create_dir_all(workspace.as_path()).expect("workspace created");

    let launch = CellRequest::LaunchCell(LaunchCell {
        requested_name: Some("cli-proof".to_owned()),
        working_directory: Some(path_atom(workspace.as_path())),
        command: CliFixture::binary("agent-terminal-fixture"),
        arguments: Vec::new(),
        environment: Vec::new(),
    })
    .to_dotos();
    let launched = match fixture.successful(&launch) {
        CellResponse::CellLaunched(launched) => launched,
        other => panic!("expected CellLaunched, got {other:?}"),
    };

    fixture.successful(
        &CellRequest::SendLine(SendLine {
            cell: launched.session_path.clone(),
            line: "hello from dotos".to_owned(),
        })
        .to_dotos(),
    );

    let observed = observe_until_transcript(&fixture, &launched.session_path);
    assert!(
        observed.transcript_bytes > 0,
        "observation reports transcript bytes"
    );

    let closed = fixture.successful(
        &CellRequest::CloseCell(CloseCell {
            cell: launched.session_path,
        })
        .to_dotos(),
    );
    match closed {
        CellResponse::CellClosed(closed) => assert!(closed.terminated),
        other => panic!("expected CellClosed, got {other:?}"),
    }
}

#[test]
fn close_cell_terminates_daemon_and_pty_child_process_group() {
    let fixture = CliFixture::new("c");
    let workspace = fixture.runtime.join("workspace");
    fs::create_dir_all(workspace.as_path()).expect("workspace created");

    let launch = CellRequest::LaunchCell(LaunchCell {
        requested_name: Some("c".to_owned()),
        working_directory: Some(path_atom(workspace.as_path())),
        command: shell_command(),
        arguments: vec![
            "-c".to_owned(),
            "trap '' TERM HUP; while true; do sleep 1; done".to_owned(),
        ],
        environment: Vec::new(),
    })
    .to_dotos();
    let launched = match fixture.successful(&launch) {
        CellResponse::CellLaunched(launched) => launched,
        other => panic!("expected CellLaunched, got {other:?}"),
    };
    let child_pid = child_pid(&launched.session_path);
    assert!(process_is_live(launched.daemon_pid));
    assert!(process_is_live(child_pid));

    let closed = match fixture.successful(
        &CellRequest::CloseCell(CloseCell {
            cell: launched.session_path,
        })
        .to_dotos(),
    ) {
        CellResponse::CellClosed(closed) => closed,
        other => panic!("expected CellClosed, got {other:?}"),
    };

    assert_eq!(closed.child_pid, Some(child_pid));
    assert!(closed.daemon_terminated, "daemon cleanup is reported");
    assert!(closed.child_terminated, "PTY child cleanup is reported");
    assert!(closed.terminated, "aggregate cleanup is reported");
    assert!(!process_is_live(launched.daemon_pid), "daemon pid is gone");
    assert!(!process_is_live(child_pid), "PTY child pid is gone");
}

fn observe_until_transcript(
    fixture: &CliFixture,
    session_path: &str,
) -> terminal_cell::CellObservation {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let observed = match fixture.successful(
            &CellRequest::ObserveCell(ObserveCell {
                cell: session_path.to_owned(),
            })
            .to_dotos(),
        ) {
            CellResponse::CellObserved(observed) => observed,
            other => panic!("expected CellObserved, got {other:?}"),
        };
        if observed.transcript_bytes > 0 || Instant::now() >= deadline {
            return observed;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn path_atom(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn shell_command() -> String {
    env::var("TERMINAL_CELL_TEST_SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
}

fn child_pid(session_path: &str) -> u64 {
    fs::read_to_string(Path::new(session_path).join("child.pid"))
        .expect("child pid file exists")
        .trim()
        .parse::<u64>()
        .expect("child pid parses")
}

fn process_is_live(pid: u64) -> bool {
    let path = Path::new("/proc").join(pid.to_string());
    if !path.exists() {
        return false;
    }
    !fs::read_to_string(path.join("status"))
        .unwrap_or_default()
        .lines()
        .any(|line| line.starts_with("State:") && line.contains('Z'))
}
