//! Shadow copy of question history to an EvoLoop work-question ledger.
//!
//! Off unless `TSK_EVOLOOP_URL` and a token are set. The local Agent store stays the
//! record of truth here: a copy runs after the local save has committed and never fails
//! or undoes it. A question whose copy fails (EvoLoop down, a refused call) stays in a
//! local pending list and is sent again, in history order, on the next question write.
//!
//! Each local history event maps to one EvoLoop event, keyed by the local question UUID
//! (`requestId`) and the local revision UUID (`commandId`), so a resend after a lost
//! response is answered from what EvoLoop already stored instead of a second copy.
//! A secret's value never leaves this machine: only `set` or `rejected` is sent.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::questions::QuestionStore;
use crate::domain::*;
use crate::fsperm;

const DIR: &str = "evoloop";
const MAP_FILE: &str = "map.json";
const PENDING_FILE: &str = "pending.json";
const LOCK_FILE: &str = "mirror.lock";
#[cfg(unix)]
const TIMEOUT_SECS: u32 = 5;

/// One HTTP call to EvoLoop: POST a JSON body to a path under the base URL and return
/// the JSON reply, or `None` on any failure (network, HTTP error, unreadable reply).
pub trait Transport {
    fn post(&self, path: &str, body: &Value) -> Option<Value>;
}

#[cfg(unix)]
#[derive(Debug, Clone)]
pub struct MirrorConfig {
    pub url: String,
    token: String,
    pub organization: Option<String>,
    pub expected_user_id: Option<String>,
}

#[cfg(unix)]
impl MirrorConfig {
    /// `TSK_EVOLOOP_URL` (an `https://` base), `TSK_EVOLOOP_TOKEN_FILE` (a file holding
    /// the Bearer token) and optional `TSK_EVOLOOP_ORG`. Anything missing turns the copy off.
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("TSK_EVOLOOP_URL").ok()?;
        let url = url.trim().trim_end_matches('/').to_string();
        if !url.starts_with("https://") || url.contains(['"', '\\', '\n', '\r']) {
            return None;
        }
        let token_file = std::env::var_os("TSK_EVOLOOP_TOKEN_FILE")?;
        let token = fs::read_to_string(token_file).ok()?.trim().to_string();
        if token.is_empty()
            || !token.chars().all(|c| c.is_ascii_graphic())
            || token.contains(['"', '\\'])
        {
            return None;
        }
        let organization = std::env::var("TSK_EVOLOOP_ORG")
            .ok()
            .map(|org| org.trim().to_string())
            .filter(|org| {
                !org.is_empty()
                    && org.chars().all(|c| c.is_ascii_graphic())
                    && !org.contains(['"', '\\'])
            });
        Some(Self {
            url,
            token,
            organization,
            expected_user_id: std::env::var("EVOLOOP_USER_ID").ok(),
        })
    }
}

/// The real transport: the hardened system curl, configured through stdin so the token
/// and the body never appear in the process arguments.
#[cfg(unix)]
pub struct CurlTransport {
    pub config: MirrorConfig,
    pub curl: PathBuf,
}

