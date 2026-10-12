//! Three-way merge of two `tsk.json` documents against their common ancestor.
//!
//! `tsk sync` shares the store through git, and git calls `tsk merge-driver` when both
//! sides changed `tsk.json`. This is the pure half of that driver: documents in, one
//! merged document out, no I/O. Rules, by task id:
//!
//! - present on one side only and absent from base: kept (a new task);
//! - changed on one side only: that side wins. "Changed" means the revision differs
//!   from base, or the number does (a renumber from an earlier merge);
//! - changed on both sides to different revisions: the later `updated_at` wins, a tie
//!   keeps ours;
//! - present in base but gone from one side (trashed or purged there): dropped when the
//!   other side still holds the base revision, otherwise the other side's edit is kept;
//! - two distinct tasks holding one `T` number (or notice `N` number): the task that held
//!   it in base keeps it, else the one that held it on our side, and every other holder
//!   gets a fresh number above every number either side has handed out.
//!
//! Project records take ours and add any project only theirs has. The undo stack is ours,
//! pruned of entries whose target no longer matches.

use std::collections::{BTreeMap, BTreeSet};

use uuid::Uuid;

use super::{DomainState, Task, STORE_FORMAT_VERSION};

/// Why two documents cannot be merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeError {
    /// A side was written in a store format other than this binary's.
    FormatMismatch {
        ours: u32,
        theirs: u32,
        supported: u32,
    },
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeError::FormatMismatch {
                ours,
                theirs,
                supported,
            } => write!(
                f,
                "store formats differ (ours {ours}, theirs {theirs}, this tsk writes {supported})"
            ),
        }
    }
}

impl std::error::Error for MergeError {}

/// Merge `ours` and `theirs` against `base` (`None` when the sides share no ancestor,
/// which merges two independent boards as if both were new).
///
/// Task order is ours, then tasks only theirs holds in their order.
pub fn merge_documents(
    base: Option<&DomainState>,
    ours: &DomainState,
    theirs: &DomainState,
) -> Result<DomainState, MergeError> {
    if ours.format_version != STORE_FORMAT_VERSION || theirs.format_version != STORE_FORMAT_VERSION
    {
        return Err(MergeError::FormatMismatch {
            ours: ours.format_version,
            theirs: theirs.format_version,
            supported: STORE_FORMAT_VERSION,
        });
    }
    let base_tasks = index(base.map(|state| state.tasks.as_slice()).unwrap_or(&[]));
    let our_tasks = index(&ours.tasks);
    let their_tasks = index(&theirs.tasks);

    let mut order: Vec<Uuid> = ours.tasks.iter().map(|task| task.id).collect();
    order.extend(
        theirs
            .tasks
            .iter()
            .map(|task| task.id)
            .filter(|id| !our_tasks.contains_key(id)),
    );
    let mut tasks: Vec<Task> = order
        .iter()
        .filter_map(|id| {
            pick(
                base_tasks.get(id).copied(),
                our_tasks.get(id).copied(),
                their_tasks.get(id).copied(),
            )
        })
        .map(|task| {
            let mut task = task.clone();
            task.merge_base_revision = None;
            task
        })
        .collect();

    let next_task_number = resolve_numbers(
        &mut tasks,
        ours.next_task_number.max(theirs.next_task_number),
        |task| task.notice.is_none().then_some(task.number).flatten(),
        |task, number| task.number = Some(number),
        &base_tasks,
        &our_tasks,
    );
    let next_notice_number = resolve_numbers(
        &mut tasks,
        ours.next_notice_number.max(theirs.next_notice_number),
        |task| task.notice.as_ref().and_then(|notice| notice.number),
        |task, number| {
            if let Some(notice) = task.notice.as_mut() {
                notice.number = Some(number);
            }
        },
        &base_tasks,
        &our_tasks,
    );

    let mut projects = ours.projects.clone();
    for (path, record) in &theirs.projects {
        projects
            .entry(path.clone())
            .or_insert_with(|| record.clone());
    }

    let mut merged = DomainState {
        format_version: STORE_FORMAT_VERSION,
        next_task_number,
        next_notice_number,
        tasks,
        projects,
        project_intents: BTreeMap::new(),
        undo_stack: ours.undo_stack.clone(),
    };
    merged.prune_undo_for_persistence();
    Ok(merged)
}

fn index(tasks: &[Task]) -> BTreeMap<Uuid, &Task> {
    tasks.iter().map(|task| (task.id, task)).collect()
}

/// A side changed a task relative to base: a new revision, or a renumber.
fn changed(base: &Task, side: &Task) -> bool {
    side.revision != base.revision || side.number != base.number || side.notice != base.notice
}

