//! `tsk ui-answer`: the user-input entrypoint for an answer the user gave in another
//! interface (a Claude Code ask prompt). It records origin `UserUi` through the same
//! question store, revision CAS, approval checks, and history as the board's questions tab.
//!
//! Trust boundary: this is a local entrypoint, not authentication. Any process running as
//! the same OS user can call it; agent tool calls are kept out by the caller's own hooks.
use std::io::Read;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::questions::{lookup, parse, read_file, Lookup};
use super::CliOutput;
use crate::domain::{AnswerDraft, AnswerOrigin, ApprovalTerms, QuestionKind, QuestionRecord};
use crate::question_actions::USER;
use crate::store::{
    notify_question, QuestionDurability, QuestionError, QuestionNotification, QuestionStore,
};

/// What the user picked or typed. Exactly one of `choice`, `text`, `approve`, `reject`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UiAnswerInput {
    expected_revision: Uuid,
    /// Zero-based index into the question's `options`; saved as that option's text.
    #[serde(default)]
    choice: Option<usize>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    approve: bool,
    #[serde(default)]
    reject: bool,
    /// The approval terms the user saw. Must equal the stored terms exactly.
    #[serde(default)]
    approval: Option<ApprovalTerms>,
    /// The current answer's revision when this answer corrects it.
    #[serde(default)]
    corrects_revision: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct UiAnswerOutput {
    question_id: Uuid,
    number: u64,
    task_id: Uuid,
    revision: Uuid,
    /// `durable`, or `uncertain` (exit 1: read back before acting).
    status: QuestionDurability,
    notification: QuestionNotification,
}

struct Refusal {
    code: &'static str,
    message: String,
    exit: u8,
}

fn usage(message: impl Into<String>) -> Refusal {
    Refusal {
        code: "usage",
        message: message.into(),
        exit: 2,
    }
}

fn invalid(message: &str) -> Refusal {
    Refusal {
        code: "invalid-input",
        message: message.to_string(),
        exit: 2,
    }
}

fn store_refusal(error: QuestionError) -> Refusal {
    let code = match error {
        QuestionError::AgentStoreRequired => "agent-store-required",
        QuestionError::InvalidInput => "invalid-input",
        QuestionError::UnknownTask => "unknown-task",
        QuestionError::UnknownQuestion => "unknown-question",
        QuestionError::StaleRevision => "stale-revision",
        QuestionError::InvalidCorrection => "invalid-correction",
        QuestionError::ApprovalRefused => "approval-refused",
        QuestionError::PrivateRecordUnavailable => "private-record-unavailable",
        QuestionError::NotSaved => "not-saved",
        QuestionError::CommitUnknown => "commit-unknown",
        QuestionError::StoreUnavailable => "store-unavailable",
        QuestionError::LinkAmbiguous => "link-ambiguous",
        QuestionError::InvalidSecret => "invalid-secret",
        QuestionError::SecretNeedsBoard => "secret-needs-board",
    };
    Refusal {
        code,
        message: error.to_string(),
        exit: 1,
    }
}

pub fn run(args: &[String], stdin: &mut dyn Read) -> CliOutput {
    match execute(args, stdin) {
        Ok(output) => output,
        Err(refusal) => CliOutput {
            stdout: String::new(),
            stderr: format!("tsk ui-answer: {}: {}\n", refusal.code, refusal.message),
            code: refusal.exit,
        },
    }
}

