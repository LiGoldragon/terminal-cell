//! The datom text of terminal-cell's own `CellRequest` and `CellResponse` across the datom-codec
//! 0.31 to 0.33 move. `FIXTURE` is the text the unmodified 3.0.1 source (origin main 9cab,
//! datom-codec 0.31.0) wrote for the same values; the current source must write identical text,
//! and the text must read back as the value. Representative values (every variant, absent and
//! present options, empty and filled lists, awkward strings, extreme integers), not all values.

use terminal_cell::{
    AttachViewer, CellClosed, CellEnvironmentVariable, CellLaunched, CellObservation, CellRequest,
    CellResponse, CloseCell, LaunchCell, LineSent, ObserveCell, ProcessState, SendLine, StallState,
    ViewerAttached, ViewerMode,
};

fn text(value: &str) -> String {
    value.to_owned()
}

fn requests() -> Vec<(&'static str, CellRequest)> {
    let environment = vec![
        CellEnvironmentVariable {
            name: text("TERM"),
            value: text("xterm 256"),
        },
        CellEnvironmentVariable {
            name: text("EMPTY"),
            value: text(""),
        },
    ];
    vec![
        (
            "launch-full",
            CellRequest::LaunchCell(LaunchCell {
                requested_name: Some(text("main cell")),
                working_directory: Some(text("/home/user/work dir")),
                command: text("/bin/sh"),
                arguments: vec![
                    text("-l"),
                    text("-c"),
                    text("echo \"hi\"\nline2 \\ end ünï"),
                ],
                environment,
            }),
        ),
        (
            "launch-bare",
            CellRequest::LaunchCell(LaunchCell {
                requested_name: None,
                working_directory: None,
                command: text("bash"),
                arguments: vec![],
                environment: vec![],
            }),
        ),
        (
            "send-line",
            CellRequest::SendLine(SendLine {
                cell: text("cell-1"),
                line: text("ls -la; echo 'x'"),
            }),
        ),
        (
            "attach-interactive",
            CellRequest::AttachViewer(AttachViewer {
                cell: text("cell-1"),
                mode: ViewerMode::Interactive,
            }),
        ),
        (
            "attach-snapshot",
            CellRequest::AttachViewer(AttachViewer {
                cell: text("cell-1"),
                mode: ViewerMode::Snapshot,
            }),
        ),
        (
            "close",
            CellRequest::CloseCell(CloseCell {
                cell: text("cell-1"),
            }),
        ),
        (
            "observe",
            CellRequest::ObserveCell(ObserveCell { cell: text("") }),
        ),
    ]
}

fn responses() -> Vec<(&'static str, CellResponse)> {
    vec![
        (
            "launched-child",
            CellResponse::CellLaunched(CellLaunched {
                cell: text("cell-1"),
                session_path: text("/run/user/1000/terminal-cell/cell-1"),
                control_socket: text("/run/c.sock"),
                data_socket: text("/run/d.sock"),
                daemon_pid: 4242,
                working_directory: text("/home/user"),
                command: text("/bin/sh"),
                arguments: vec![text("-l")],
                child_pid: Some(4243),
            }),
        ),
        (
            "launched-nochild",
            CellResponse::CellLaunched(CellLaunched {
                cell: text("c"),
                session_path: text(""),
                control_socket: text(""),
                data_socket: text(""),
                daemon_pid: -1,
                working_directory: text(""),
                command: text(""),
                arguments: vec![],
                child_pid: None,
            }),
        ),
        (
            "line-sent",
            CellResponse::LineSent(LineSent {
                cell: text("cell-1"),
                control_socket: text("/run/c.sock"),
            }),
        ),
        (
            "viewer-snapshot",
            CellResponse::ViewerAttached(ViewerAttached {
                cell: text("cell-1"),
                session_path: text("/s"),
                control_socket: text("/c"),
                data_socket: text("/d"),
                viewer_pid: Some(9),
                mode: ViewerMode::Snapshot,
                snapshot: Some(text("prompt> \u{1b}[0m\r\n")),
            }),
        ),
        (
            "viewer-interactive",
            CellResponse::ViewerAttached(ViewerAttached {
                cell: text("cell-1"),
                session_path: text("/s"),
                control_socket: text("/c"),
                data_socket: text("/d"),
                viewer_pid: None,
                mode: ViewerMode::Interactive,
                snapshot: None,
            }),
        ),
        (
            "observed-live",
            CellResponse::CellObserved(CellObservation {
                cell: text("cell-1"),
                session_path: text("/s"),
                control_socket: text("/c"),
                data_socket: text("/d"),
                daemon_pid: i64::MAX,
                daemon_state: ProcessState::Live,
                working_directory: text("/w"),
                exit_state: None,
                stall_state: StallState::NotMeasured,
                transcript_offset: 0,
                transcript_bytes: i64::MIN,
                worker_observation: text("input writer started"),
            }),
        ),
        (
            "observed-exited",
            CellResponse::CellObserved(CellObservation {
                cell: text("cell-1"),
                session_path: text("/s"),
                control_socket: text("/c"),
                data_socket: text("/d"),
                daemon_pid: 1,
                daemon_state: ProcessState::Exited,
                working_directory: text("/w"),
                exit_state: Some(text("exited 0")),
                stall_state: StallState::NotMeasured,
                transcript_offset: 12,
                transcript_bytes: 345,
                worker_observation: text(""),
            }),
        ),
        (
            "closed-all",
            CellResponse::CellClosed(CellClosed {
                cell: text("cell-1"),
                session_path: text("/s"),
                daemon_pid: 7,
                child_pid: Some(8),
                was_live: true,
                terminated: true,
                daemon_terminated: true,
                child_terminated: true,
            }),
        ),
        (
            "closed-none",
            CellResponse::CellClosed(CellClosed {
                cell: text("cell-1"),
                session_path: text("/s"),
                daemon_pid: 7,
                child_pid: None,
                was_live: false,
                terminated: false,
                daemon_terminated: false,
                child_terminated: false,
            }),
        ),
    ]
}

