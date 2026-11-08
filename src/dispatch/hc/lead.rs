//! 저장된 hc lead binding만 사용한다. 호출자의 runtime 선택은 사용하지 않는다.
use super::{field, label, live, local, run_saved, same_directory, strings, terminals, token};
use serde_json::Value;
use std::{path::Path, process::Command, time::Duration};

pub(crate) fn has_binding(notes: Option<&str>) -> bool {
    notes
        .unwrap_or("")
        .lines()
        .any(|line| line.trim().starts_with("Owner lead runtime:"))
}

struct Binding<'a> {
    machine: &'a str,
    handle: &'a str,
    session: &'a str,
    address: &'a str,
    uuid: &'a str,
    canonical: &'a str,
}

fn one_line<'a>(notes: &'a str, prefix: &str) -> Result<&'a str, String> {
    let lines: Vec<_> = notes
        .lines()
        .filter_map(|line| line.trim().strip_prefix(prefix))
        .collect();
    match lines.as_slice() {
        [line] => Ok(line.trim()),
        _ => Err(format!("hc lead requires exactly one {prefix} line")),
    }
}

fn lead_handle(value: &str) -> bool {
    value.strip_prefix("lead-").is_some_and(|tail| {
        !tail.is_empty()
            && tail.len() <= 27
            && tail
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_-".contains(&c))
    })
}

#[cfg(unix)]
fn observed_hostname() -> Result<String, String> {
    let raw = crate::backup::raw_hostname().ok_or("hc lead OS hostname unavailable")?;
    crate::machine::normalize(raw.split('.').next().unwrap_or(""))
}

#[cfg(not(unix))]
fn observed_hostname() -> Result<String, String> {
    Err("hc lead requires Unix host identity checks".into())
}

impl<'a> Binding<'a> {
    fn parse(notes: &'a str, recipient: &str) -> Result<Self, String> {
        let owner: Vec<_> = one_line(notes, "Owner lead:")?.split_whitespace().collect();
        let [machine, owner_session, handle] = owner.as_slice() else {
            return Err("hc lead requires a machine/session/handle owner".into());
        };
        // 처음 다섯 필드 뒤의 나머지 전체가 경로다. 경로 안의 공백을 보존한다.
        let mut rest = one_line(notes, "Owner lead runtime:")?;
        let mut fields = Vec::new();
        for _ in 0..5 {
            let (word, remaining) = rest
                .split_once(char::is_whitespace)
                .ok_or("incomplete hc lead runtime")?;
            fields.push(word);
            rest = remaining.trim_start();
        }
        let [backend, session, address, uuid, hostname] = fields.as_slice() else {
            unreachable!()
        };
        let canonical = rest;
        if *backend != "hc"
            || !token(session)
            || session.len() > 64
            || session != owner_session
            || !local(address)
            || !lead_handle(handle)
            || *handle != recipient
            || uuid::Uuid::parse_str(uuid).is_err()
            || uuid.len() != 36
            || !Path::new(canonical).is_absolute()
            || canonical.chars().any(char::is_control)
            || crate::machine::normalize(machine)? != crate::machine::current()?
            || *hostname != observed_hostname()?
        {
            return Err("invalid or mismatched hc lead binding".into());
        }
        Ok(Self {
            machine,
            handle,
            session,
            address,
            uuid,
            canonical,
        })
    }

