//! Hand one secret to `fnox set`. The value goes to the child's stdin only: never argv,
//! never the environment, never a file, never a log. The child's own output is discarded,
//! so nothing fnox prints can reach the board either.

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::domain::SecretTarget;

/// Overrides the fnox binary (tests point it at a fake). Default: `fnox` on `PATH`.
pub const FNOX_BIN_ENV: &str = "TSK_FNOX_BIN";

/// How long fnox may take before the board stops waiting and kills it.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Why fnox did not take the value. Carries no output from fnox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnoxFailure {
    /// The binary could not be started.
    NotStarted,
    /// It exited non-zero (or by signal: `None`).
    Exit(Option<i32>),
    /// It did not finish in time and was killed.
    TimedOut,
    /// Writing the value or waiting failed.
    Io,
}

impl std::fmt::Display for FnoxFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotStarted => f.write_str("fnox set failed · fnox could not be started"),
            Self::Exit(Some(code)) => write!(f, "fnox set failed · exit {code}"),
            Self::Exit(None) => f.write_str("fnox set failed · killed by a signal"),
            Self::TimedOut => f.write_str("fnox set failed · timed out"),
            Self::Io => f.write_str("fnox set failed"),
        }
    }
}

/// The fnox binary: `TSK_FNOX_BIN` when set and non-empty, else `fnox`.
pub fn binary() -> OsString {
    std::env::var_os(FNOX_BIN_ENV)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("fnox"))
}

/// `set [-g | -c <config>] [--profile <p>] <KEY>`: the target only, never a value.
pub fn args(target: &SecretTarget) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["set".into()];
    match target.config.as_deref() {
        None => args.push("-g".into()),
        Some(config) => {
            args.push("-c".into());
            args.push(config.into());
        }
    }
    if let Some(profile) = target.profile.as_deref() {
        args.push("--profile".into());
        args.push(profile.into());
    }
    args.push(target.key.as_str().into());
    args
}

/// Run `bin set …` with `value` on stdin and wait for it. Success means exit 0.
pub fn set(bin: &OsStr, target: &SecretTarget, value: &[u8]) -> Result<(), FnoxFailure> {
    let mut child = Command::new(bin)
        .args(args(target))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| FnoxFailure::NotStarted)?;
    let written = child
        .stdin
        .take()
        .ok_or(FnoxFailure::Io)
        .and_then(|mut stdin| stdin.write_all(value).map_err(|_| FnoxFailure::Io));
    // stdin is dropped (closed) here, so fnox sees end of input.
    if written.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return written;
    }
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(FnoxFailure::Exit(status.code())),
            Ok(None) if started.elapsed() >= TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(FnoxFailure::TimedOut);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(FnoxFailure::Io);
            }
        }
    }
}

/// Match fnox's global write destination, never a config discovered from tsk's cwd.
fn global_config() -> Option<PathBuf> {
    let absolute_env = |key| {
        std::env::var_os(key)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let directory = absolute_env("FNOX_CONFIG_DIR")
        .or_else(|| absolute_env("XDG_CONFIG_HOME").map(|p| p.join("fnox")))
        .or_else(|| {
            #[cfg(unix)]
            let (home, suffix) = (absolute_env("HOME"), ".config/fnox");
            #[cfg(windows)]
            let (home, suffix) = (absolute_env("USERPROFILE"), "AppData/Local/fnox");
            home.map(|p| p.join(suffix))
        })?;
    Some(directory.join("config.toml"))
}

/// Check configured key names, not values or service validity. fnox 1.36's completion
/// mode exits before value resolution. The selected config/profile's normal inheritance
/// applies; this is not proof of a fresh write or a direct declaration in that file.
/// A bounded reader drains stdout while the child runs so a pipe cannot deadlock it.
pub(crate) fn contains(bin: &OsStr, target: &SecretTarget) -> Result<bool, FnoxFailure> {
    let config = match target.config.as_deref() {
        Some(path) => PathBuf::from(path),
        None => global_config().ok_or(FnoxFailure::Io)?,
    };
    // A missing global config must not trigger fnox's parent-directory discovery.
    if !config.is_file() {
        return Err(FnoxFailure::Io);
    }
    let mut child = Command::new(bin)
        .args([
            "list",
            "--complete",
            "--no-color",
            "--non-interactive",
            "--config",
        ])
        .arg(config)
        .args(["--profile", target.profile.as_deref().unwrap_or("default")])
        .env_remove("FNOX_PROFILE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| FnoxFailure::Io)?;
    let stdout = child.stdout.take().ok_or(FnoxFailure::Io)?;
    const MAX_NAMES: u64 = 1024 * 1024;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(MAX_NAMES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| FnoxFailure::Io)?;
        if bytes.len() as u64 > MAX_NAMES {
            return Err(FnoxFailure::Io);
        }
        String::from_utf8(bytes).map_err(|_| FnoxFailure::Io)
    });
    let started = Instant::now();
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if started.elapsed() < TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let names = reader.join().map_err(|_| FnoxFailure::Io)??;
    if !success {
        return Err(FnoxFailure::Io);
    }
    Ok(names.lines().any(|name| name == target.key))
}

/// Overwrite a secret buffer before it is dropped. A best effort without unsafe code:
/// the bytes are zeroed in place, then the allocation is released.
pub fn wipe(text: &mut String) {
    let mut bytes = std::mem::take(text).into_bytes();
    bytes.fill(0);
    std::hint::black_box(&bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(profile: Option<&str>, config: Option<&str>) -> SecretTarget {
        SecretTarget {
            key: "API_KEY".into(),
            profile: profile.map(Into::into),
            config: config.map(Into::into),
        }
    }

    #[test]
    fn argv_names_the_target_only() {
        let strings = |t: &SecretTarget| -> Vec<String> {
            args(t)
                .into_iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(strings(&target(None, None)), ["set", "-g", "API_KEY"]);
        assert_eq!(
            strings(&target(Some("prod"), Some("/repo/fnox.toml"))),
            [
                "set",
                "-c",
                "/repo/fnox.toml",
                "--profile",
                "prod",
                "API_KEY"
            ]
        );
    }

    #[test]
    fn a_missing_binary_is_not_started_and_says_nothing_else() {
        let failure = set(
            OsStr::new("/nonexistent/tsk-no-fnox"),
            &target(None, None),
            b"v",
        )
        .expect_err("no binary");
        assert_eq!(failure, FnoxFailure::NotStarted);
        assert!(failure.to_string().starts_with("fnox set failed"));
    }

    #[test]
    fn wipe_empties_the_buffer() {
        let mut text = String::from("hunter2");
        wipe(&mut text);
        assert!(text.is_empty());
    }
}
