//! The password prompt, typed at in a pseudo-terminal, against the
//! repository the real `restic` binary writes (password: restoric).
//! Skipped, with a note, when restic isn't installed; CI installs it.

mod common;

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::fixture::{Fixture, fixture};
use restoric::repo::rustic::{Connection, Locked, OpenOptions};

#[test]
fn locked_says_why() {
    let Some(f) = fixture() else { return };
    let open = |password: Option<&str>| {
        Connection::open(&OpenOptions {
            repo: Some(f.root.join("repo").to_string_lossy().into_owned()),
            password: password.map(|p| String::from(p).into()),
            cache_dir: Some(f.root.join("rustic-cache")),
            ..OpenOptions::default()
        })
        .err()
        .unwrap()
    };
    assert!(matches!(
        open(None).downcast_ref(),
        Some(Locked::NoPassword)
    ));
    assert!(matches!(
        open(Some("nope")).downcast_ref(),
        Some(Locked::WrongPassword)
    ));
}

const PROMPT: &str = "password for ";

/// Columns of the pseudo-terminal: narrower than any repository path here,
/// so the prompt has to shorten it.
const COLS: u16 = 30;

fn openpty() -> (OwnedFd, OwnedFd) {
    let (mut master, mut slave) = (0, 0);
    let mut size = libc::winsize {
        ws_row: 24,
        ws_col: COLS,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let r = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    assert_eq!(r, 0, "openpty: {}", std::io::Error::last_os_error());
    unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) }
}

fn echo_off(fd: &OwnedFd) -> bool {
    let mut t = unsafe { std::mem::zeroed::<libc::termios>() };
    unsafe { libc::tcgetattr(fd.as_raw_fd(), &mut t) == 0 && t.c_lflag & libc::ECHO == 0 }
}

