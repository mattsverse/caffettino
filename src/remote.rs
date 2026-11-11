//! The `caffettino start|stop|status` command line, and the socket the running app answers it on.
//!
//! The command line never launches `caffeinate` itself: it asks the running app, which owns the
//! one child (watching the app's pid) that the window and menu bar control. One request line goes
//! in and one reply line comes back over a Unix socket in the app's support folder.
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const SOCKET: &str = "caffettino.sock";
/// How long `start` waits for a freshly opened app to listen.
const LAUNCH_WAIT: Duration = Duration::from_secs(10);
/// How long either side waits on the other once connected.
const IO_TIMEOUT: Duration = Duration::from_secs(5);

pub const USAGE: &str = "\
Usage: caffettino <command>

Commands:
  start [TIME]  Brew a coffee break: until you stop it, or for one of your
                presets, as minutes (25, 25m) or hours (1h, 1h30)
  stop          Let your Mac snooze
  status        Show the coffee break
  help          Show this help

The commands drive the menu bar app's own coffee break, opening it if needed.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Minutes of a saved preset, or `None` for "Until I stop".
    Start(Option<u32>),
    Stop,
    Status,
}

impl Request {
    fn encode(self) -> String {
        match self {
            Self::Start(None) => "start".into(),
            Self::Start(Some(minutes)) => format!("start {minutes}"),
            Self::Stop => "stop".into(),
            Self::Status => "status".into(),
        }
    }

    fn decode(line: &str) -> Option<Self> {
        let mut words = line.split_whitespace();
        let request = match (words.next()?, words.next()) {
            ("start", None) => Self::Start(None),
            ("start", Some(minutes)) => Self::Start(Some(minutes.parse().ok()?)),
            ("stop", None) => Self::Stop,
            ("status", None) => Self::Status,
            _ => return None,
        };
        words.next().is_none().then_some(request)
    }
}

/// The app's answer: whether the request did what it asked, and a line for the person.
pub type Reply = Result<String, String>;

/// Minutes from `25`, `25m`, `25min`, `1h`, `1h30` or `1h30m`. Presets are checked by the app.
pub fn parse_time(text: &str) -> Option<u32> {
    let text = text.trim().to_ascii_lowercase();
    let number = |digits: &str| {
        (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| digits.parse::<u32>().ok())
            .flatten()
    };
    let minutes = |rest: &str| {
        number(
            rest.strip_suffix("min")
                .or_else(|| rest.strip_suffix('m'))
                .unwrap_or(rest),
        )
    };
    let total = match text.split_once('h') {
        Some((hours, "")) => number(hours)?.checked_mul(60)?,
        Some((hours, rest)) => number(hours)?
            .checked_mul(60)?
            .checked_add(minutes(rest)?)?,
        None => minutes(&text)?,
    };
    (total > 0).then_some(total)
}

/// Parses the command line (without the program name), or explains what's wrong.
pub fn parse_args(args: &[String]) -> Result<Option<Request>, String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["help" | "-h" | "--help"] => Ok(None),
        ["start"] => Ok(Some(Request::Start(None))),
        ["start", time] => parse_time(time)
            .map(|minutes| Some(Request::Start(Some(minutes))))
            .ok_or_else(|| format!("{time:?} isn't a time. Try 25, 25m or 1h.")),
        ["stop"] => Ok(Some(Request::Stop)),
        ["status"] => Ok(Some(Request::Status)),
        _ => Err(USAGE.into()),
    }
}

/// Whether these arguments are a command-line command rather than a launch of the app.
pub fn is_command(args: &[String]) -> bool {
    args.first().is_some_and(|first| {
        matches!(
            first.as_str(),
            "start" | "stop" | "status" | "help" | "-h" | "--help"
        )
    })
}

pub fn socket_path(support: &Path) -> PathBuf {
    support.join(SOCKET)
}