#[cfg(unix)]
fn curl_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(unix)]
impl CurlTransport {
    fn request(&self, path: &str, body: Option<&Value>, user_id: Option<&str>) -> Option<Value> {
        let mut config = format!(
            "url = {}\nheader = {}\nheader = \"Content-Type: application/json\"\n",
            curl_quote(&format!("{}{}", self.config.url, path)),
            curl_quote(&format!("Authorization: Bearer {}", self.config.token)),
        );
        if let Some(org) = &self.config.organization {
            config.push_str(&format!(
                "header = {}\n",
                curl_quote(&format!("X-Organization-ID: {org}"))
            ));
        }
        if let Some(user_id) = user_id {
            config.push_str(&format!(
                "header = {}\n",
                curl_quote(&format!("X-QAI-Expected-User-ID: {user_id}"))
            ));
        }
        if let Some(body) = body {
            config.push_str(&format!(
                "data-binary = {}\n",
                curl_quote(&serde_json::to_string(body).ok()?)
            ));
        }
        let mut child = crate::cli::update::hardened_curl(&self.curl, TIMEOUT_SECS)
            .args(["-K", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        child.stdin.take()?.write_all(config.as_bytes()).ok()?;
        let output = child.wait_with_output().ok()?;
        if !output.status.success() {
            return None;
        }
        serde_json::from_slice(&output.stdout).ok()
    }
}

#[cfg(unix)]
impl Transport for CurlTransport {
    fn post(&self, path: &str, body: &Value) -> Option<Value> {
        let context = self.request("/api/qai/auth/context", None, None)?;
        let user_id = context["user_id"].as_str()?;
        if user_id.is_empty()
            || user_id.len() > 128
            || !user_id.chars().all(|c| c.is_ascii_graphic())
            || self
                .config
                .expected_user_id
                .as_deref()
                .is_some_and(|expected| expected != user_id)
        {
            return None;
        }
        self.request(path, Some(body), Some(user_id))
    }
}

/// The configured transport, if the copy is on. macOS and Linux only, like the update check.
#[cfg(unix)]
pub fn configured_transport() -> Option<CurlTransport> {
    let config = MirrorConfig::from_env()?;
    let curl = crate::cli::update::curl_path().ok()?;
    Some(CurlTransport { config, curl })
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Mapped {
    remote: String,
    head: String,
    /// Local revision UUID -> EvoLoop revision, for every event already copied.
    revisions: BTreeMap<Uuid, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct MirrorMap {
    questions: BTreeMap<Uuid, Mapped>,
}

/// What one copy pass left behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirrorOutcome {
    pub pending: usize,
}

fn read_json<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> T {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_json<T: Serialize>(dir: &Path, name: &str, value: &T) -> std::io::Result<()> {
    let temp = dir.join(format!(".{name}.tmp.{}", Uuid::new_v4()));
    let mut file = fsperm::create_private_file(&temp)?;
    file.write_all(&serde_json::to_vec(value).map_err(std::io::Error::other)?)?;
    file.sync_all()?;
    fsperm::replace_file(&temp, &dir.join(name))
}

enum Event<'a> {
    Route(&'a QuestionRoute),
    Answer(&'a QuestionAnswer),
}

/// History in the order it happened: a route written after `k` answers follows them.
fn history(record: &QuestionRecord) -> Vec<Event<'_>> {
    let mut events = Vec::new();
    for index in 0..=record.answers.len() {
        events.extend(
            record
                .routes
                .iter()
                .filter(|route| route.after_answers == index)
                .map(Event::Route),
        );
        if let Some(answer) = record.answers.get(index) {
            events.push(Event::Answer(answer));
        }
    }
    events
}

fn kind_name(kind: QuestionKind) -> &'static str {
    match kind {
        QuestionKind::Approval => "approval",
        QuestionKind::Finding => "finding",
        QuestionKind::Secret => "secret",
        _ => "question",
    }
}

fn ask_body(record: &QuestionRecord, task_number: Option<u64>) -> Value {
    let draft = &record.draft;
    let mut body = serde_json::to_value(draft).unwrap_or_else(|_| json!({}));
    if let Some(approval) = &draft.approval {
        body["approvalTerms"] = serde_json::to_value(approval).unwrap_or(Value::Null);
    }
    let mut external = json!({
        "system": "tsk",
        "taskId": draft.task_id,
        "questionNumber": record.reference.number,
    });
    if let Some(number) = task_number {
        external["task"] = json!(format!("T{number}"));
    }
    json!({
        "requestId": record.reference.id,
        "kind": kind_name(draft.kind),
        "summary": record.reference.summary,
        "body": body,
        "audience": {"kind": draft.audience, "recipient": draft.recipient},
        "externalRef": external,
        "actor": draft.origin_thread,
        "minNumber": record.reference.number,
    })
}

fn answer_payload(record: &QuestionRecord, answer: &QuestionAnswer) -> Value {
    if record.draft.kind == QuestionKind::Secret {
        return json!({"text": answer.text});
    }
    let mut payload = json!({"text": answer.text, "origin": answer.origin});
    if let Some(comment) = &answer.comment {
        payload["comment"] = json!(comment);
    }
    if let Some(action) = &answer.action {
        payload["action"] = json!(action);
    }
    if let Some(evidence) = &answer.user_chat {
        payload["userChat"] = serde_json::to_value(evidence).unwrap_or(Value::Null);
    }
    if record.draft.kind == QuestionKind::Approval {
        let approved = answer.approval.is_some() && answer.approval == record.draft.approval;
        payload["approved"] = json!(approved);
        if approved {
            payload["approvalTerms"] =
                serde_json::to_value(&answer.approval).unwrap_or(Value::Null);
        } else if let Some(partial) = &answer.approval {
            payload["partialApproval"] = serde_json::to_value(partial).unwrap_or(Value::Null);
        }
    }
    payload
}

fn event_body(
    record: &QuestionRecord,
    event: &Event<'_>,
    mapped: &Mapped,
) -> Option<(Uuid, Value)> {
    Some(match event {
        Event::Route(route) => (
            route.revision,
            json!({
                "commandId": route.revision,
                "expectedRevision": mapped.head,
                "kind": "routed",
                "actor": route.lead_recipient.clone().unwrap_or_else(|| "lead".into()),
                "actorKind": "agent",
                "payload": {"audience": route.audience, "leadRecipient": route.lead_recipient},
            }),
        ),
        Event::Answer(answer) => {
            let person = matches!(
                answer.origin,
                AnswerOrigin::UserUi | AnswerOrigin::LeadUserChat
            );
            let mut body = json!({
                "commandId": answer.revision,
                "expectedRevision": mapped.head,
                "kind": if answer.corrects_revision.is_some() { "corrected" } else { "answered" },
                "actor": answer.actor,
                "actorKind": if person { "person" } else { "agent" },
                "payload": answer_payload(record, answer),
            });
            if let Some(corrects) = answer.corrects_revision {
                body["correctsRevision"] = json!(mapped.revisions.get(&corrects)?);
            }
            (answer.revision, body)
        }
    })
}

/// Copy one record's missing history. `false` stops the pass and keeps it pending.
fn sync_record(
    transport: &dyn Transport,
    record: &QuestionRecord,
    task_number: Option<u64>,
    map: &mut MirrorMap,
) -> bool {
    let id = record.reference.id;
    if let std::collections::btree_map::Entry::Vacant(slot) = map.questions.entry(id) {
        let Some(reply) = transport.post("/api/qai/work-questions", &ask_body(record, task_number))
        else {
            return false;
        };
        let (Some(remote), Some(head)) =
            (reply["questionId"].as_str(), reply["headRevision"].as_str())
        else {
            return false;
        };
        let mut mapped = Mapped {
            remote: remote.into(),
            head: head.into(),
            revisions: BTreeMap::new(),
        };
        // A resend after a lost reply: EvoLoop already holds later events too.
        for (event, remote_event) in history(record)
            .iter()
            .zip(reply["events"].as_array().into_iter().flatten().skip(1))
        {
            let local = match event {
                Event::Route(route) => route.revision,
                Event::Answer(answer) => answer.revision,
            };
            if let Some(revision) = remote_event["revision"].as_str() {
                mapped.revisions.insert(local, revision.into());
            }
        }
        slot.insert(mapped);
    }
    for event in history(record) {
        let mapped = map.questions.get_mut(&id).expect("inserted above");
        let local = match &event {
            Event::Route(route) => route.revision,
            Event::Answer(answer) => answer.revision,
        };
        if mapped.revisions.contains_key(&local) {
            continue;
        }
        let Some((local, body)) = event_body(record, &event, mapped) else {
            return false;
        };
        let path = format!("/api/qai/work-questions/{}/events", mapped.remote);
        let Some(reply) = transport.post(&path, &body) else {
            return false;
        };
        let Some(head) = reply["headRevision"].as_str() else {
            return false;
        };
        mapped.head = head.into();
        mapped.revisions.insert(local, head.into());
    }
    true
}

impl QuestionStore {
    /// Copy `saved` and every earlier pending question to EvoLoop. Never fails the
    /// caller: a failed copy is kept pending. `None` when the copy is off.
    #[cfg(unix)]
    pub fn mirror_saved(&self, saved: Uuid) -> Option<MirrorOutcome> {
        let transport = configured_transport()?;
        self.mirror_with(&transport, saved).ok()
    }

    /// The copy runs on macOS and Linux only.
    #[cfg(not(unix))]
    pub fn mirror_saved(&self, _saved: Uuid) -> Option<MirrorOutcome> {
        None
    }

    pub fn mirror_with(
        &self,
        transport: &dyn Transport,
        saved: Uuid,
    ) -> std::io::Result<MirrorOutcome> {
        let dir = self.path().join(DIR);
        fsperm::ensure_private_dir(&dir)?;
        let lock_path = dir.join(LOCK_FILE);
        let lock = fsperm::open_lock_file(&lock_path)?;
        fsperm::tighten_file(&lock_path);
        lock.lock()?;
        let mut map: MirrorMap = read_json(&dir.join(MAP_FILE));
        let mut pending: Vec<Uuid> = read_json(&dir.join(PENDING_FILE));
        if !pending.contains(&saved) {
            pending.push(saved);
        }
        let numbers: BTreeMap<Uuid, u64> = self.task_numbers().unwrap_or_default();
        let mut done = BTreeSet::new();
        for id in &pending {
            let record = match self.read(*id) {
                Ok(record) => record,
                Err(crate::store::QuestionError::UnknownQuestion) => {
                    done.insert(*id);
                    continue;
                }
                Err(_) => break,
            };
            if !sync_record(
                transport,
                &record,
                numbers.get(&record.draft.task_id).copied(),
                &mut map,
            ) {
                break;
            }
            done.insert(*id);
        }
        pending.retain(|id| !done.contains(id));
        write_json(&dir, MAP_FILE, &map)?;
        write_json(&dir, PENDING_FILE, &pending)?;
        Ok(MirrorOutcome {
            pending: pending.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A fake EvoLoop: stores what it was sent and answers like the real ledger.
    #[derive(Default)]
    struct Fake {
        calls: RefCell<Vec<(String, Value)>>,
        down: RefCell<bool>,
        heads: RefCell<BTreeMap<String, String>>,
    }

    impl Transport for Fake {
        fn post(&self, path: &str, body: &Value) -> Option<Value> {
            if *self.down.borrow() {
                return None;
            }
            self.calls.borrow_mut().push((path.into(), body.clone()));
            let mut heads = self.heads.borrow_mut();
            if path == "/api/qai/work-questions" {
                let remote = format!("remote-{}", body["requestId"].as_str()?);
                let head = format!("{remote}-0");
                heads.insert(remote.clone(), head.clone());
                return Some(
                    json!({"questionId": remote, "headRevision": head, "events": [{"revision": head}]}),
                );
            }
            let remote = path
                .strip_prefix("/api/qai/work-questions/")?
                .strip_suffix("/events")?
                .to_string();
            if heads.get(&remote)? != body["expectedRevision"].as_str()? {
                return None;
            }
            let head = format!("{remote}-{}", body["commandId"].as_str()?);
            heads.insert(remote, head.clone());
            Some(json!({"headRevision": head}))
        }
    }

    fn record(kind: QuestionKind) -> QuestionRecord {
        let draft: QuestionDraft = serde_json::from_value(json!({
            "task_id": Uuid::new_v4(), "kind": kind, "summary": "Which browser tool?",
            "origin_thread": "t12", "recipient": "user", "text": "Pick one",
            "options": [], "recommendation": null, "impact": null, "blocking_scope": null,
            "approval": null, "finding": null, "source": null, "audience": "user",
        }))
        .expect("draft shape");
        QuestionRecord {
            reference: QuestionRef {
                id: Uuid::new_v4(),
                number: 42,
                task_id: draft.task_id,
                kind,
                summary: draft.summary.clone(),
                revision: Uuid::new_v4(),
            },
            draft,
            created_at: std::time::SystemTime::now(),
            answers: Vec::new(),
            routes: Vec::new(),
        }
    }

    fn answer(text: &str, origin: AnswerOrigin, corrects: Option<Uuid>) -> QuestionAnswer {
        QuestionAnswer {
            revision: Uuid::new_v4(),
            created_task_id: None,
            corrects_revision: corrects,
            text: text.into(),
            actor: "user".into(),
            origin,
            at: std::time::SystemTime::now(),
            approval: None,
            user_chat: None,
            source: None,
            action: None,
            comment: None,
        }
    }

    #[test]
    fn copies_ask_answer_and_correction_once_in_order() {
        let fake = Fake::default();
        let mut map = MirrorMap::default();
        let mut question = record(QuestionKind::Question);
        assert!(sync_record(&fake, &question, Some(12), &mut map));
        let first = answer("Playwright CLI", AnswerOrigin::UserUi, None);
        question.answers.push(first.clone());
        question
            .answers
            .push(answer("MCP", AnswerOrigin::Agent, Some(first.revision)));
        assert!(sync_record(&fake, &question, Some(12), &mut map));
        assert!(sync_record(&fake, &question, Some(12), &mut map));
        let calls = fake.calls.borrow();
        assert_eq!(calls.len(), 3, "a repeated pass sends nothing new");
        assert_eq!(calls[0].1["externalRef"]["task"], "T12");
        assert_eq!(calls[0].1["minNumber"], 42);
        assert_eq!(calls[1].1["kind"], "answered");
        assert_eq!(calls[1].1["actorKind"], "person");
        assert_eq!(calls[2].1["kind"], "corrected");
        assert_eq!(calls[2].1["actorKind"], "agent");
        let remote = &map.questions[&question.reference.id];
        assert_eq!(
            calls[2].1["correctsRevision"],
            json!(remote.revisions[&first.revision])
        );
    }

    #[test]
    fn a_failed_copy_resumes_where_it_stopped() {
        let fake = Fake::default();
        let mut map = MirrorMap::default();
        let mut question = record(QuestionKind::Question);
        *fake.down.borrow_mut() = true;
        assert!(!sync_record(&fake, &question, None, &mut map));
        assert!(map.questions.is_empty());
        *fake.down.borrow_mut() = false;
        question
            .answers
            .push(answer("yes", AnswerOrigin::UserUi, None));
        assert!(sync_record(&fake, &question, None, &mut map));
        assert_eq!(fake.calls.borrow().len(), 2);
        assert!(fake.calls.borrow()[0].1["externalRef"]
            .get("task")
            .is_none());
    }

    #[test]
    fn a_secret_sends_only_set_or_rejected() {
        let fake = Fake::default();
        let mut map = MirrorMap::default();
        let mut secret = record(QuestionKind::Secret);
        let mut set = answer("set", AnswerOrigin::UserUi, None);
        set.comment = Some("pasted ghp_value".into());
        secret.answers.push(set);
        assert!(sync_record(&fake, &secret, None, &mut map));
        assert_eq!(fake.calls.borrow()[1].1["payload"], json!({"text": "set"}));
    }

    #[test]
    fn routes_land_between_the_answers_they_followed() {
        let mut question = record(QuestionKind::Question);
        let first = answer("a", AnswerOrigin::Agent, None);
        question.answers.push(first.clone());
        question.routes.push(QuestionRoute {
            revision: Uuid::new_v4(),
            after_answers: 0,
            audience: Audience::Lead,
            lead_recipient: None,
            at: std::time::SystemTime::now(),
        });
        let order: Vec<Uuid> = history(&question)
            .iter()
            .map(|event| match event {
                Event::Route(route) => route.revision,
                Event::Answer(answer) => answer.revision,
            })
            .collect();
        assert_eq!(order, vec![question.routes[0].revision, first.revision]);
    }

    #[cfg(unix)]
    #[test]
    fn curl_binds_protected_requests_to_authenticated_context() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tsk-mirror-curl-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let curl = dir.join("curl");
        fs::write(
            &curl,
            r##"#!/bin/sh
root=$(dirname "$0")
config=$(cat)
case "$*" in *test-bearer*|*requestId*) exit 1 ;; esac
case "$config" in *'header = "Authorization: Bearer test-bearer"'*) ;; *) exit 1 ;; esac
case "$config" in *'header = "X-Organization-ID: test-org"'*) ;; *) exit 1 ;; esac
case "$config" in
  *'https://evo.example/api/qai/auth/context"'*)
    case "$config" in *data-binary*|*X-QAI-Expected-User-ID*) exit 1 ;; esac
    if test -f "$root/auth-failed"; then exit 22; fi
    cat "$root/context.json" ;;
  *)
    printf 'sent\n' >> "$root/sent"
    case "$config" in *data-binary*requestId*) ;; *) exit 1 ;; esac
    case "$config" in
      *'header = "X-QAI-Expected-User-ID: mirror-user"'*)
        printf '%s' '{"accepted":true}' ;;
      *) exit 22 ;;
    esac ;;
