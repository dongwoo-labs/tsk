//! Full-text JSON input/output for Agent questions.
use std::io::Read;
use std::path::PathBuf;

use serde::Serialize;
use uuid::Uuid;

use super::{parser::parse_task_address, CliOutput};
use crate::domain::{
    AnswerDraft, AnswerOrigin, FindingAction, QuestionDraft, QuestionKind, QuestionRecord,
    RouteDraft,
};
use crate::store::{
    notify_question, question_problems, QuestionDurability, QuestionError, QuestionStore, TaskStore,
};

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

pub fn run(args: Vec<String>, stdin: &mut dyn Read) -> CliOutput {
    if args.iter().skip(2).any(|argument| argument == "--help") {
        return help();
    }
    if args.get(1).map(String::as_str) == Some("ui-answer") {
        return super::ui_answer::run(&args, stdin);
    }
    match parse(&args).and_then(|parsed| execute(parsed, stdin)) {
        Ok(output) => output,
        Err((message, code)) => rejected(message, code),
    }
}

/// The arguments every question verb shares.
pub(super) struct Parsed {
    pub verb: String,
    pub state_dir: Option<PathBuf>,
    /// `-` reads the JSON input from stdin.
    pub file: Option<PathBuf>,
    pub operand: Option<String>,
    pub task: Option<super::parser::TaskAddress>,
}

pub(super) fn parse(args: &[String]) -> Result<Parsed, (String, u8)> {
    let verb = args.get(1).cloned().unwrap_or_default();
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
                    .filter(|value| {
                        !value.starts_with('-') || (argument == "--file" && *value == "-")
                    })
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
    Ok(Parsed {
        verb,
        state_dir,
        file,
        operand,
        task,
    })
}

fn execute(parsed: Parsed, stdin: &mut dyn Read) -> Result<CliOutput, (String, u8)> {
    let Parsed {
        verb,
        state_dir,
        file,
        operand,
        task,
    } = parsed;
    let verb = verb.as_str();
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
    let mut read_input = || -> Result<Vec<u8>, (String, u8)> {
        read_file(
            file.as_ref()
                .ok_or_else(|| ("--file is required".into(), 2))?,
            stdin,
        )
    };
    match verb {
        "ask" => {
            if operand.is_some() {
                return Err(("ask has no positional operand".into(), 2));
            }
            // serde's text names the field or variant; it never echoes a field value.
            let draft: QuestionDraft = serde_json::from_slice(&read_input()?)
                .map_err(|error| (format!("invalid question JSON: {error}"), 2))?;
            let problems = question_problems(&draft);
            let saved = store.ask(draft).map_err(|error| match error {
                // Name every bad field at once so one retry fixes them all.
                QuestionError::InvalidInput if !problems.is_empty() => {
                    (format!("invalid question: {}", problems.join("; ")), 1)
                }
                _ => (error.to_string(), 1),
            })?;
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
            // A secret is typed on the board, which hands it to fnox. No CLI answer exists.
            if store
                .read(id)
                .map_err(|error| (error.to_string(), 1))?
                .reference
                .kind
                == QuestionKind::Secret
            {
                return Err((
                    format!("secret-needs-board: {}", QuestionError::SecretNeedsBoard),
                    1,
                ));
            }
            let draft: AnswerDraft = serde_json::from_slice(&read_input()?)
                .map_err(|_| ("invalid answer JSON".into(), 2))?;
            if draft.origin == AnswerOrigin::UserUi {
                return Err(("UserUi answers are not a CLI capability".into(), 1));
            }
            // Creating a task from a finding happens in the board, under the same lock as
            // its answer. The CLI may record a close, never a creation it did not do.
            if matches!(
                draft.action,
                Some(FindingAction::CreateAndRun | FindingAction::CreateOnly)
            ) {
                return Err(("finding task creation is a board action".into(), 1));
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
        "route" => {
            let operand = operand.ok_or_else(|| ("Q number or UUID required".into(), 2))?;
            let id = resolve(&store, &operand, None)?;
            let draft: RouteDraft = serde_json::from_slice(&read_input()?)
                .map_err(|_| ("invalid route JSON".into(), 2))?;
            // A route changes who acts; it sends no prompt of its own.
            let saved = store
                .route(id, draft)
                .map_err(|error| (error.to_string(), 1))?;
            let code = if saved.durability == QuestionDurability::Uncertain {
                1
            } else {
                0
            };
            Ok(json(&serde_json::json!({"saved": saved}), code))
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
        _ => Err(("expected ask, answer, route, or questions".into(), 2)),
    }
}

/// The JSON input: a file, or stdin for `-`.
pub(super) fn read_file(path: &PathBuf, stdin: &mut dyn Read) -> Result<Vec<u8>, (String, u8)> {
    if path.as_os_str() == "-" {
        let mut input = Vec::new();
        stdin
            .read_to_end(&mut input)
            .map_err(|_| ("input unavailable on stdin".to_string(), 1))?;
        return Ok(input);
    }
    std::fs::read(path).map_err(|_| ("input file unavailable".into(), 1))
}

/// How a Q address failed to resolve.
pub(super) enum Lookup {
    /// Not a Q number or UUID.
    Syntax,
    Store(QuestionError),
}

/// A Q UUID or Q number in this store, read back with its record.
pub(super) fn lookup(
    store: &QuestionStore,
    operand: &str,
    task_id: Option<Uuid>,
) -> Result<QuestionRecord, Lookup> {
    if let Ok(id) = Uuid::parse_str(operand) {
        return store.read(id).map_err(Lookup::Store);
    }
    let number = operand
        .strip_prefix('Q')
        .or_else(|| operand.strip_prefix('q'))
        .unwrap_or(operand)
        .parse::<u64>()
        .map_err(|_| Lookup::Syntax)?;
    store
        .list(task_id)
        .map_err(Lookup::Store)?
        .into_iter()
        .find(|record| record.reference.number == number)
        .ok_or(Lookup::Store(QuestionError::UnknownQuestion))
}

fn resolve(
    store: &QuestionStore,
    operand: &str,
    task_id: Option<Uuid>,
) -> Result<Uuid, (String, u8)> {
    let record = lookup(store, operand, task_id).map_err(|error| match error {
        Lookup::Syntax => ("invalid Q number or UUID".to_string(), 2),
        Lookup::Store(error) => (error.to_string(), 1),
    })?;
    if task_id.is_some_and(|task| task != record.reference.task_id) {
        return Err(("question belongs to another task".into(), 1));
    }
    Ok(record.reference.id)
}