fn wait_for(what: &str, timeout: Duration, mut ok: impl FnMut() -> bool, screen: &Mutex<Vec<u8>>) {
    let start = Instant::now();
    while !ok() {
        let shown = String::from_utf8_lossy(&screen.lock().unwrap()).into_owned();
        assert!(start.elapsed() < timeout, "no {what}; screen: {shown:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// `program` in a session of its own, with nothing from the environment
/// to unlock a repository, and its own home for every file it keeps.
fn command(program: &str, home: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.env("HOME", home);
    for var in [
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
        "XDG_DATA_HOME",
    ] {
        cmd.env(var, home);
    }
    for var in [
        "RESTIC_PASSWORD",
        "RESTIC_PASSWORD_FILE",
        "RESTIC_PASSWORD_COMMAND",
        "RESTIC_REPOSITORY",
        "RESTIC_REPOSITORY_FILE",
    ] {
        cmd.env_remove(var);
    }
    unsafe {
        cmd.pre_exec(|| match libc::setsid() {
            -1 => Err(std::io::Error::last_os_error()),
            _ => Ok(()),
        });
    }
    cmd
}

fn log_args(f: &Fixture) -> [std::ffi::OsString; 6] {
    [
        "log".into(),
        f.src().join("a.txt").into(),
        "--json".into(),
        "-r".into(),
        f.root.join("repo").into(),
        "--host=restoric-fixture".into(),
    ]
}

/// Runs `restoric log` in a pseudo-terminal, typing each of `keys` at its
/// own prompt, and returns the screen. A shell around it prints `RC=` and
/// `stty -a` after, from the same terminal.
fn run(f: &Fixture, keys: &[&str]) -> String {
    let (master, slave) = openpty();
    let home = tempfile::tempdir().unwrap();
    let mut cmd = command("sh", home.path());
    cmd.arg("-c")
        // `trap :` is a handler, not an ignore, so restoric still gets Ctrl-C.
        .arg(r#"trap : INT; "$0" "$@"; echo "RC=$?"; stty -a"#)
        .arg(env!("CARGO_BIN_EXE_restoric"))
        .args(log_args(f))
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave.try_clone().unwrap()));
    unsafe {
        // The terminal becomes the session's, for `/dev/tty`.
        cmd.pre_exec(|| match libc::ioctl(0, libc::TIOCSCTTY as _, 0) {
            -1 => Err(std::io::Error::last_os_error()),
            _ => Ok(()),
        });
    }
    let mut child = cmd.spawn().unwrap();
    drop(cmd);

    let screen = Arc::new(Mutex::new(Vec::new()));
    let mut reader = File::from(master.try_clone().unwrap());
    let shown = screen.clone();
    let reading = std::thread::spawn(move || {
        let mut buf = [0; 4096];
        while let Ok(n @ 1..) = reader.read(&mut buf) {
            shown.lock().unwrap().extend_from_slice(&buf[..n]);
        }
    });
    let prompts = || {
        String::from_utf8_lossy(&screen.lock().unwrap())
            .matches(PROMPT)
            .count()
    };
    let mut tty = File::from(master.try_clone().unwrap());
    for (n, k) in keys.iter().enumerate() {
        let long = Duration::from_secs(60);
        wait_for("prompt", long, || prompts() > n, &screen);
        // Raw mode is on, and the typed-ahead flush done, before typing.
        wait_for("raw mode", long, || echo_off(&slave), &screen);
        std::thread::sleep(Duration::from_millis(200));
        tty.write_all(k.as_bytes()).unwrap();
    }
    // The terminal ends when only the master is left.
    drop(slave);
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(120) {
            let _ = child.kill();
            let shown = String::from_utf8_lossy(&screen.lock().unwrap()).into_owned();
            panic!("restoric hung; screen: {shown:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    drop((tty, master));
    reading.join().unwrap();
    String::from_utf8_lossy(&screen.lock().unwrap()).into_owned()
}

fn rc(screen: &str) -> &str {
    screen
        .split("RC=")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or("?")
}

/// The terminal is cooked again, with echo: not left as the prompt set it.
fn cooked(screen: &str) -> bool {
    let words: Vec<&str> = screen.split_whitespace().collect();
    ["echo", "icanon", "isig"].iter().all(|w| words.contains(w))
}

#[test]
fn asks_when_nothing_gives_the_password() {
    let Some(f) = fixture() else { return };

    let s = run(f, &["restoric\r"]);
    assert_eq!(rc(&s), "0", "{s}");
    assert!(s.contains("\"snapshot\""), "{s}");
    // One line, the middle of the location cut, and nothing echoed after it.
    let line = s.split(PROMPT).nth(1).and_then(|l| l.split("\r\n").next());
    let line = format!("{PROMPT}{}", line.unwrap_or_default());
    assert!(line.chars().count() < usize::from(COLS), "{line:?}");
    assert!(line.contains('…') && line.ends_with("/repo: "), "{line:?}");
    assert!(cooked(&s), "{s}");

    let s = run(f, &["nope\r", "restoric\r"]);
    assert_eq!(rc(&s), "0", "{s}");
    assert!(s.contains("wrong password, 2 tries left"), "{s}");
    assert!(!s.contains("nope"), "echoed: {s}");

    // Ctrl-C stops restoric quietly and leaves the terminal as it was.
    let s = run(f, &["re\x03"]);
    assert_eq!(rc(&s), "130", "{s}");
    assert!(cooked(&s), "{s}");

    // Three wrong tries.
    let s = run(f, &["a\r", "b\r", "c\r"]);
    assert_eq!(rc(&s), "1", "{s}");
    assert!(s.contains("1 try left"), "{s}");
    assert!(s.contains("restoric: wrong password"), "{s}");
}

/// As under cron: no terminal at all, so the old error, and no hang.
#[test]
fn no_terminal_no_prompt() {
    let Some(f) = fixture() else { return };
    let home = tempfile::tempdir().unwrap();
    let mut child = command(env!("CARGO_BIN_EXE_restoric"), home.path())
        .args(log_args(f))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            panic!("restoric hung");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    // The error, with no prompt before it.
    assert!(err.starts_with("restoric: no password given"), "{err}");
}