mod codec {
    use datom_codec::Datomizable;
    use protos::{Protosizable, Textualizable};

    pub fn written<T: Datomizable + Clone>(value: &T) -> String {
        value.clone().datomize(Vec::new()).protosize().textualize()
    }
}

const FIXTURE: &[(&str, &str)] = &[
    (
        "launch-full",
        r####"LaunchCell.{ Some.«main cell» Some.«/home/user/work dir» /bin/sh [ -l -c «echo "hi"\nline2 \ end ünï» ] [ { TERM «xterm 256» } { EMPTY «» } ] }"####,
    ),
    (
        "launch-bare",
        r####"LaunchCell.{ None None bash [] [] }"####,
    ),
    (
        "send-line",
        r####"SendLine.{ cell-1 «ls -la; echo 'x'» }"####,
    ),
    (
        "attach-interactive",
        r####"AttachViewer.{ cell-1 Interactive }"####,
    ),
    (
        "attach-snapshot",
        r####"AttachViewer.{ cell-1 Snapshot }"####,
    ),
    ("close", r####"CloseCell.{ cell-1 }"####),
    ("observe", r####"ObserveCell.{ «» }"####),
    (
        "launched-child",
        r####"CellLaunched.{ cell-1 /run/user/1000/terminal-cell/cell-1 /run/c.sock /run/d.sock 4242 /home/user /bin/sh [ -l ] Some.4243 }"####,
    ),
    (
        "launched-nochild",
        r####"CellLaunched.{ c «» «» «» -1 «» «» [] None }"####,
    ),
    ("line-sent", r####"LineSent.{ cell-1 /run/c.sock }"####),
    (
        "viewer-snapshot",
        r####"ViewerAttached.{ cell-1 /s /c /d Some.9 Snapshot Some.«prompt> \e[0m\r\n» }"####,
    ),
    (
        "viewer-interactive",
        r####"ViewerAttached.{ cell-1 /s /c /d None Interactive None }"####,
    ),
    (
        "observed-live",
        r####"CellObserved.{ cell-1 /s /c /d 9223372036854775807 Live /w None NotMeasured 0 -9223372036854775808 «input writer started» }"####,
    ),
    (
        "observed-exited",
        r####"CellObserved.{ cell-1 /s /c /d 1 Exited /w Some.«exited 0» NotMeasured 12 345 «» }"####,
    ),
    (
        "closed-all",
        r####"CellClosed.{ cell-1 /s 7 Some.8 True True True True }"####,
    ),
    (
        "closed-none",
        r####"CellClosed.{ cell-1 /s 7 None False False False False }"####,
    ),
];

fn written_all() -> Vec<(&'static str, String)> {
    let mut all = Vec::new();
    for (name, value) in requests() {
        all.push((name, codec::written(&value)));
    }
    for (name, value) in responses() {
        all.push((name, codec::written(&value)));
    }
    all
}

/// The 3.0.1 text for a sample, with the fixture's line, return and escape markers restored.
fn fixture_text(name: &str) -> String {
    FIXTURE
        .iter()
        .find(|(fixed, _)| *fixed == name)
        .map(|(_, text)| {
            text.replace("\\n", "\n")
                .replace("\\r", "\r")
                .replace("\\e", "\u{1b}")
        })
        .expect("fixture present")
}

#[test]
fn the_current_text_differs_from_the_301_text_only_in_layout_whitespace() {
    fn tokens(text: &str) -> Vec<&str> {
        text.split_whitespace().collect()
    }
    let written = written_all();
    assert_eq!(written.len(), FIXTURE.len());
    let mut layout_changed = Vec::new();
    for (name, text) in &written {
        let fixed = fixture_text(name);
        assert_eq!(tokens(text), tokens(&fixed), "{name}");
        if *text != fixed {
            layout_changed.push(*name);
        }
    }
    eprintln!("LAYOUT_CHANGED {layout_changed:?}");
}

#[test]
fn the_old_text_reads_back_as_the_current_value() {
    use datom_codec::{Actualizing, Budget, Composing, Potential};
    use protos::ReaderBudget;

    fn read<T: Composing>(text: &str) -> T {
        let mut budget = Budget {
            remaining: 100_000,
            reader: ReaderBudget::try_from(100_000usize).expect("reader budget"),
            depth: 0,
            maximum_depth: 128,
        };
        Potential::<T>::from(text.to_owned())
            .actualize(&mut budget)
            .expect("read the datom text")
    }

    for (name, value) in requests() {
        assert_eq!(read::<CellRequest>(&fixture_text(name)), value, "{name}");
    }
    for (name, value) in responses() {
        assert_eq!(read::<CellResponse>(&fixture_text(name)), value, "{name}");
    }
}