/// Both sides hold the task and both changed it: the later edit wins, a tie keeps ours.
fn later<'a>(ours: &'a Task, theirs: &'a Task) -> &'a Task {
    if theirs.revision != ours.revision && theirs.updated_at > ours.updated_at {
        theirs
    } else {
        ours
    }
}

fn pick<'a>(
    base: Option<&Task>,
    ours: Option<&'a Task>,
    theirs: Option<&'a Task>,
) -> Option<&'a Task> {
    match (base, ours, theirs) {
        (_, None, None) => None,
        (None, Some(ours), None) => Some(ours),
        (None, None, Some(theirs)) => Some(theirs),
        (None, Some(ours), Some(theirs)) => Some(later(ours, theirs)),
        // Gone on one side: an unchanged survivor follows the removal, an edited one stays.
        (Some(base), Some(ours), None) => (ours.revision != base.revision).then_some(ours),
        (Some(base), None, Some(theirs)) => (theirs.revision != base.revision).then_some(theirs),
        (Some(base), Some(ours), Some(theirs)) => {
            match (changed(base, ours), changed(base, theirs)) {
                (false, true) => Some(theirs),
                (true, true) => Some(later(ours, theirs)),
                _ => Some(ours),
            }
        }
    }
}

/// Give every duplicate holder of one number but the keeper a fresh number above
/// `floor` and every number in use; return the next free number.
fn resolve_numbers(
    tasks: &mut [Task],
    floor: u64,
    number_of: impl Fn(&Task) -> Option<u64>,
    assign: impl Fn(&mut Task, u64),
    base: &BTreeMap<Uuid, &Task>,
    ours: &BTreeMap<Uuid, &Task>,
) -> u64 {
    let mut holders: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (index, task) in tasks.iter().enumerate() {
        if let Some(number) = number_of(task) {
            holders.entry(number).or_default().push(index);
        }
    }
    let mut next = holders
        .keys()
        .next_back()
        .map_or(1, |max| max.saturating_add(1))
        .max(floor)
        .max(1);
    let mut renumber: BTreeSet<usize> = BTreeSet::new();
    for (number, indexes) in &holders {
        if indexes.len() < 2 {
            continue;
        }
        let held_in = |side: &BTreeMap<Uuid, &Task>, index: usize| {
            side.get(&tasks[index].id)
                .is_some_and(|task| number_of(task) == Some(*number))
        };
        let keeper = indexes
            .iter()
            .copied()
            .find(|&index| held_in(base, index))
            .or_else(|| indexes.iter().copied().find(|&index| held_in(ours, index)))
            .unwrap_or_else(|| {
                *indexes
                    .iter()
                    .min_by_key(|&&index| (tasks[index].created_at, tasks[index].id))
                    .expect("a duplicate group has holders")
            });
        renumber.extend(indexes.iter().copied().filter(|&index| index != keeper));
    }
    let mut renumber: Vec<usize> = renumber.into_iter().collect();
    renumber.sort_by_key(|&index| (tasks[index].created_at, tasks[index].id));
    for index in renumber {
        assign(&mut tasks[index], next);
        next = next.checked_add(1).expect("task number exhausted");
    }
    next
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;
    use crate::domain::{HumanStatus, ProvenanceOrigin, TaskScope};

    fn add(state: &mut DomainState, title: &str) -> Uuid {
        state
            .create(
                title,
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create")
    }

    /// What a save leaves on disk: numbers assigned, no merge bases.
    fn persisted(mut state: DomainState) -> DomainState {
        state.assign_numbers_for_persistence();
        state.clear_merge_bases();
        state
    }

    fn base_with(titles: &[&str]) -> (DomainState, Vec<Uuid>) {
        let mut state = DomainState::new();
        let ids = titles.iter().map(|title| add(&mut state, title)).collect();
        (persisted(state), ids)
    }

    fn task(state: &DomainState, id: Uuid) -> &Task {
        state.get(id).expect("task present")
    }

    fn set_updated(state: &mut DomainState, id: Uuid, at: SystemTime) {
        state
            .tasks
            .iter_mut()
            .find(|task| task.id == id)
            .expect("task")
            .updated_at = at;
    }

    #[test]
    fn a_change_on_one_side_only_wins_from_either_side() {
        let (base, ids) = base_with(&["one", "two"]);
        let mut ours = base.clone();
        ours.set_status(ids[0], HumanStatus::Started).unwrap();
        let ours = persisted(ours);
        let mut theirs = base.clone();
        theirs
            .edit(ids[1], "two, edited", None, TaskScope::Global, None)
            .unwrap();
        let theirs = persisted(theirs);

        let merged = merge_documents(Some(&base), &ours, &theirs).expect("merge");

        assert_eq!(task(&merged, ids[0]).status, HumanStatus::Started);
        assert_eq!(task(&merged, ids[1]).title, "two, edited");
        assert_eq!(merged.tasks().len(), 2);
    }

    #[test]
    fn a_change_on_both_sides_takes_the_later_updated_at_and_a_tie_keeps_ours() {
        let (base, ids) = base_with(&["shared"]);
        let id = ids[0];
        let early = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let late = early + Duration::from_secs(60);

        let mut ours = base.clone();
        ours.set_status(id, HumanStatus::Blocked).unwrap();
        let mut theirs = base.clone();
        theirs.set_status(id, HumanStatus::Review).unwrap();

        set_updated(&mut ours, id, early);
        set_updated(&mut theirs, id, late);
        let merged = merge_documents(
            Some(&base),
            &persisted(ours.clone()),
            &persisted(theirs.clone()),
        )
        .unwrap();
        assert_eq!(task(&merged, id).status, HumanStatus::Review);

        set_updated(&mut ours, id, late);
        set_updated(&mut theirs, id, early);
        let merged = merge_documents(
            Some(&base),
            &persisted(ours.clone()),
            &persisted(theirs.clone()),
        )
        .unwrap();
        assert_eq!(task(&merged, id).status, HumanStatus::Blocked);

        set_updated(&mut theirs, id, late);
        let merged = merge_documents(Some(&base), &persisted(ours), &persisted(theirs)).unwrap();
        assert_eq!(
            task(&merged, id).status,
            HumanStatus::Blocked,
            "tie keeps ours"
        );
    }

    #[test]
    fn a_task_removed_on_one_side_and_unchanged_on_the_other_is_dropped() {
        let (base, ids) = base_with(&["keep", "trashed"]);
        let mut theirs = base.clone();
        theirs.remove_tasks(&BTreeSet::from([ids[1]]));
        let theirs = persisted(theirs);

        let merged = merge_documents(Some(&base), &base, &theirs).unwrap();
        assert!(merged.get(ids[1]).is_none());
        assert!(merged.get(ids[0]).is_some());

        let merged = merge_documents(Some(&base), &theirs, &base).unwrap();
        assert!(
            merged.get(ids[1]).is_none(),
            "symmetric when ours removed it"
        );
    }

    #[test]
    fn a_task_removed_on_one_side_and_changed_on_the_other_keeps_the_change() {
        let (base, ids) = base_with(&["contested"]);
        let id = ids[0];
        let mut ours = base.clone();
        ours.remove_tasks(&BTreeSet::from([id]));
        let ours = persisted(ours);
        let mut theirs = base.clone();
        theirs.set_status(id, HumanStatus::Started).unwrap();
        let theirs = persisted(theirs);

        let merged = merge_documents(Some(&base), &ours, &theirs).unwrap();
        assert_eq!(task(&merged, id).status, HumanStatus::Started);
        let merged = merge_documents(Some(&base), &theirs, &ours).unwrap();
        assert_eq!(task(&merged, id).status, HumanStatus::Started);
    }

    #[test]
    fn new_tasks_on_both_sides_with_one_number_get_unique_numbers() {
        let (base, ids) = base_with(&["existing"]);
        assert_eq!(task(&base, ids[0]).number, Some(1));
        let mut ours = base.clone();
        let ours_new = add(&mut ours, "ours new");
        let ours = persisted(ours);
        let mut theirs = base.clone();
        let theirs_new = add(&mut theirs, "theirs new");
        let theirs_second = add(&mut theirs, "theirs second");
        let theirs = persisted(theirs);
        assert_eq!(task(&ours, ours_new).number, Some(2));
        assert_eq!(task(&theirs, theirs_new).number, Some(2));
        assert_eq!(task(&theirs, theirs_second).number, Some(3));

        let merged = merge_documents(Some(&base), &ours, &theirs).unwrap();

        assert_eq!(task(&merged, ids[0]).number, Some(1));
        assert_eq!(task(&merged, ours_new).number, Some(2), "ours keeps it");
        assert_eq!(task(&merged, theirs_second).number, Some(3));
        assert_eq!(task(&merged, theirs_new).number, Some(4), "fresh above max");
        let numbers: BTreeSet<u64> = merged.tasks().iter().filter_map(|t| t.number).collect();
        assert_eq!(numbers.len(), merged.tasks().len());
        assert_eq!(merged.next_task_number, 5);
    }

    #[test]
    fn the_holder_from_base_keeps_a_contested_number() {
        let (base, ids) = base_with(&["old"]);
        let mut theirs = base.clone();
        let newcomer = add(&mut theirs, "new");
        let mut theirs = persisted(theirs);
        // A newcomer that somehow carries the base holder's number yields it.
        theirs
            .tasks
            .iter_mut()
            .find(|task| task.id == newcomer)
            .unwrap()
            .number = Some(1);

        let merged = merge_documents(Some(&base), &base, &theirs).unwrap();
        assert_eq!(task(&merged, ids[0]).number, Some(1));
        // Their counter already handed out 2, so the newcomer draws 3.
        assert_eq!(task(&merged, newcomer).number, Some(3));
    }

    #[test]
    fn next_numbers_cover_both_sides_counters_and_numbers() {
        let (base, _) = base_with(&["a"]);
        let mut ours = base.clone();
        ours.next_task_number = 40;
        ours.next_notice_number = 3;
        let mut theirs = base.clone();
        theirs.next_task_number = 7;
        theirs.next_notice_number = 9;
        let merged = merge_documents(Some(&base), &ours, &theirs).unwrap();
        assert_eq!(merged.next_task_number, 40);
        assert_eq!(merged.next_notice_number, 9);

        // A renumbered collision draws above both counters.
        let mut ours = base.clone();
        let a = add(&mut ours, "ours");
        let mut ours = persisted(ours);
        ours.next_task_number = 10;
        let mut theirs = base.clone();
        let b = add(&mut theirs, "theirs");
        let theirs = persisted(theirs);
        let merged = merge_documents(Some(&base), &ours, &theirs).unwrap();
        assert_eq!(task(&merged, a).number, Some(2));
        assert_eq!(task(&merged, b).number, Some(10));
        assert_eq!(merged.next_task_number, 11);
    }

    #[test]
    fn notice_numbers_are_deduplicated_separately_from_task_numbers() {
        let base = persisted(DomainState::new());
        let mut ours = base.clone();
        let first = ours
            .create_notice(
                "a",
                "ours notice",
                None,
                HumanStatus::Review,
                TaskScope::Global,
                Vec::new(),
            )
            .unwrap();
        let ours = persisted(ours);
        let mut theirs = base.clone();
        let second = theirs
            .create_notice(
                "b",
                "their notice",
                None,
                HumanStatus::Review,
                TaskScope::Global,
                Vec::new(),
            )
            .unwrap();
        let theirs = persisted(theirs);

        let merged = merge_documents(Some(&base), &ours, &theirs).unwrap();
        let number = |id| task(&merged, id).notice.as_ref().unwrap().number;
        assert_eq!(number(first), Some(1));
        assert_eq!(number(second), Some(2));
        assert_eq!(merged.next_notice_number, 3);
        assert!(merged.tasks().iter().all(|task| task.number.is_none()));
    }

    #[test]
    fn a_renumber_from_an_earlier_merge_wins_over_an_unchanged_side() {
        let (base, ids) = base_with(&["renumbered"]);
        let mut theirs = base.clone();
        theirs.tasks[0].number = Some(9);
        let merged = merge_documents(Some(&base), &base, &theirs).unwrap();
        assert_eq!(task(&merged, ids[0]).number, Some(9));
    }

    #[test]
    fn unrelated_boards_merge_as_two_sets_of_new_tasks() {
        let (ours, our_ids) = base_with(&["mine"]);
        let (theirs, their_ids) = base_with(&["yours"]);
        let merged = merge_documents(None, &ours, &theirs).unwrap();
        assert_eq!(task(&merged, our_ids[0]).number, Some(1));
        assert_eq!(task(&merged, their_ids[0]).number, Some(2));
    }

    #[test]
    fn projects_take_ours_and_add_theirs_and_undo_keeps_ours() {
        let mut base = DomainState::new();
        let id = base
            .create(
                "p",
                None,
                TaskScope::Project {
                    path: "/work/a".into(),
                },
                ProvenanceOrigin::Manual,
                None,
            )
            .unwrap();
        let base = persisted(base);
        let mut ours = base.clone();
        ours.complete(id).unwrap();
        let ours = persisted(ours);
        let mut theirs = base.clone();
        theirs.archive_project("/work/a").unwrap();
        let theirs = persisted(theirs);
        assert!(!ours.undo_stack.is_empty());

        let merged = merge_documents(Some(&base), &ours, &theirs).unwrap();
        assert!(merged.is_project_archived("/work/a"));
        assert_eq!(merged.undo_stack, ours.undo_stack);
    }

    #[test]
    fn a_side_in_another_format_refuses() {
        let base = DomainState::new();
        let mut theirs = DomainState::new();
        theirs.format_version = STORE_FORMAT_VERSION + 1;
        assert_eq!(
            merge_documents(Some(&base), &base, &theirs).unwrap_err(),
            MergeError::FormatMismatch {
                ours: STORE_FORMAT_VERSION,
                theirs: STORE_FORMAT_VERSION + 1,
                supported: STORE_FORMAT_VERSION,
            }
        );
    }
}
