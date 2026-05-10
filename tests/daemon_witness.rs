use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

struct DaemonFixture {
    child: Child,
    root: PathBuf,
    socket: PathBuf,
}

impl DaemonFixture {
    fn spawn(name: &str) -> Self {
        let root = env::temp_dir().join(format!("terminal-cell-lab-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("daemon test root created");
        let socket = root.join("cell.sock");

        let mut child = Command::new(env!("CARGO_BIN_EXE_terminal-cell-lab-daemon"))
            .arg("--socket")
            .arg(&socket)
            .arg("--")
            .arg(env!("CARGO_BIN_EXE_agent-terminal-fixture"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("daemon spawned");

        let stdout = child.stdout.take().expect("daemon stdout is captured");
        let mut reader = BufReader::new(stdout);
        let mut ready = String::new();
        reader
            .read_line(&mut ready)
            .expect("daemon announces readiness");
        assert!(
            ready.contains(socket.to_string_lossy().as_ref()),
            "daemon ready line names socket path: {ready}"
        );

        Self {
            child,
            root,
            socket,
        }
    }

    fn wait_for_text(&self, text: &str) {
        let status = Command::new(env!("CARGO_BIN_EXE_terminal-cell-lab-wait"))
            .arg("--socket")
            .arg(&self.socket)
            .arg("--text")
            .arg(text)
            .status()
            .expect("wait command runs");
        assert!(status.success(), "wait command succeeded");
    }

    fn send_line(&self, line: &str) {
        let status = Command::new(env!("CARGO_BIN_EXE_terminal-cell-lab-send"))
            .arg("--socket")
            .arg(&self.socket)
            .arg("--line")
            .arg(line)
            .status()
            .expect("send command runs");
        assert!(status.success(), "send command succeeded");
    }

    fn capture_text(&self) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_terminal-cell-lab-capture"))
            .arg("--socket")
            .arg(&self.socket)
            .output()
            .expect("capture command runs");
        assert!(output.status.success(), "capture command succeeded");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn view_once_text(&self) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_terminal-cell-lab-view"))
            .arg("--socket")
            .arg(&self.socket)
            .arg("--once")
            .output()
            .expect("view command runs");
        assert!(output.status.success(), "view --once command succeeded");
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
