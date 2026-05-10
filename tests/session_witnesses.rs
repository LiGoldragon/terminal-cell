use kameo::actor::ActorRef;

use terminal_cell::{
    InputSource, ScreenProjectionRequest, TerminalCell, TerminalCommand, TerminalExitRequest,
    TerminalInput, TerminalLaunch, TerminalSequence, TerminalSize, TranscriptSnapshotRequest,
    TranscriptSubscriptionRequest, WaitForTerminalExit, WaitForTranscriptText,
};

struct TerminalFixture;

impl TerminalFixture {
    fn shell(arguments: &str) -> TerminalLaunch {
        TerminalLaunch::new(
            TerminalCommand::new("sh", vec!["-lc".to_string(), arguments.to_string()]),
            TerminalSize::new(24, 80),
        )
    }

    fn spawn_shell(arguments: &str) -> ActorRef<TerminalCell> {
        TerminalCell::spawn_cell(Self::shell(arguments))
    }
}

#[tokio::test]
async fn detached_output_is_replayed_to_late_subscriber() {
    let terminal = TerminalFixture::spawn_shell(
        "printf 'before-detached\\n'; sleep 0.1; printf 'after-detached\\n'",
    );

    assert!(
        terminal
            .ask(WaitForTranscriptText::new(b"after-detached".to_vec()))
            .await
            .expect("wait message delivered"),
        "terminal output reached transcript"
    );

    let subscription = terminal
        .ask(TranscriptSubscriptionRequest::from_beginning())
        .await
        .expect("subscription created");
    let replayed = String::from_utf8_lossy(&subscription.replay_bytes()).into_owned();

    assert!(replayed.contains("before-detached"));
    assert!(replayed.contains("after-detached"));
}

#[tokio::test]
async fn programmatic_input_uses_the_same_pty_input_port() {
    let terminal = TerminalFixture::spawn_shell("IFS= read -r line; printf 'seen:%s\\n' \"$line\"");

    let acceptance = terminal
        .ask(TerminalInput::new(
            b"/usage\r".to_vec(),
            InputSource::Programmatic,
        ))
        .await
        .expect("input accepted");
    assert_eq!(acceptance.source(), InputSource::Programmatic);

    assert!(
        terminal
            .ask(WaitForTranscriptText::new(b"seen:/usage".to_vec()))
            .await
            .expect("wait message delivered"),
        "programmatic input passed through the PTY"
    );
}

#[tokio::test]
async fn screen_projection_is_derived_from_transcript() {
    let terminal = TerminalFixture::spawn_shell("printf 'alpha\\nbeta\\n'");

    assert!(
        terminal
            .ask(WaitForTranscriptText::new(b"beta".to_vec()))
            .await
            .expect("wait message delivered"),
        "fixture output reached transcript"
    );

    let snapshot = terminal
        .ask(TranscriptSnapshotRequest)
        .await
        .expect("snapshot returned");
    assert!(snapshot.contains(b"alpha"));
    assert!(snapshot.contains(b"beta"));

    let projection = terminal
        .ask(ScreenProjectionRequest::new(TerminalSize::new(24, 80)))
        .await
        .expect("projection returned");

    assert!(projection.visible_text().contains("alpha"));
    assert!(projection.visible_text().contains("beta"));
    assert!(snapshot.last_sequence() > TerminalSequence::new(0));
}

#[tokio::test]
async fn terminal_exit_is_observable_without_polling_the_child() {
    let terminal = TerminalFixture::spawn_shell("printf 'before-exit\\n'; exit 7");

    assert!(
        terminal
            .ask(WaitForTranscriptText::new(b"before-exit".to_vec()))
            .await
            .expect("wait message delivered"),
        "fixture output reached transcript before exit"
    );

    let exit = terminal
        .ask(WaitForTerminalExit)
        .await
        .expect("exit wait delivered");
    assert!(
        !exit.status().trim().is_empty(),
        "terminal exit status is recorded"
    );

    let observed = terminal
        .ask(TerminalExitRequest)
        .await
        .expect("exit snapshot delivered")
        .expect("terminal exit already recorded");
    assert_eq!(observed, exit);
}
