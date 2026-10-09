//! `command` provider: runs a program (hidden, with a timeout) that prints
//! Limitbar's JSON format (see `usage::parse_report`) on standard output.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{Outcome, from_report, now};
use crate::config::{CommandLine, expand_path, home};
use crate::usage::{Reading, parse_report};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const MAX_OUTPUT: u64 = 1 << 20;

pub fn fetch(line: &CommandLine, timeout: u64) -> Result<Reading, Outcome> {
    let out = run(line, timeout).map_err(Outcome::Failed)?;
    let v: serde_json::Value = serde_json::from_str(out.trim_start_matches('\u{feff}').trim())
        .map_err(|e| Outcome::Failed(format!("command output is not JSON ({e})")))?;
    from_report(parse_report(&v, now()))
}

/// Runs `line` without a console window and returns its standard output.
/// Fails on a non-zero exit (with the first line of standard error) or when it
/// takes longer than `timeout` seconds.
pub fn run(line: &CommandLine, timeout: u64) -> Result<String, String> {
    let mut cmd = match line {
        CommandLine::Shell(s) => {
            let mut c = Command::new("cmd.exe");
            c.raw_arg(format!("/d /s /c \"{s}\""));
            c
        }
        CommandLine::Argv(argv) => {
            let home = home();
            let arg = |a: &String| {
                if a.starts_with('~') || a.contains('%') { expand_path(a, &home).to_string_lossy().into_owned() } else { a.clone() }
            };
            let mut c = Command::new(arg(&argv[0]));
            c.args(argv[1..].iter().map(arg));
            c
        }
    };
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("could not start the command: {e}"))?;
    // Drain both pipes on their own threads so a chatty command cannot block.
    let drain = |p: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(p) = p {
                let _ = p.take(MAX_OUTPUT).read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let deadline = Instant::now() + Duration::from_secs(timeout);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("the command did not finish within {timeout} s"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("waiting for the command failed: {e}")),
        }
    };
    let out = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    if !status.success() {
        let err = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
        let first = err.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
        let code = status.code().map_or_else(|| "?".into(), |c| c.to_string());
        return Err(if first.is_empty() {
            format!("the command exited with code {code}")
        } else {
            format!("the command exited with code {code}: {first}")
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::Window;

    #[test]
    fn shell_command_output() {
        let line = CommandLine::Shell(r#"echo {"windows":[{"key":"daily","percent":40,"resets_at":1791496800}],"plan":"Team"}"#.into());
        let r = fetch(&line, 10).unwrap();
        assert_eq!(r.get("daily"), Some(Window::pct(40.0, 1_791_496_800)));
        assert_eq!(r.plan.as_deref(), Some("Team"));
    }

    #[test]
    fn failures() {
        let bad_exit = run(&CommandLine::Shell("echo oops 1>&2 & exit /b 3".into()), 10).unwrap_err();
        assert_eq!(bad_exit, "the command exited with code 3: oops");
        let started = Instant::now();
        let slow = run(&CommandLine::Argv(vec!["ping".into(), "-n".into(), "6".into(), "127.0.0.1".into()]), 1).unwrap_err();
        assert!(slow.contains("did not finish"), "{slow}");
        assert!(started.elapsed() < Duration::from_secs(4));
        assert!(run(&CommandLine::Argv(vec!["limitbar-no-such-program.exe".into()]), 5).is_err());
        assert!(matches!(fetch(&CommandLine::Shell("echo not json".into()), 5), Err(Outcome::Failed(_))));
        assert!(matches!(fetch(&CommandLine::Shell(r#"echo {"idle":"signed out"}"#.into()), 5), Err(Outcome::Idle(_))));
    }
}
