use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, Receiver};
use std::thread;

struct OutputFloodFixture<Writer> {
    input: Receiver<String>,
    writer: Writer,
}

impl<Writer> OutputFloodFixture<Writer>
where
    Writer: Write,
{
    fn new(input: Receiver<String>, writer: Writer) -> Self {
        Self { input, writer }
    }

    fn run(&mut self) -> io::Result<()> {
        writeln!(self.writer, "flood-ready\r")?;
        self.writer.flush()?;

        for sequence in 0..100_000_u32 {
            self.write_pending_input()?;
            writeln!(
                self.writer,
                "flood-{sequence:05} abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz\r"
            )?;
            if sequence % 64 == 0 {
                self.writer.flush()?;
            }
        }

        self.write_pending_input()?;
        writeln!(self.writer, "flood-complete\r")?;
        self.writer.flush()
    }

    fn write_pending_input(&mut self) -> io::Result<()> {
        while let Ok(line) = self.input.try_recv() {
            writeln!(self.writer, "latency-response:{line}\r")?;
            self.writer.flush()?;
        }
        Ok(())
    }
}

struct InputReader<Reader> {
    reader: Reader,
    sender: mpsc::Sender<String>,
}

impl<Reader> InputReader<Reader>
where
    Reader: BufRead + Send + 'static,
{
    fn new(reader: Reader, sender: mpsc::Sender<String>) -> Self {
        Self { reader, sender }
    }

    fn spawn(self) -> thread::JoinHandle<io::Result<()>> {
        thread::Builder::new()
            .name("output-flood-fixture-input".to_string())
            .spawn(move || self.run())
            .expect("input reader thread spawned")
    }

    fn run(mut self) -> io::Result<()> {
        let mut line = String::new();
        loop {
            line.clear();
            let count = self.reader.read_line(&mut line)?;
            if count == 0 {
                break;
            }
            let input = line.trim_end_matches(&['\r', '\n'][..]).to_string();
            if self.sender.send(input).is_err() {
                break;
            }
        }
        Ok(())
    }
}

fn main() {
    let (sender, receiver) = mpsc::channel();
    let _input = InputReader::new(io::BufReader::new(io::stdin()), sender).spawn();

    let stdout = io::stdout();
    let mut fixture = OutputFloodFixture::new(receiver, stdout.lock());
    let result = fixture.run();

    if let Err(error) = result {
        eprintln!("output-flood-fixture failed: {error}");
        std::process::exit(1);
    }
}