esac
"##,
        )
        .unwrap();
        fs::set_permissions(&curl, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(dir.join("context.json"), r#"{"user_id":"mirror-user"}"#).unwrap();
        let mut transport = CurlTransport {
            config: MirrorConfig {
                url: "https://evo.example".into(),
                token: "test-bearer".into(),
                organization: Some("test-org".into()),
                expected_user_id: None,
            },
            curl,
        };
        for path in [
            "/api/qai/work-questions",
            "/api/qai/work-questions/remote/events",
        ] {
            assert_eq!(
                transport.post(path, &json!({"requestId": "test-request"})),
                Some(json!({"accepted": true})),
                "the protected request must carry the authenticated identity"
            );
        }
        transport.config.expected_user_id = Some("mirror-user".into());
        assert!(transport
            .post("/api/qai/work-questions", &json!({"requestId": "test"}))
            .is_some());
        let sent = fs::read_to_string(dir.join("sent")).unwrap();
        assert_eq!(sent.lines().count(), 3);
        transport.config.expected_user_id = Some("different-user".into());
        assert!(transport
            .post("/api/qai/work-questions", &json!({}))
            .is_none());
        transport.config.expected_user_id = None;
        for context in [
            json!({}),
            json!({"user_id": null}),
            json!({"user_id": 42}),
            json!({"user_id": ""}),
            json!({"user_id": "bad\r\nInjected: value"}),
            json!({"user_id": "bad user"}),
            json!({"user_id": "a".repeat(129)}),
        ] {
            fs::write(
                dir.join("context.json"),
                serde_json::to_vec(&context).unwrap(),
            )
            .unwrap();
            assert!(transport
                .post("/api/qai/work-questions", &json!({}))
                .is_none());
        }
        fs::write(dir.join("context.json"), b"not json").unwrap();
        assert!(transport
            .post("/api/qai/work-questions", &json!({}))
            .is_none());
        fs::write(dir.join("context.json"), r#"{"user_id":"mirror-user"}"#).unwrap();
        fs::write(dir.join("auth-failed"), b"").unwrap();
        assert!(transport
            .post("/api/qai/work-questions", &json!({}))
            .is_none());
        assert_eq!(
            fs::read_to_string(dir.join("sent")).unwrap(),
            sent,
            "identity failures must not send a protected POST"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn curl_config_quotes_values() {
        assert_eq!(curl_quote(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
