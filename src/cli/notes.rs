use std::path::PathBuf;

use crate::cli::parser::{parse_task_address, TaskAddress};
use crate::cli::{presenter, CliOutput};
use crate::store::{default_state_dir, TaskStore};

#[derive(Debug)]
enum Action {
    Set(String, String),
    Remove(String),
    Append(String),
}

struct Input {
    task: TaskAddress,
    action: Action,
    state_dir: Option<PathBuf>,
}

pub fn run(args: Vec<String>) -> CliOutput {
    let input = match parse(&args) {
        Ok(Some(input)) => input,
        Ok(None) => return presenter::notes_help(),
        Err(reason) => return presenter::notes_usage(&reason),
    };
    let store = TaskStore::new(input.state_dir.unwrap_or_else(default_state_dir));
    let result = store.locked_transition_if_changed(|state| {
        let Some(task) = state.tasks().iter().find(|task| input.task.matches(task)) else {
            return Ok((Err("unknown-task"), false));
        };
        if task.soft_deleted {
            return Ok((Err("soft-deleted-task"), false));
        }
        let next = update(task.notes.as_deref().unwrap_or_default(), &input.action);
        let next = (!next.is_empty()).then_some(next);
        let number = task.number.expect("loaded tasks have numbers");
        let title = task.title.clone();
        let changed = next != task.notes;
        let bytes = next.as_ref().map_or(0, String::len);
        if changed {
            state
                .edit(task.id, &title, next, task.scope.clone())
                .map_err(|error| error.to_string())?;
        }
        Ok((Ok((number, title, bytes)), changed))
    });
    match result {
        Ok(Ok((number, title, bytes))) => CliOutput {
            stdout: format!("notes T{number} {}\n", crate::ui::terminal_text(&title)),
            stderr: presenter::notes_size_warning("notes", number, bytes),
            code: 0,
        },
        Ok(Err(code)) => CliOutput {
            stdout: String::new(),
            stderr: format!("tsk notes: {code}: {}\n", input.task.display()),
            code: 1,
        },
        Err(reason) => CliOutput {
            stdout: String::new(),
            stderr: format!("tsk notes: {}\n", crate::ui::terminal_text(&reason)),
            code: 3,
        },
    }
}

fn parse(args: &[String]) -> Result<Option<Input>, String> {
    let mut state_dir = None;
    let mut positionals = Vec::new();
    let mut literal = false;
    let mut help = false;
    let mut index = 2;
    while let Some(arg) = args.get(index).map(String::as_str) {
        match arg {
            "--" if !literal => literal = true,
            "--help" if !literal => help = true,
            "--state-dir" if !literal => {
                index += 1;
                let path = args
                    .get(index)
                    .filter(|value| !value.starts_with('-'))
                    .ok_or("missing value for --state-dir")?;
                state_dir = Some(PathBuf::from(path));
            }
            flag if !literal && flag.starts_with("--state-dir=") => {
                state_dir = Some(PathBuf::from(&flag["--state-dir=".len()..]));
            }
            flag if !literal && flag.starts_with('-') => {
                return Err("unknown notes flag; use -- before dash-leading text".into());
            }
            value => positionals.push(value),
        }
        index += 1;
    }
    if help {
        return Ok(None);
    }
    let task = parse_task_address(positionals.first().ok_or("task id is required")?)?;
    let action = match positionals.get(1..).unwrap_or_default() {
        ["set", key, text] => Action::Set(prefix(key)?, line_text(text)?),
        ["remove", key] => Action::Remove(prefix(key)?),
        ["append", text] => Action::Append(line_text(text)?),
        _ => {
            return Err(
                "expected <task> set <prefix> <text>, remove <prefix>, or append <text>".into(),
            )
        }
    };
    Ok(Some(Input {
        task,
        action,
        state_dir,
    }))
}

fn invalid_line_character(character: char) -> bool {
    (character.is_control() && character != '\t') || matches!(character, '\u{2028}' | '\u{2029}')
}

fn line_text(value: &str) -> Result<String, String> {
    if value.chars().any(invalid_line_character) || value.trim().is_empty() {
        return Err(
            "text must be a non-blank single line without control characters (tabs allowed)".into(),
        );
    }
    Ok(value.trim().to_string())
}

pub(crate) fn prefix(value: &str) -> Result<String, String> {
    if value
        .chars()
        .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
    {
        return Err("prefix must be a non-empty label ending in :".into());
    }
    let head = value
        .trim()
        .strip_suffix(':')
        .ok_or("prefix must end in :")?
        .trim();
    if head.is_empty() || head.contains(':') || head.starts_with("- ") {
        return Err("prefix must be a non-empty label with only a final :".into());
    }
    Ok(format!("{head}:"))
}

fn matched_value<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let line = line.trim();
    let line = line.strip_prefix("- ").unwrap_or(line).trim_start();
    let (head, rest) = line.split_once(':')?;
    head.trim()
        .eq_ignore_ascii_case(prefix.strip_suffix(':')?)
        .then(|| rest.trim())
}

pub(crate) fn value<'a>(notes: &'a str, prefix: &str) -> Option<&'a str> {
    notes.lines().find_map(|line| matched_value(line, prefix))
}

fn append(notes: &mut String, line: &str) {
    if !notes.is_empty() && !notes.ends_with('\n') {
        notes.push('\n');
    }
    notes.push_str(line);
}

fn update(notes: &str, action: &Action) -> String {
    if let Action::Append(text) = action {
        let mut next = notes.to_string();
        append(&mut next, text);
        return next;
    }
    let (prefix, replacement) = match action {
        Action::Set(prefix, text) => (prefix, Some(format!("{prefix} {text}"))),
        Action::Remove(prefix) => (prefix, None),
        Action::Append(_) => unreachable!(),
    };
    let mut next = String::new();
    let mut found = false;
    for line in notes.split_inclusive('\n') {
        if matched_value(line, prefix).is_none() {
            next.push_str(line);
            continue;
        }
        if !found {
            if let Some(replacement) = &replacement {
                next.push_str(replacement);
                if line.ends_with("\r\n") {
                    next.push_str("\r\n");
                } else if line.ends_with('\n') {
                    next.push('\n');
                }
            }
        }
        found = true;
    }
    if !found {
        if let Some(replacement) = replacement {
            append(&mut next, &replacement);
        }
    }
    next
}
