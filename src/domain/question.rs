//! Task-linked questions. Private content is never serialized into the shared task store.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    Question,
    Approval,
    Finding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionRef {
    pub id: Uuid,
    pub number: u64,
    pub task_id: Uuid,
    pub kind: QuestionKind,
    pub summary: String,
    /// Immutable private snapshot UUID, not a user-provided path.
    pub revision: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalTerms {
    pub target: String,
    pub action: String,
    /// Exact Git SHA (40 hex digits) or plan digest (64 hex digits).
    pub h: String,
    pub scope: String,
    pub rollback: String,
}

/// Original identifiers are metadata, never authority for a new action.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionSource {
    pub system: Option<String>,
    pub question_uuid: Option<String>,
    pub comment_id: Option<String>,
    pub url: Option<String>,
    pub answer_id: Option<String>,
    pub parent_question_id: Option<String>,
    pub actor: Option<String>,
    pub time: Option<String>,
    pub revision: Option<String>,
    /// Preserve imported uncertainty verbatim. This field grants no authority.
    pub applied: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingDetails {
    pub kind: FindingKind,
    pub detail: String,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    Issue,
    Opportunity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionDraft {
    pub task_id: Uuid,
    pub kind: QuestionKind,
    pub summary: String,
    pub origin_thread: String,
    pub recipient: String,
    pub text: String,
    #[serde(default)]
    pub options: Vec<String>,
    pub recommendation: Option<String>,
    pub impact: Option<String>,
    pub blocking_scope: Option<String>,
    pub approval: Option<ApprovalTerms>,
    pub finding: Option<FindingDetails>,
    pub source: Option<QuestionSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerOrigin {
    Agent,
    UserUi,
    LeadUserChat,
    Imported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserChatEvidence {
    pub original_user: String,
    pub original_text: String,
    pub source: String,
    pub original_time: String,
    /// The Lead checked the original user message, not an agent's paraphrase.
    pub original_verified: bool,
}

/// CLI ordinary replies construct Agent origin; UserUi is the UI-only API boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerDraft {
    pub expected_revision: Uuid,
    pub text: String,
    pub actor: String,
    pub origin: AnswerOrigin,
    pub corrects_revision: Option<Uuid>,
    #[serde(default)]
    pub approve: bool,
    #[serde(default)]
    pub confirmed: bool,
    pub approval: Option<ApprovalTerms>,
    pub user_chat: Option<UserChatEvidence>,
    pub source: Option<QuestionSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionAnswer {
    pub revision: Uuid,
    /// Finding closure can link the task created in the same locked shared commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_task_id: Option<Uuid>,
    pub corrects_revision: Option<Uuid>,
    pub text: String,
    pub actor: String,
    pub origin: AnswerOrigin,
    #[serde(with = "super::time_serde")]
    pub at: std::time::SystemTime,
    pub approval: Option<ApprovalTerms>,
    pub user_chat: Option<UserChatEvidence>,
    pub source: Option<QuestionSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionRecord {
    pub reference: QuestionRef,
    pub draft: QuestionDraft,
    #[serde(with = "super::time_serde")]
    pub created_at: std::time::SystemTime,
    pub answers: Vec<QuestionAnswer>,
}

impl QuestionRecord {
    /// Latest answer supersedes earlier answers but never removes them.
    pub fn current_answer(&self) -> Option<&QuestionAnswer> {
        self.answers.last()
    }

    pub fn is_open(&self) -> bool {
        self.answers.is_empty()
    }

    /// Check authority against the action about to run, not just a saved yes/no label.
    pub fn approval_for(&self, terms: &ApprovalTerms) -> Option<&QuestionAnswer> {
        self.current_answer().filter(|answer| {
            matches!(
                answer.origin,
                AnswerOrigin::UserUi | AnswerOrigin::LeadUserChat
            ) && answer.approval.as_ref() == Some(terms)
                && self.draft.approval.as_ref() == Some(terms)
        })
    }
}
