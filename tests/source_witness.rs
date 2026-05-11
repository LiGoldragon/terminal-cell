struct SourceText<'source> {
    name: &'static str,
    text: &'source str,
}

impl<'source> SourceText<'source> {
    fn new(name: &'static str, text: &'source str) -> Self {
        Self { name, text }
    }

    fn between(&self, start_marker: &str, end_marker: &str) -> SourceSpan<'source> {
        let start = self
            .text
            .find(start_marker)
            .unwrap_or_else(|| panic!("{} misses start marker {start_marker:?}", self.name));
        let remaining = &self.text[start..];
        let end = remaining
            .find(end_marker)
            .unwrap_or_else(|| panic!("{} misses end marker {end_marker:?}", self.name));
        SourceSpan::new(self.name, &remaining[..end])
    }

    fn assert_contains(&self, needle: &str) {
        assert!(
            self.text.contains(needle),
            "{} should contain {needle:?}",
            self.name
        );
    }

    fn assert_excludes(&self, needle: &str) {
        assert!(
            !self.text.contains(needle),
            "{} should not contain {needle:?}",
            self.name
        );
    }
}

struct SourceSpan<'source> {
    name: &'static str,
    text: &'source str,
}

impl<'source> SourceSpan<'source> {
    fn new(name: &'static str, text: &'source str) -> Self {
        Self { name, text }
    }

    fn assert_contains(&self, needle: &str) {
        assert!(
            self.text.contains(needle),
            "{} span should contain {needle:?}",
            self.name
        );
    }

    fn assert_excludes(&self, needle: &str) {
        assert!(
            !self.text.contains(needle),
            "{} span should not contain {needle:?}",
            self.name
        );
    }

    fn assert_before(&self, first: &str, second: &str) {
        let first_index = self
            .text
            .find(first)
            .unwrap_or_else(|| panic!("{} span misses first marker {first:?}", self.name));
        let second_index = self
            .text
            .find(second)
            .unwrap_or_else(|| panic!("{} span misses second marker {second:?}", self.name));
        assert!(
            first_index < second_index,
            "{} span should put {first:?} before {second:?}",
            self.name
        );
    }
}

#[test]
fn live_attach_input_bypasses_actor_mailbox_and_terminal_semantics() {
    let daemon = SourceText::new(
        "terminal-cell-daemon.rs",
        include_str!("../src/bin/terminal-cell-daemon.rs"),
    );
    let attach_loop = daemon.between(
        "let mut buffer = [0_u8; 8192];",
        "    fn write_input(&mut self",
    );

    attach_loop.assert_contains("self.stream.read(&mut buffer)?");
    attach_loop.assert_contains("self.input_port");
    attach_loop.assert_contains(".accept(TerminalInput::new(");
    attach_loop.assert_contains("InputSource::Viewer");
    attach_loop.assert_excludes("self.terminal.ask");
    attach_loop.assert_excludes("self.terminal.tell");
    attach_loop.assert_excludes("Transcript");
    attach_loop.assert_excludes("ScreenProjection");
    attach_loop.assert_excludes("subscription");
    attach_loop.assert_excludes("WaitFor");
}

#[test]
fn live_attach_view_is_a_raw_stdin_stdout_pump() {
    let view = SourceText::new(
        "terminal-cell-view.rs",
        include_str!("../src/bin/terminal-cell-view.rs"),
    );
    let attach = view.between(
        "let output = thread::Builder::new()",
        "        output\n            .join()",
    );

    attach.assert_contains("output_stream.read(&mut buffer)?");
    attach.assert_contains("stdout.write_all(&buffer[..count])?");
    attach.assert_contains("stdout.flush()?");
    attach.assert_contains("let _raw_mode = TerminalRawMode::enter()?");
    attach.assert_contains("stdin.read(&mut buffer)?");
    attach.assert_contains("attach_stream.write_all(&buffer[..count])?");
    attach.assert_excludes("capture()");
    attach.assert_excludes("subscribe");
    attach.assert_excludes("Transcript");
    attach.assert_excludes("ScreenProjection");
    attach.assert_excludes("SocketRequestWriter");
}

#[test]
fn live_output_reaches_viewers_before_transcript_actor() {
    let session = SourceText::new("session.rs", include_str!("../src/session.rs"));
    let fanout = session.between(
        "fn write_bytes(&mut self, bytes: Vec<u8>)",
        "#[derive(Debug, Clone)]\npub struct TerminalCellSession",
    );

    fanout.assert_contains(".write_all(&bytes)");
    fanout.assert_contains("self.actor.tell(TerminalOutput::new(bytes)).try_send()");
    fanout.assert_before(
        ".write_all(&bytes)",
        "self.actor.tell(TerminalOutput::new(bytes)).try_send()",
    );
}

#[test]
fn terminal_input_writer_owns_the_pty_input_gate_without_actor_message_input() {
    let session = SourceText::new("session.rs", include_str!("../src/session.rs"));

    session.assert_contains("struct TerminalInputWriter");
    session.assert_contains("input_gate: TerminalInputGate");
    session.assert_contains("TerminalInputWriter::new(input_writer).run(input_receiver)");
    session.assert_excludes("impl Message<TerminalInput> for TerminalCell");
}