/// Runs a command-line command and returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    let request = match parse_args(args) {
        Ok(Some(request)) => request,
        Ok(None) => {
            println!("{USAGE}");
            return 0;
        }
        Err(message) => {
            eprintln!("{message}");
            return 2;
        }
    };
    let Some(socket) = crate::presets::support_dir().map(|dir| socket_path(&dir)) else {
        eprintln!("Caffettino needs HOME to find the app.");
        return 1;
    };
    match ask(&socket, request) {
        Ok(Ok(message)) => {
            println!("{message}");
            0
        }
        Ok(Err(message)) => {
            eprintln!("{message}");
            1
        }
        Err(error) => {
            eprintln!("Caffettino: {error}");
            1
        }
    }
}

/// Sends `request` to the running app, opening it first for `start`.
fn ask(socket: &Path, request: Request) -> io::Result<Reply> {
    let stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(error) if not_running(&error) => match request {
            Request::Start(_) => {
                launch_app()?;
                wait_for(socket)?
            }
            Request::Stop | Request::Status => {
                return Ok(Ok(
                    "Caffettino isn't running, so your Mac can snooze.".into()
                ));
            }
        },
        Err(error) => return Err(error),
    };
    exchange(stream, request)
}

fn not_running(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    )
}

fn exchange(mut stream: UnixStream, request: Request) -> io::Result<Reply> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    writeln!(stream, "{}", request.encode())?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    decode_reply(line.trim_end_matches('\n'))
        .ok_or_else(|| io::Error::other("the app gave an answer this command doesn't understand"))
}

fn encode_reply(reply: &Reply) -> String {
    let (status, message) = match reply {
        Ok(message) => ("ok", message),
        Err(message) => ("error", message),
    };
    // One line per reply: messages never span lines.
    format!("{status} {}\n", message.replace('\n', " "))
}

fn decode_reply(line: &str) -> Option<Reply> {
    let (status, message) = line.split_once(' ')?;
    match status {
        "ok" => Some(Ok(message.into())),
        "error" => Some(Err(message.into())),
        _ => None,
    }
}

/// Opens the app bundle this command runs from (following Homebrew's symlink), in the background.
fn launch_app() -> io::Result<()> {
    let exe = std::env::current_exe()?.canonicalize()?;
    let bundle = exe
        .ancestors()
        .find(|dir| dir.extension().is_some_and(|ext| ext == "app"))
        .ok_or_else(|| io::Error::other("Caffettino isn't running. Open it, then try again."))?;
    let status = std::process::Command::new("/usr/bin/open")
        .arg("-g")
        .arg("-a")
        .arg(bundle)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "couldn't open {}",
            bundle.display()
        )))
    }
}

fn wait_for(socket: &Path) -> io::Result<UnixStream> {
    let deadline = Instant::now() + LAUNCH_WAIT;
    loop {
        match UnixStream::connect(socket) {
            Ok(stream) => return Ok(stream),
            Err(error) if not_running(&error) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) if not_running(&error) => {
                return Err(io::Error::other(
                    "Caffettino opened but didn't answer. Try again.",
                ));
            }
            Err(error) => return Err(error),
        }
    }
}

/// Starts answering the command line on a background thread. `answer` is called once per
/// request and must send exactly one reply. Leaves another running copy's socket alone.
pub fn listen<F>(socket: &Path, answer: F) -> io::Result<()>
where
    F: Fn(Request, mpsc::Sender<Reply>) + Send + 'static,
{
    let listener = bind(socket)?;
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    if let Err(error) = serve(&stream, &answer) {
                        eprintln!("Caffettino command line: {error}");
                    }
                }
                Err(error) => eprintln!("Caffettino command line: {error}"),
            }
        }
    });
    Ok(())
}

fn bind(socket: &Path) -> io::Result<UnixListener> {
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match UnixStream::connect(socket) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "another Caffettino is answering the command line",
            ));
        }
        // A socket left by a copy that has quit.
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
            std::fs::remove_file(socket)?;
        }
        Err(_) => {}
    }
    let listener = UnixListener::bind(socket)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(listener)
}

