use std::env;
use std::error::Error;
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

type SessionSelectResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

enum SelectionMode {
    NewestLiveUnder(PathBuf),
    ExactLiveSession(PathBuf),
}

struct SelectionArguments {
    mode: SelectionMode,
}

impl SelectionArguments {
    fn from_environment() -> SessionSelectResult<Self> {
        let mut arguments = env::args().skip(1);
        let mut mode = None;

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--root" => {
                    Self::set_mode(
                        &mut mode,
                        SelectionMode::NewestLiveUnder(PathBuf::from(
                            arguments.next().ok_or(
                                "terminal-cell-session-select requires a path after --root",
                            )?,
                        )),
                    )?;
                }
                "--session" => {
                    Self::set_mode(
                        &mut mode,
                        SelectionMode::ExactLiveSession(PathBuf::from(arguments.next().ok_or(
                            "terminal-cell-session-select requires a path after --session",
                        )?)),
                    )?;
                }
                other => {
                    return Err(format!("unknown session-select argument: {other}").into());
                }
            }
        }

        Ok(Self {
            mode: mode
                .ok_or("terminal-cell-session-select requires --root <path> or --session <path>")?,
        })
    }

    fn set_mode(mode: &mut Option<SelectionMode>, value: SelectionMode) -> SessionSelectResult<()> {
        if mode.is_some() {
            return Err("terminal-cell-session-select accepts only one selection mode".into());
        }
        *mode = Some(value);
        Ok(())
    }
}

struct RuntimeSession {
    path: PathBuf,
}

impl RuntimeSession {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn is_live(&self) -> bool {
        self.socket_is_present() && self.daemon_is_live()
    }

    fn socket_is_present(&self) -> bool {
        fs::symlink_metadata(self.path.join("cell.sock"))
            .map(|metadata| metadata.file_type().is_socket())
            .unwrap_or(false)
    }

    fn daemon_is_live(&self) -> bool {
        let Ok(pid_text) = fs::read_to_string(self.path.join("daemon.pid")) else {
            return false;
        };
        let Ok(pid) = pid_text.trim().parse::<u32>() else {
            return false;
        };
        process_is_alive(pid)
    }

    fn modified(&self) -> SystemTime {
        fs::metadata(&self.path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH)
    }
}

struct SessionSelector;

impl SessionSelector {
    fn select(arguments: SelectionArguments) -> SessionSelectResult<PathBuf> {
        match arguments.mode {
            SelectionMode::NewestLiveUnder(root) => Self::newest_live_under(root.as_path()),
            SelectionMode::ExactLiveSession(path) => Self::exact_live_session(path),
        }
    }

    fn exact_live_session(path: PathBuf) -> SessionSelectResult<PathBuf> {
        let session = RuntimeSession::new(path);
        if session.is_live() {
            Ok(session.path)
        } else {
            Err(format!(
                "terminal-cell session is not live: {}",
                session.path.display()
            )
            .into())
        }
    }

    fn newest_live_under(root: &Path) -> SessionSelectResult<PathBuf> {
        if !root.is_dir() {
            return Err(format!("no terminal-cell sessions under {}", root.display()).into());
        }

        let mut newest: Option<RuntimeSession> = None;
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() || !Self::looks_like_session_directory(path.as_path()) {
                continue;
            }

            let candidate = RuntimeSession::new(path);
            if !candidate.is_live() {
                continue;
            }

            if newest
                .as_ref()
                .is_none_or(|current| candidate.modified() > current.modified())
            {
                newest = Some(candidate);
            }
        }

        newest.map(|session| session.path).ok_or_else(|| {
            format!("no live terminal-cell sessions under {}", root.display()).into()
        })
    }

    fn looks_like_session_directory(path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("session-"))
    }
}

#[cfg(target_os = "linux")]
fn process_is_alive(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

#[cfg(not(target_os = "linux"))]
fn process_is_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn main() {
    match SelectionArguments::from_environment().and_then(SessionSelector::select) {
        Ok(path) => println!("{}", path.display()),
        Err(error) => {
            eprintln!("terminal-cell-session-select failed: {error}");
            std::process::exit(1);
        }
    }
}
