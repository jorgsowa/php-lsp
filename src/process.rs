//! Running short-lived external programs from synchronous code.

use std::io::{Read, Write};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Run `command` with `stdin` as its input and return its output, or `None`
/// when it can't start or outlives `timeout`, in which case it and the
/// processes it forked are killed. Stderr is discarded.
pub(crate) fn output_with_timeout(
    mut command: Command,
    stdin: &[u8],
    timeout: Duration,
) -> Option<Output> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            tracing::debug!("{program} did not run: {e}");
            return None;
        }
    };
    // Writing and reading on separate threads: a child that fills its stdout
    // pipe before draining stdin would otherwise deadlock against us.
    let mut input = child.stdin.take()?;
    let stdin = stdin.to_vec();
    let writer = std::thread::spawn(move || input.write_all(&stdin));
    let mut output = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        output.read_to_end(&mut buf).map(|_| buf)
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                tracing::warn!("{program} killed after {timeout:?}");
                break None;
            }
            Err(e) => {
                tracing::debug!("{program} did not run: {e}");
                break None;
            }
        }
    };
    // A forked process left holding stdout would block the reader forever.
    kill_tree(&mut child);
    let _ = writer.join();
    let stdout = reader.join().ok()?.ok()?;
    Some(Output {
        status: status?,
        stdout,
        stderr: Vec::new(),
    })
}

fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    if let Ok(pgid) = i32::try_from(child.id()) {
        // SAFETY: `killpg` has no memory-safety preconditions.
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn returns_output_of_a_finished_program() {
        let input = "x".repeat(1 << 20);
        let output =
            output_with_timeout(sh("cat"), input.as_bytes(), Duration::from_secs(30)).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), input.len());
    }

    #[test]
    fn kills_a_program_that_outlives_the_timeout() {
        let start = Instant::now();
        let output = output_with_timeout(sh("sleep 30"), b"", Duration::from_millis(200));
        assert!(output.is_none());
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn does_not_wait_for_a_forked_process_holding_stdout() {
        let start = Instant::now();
        let output =
            output_with_timeout(sh("sleep 30 & echo done"), b"", Duration::from_secs(20)).unwrap();
        assert_eq!(output.stdout, b"done\n");
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn missing_program_yields_none() {
        let command = Command::new("php-lsp-no-such-program");
        assert!(output_with_timeout(command, b"", Duration::from_secs(1)).is_none());
    }
}