fn execute(args: &[String], stdin: &mut dyn Read) -> Result<CliOutput, Refusal> {
    let parsed = parse(args).map_err(|(message, _)| usage(message))?;
    if parsed.task.is_some() {
        return Err(usage("--task is a questions filter"));
    }
    let operand = parsed
        .operand
        .ok_or_else(|| usage("Q number or UUID required"))?;
    let file: PathBuf = parsed.file.ok_or_else(|| usage("--file is required"))?;
    let state_dir = parsed
        .state_dir
        .ok_or_else(|| store_refusal(QuestionError::AgentStoreRequired))?;
    let store = QuestionStore::new(state_dir).map_err(store_refusal)?;
    let record = lookup(&store, &operand, None).map_err(|error| match error {
        Lookup::Syntax => usage("invalid Q number or UUID"),
        Lookup::Store(error) => store_refusal(error),
    })?;
    let bytes = read_file(&file, stdin).map_err(|(message, exit)| Refusal {
        code: "input-unavailable",
        message,
        exit,
    })?;
    let input: UiAnswerInput =
        serde_json::from_slice(&bytes).map_err(|_| invalid("invalid ui-answer JSON"))?;
    let draft = draft(&record, input)?;
    let saved = store
        .answer(record.reference.id, draft)
        .map_err(store_refusal)?;
    // Same hint as an ordinary answer: identifiers only, to the asking Thread.
    let notification = notify_question(&saved, false);
    let reference = &saved.record.reference;
    let output = UiAnswerOutput {
        question_id: reference.id,
        number: reference.number,
        task_id: reference.task_id,
        revision: reference.revision,
        status: saved.durability,
        notification,
    };
    let stdout = serde_json::to_string(&output).map_err(|_| Refusal {
        code: "output-unavailable",
        message: "output serialization failed".into(),
        exit: 1,
    })?;
    Ok(CliOutput {
        stdout: format!("{stdout}\n"),
        stderr: String::new(),
        // Never report success on a save the store cannot prove durable.
        code: if saved.durability == QuestionDurability::Durable {
            0
        } else {
            1
        },
    })
}

/// The board's answer for the same pick: a plain answer stays plain, an approval carries
/// the exact terms the user saw, and the store re-checks both under its lock.
fn draft(record: &QuestionRecord, input: UiAnswerInput) -> Result<AnswerDraft, Refusal> {
    // A secret never travels through this entrypoint: only the board hands it to fnox.
    if record.reference.kind == QuestionKind::Secret {
        return Err(store_refusal(QuestionError::SecretNeedsBoard));
    }
    // A stale read refuses before any shape check, so the caller always learns to reread.
    if input.expected_revision != record.reference.revision {
        return Err(store_refusal(QuestionError::StaleRevision));
    }
    if record.draft.audience.is_none() {
        return Err(Refusal {
            code: "unrouted-question",
            message: format!(
                "Q{} has no target; route it with tsk route before answering",
                record.reference.number
            ),
            exit: 1,
        });
    }
    let picks = [
        input.choice.is_some(),
        input.text.is_some(),
        input.approve,
        input.reject,
    ]
    .iter()
    .filter(|pick| **pick)
    .count();
    if picks != 1 {
        return Err(invalid(
            "give exactly one of choice, text, approve, or reject",
        ));
    }
    let mut answer = AnswerDraft {
        expected_revision: input.expected_revision,
        text: String::new(),
        actor: USER.to_string(),
        origin: AnswerOrigin::UserUi,
        corrects_revision: input.corrects_revision,
        approve: false,
        confirmed: false,
        // Passed through so the store refuses terms on a plain answer.
        approval: input.approval,
        user_chat: None,
        source: None,
        action: None,
    };
    match record.reference.kind {
        QuestionKind::Secret => Err(store_refusal(QuestionError::SecretNeedsBoard)),
        QuestionKind::Finding => Err(Refusal {
            code: "finding-board-action",
            message: "a finding is decided on the board".into(),
            exit: 1,
        }),
        QuestionKind::Approval => {
            if input.choice.is_some() || input.text.is_some() {
                return Err(invalid("an approval takes approve or reject"));
            }
            if input.corrects_revision.is_some() {
                return Err(Refusal {
                    code: "invalid-correction",
                    message: "an approval is not corrected; ask a new question for new terms"
                        .into(),
                    exit: 1,
                });
            }
            if input.approve {
                answer.text = "approved".into();
                answer.approve = true;
                answer.confirmed = true;
            } else {
                answer.text = "rejected".into();
            }
            Ok(answer)
        }
        QuestionKind::Question => {
            if input.approve {
                return Err(store_refusal(QuestionError::ApprovalRefused));
            }
            if input.reject {
                return Err(invalid("reject answers an approval only"));
            }
            answer.text = match (input.choice, input.text) {
                (Some(choice), _) => record
                    .draft
                    .options
                    .get(choice)
                    .cloned()
                    .ok_or_else(|| invalid("choice is not one of the question's options"))?,
                (None, Some(text)) => {
                    let text = text.trim();
                    if text.is_empty() {
                        return Err(invalid("text is blank"));
                    }
                    text.to_string()
                }
                (None, None) => unreachable!("one pick is present"),
            };
            Ok(answer)
        }
    }
}
