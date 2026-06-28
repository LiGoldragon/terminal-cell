use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use nota_next::NotaSource;
use terminal_cell::CellResponse;

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
        NotaSource::new(&String::from_utf8_lossy(&output.stdout))
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
fn nota_cli_launches_sends_observes_and_closes_arbitrary_command() {
    let fixture = CliFixture::new("lifecycle");
    let workspace = fixture.runtime.join("workspace");
    fs::create_dir_all(workspace.as_path()).expect("workspace created");

    let launch = format!(
        "(LaunchCell ((Some cli-proof) (Some {}) {} [] []))",
        path_atom(workspace.as_path()),
        CliFixture::binary("agent-terminal-fixture"),
    );
    let launched = match fixture.successful(&launch) {
        CellResponse::CellLaunched(launched) => launched,
        other => panic!("expected CellLaunched, got {other:?}"),
    };

    fixture.successful(&format!(
        "(SendLine ({} [hello from nota]))",
        launched.session_path
    ));

    let observed = observe_until_transcript(&fixture, &launched.session_path);
    assert!(
        observed.transcript_bytes > 0,
        "observation reports transcript bytes"
    );

    let closed = fixture.successful(&format!("(CloseCell ({}))", launched.session_path));
    assert!(matches!(closed, CellResponse::CellClosed(_)));
}

fn observe_until_transcript(
    fixture: &CliFixture,
    session_path: &str,
) -> terminal_cell::CellObservation {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let observed = match fixture.successful(&format!("(ObserveCell ({}))", session_path)) {
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
