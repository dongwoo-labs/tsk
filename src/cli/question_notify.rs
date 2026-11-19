//! `tsk question-notify`: send the identifier-only question hint for a record saved
//! elsewhere. A board kept on another machine (Evo) cannot reach this machine's Herdr or
//! hc sessions, so a local watcher replays the hint the saving `tsk` would have sent.
//!
//! The record arrives on stdin and is never stored. The state directory supplies only the
//! tasks whose notes name the recipient, through the same lookup a local save uses.
use std::io::Read;
use std::path::PathBuf;

use serde::Serialize;
use uuid::Uuid;

use super::questions::{parse, read_file};
use super::CliOutput;
use crate::domain::QuestionRecord;
use crate::store::{
    notify_question, QuestionDurability, QuestionNotification, QuestionSaveResult, QuestionStore,
};

#[derive(Debug, Serialize)]
struct NotifyOutput {
    question_id: Uuid,
    revision: Uuid,
    notification: QuestionNotification,
}

fn refused(message: impl Into<String>, code: u8) -> CliOutput {
    CliOutput {
        stdout: String::new(),
        stderr: format!("tsk question-notify: {}\n", message.into()),
        code,
    }
}

pub fn help() -> CliOutput {
    CliOutput {
        stdout: "Usage: tsk question-notify <asker|recipient> --file - --state-dir <dir>\n\n\
Sends the question hint for a record saved on another board: `asker` after an answer or \
route, `recipient` after an ask. The record is read from the file or stdin and not \
stored. A verified lead receives only open lead questions, once per revision. Intentional \
skips return not_attempted and exit 0.\n"
            .into(),
        stderr: String::new(),
        code: 0,
    }
}

pub fn run(args: &[String], stdin: &mut dyn Read) -> CliOutput {
    if args.iter().skip(2).any(|argument| argument == "--help") {
        return help();
    }
    let parsed = match parse(args) {
        Ok(parsed) => parsed,
        Err((message, code)) => return refused(message, code),
    };
    let registration = match parsed.operand.as_deref() {
        Some("recipient") => true,
        Some("asker") => false,
        _ => return refused("expected asker or recipient", 2),
    };
    if parsed.task.is_some() || parsed.inventory {
        return refused("question-notify takes only --file and --state-dir", 2);
    }
    let Some(file) = parsed.file else {
        return refused("--file is required", 2);
    };
    let Some(state_dir) = parsed.state_dir else {
        return refused("--state-dir is required", 2);
    };
    let store = match QuestionStore::new(state_dir) {
        Ok(store) => store,
        Err(error) => return refused(error.to_string(), 1),
    };
    let bytes = match read_file(&PathBuf::from(&file), stdin) {
        Ok(bytes) => bytes,
        Err((message, code)) => return refused(message, code),
    };
    let record: QuestionRecord = match serde_json::from_slice(&bytes) {
        Ok(record) => record,
        Err(_) => return refused("invalid question record JSON", 2),
    };
    let saved = QuestionSaveResult {
        record,
        durability: QuestionDurability::Durable,
    };
    let notification = notify_question(&store, &saved, registration);
    let failed = notification == QuestionNotification::Failed;
    let output = NotifyOutput {
        question_id: saved.record.reference.id,
        revision: saved.record.reference.revision,
        notification,
    };
    match serde_json::to_string(&output) {
        Ok(line) => CliOutput {
            stdout: format!("{line}\n"),
            stderr: String::new(),
            code: u8::from(failed),
        },
        Err(_) => refused("output serialization failed", 1),
    }
}
