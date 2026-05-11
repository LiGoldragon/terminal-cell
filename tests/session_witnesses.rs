use std::env;

use kameo::actor::ActorRef;

use terminal_cell::{
    InputSource, ScreenProjectionRequest, TerminalCell, TerminalCellSession, TerminalCommand,
    TerminalExitRequest, TerminalInput, TerminalLaunch, TerminalSequence, TerminalSize,
    TerminalWorkerKind, TerminalWorkerObservationRequest, TerminalWorkerStop,
    TranscriptSnapshotRequest, TranscriptSubscriptionRequest, WaitForTerminalExit,
    WaitForTerminalWorkerStop, WaitForTranscriptText,
};

struct TerminalFixture {
    launch: TerminalLaunch,
}

impl TerminalFixture {
    fn shell(arguments: &str) -> Self {
        let shell = env::var("TERMINAL_CELL_TEST_SHELL").unwrap_or_else(|_| "bash".to_string());
        Self {
            launch: TerminalLaunch::new(
                TerminalCommand::new(shell, vec!["-lc".to_string(), arguments.to_string()]),
                TerminalSize::new(24, 80),
            ),
        }
    }

    fn spawn_shell(arguments: &str) -> ActorRef<TerminalCell> {
        Self::shell(arguments).spawn_cell()
    }

    fn spawn_shell_session(arguments: &str) -> TerminalCellSession {
        Self::shell(arguments).spawn_session()
    }

    fn spawn_cell(self) -> ActorRef<TerminalCell> {
        self.spawn_session().actor()
    }

    fn spawn_session(self) -> TerminalCellSession {
        TerminalCell::spawn_session(self.launch)
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
    let session =
        TerminalFixture::spawn_shell_session("IFS= read -r line; printf 'seen:%s\\n' \"$line\"");
    let terminal = session.actor();

    let acceptance = session
        .input_port()
        .accept(TerminalInput::new(
            b"/usage\r".to_vec(),
            InputSource::Programmatic,
        ))
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

#[tokio::test]
async fn terminal_worker_lifecycle_is_actor_observable() {
    let terminal = TerminalFixture::spawn_shell("printf 'worker-before-exit\\n'; exit 3");

    assert!(
        terminal
            .ask(WaitForTranscriptText::new(b"worker-before-exit".to_vec()))
            .await
            .expect("wait message delivered"),
        "fixture output reached transcript before exit"
    );

    let output_reader_stop = terminal
        .ask(WaitForTerminalWorkerStop::new(
            TerminalWorkerKind::OutputReader,
        ))
        .await
        .expect("output reader stop wait delivered");
    assert_eq!(output_reader_stop, TerminalWorkerStop::OutputReaderFinished);

    let child_exit_stop = terminal
        .ask(WaitForTerminalWorkerStop::new(
            TerminalWorkerKind::ChildExitWatcher,
        ))
        .await
        .expect("child exit watcher stop wait delivered");
    assert!(
        matches!(child_exit_stop, TerminalWorkerStop::ChildExited(_)),
        "child exit watcher records the real child wait result"
    );

    let observation = terminal
        .ask(TerminalWorkerObservationRequest)
        .await
        .expect("worker observation returned");

    for worker in [
        TerminalWorkerKind::InputWriter,
        TerminalWorkerKind::OutputFanout,
        TerminalWorkerKind::OutputReader,
        TerminalWorkerKind::ChildExitWatcher,
    ] {
        assert!(
            observation.has_started(worker),
            "{worker:?} start is reported through the TerminalCell actor"
        );
    }

    assert_eq!(
        observation.stopped_reason(TerminalWorkerKind::OutputReader),
        Some(&TerminalWorkerStop::OutputReaderFinished)
    );
    assert!(
        matches!(
            observation.stopped_reason(TerminalWorkerKind::ChildExitWatcher),
            Some(TerminalWorkerStop::ChildExited(_))
        ),
        "child exit watcher stop is reported through the TerminalCell actor"
    );
}
