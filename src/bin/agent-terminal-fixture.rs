use std::io::{self, BufRead, Write};

struct AgentTerminalFixture<Reader, Writer> {
    reader: Reader,
    writer: Writer,
}

impl<Reader, Writer> AgentTerminalFixture<Reader, Writer>
where
    Reader: BufRead,
    Writer: Write,
{
    fn new(reader: Reader, writer: Writer) -> Self {
        Self { reader, writer }
    }

    fn run(&mut self) -> io::Result<()> {
        self.write_ready()?;
        let mut line = String::new();
        loop {
            line.clear();
            let count = self.reader.read_line(&mut line)?;
            if count == 0 {
                break;
            }
            let prompt = line.trim_end_matches(&['\r', '\n'][..]).to_string();
            self.write_response(prompt.as_str())?;
        }
        Ok(())
    }

    fn write_ready(&mut self) -> io::Result<()> {
        self.writer.write_all(b"agent-ready\r\n")?;
        self.writer.flush()
    }

    fn write_response(&mut self, prompt: &str) -> io::Result<()> {
        if prompt == "/usage" {
            self.writer
                .write_all(b"usage-window: five-hour=73 weekly=41\r\n")?;
        } else {
            writeln!(self.writer, "agent-response: {prompt}\r")?;
        }
        self.write_ready()
    }
}

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let reader = io::BufReader::new(stdin.lock());
    let writer = stdout.lock();
    let mut fixture = AgentTerminalFixture::new(reader, writer);
    if let Err(error) = fixture.run() {
        eprintln!("agent-terminal-fixture failed: {error}");
        std::process::exit(1);
    }
}
