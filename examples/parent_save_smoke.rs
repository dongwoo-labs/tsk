//! Repeatable check of the real stale-snapshot save boundary, not a unit-test seam.
//! cargo run --example parent_save_smoke -- <new task-local state directory>

use std::fs;
use std::path::{Path, PathBuf};

use tsk_tui::domain::{DomainState, ProvenanceOrigin, TaskScope};
use tsk_tui::store::TaskStore;

fn files(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut entries = fs::read_dir(root)
        .expect("state directory")
        .map(|entry| {
            let path = entry.expect("entry").path();
            let bytes = fs::read(&path).expect("state file");
            (path, bytes)
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

fn main() {
    let root = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("new state directory argument"),
    );
    fs::create_dir(&root).expect("refuse a pre-existing state directory");
    let store = TaskStore::new(&root);
    let mut initial = DomainState::new();
    let a = initial
        .create("A", None, TaskScope::Global, ProvenanceOrigin::Manual)
        .expect("create A");
    let b = initial
        .create("B", None, TaskScope::Global, ProvenanceOrigin::Manual)
        .expect("create B");
    store.save(&initial).expect("seed state");
    let mut first = store.load().expect("first snapshot");
    let mut stale = store.load().expect("second snapshot");
    first
        .set_parent(a, Some(b))
        .expect("A -> B is locally acyclic");
    stale
        .set_parent(b, Some(a))
        .expect("B -> A is locally acyclic");
    store.reload_merge_save(&mut first).expect("first save");
    fs::write(root.join("trash.jsonl"), []).expect("seed empty trash");
    let before = files(&root);
    let error = store
        .reload_merge_save(&mut stale)
        .expect_err("merged cycle must refuse");
    assert!(error.to_string().contains("cycle"), "{error}");
    assert_eq!(
        files(&root),
        before,
        "refused save changed live, backups, or trash"
    );
    let retained = store.load().expect("retained state");
    assert_eq!(retained.get(a).expect("A retained").parent_task_id, Some(b));
    assert_eq!(retained.get(b).expect("B retained").parent_task_id, None);
    println!("PASS stale snapshot: second save refused, A -> B retained, every state-file byte unchanged");
    // Save Recovery retains the original intent. Retry it after another writer
    // removes the conflicting edge from disk, without changing the stale intent.
    let mut correction = store.load().expect("correction snapshot");
    correction
        .set_parent(a, None)
        .expect("clear conflicting disk edge");
    store
        .reload_merge_save(&mut correction)
        .expect("save correction");
    store
        .reload_merge_save(&mut stale)
        .expect("retry original intent after correction");
    let retried = store.load().expect("retry state");
    assert_eq!(retried.get(a).expect("A").parent_task_id, None);
    assert_eq!(retried.get(b).expect("B").parent_task_id, Some(a));
    println!("PASS retry original intent after another writer clears conflicting edge");
}
