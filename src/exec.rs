use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

fn drain<R: Read + Send + 'static>(r: Option<R>) -> Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut r) = r {
            let _ = r.read_to_end(&mut buf);
        }
        let _ = tx.send(buf);
    });
    rx
}

/// Runs `args`, returning (exit code, stdout then stderr). Spawn failure or timeout gives (1, error text).
pub fn run(args: &[&str], timeout: Duration) -> (i32, String) {
    run_with(args, None, timeout)
}

/// `run` with `input` written to the child's stdin, which is then closed.
#[allow(dead_code)] // consumed by the display, lid and light paths in the next tasks
pub fn run_input(args: &[&str], input: &str, timeout: Duration) -> (i32, String) {
    run_with(args, Some(input), timeout)
}

fn run_with(args: &[&str], input: Option<&str>, timeout: Duration) -> (i32, String) {
    let Some((prog, rest)) = args.split_first() else {
        return (1, "empty command".into());
    };
    let mut child = match Command::new(prog)
        .args(rest)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return (1, e.to_string()),
    };
    if let (Some(mut stdin), Some(input)) = (child.stdin.take(), input) {
        let input = input.to_owned();
        // Off-thread so a child that never reads cannot stall past the deadline; dropping `stdin` sends EOF.
        thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return (1, format!("{prog} timed out after {}s", timeout.as_secs()));
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(e) => return (1, e.to_string()),
        }
    };
    // A background grandchild can hold the pipes open past the child's exit; the deadline covers EOF too.
    let timed_out = || (1, format!("{prog} timed out after {}s", timeout.as_secs()));
    let left = || deadline.saturating_duration_since(Instant::now());
    let Ok(out) = out.recv_timeout(left()) else { return timed_out() };
    let Ok(err) = err.recv_timeout(left()) else { return timed_out() };
    let text = format!("{}{}", String::from_utf8_lossy(&out), String::from_utf8_lossy(&err));
    (status.code().unwrap_or(1), text)
}

pub fn executable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// `shutil.which`: an executable regular file on `PATH`.
pub fn on_path(tool: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|d| executable(&d.join(tool))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_both_streams_and_code() {
        let (code, out) = run(&["sh", "-c", "echo a; echo b >&2; exit 3"], Duration::from_secs(2));
        assert_eq!((code, out.as_str()), (3, "a\nb\n"));
    }

    #[test]
    fn input_reaches_the_child() {
        assert_eq!(run_input(&["cat"], "x", Duration::from_secs(2)), (0, "x".into()));
    }

    #[test]
    fn spawn_failure_and_timeout_give_code_1() {
        assert_eq!(run(&["/nonexistent/x"], Duration::from_secs(1)).0, 1);
        let (code, out) = run(&["sleep", "5"], Duration::from_millis(50));
        assert_eq!(code, 1);
        assert!(out.contains("timed out"));
    }

    #[test]
    fn timeout_covers_pipes_held_by_a_background_child() {
        let t = Instant::now();
        let (code, _) = run(&["sh", "-c", "sleep 3 & echo started"], Duration::from_millis(200));
        assert_eq!(code, 1);
        assert!(t.elapsed() < Duration::from_secs(1));
    }
}
