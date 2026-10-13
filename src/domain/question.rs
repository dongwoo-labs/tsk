//! Task-linked questions. Private content is never serialized into the shared task store.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    Question,
    Approval,
    Finding,
    /// The user types a secret on the board, which hands it to `fnox set`. The value is
    /// never stored, printed, or readable by an agent; the answer is only `set`.
    Secret,
}

/// Where a secret question's value goes: one `fnox set` target. It names the key only and
/// never carries a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretTarget {
    /// Environment-style key: `[A-Za-z_][A-Za-z0-9_]*`.
    pub key: String,
    /// fnox profile (`[A-Za-z0-9_-]+`); absent means fnox's default profile.
    #[serde(default)]
    pub profile: Option<String>,
    /// Absolute path to an existing regular (non-symlink) `fnox.toml`; absent means the
    /// global config (`fnox set -g`).
    #[serde(default)]
    pub config: Option<String>,
}

impl SecretTarget {
    /// Whether the key, profile, and config path are well formed. Pure: a stored record
    /// stays readable even after its config file goes away.
    pub fn is_well_formed(&self) -> bool {
        let key = self.key.as_bytes();
        let key_ok = key
            .first()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
            && key
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_');
        let profile_ok = self.profile.as_deref().is_none_or(|profile| {
            !profile.is_empty()
                && profile
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        });
        let config_ok = self
            .config
            .as_deref()
            .is_none_or(|config| std::path::Path::new(config).is_absolute());
        key_ok && profile_ok && config_ok
    }

    /// Whether the named config, if any, is a regular file (not a symlink) right now.
    /// Checked when asked and again right before the board runs fnox.
    pub fn config_present(&self) -> bool {
        self.config.as_deref().is_none_or(|config| {
            std::fs::symlink_metadata(config).is_ok_and(|meta| meta.file_type().is_file())
        })
    }
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

/// Who is expected to act on an open question. A routing label only: it grants no
/// approval or execution authority. A record without it reads as unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Audience {
    User,
    Lead,
}

/// The user's intent when deciding a finding. Intent only: it never proves that a
/// task ran or a thread started. A record without it reads as unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingAction {
    CreateAndRun,
    CreateOnly,
    Close,
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
    /// Routing label; absent means unknown, never guessed from names or status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<Audience>,
    /// Explicit Herdr agent name that receives a finding's run request. Absent means
    /// not set; `recipient` and `origin_thread` are never used in its place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_recipient: Option<String>,
    /// A secret question's `fnox set` target. Only `kind: secret` carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<SecretTarget>,
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
    /// Finding decision intent. Only findings accept it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<FindingAction>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<FindingAction>,
}

/// One audience change: same Q-ID, new immutable revision, nothing else moves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteDraft {
    pub expected_revision: Uuid,
    pub audience: Audience,
    /// Absent or null keeps the stored address.
    #[serde(default)]
    pub lead_recipient: Option<String>,
}

/// What a finding's answer history says about its created task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingLink {
    None,
    One(Uuid),
    /// Several distinct created tasks: never pick one, never create another.
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionRecord {
    pub reference: QuestionRef,
    pub draft: QuestionDraft,
    #[serde(with = "super::time_serde")]
    pub created_at: std::time::SystemTime,
    pub answers: Vec<QuestionAnswer>,
    /// Audience changes, oldest first. Never rewrites an answer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<QuestionRoute>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionRoute {
    pub revision: Uuid,
    /// How many answers existed when the route was written; orders it in history.
    pub after_answers: usize,
    pub audience: Audience,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_recipient: Option<String>,
    #[serde(with = "super::time_serde")]
    pub at: std::time::SystemTime,
}

impl QuestionRecord {
    /// Latest answer supersedes earlier answers but never removes them.
    pub fn current_answer(&self) -> Option<&QuestionAnswer> {
        self.answers.last()
    }

    /// The revision of the newest history event (answer or route), if any. The
    /// shared reference must point at it.
    pub fn head_revision(&self) -> Option<Uuid> {
        match self.routes.last() {
            Some(route) if route.after_answers >= self.answers.len() => Some(route.revision),
            _ => self.answers.last().map(|answer| answer.revision),
        }
    }

    pub fn is_open(&self) -> bool {
        self.answers.is_empty()
    }

    /// The created task across the whole preserved history, not just the current
    /// answer: a correction or close must never lose or duplicate the link.
    pub fn finding_link(&self) -> FindingLink {
        let ids: std::collections::BTreeSet<Uuid> = self
            .answers
            .iter()
            .filter_map(|answer| answer.created_task_id)
            .collect();
        let mut ids = ids.into_iter();
        match (ids.next(), ids.next()) {
            (None, _) => FindingLink::None,
            (Some(id), None) => FindingLink::One(id),
            _ => FindingLink::Ambiguous,
        }
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
