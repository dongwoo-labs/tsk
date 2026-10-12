//! Full-text JSON input/output for Agent questions.
use std::path::PathBuf;

use serde::Serialize;
use uuid::Uuid;

use super::{parser::parse_task_address, CliOutput};
use crate::domain::{AnswerDraft, AnswerOrigin, QuestionDraft};
use crate::store::{notify_question, QuestionDurability, QuestionStore, TaskStore};

pub fn help() -> CliOutput {
    super::presenter::questions_help()
}

fn rejected(message: impl std::fmt::Display, code: u8) -> CliOutput {
    CliOutput {
        stdout: String::new(),
        stderr: format!("tsk questions: {message}\n"),
        code,
    }
}

fn json(value: &impl Serialize, code: u8) -> CliOutput {
    match serde_json::to_string_pretty(value) {
        Ok(value) => CliOutput {
            stdout: format!("{value}\n"),
            stderr: String::new(),
            code,
        },
        Err(_) => rejected("output serialization failed", 1),
    }
}

pub fn run(args: Vec<String>) -> CliOutput {
    if args.iter().skip(2).any(|argument| argument == "--help") {
        return help();
    }
    match execute(&args) {
        Ok(output) => output,
        Err((message, code)) => rejected(message, code),
    }
}

fn execute(args: &[String]) -> Result<CliOutput, (String, u8)> {
    let verb = args.get(1).map(String::as_str).unwrap_or("");
    let mut state_dir = std::env::var_os("TSK_STATE_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let mut file = None;
    let mut operand = None;
    let mut task = None;
    let mut index = 2;
    while let Some(argument) = args.get(index) {
        match argument.as_str() {
            "--json" => index += 1,
            "--state-dir" | "--file" | "--task" => {
                let value = args
                    .get(index + 1)
                    .filter(|value| !value.starts_with('-'))
                    .ok_or_else(|| (format!("missing value for {argument}"), 2))?;
                match argument.as_str() {
                    "--state-dir" => state_dir = Some(PathBuf::from(value)),
                    "--file" => file = Some(PathBuf::from(value)),
                    _ => task = Some(parse_task_address(value).map_err(|error| (error, 2))?),
                }
                index += 2;
            }
            value if value.starts_with("--state-dir=") => {
                state_dir = Some(PathBuf::from(&value[12..]));
                index += 1;
            }
            value if value.starts_with('-') => {
                return Err((format!("unknown argument {value}"), 2))
            }
            value => {
                if operand.replace(value.to_string()).is_some() {
                    return Err(("too many operands".into(), 2));
                }
                index += 1;
            }
        }
    }
    let state_dir =
        state_dir.ok_or_else(|| ("explicit Agent state directory required".into(), 1))?;
    let store = QuestionStore::new(state_dir.clone()).map_err(|error| (error.to_string(), 1))?;
    let task_id = if let Some(address) = task {
        let state = TaskStore::new(&state_dir)
            .load()
            .map_err(|_| ("task store unavailable".into(), 1))?;
        Some(
            state
                .tasks()
                .iter()
                .find(|task| address.matches(task))
                .ok_or_else(|| ("unknown task".into(), 1))?
                .id,
        )
    } else {
        None
    };
    if verb != "questions" && task_id.is_some() {
        return Err(("--task is a questions filter".into(), 2));
    }
    let read_input = || -> Result<Vec<u8>, (String, u8)> {
        fs_read(
            file.as_ref()
                .ok_or_else(|| ("--file is required".into(), 2))?,
        )
    };
    match verb {
        "ask" => {
            if operand.is_some() {
                return Err(("ask has no positional operand".into(), 2));
            }
            let draft: QuestionDraft = serde_json::from_slice(&read_input()?)
                .map_err(|_| ("invalid question JSON".into(), 2))?;
            let saved = store.ask(draft).map_err(|error| (error.to_string(), 1))?;
            let notification = notify_question(&saved, true);
            let code = if saved.durability == QuestionDurability::Uncertain {
                1
            } else {
                0
            };
            Ok(json(
                &serde_json::json!({"saved": saved, "notification": notification}),
                code,
            ))
        }
        "answer" => {
            let operand = operand.ok_or_else(|| ("Q number or UUID required".into(), 2))?;
            let id = resolve(&store, &operand, None)?;
            let draft: AnswerDraft = serde_json::from_slice(&read_input()?)
                .map_err(|_| ("invalid answer JSON".into(), 2))?;
            if draft.origin == AnswerOrigin::UserUi {
                return Err(("UserUi answers are not a CLI capability".into(), 1));
            }
            let saved = store
                .answer(id, draft)
                .map_err(|error| (error.to_string(), 1))?;
            let notification = notify_question(&saved, false);
            let code = if saved.durability == QuestionDurability::Uncertain {
                1
            } else {
                0
            };
            Ok(json(
                &serde_json::json!({"saved": saved, "notification": notification}),
                code,
            ))
        }
        "questions" => {
            if file.is_some() {
                return Err(("questions does not take --file".into(), 2));
            }
            if let Some(operand) = operand {
                let id = resolve(&store, &operand, task_id)?;
                let record = store.read(id).map_err(|error| (error.to_string(), 1))?;
                Ok(json(&record, 0))
            } else {
                let records = store
                    .list(task_id)
                    .map_err(|error| (error.to_string(), 1))?;
                Ok(json(&records, 0))
            }
        }
        _ => Err(("expected ask, answer, or questions".into(), 2)),
    }
}

fn fs_read(path: &PathBuf) -> Result<Vec<u8>, (String, u8)> {
    std::fs::read(path).map_err(|_| ("input file unavailable".into(), 1))
}

fn resolve(
    store: &QuestionStore,
    operand: &str,
    task_id: Option<Uuid>,
) -> Result<Uuid, (String, u8)> {
    if let Ok(id) = Uuid::parse_str(operand) {
        let record = store.read(id).map_err(|error| (error.to_string(), 1))?;
        if task_id.is_some_and(|task| task != record.reference.task_id) {
            return Err(("question belongs to another task".into(), 1));
        }
        return Ok(id);
    }
    let number = operand
        .strip_prefix('Q')
        .or_else(|| operand.strip_prefix('q'))
        .unwrap_or(operand)
        .parse::<u64>()
        .map_err(|_| ("invalid Q number or UUID".into(), 2))?;
    store
        .list(task_id)
        .map_err(|error| (error.to_string(), 1))?
        .iter()
        .find(|record| record.reference.number == number)
        .map(|record| record.reference.id)
        .ok_or_else(|| ("unknown question".into(), 1))
}