    fn verify(&self, value: &Value) -> Result<u64, String> {
        if !live(value)?
            || field(value, "address")? != self.address
            || label(value, "role")? != "lead"
            || label(value, "owner_lead")? != self.handle
            || label(value, "machine")? != self.machine
            || label(value, "claude_session")? != self.uuid
            || !same_directory(label(value, "canonical")?, self.canonical)
            || value.get("reported").and_then(Value::as_bool) != Some(true)
            || value.pointer("/status/app").and_then(Value::as_str) != Some("claude-code")
            || !matches!(
                value.pointer("/status/state").and_then(Value::as_str),
                Some("idle" | "working" | "done")
            )
        {
            return Err("hc lead terminal identity or status mismatch".into());
        }
        let foreground = value.get("foreground").ok_or("hc lead has no foreground")?;
        let pid = foreground
            .get("pid")
            .and_then(Value::as_u64)
            .filter(|pid| *pid > 0)
            .ok_or("hc lead has no foreground PID")?;
        let cwd = field(foreground, "cwd")?;
        if !same_directory(cwd, self.canonical) {
            return Err("hc lead foreground cwd mismatch".into());
        }
        let argv = foreground
            .get("argv")
            .and_then(Value::as_array)
            .ok_or("hc lead has no foreground argv")?;
        let argv: Vec<_> = argv
            .iter()
            .map(|arg| arg.as_str().ok_or("invalid hc lead argv"))
            .collect::<Result<_, _>>()?;
        if argv
            .first()
            .and_then(|arg| Path::new(arg).file_name())
            .and_then(|s| s.to_str())
            != Some("claude")
        {
            return Err("hc lead foreground is not Claude".into());
        }
        let mut ids = Vec::new();
        let mut args = argv.iter().skip(1);
        // 현재 lead의 실제 구문만 허용한다. 값 안의 옵션 이름은 다시 해석하지 않는다.
        while let Some(arg) = args.next() {
            let (option, inline) = arg
                .split_once('=')
                .map_or((*arg, None), |(key, value)| (key, Some(value)));
            if !matches!(option, "--settings" | "--resume" | "--session-id") {
                return Err("unsupported hc lead foreground arguments".into());
            }
            let value = inline
                .or_else(|| args.next().copied())
                .filter(|value| !value.is_empty())
                .ok_or("hc lead option value is missing")?;
            if option != "--settings" {
                ids.push(value);
            }
        }
        if ids.as_slice() != [self.uuid] {
            return Err("hc lead foreground Claude session mismatch".into());
        }
        verify_primary(cwd, self.canonical)?;
        Ok(pid)
    }

    fn resolve(&self) -> Result<u64, String> {
        let rows = terminals(
            self.session,
            &["role=lead".into(), format!("owner_lead={}", self.handle)],
        )?;
        let [row] = rows.as_slice() else {
            return Err("hc lead needs exactly one live terminal".into());
        };
        self.verify(row)
    }
}

fn verify_primary(cwd: &str, canonical: &str) -> Result<(), String> {
    let mut command = Command::new("git");
    // 호스트의 Git override로 다른 저장소를 확인하지 않는다.
    for (key, _) in std::env::vars_os().filter(|(key, _)| key.to_string_lossy().starts_with("GIT_"))
    {
        command.env_remove(key);
    }
    command.current_dir(cwd).args([
        "rev-parse",
        "--path-format=absolute",
        "--show-toplevel",
        "--git-dir",
        "--git-common-dir",
    ]);
    let output = crate::git_base::bounded_process_output(command, "git", Duration::from_secs(10))?;
    if !output.status.success() {
        return Err("hc lead Git identity unavailable".into());
    }
    let output = std::str::from_utf8(&output.stdout).map_err(|_| "invalid hc lead Git identity")?;
    let paths: Vec<_> = output.lines().collect();
    let expected_git = Path::new(canonical).join(".git");
    let expected_git = expected_git
        .to_str()
        .ok_or("invalid hc lead canonical path")?;
    let [top, git, common] = paths.as_slice() else {
        return Err("invalid hc lead Git identity".into());
    };
    if !Path::new(expected_git).is_dir()
        || !same_directory(top, canonical)
        || !same_directory(git, expected_git)
        || !same_directory(common, expected_git)
    {
        return Err("hc lead cwd is not its primary Git checkout".into());
    }
    Ok(())
}

pub(crate) fn prompt_saved(notes: Option<&str>, recipient: &str, text: &str) -> Result<(), String> {
    let binding = Binding::parse(notes.unwrap_or(""), recipient)?;
    let pid = binding.resolve()?;
    // 전송 경계에서 중복 terminal과 foreground 교체를 다시 확인한다.
    if binding.resolve()? != pid {
        return Err("hc lead foreground PID changed".into());
    }
    run_saved(
        binding.session,
        &strings(&["send", binding.address, text, "--json"]),
    )
    .map(|_| ())
}
