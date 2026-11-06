use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn drain<R: Read + Send + 'static>(r: Option<R>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut r) = r {
            let _ = r.read_to_end(&mut buf);
        }
        buf
    })
}

/// Runs `args`, returning (exit code, stdout then stderr). Spawn failure or timeout gives (1, error text).
pub fn run(args: &[&str], timeout: Duration) -> (i32, String) {
    let Some((prog, rest)) = args.split_first() else {
        return (1, "empty command".into());
    };
    let mut child = match Command::new(prog)
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return (1, e.to_string()),
    };
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
    let mut text = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    text.push_str(&String::from_utf8_lossy(&err.join().unwrap_or_default()));
    (status.code().unwrap_or(1), text)
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
    fn spawn_failure_and_timeout_give_code_1() {
        assert_eq!(run(&["/nonexistent/x"], Duration::from_secs(1)).0, 1);
        let (code, out) = run(&["sleep", "5"], Duration::from_millis(50));
        assert_eq!(code, 1);
        assert!(out.contains("timed out"));
    }
}