fn serve<F>(stream: &UnixStream, answer: &F) -> io::Result<()>
where
    F: Fn(Request, mpsc::Sender<Reply>),
{
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let reply = match Request::decode(&line) {
        Some(request) => {
            let (send, receive) = mpsc::channel();
            answer(request, send);
            receive
                .recv_timeout(IO_TIMEOUT)
                .unwrap_or_else(|_| Err("Caffettino is busy. Try again.".into()))
        }
        None => Err("Caffettino didn't understand that command.".into()),
    };
    let mut stream = stream;
    stream.write_all(encode_reply(&reply).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn times_read_as_minutes_or_hours() {
        for (text, minutes) in [
            ("25", 25),
            ("25m", 25),
            ("25min", 25),
            ("1h", 60),
            ("1H30", 90),
            ("1h05m", 65),
            ("24h", 1440),
        ] {
            assert_eq!(parse_time(text), Some(minutes), "{text}");
        }
        for bad in [
            "",
            "0",
            "0h",
            "h",
            "m",
            "-5",
            "1.5h",
            "1h1h",
            "abc",
            "99999999999",
        ] {
            assert_eq!(parse_time(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_command_line_names_one_request() {
        assert_eq!(parse_args(&args("start")), Ok(Some(Request::Start(None))));
        assert_eq!(
            parse_args(&args("start 1h")),
            Ok(Some(Request::Start(Some(60))))
        );
        assert_eq!(parse_args(&args("stop")), Ok(Some(Request::Stop)));
        assert_eq!(parse_args(&args("status")), Ok(Some(Request::Status)));
        assert_eq!(parse_args(&args("--help")), Ok(None));
        assert!(parse_args(&args("start soon")).is_err());
        assert!(parse_args(&args("stop now")).is_err());
        assert!(is_command(&args("start 25")));
        assert!(!is_command(&args("--snap target/resting.png")));
        assert!(!is_command(&[]));
    }

    #[test]
    fn requests_and_replies_survive_the_wire() {
        for request in [
            Request::Start(None),
            Request::Start(Some(25)),
            Request::Stop,
            Request::Status,
        ] {
            assert_eq!(Request::decode(&request.encode()), Some(request));
        }
        assert_eq!(Request::decode("start 25 60"), None);
        assert_eq!(Request::decode("brew"), None);
        for reply in [Ok("Brewing.".to_owned()), Err("No such preset.".to_owned())] {
            let line = encode_reply(&reply);
            assert_eq!(decode_reply(line.trim_end_matches('\n')), Some(reply));
        }
    }

    #[test]
    fn the_app_answers_over_its_socket_and_a_second_copy_leaves_it_alone() {
        let dir = crate::presets::tests::Dir::new("remote");
        let socket = socket_path(&dir.0);
        listen(&socket, |request, reply: mpsc::Sender<Reply>| {
            let _ = reply.send(match request {
                Request::Start(Some(minutes)) => Ok(format!("Brewing {minutes}.")),
                _ => Err("Nope.".into()),
            });
        })
        .unwrap();
        assert_eq!(
            ask(&socket, Request::Start(Some(25))).unwrap(),
            Ok("Brewing 25.".into())
        );
        assert_eq!(ask(&socket, Request::Stop).unwrap(), Err("Nope.".into()));
        let second = listen(&socket, |_, _| {});
        assert_eq!(second.unwrap_err().kind(), io::ErrorKind::AddrInUse);
        assert_eq!(
            ask(&socket, Request::Status).unwrap(),
            Err("Nope.".into()),
            "the first copy still answers"
        );
    }

    #[test]
    fn without_the_app_stop_and_status_say_so_and_a_stale_socket_is_replaced() {
        let dir = crate::presets::tests::Dir::new("stale");
        let socket = socket_path(&dir.0);
        assert!(ask(&socket, Request::Status).unwrap().is_ok());
        std::fs::create_dir_all(&dir.0).unwrap();
        drop(UnixListener::bind(&socket).unwrap());
        assert!(ask(&socket, Request::Stop).unwrap().is_ok());
        listen(&socket, |_, reply: mpsc::Sender<Reply>| {
            let _ = reply.send(Ok("Resting.".into()));
        })
        .unwrap();
        assert_eq!(
            ask(&socket, Request::Status).unwrap(),
            Ok("Resting.".into())
        );
    }
}
