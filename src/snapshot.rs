use crate::session::{TerminalSize, TranscriptSnapshot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenProjection {
    visible_text: String,
    cursor_row: u16,
    cursor_column: u16,
}

impl ScreenProjection {
    pub fn from_transcript(snapshot: &TranscriptSnapshot, size: TerminalSize) -> Self {
        let mut parser = vt100::Parser::new(size.rows(), size.columns(), 0);
        parser.process(snapshot.bytes());
        Self::from_screen(parser.screen())
    }

    pub(crate) fn from_screen(screen: &vt100::Screen) -> Self {
        let (cursor_row, cursor_column) = screen.cursor_position();
        Self {
            visible_text: screen.contents(),
            cursor_row,
            cursor_column,
        }
    }

    pub fn visible_text(&self) -> &str {
        self.visible_text.as_str()
    }

    pub fn cursor_row(&self) -> u16 {
        self.cursor_row
    }

    pub fn cursor_column(&self) -> u16 {
        self.cursor_column
    }
}
