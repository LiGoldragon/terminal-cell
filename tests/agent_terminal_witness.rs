use terminal_cell::{
    InputSource, TerminalCell, TerminalCommand, TerminalInput, TerminalLaunch, TerminalSize,
    TranscriptSnapshotRequest, WaitForTranscriptText,
};

struct AgentTerminalFixture {
    launch: TerminalLaunch,
}

impl AgentTerminalFixture {
    fn new() -> Self {
        Self {
            launch: TerminalLaunch::new(
                TerminalCommand::new(
                    env!("CARGO_BIN_EXE_agent-terminal-fixture"),
                    Vec::<String>::new(),
                ),
                TerminalSize::new(24, 80),
            ),
        }
    }

    fn spawn(self) -> terminal_cell::TerminalCellSession {
        TerminalCell::spawn_session(self.launch)
    }
}

#[tokio::test]
async fn agent_terminal_accepts_prompt_and_terminal_cell_reads_response() {
    let session = AgentTerminalFixture::new().spawn();
    let terminal = session.actor();

    assert!(
        terminal
            .ask(WaitForTranscriptText::new(b"agent-ready".to_vec()))
            .await
            .expect("agent readiness is observed"),
        "agent terminal became ready"
    );

    let acceptance = session
        .input_port()
        .accept(TerminalInput::new(
            b"hello terminal cell\r".to_vec(),
            InputSource::Programmatic,
        ))
        .expect("prompt accepted");
    assert_eq!(acceptance.source(), InputSource::Programmatic);

    assert!(
        terminal
            .ask(WaitForTranscriptText::new(
                b"agent-response: hello terminal cell".to_vec(),
            ))
            .await
            .expect("agent response is observed"),
        "terminal cell read the agent response from the PTY transcript"
    );

    let snapshot = terminal
        .ask(TranscriptSnapshotRequest)
        .await
        .expect("snapshot returned");
    assert!(snapshot.contains(b"agent-response: hello terminal cell"));
}

#[tokio::test]
async fn agent_terminal_usage_probe_is_prompt_input_not_terminal_semantics() {
    let session = AgentTerminalFixture::new().spawn();
    let terminal = session.actor();

    assert!(
        terminal
            .ask(WaitForTranscriptText::new(b"agent-ready".to_vec()))
            .await
            .expect("agent readiness is observed"),
        "agent terminal became ready"
    );

    session
        .input_port()
        .accept(TerminalInput::new(
            b"/usage\r".to_vec(),
            InputSource::Programmatic,
        ))
        .expect("usage probe accepted");

    assert!(
        terminal
            .ask(WaitForTranscriptText::new(
                b"usage-window: five-hour=73 weekly=41".to_vec(),
            ))
            .await
            .expect("usage response is observed"),
        "terminal cell carried the slash command as bytes and read the response"
    );
}
