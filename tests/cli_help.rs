use std::io::Cursor;
use std::process::Command;

use tsk_tui::cli::run_with;

fn help(args: &[&str]) -> tsk_tui::cli::CliOutput {
    run_with(args, Cursor::new(Vec::<u8>::new()), true)
}

fn executable(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(args)
        .output()
        .expect("run tsk")
}

/// Each heading is a whole line (`usage:` starts one), found in this order.
fn assert_sections_in_order(output: &str, headings: &[&str]) {
    let lines: Vec<&str> = output.lines().collect();
    let mut previous = 0;
    for heading in headings {
        let position = lines[previous..]
            .iter()
            .position(|line| {
                line == heading || (*heading == "usage:" && line.starts_with("usage: "))
            })
            .unwrap_or_else(|| panic!("help should contain {heading:?} after the prior section"));
        previous += position;
    }
    assert!(
        lines.contains(&"Exit:"),
        "help should contain an exit section"
    );
}

macro_rules! verb_help_test {
    ($name:ident, [$($arg:expr),+], [$($heading:expr),+]) => {
        #[test]
        fn $name() {
            let output = help(&["tsk", $($arg),+]);
            assert_eq!(output.code, 0);
            assert!(output.stderr.is_empty());
            assert_sections_in_order(&output.stdout, &[$($heading),+]);
        }
    };
}

verb_help_test!(
    add_help_has_reference_sections,
    ["add", "--help"],
    [
        "usage:",
        "Scope",
        "Options",
        "Output",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    list_help_has_reference_sections,
    ["list", "--help"],
    [
        "usage:",
        "Arguments",
        "Scope",
        "Filters",
        "Arrange",
        "Options",
        "Output",
        "Examples:",
        "Exit:"
    ]
);
verb_help_test!(
    status_help_has_reference_sections,
    ["status", "--help"],
    [
        "usage:",
        "Arguments",
        "Started",
        "Done",
        "Blocked",
        "Review",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    edit_help_has_reference_sections,
    ["edit", "--help"],
    [
        "usage:",
        "Arguments",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    steps_help_has_reference_sections,
    ["steps", "--help"],
    [
        "usage:",
        "Arguments",
        "Actions",
        "Place",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    reply_help_has_reference_sections,
    ["reply", "--help"],
    [
        "usage:",
        "Arguments",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    dispatch_help_has_reference_sections,
    ["dispatch", "--help"],
    [
        "usage:",
        "Arguments",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    clean_help_has_reference_sections,
    ["clean", "--help"],
    [
        "usage:",
        "Arguments",
        "Options",
        "Output",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    trash_help_has_reference_sections,
    ["trash", "--help"],
    [
        "usage:",
        "Arguments",
        "Actions",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    backup_help_has_reference_sections,
    ["backup", "--help"],
    [
        "usage:",
        "Arguments",
        "Actions",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    restore_help_has_reference_sections,
    ["restore", "--help"],
    [
        "usage:",
        "Arguments",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    archive_help_has_reference_sections,
    ["archive", "--help"],
    [
        "usage:",
        "Arguments",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    unarchive_help_has_reference_sections,
    ["unarchive", "--help"],
    [
        "usage:",
        "Arguments",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    project_help_has_reference_sections,
    ["project", "--help"],
    [
        "usage:",
        "Arguments",
        "Actions",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    import_help_has_reference_sections,
    ["import", "--help"],
    [
        "usage:",
        "Linear filters",
        "Scope",
        "Output",
        "Values",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    setup_help_has_reference_sections,
    ["setup", "--help"],
    [
        "usage:",
        "Actions",
        "Options",
        "Output",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    update_help_has_reference_sections,
    ["update", "--help"],
    ["usage:", "Examples:", "Refusals (exit 1):", "Exit:"]
);
verb_help_test!(
    guide_help_has_reference_sections,
    ["guide", "--help"],
    ["usage:", "Examples:", "Exit:"]
);

#[test]
fn help_without_an_operand_is_top_level_help() {
    let via_help = executable(&["help"]);
    let global = executable(&["--help"]);
    assert_eq!(via_help.status.code(), Some(0));
    assert!(via_help.stderr.is_empty());
    assert_eq!(via_help.stdout, global.stdout);
    assert_eq!(
        String::from_utf8(via_help.stdout).expect("UTF-8 help"),
        tsk_tui::cli::presenter::top_level_help()
    );
}

#[test]
fn archive_help_uses_verb_specific_exit_lines() {
    let archive = help(&["tsk", "archive", "--help"]);
    assert!(archive.stdout.contains("0 task archived, or already was"));
    assert!(!archive.stdout.contains("task unarchived"));

    let unarchive = help(&["tsk", "unarchive", "--help"]);
    assert!(unarchive
        .stdout
        .contains("0 task unarchived, or already was"));
    assert!(!unarchive.stdout.contains("archived flag set"));
}

#[test]
fn command_help_lines_have_no_trailing_whitespace() {
    for args in [
        &["--help"][..],
        &["add", "--help"],
        &["list", "--help"],
        &["status", "--help"],
        &["edit", "--help"],
        &["steps", "--help"],
        &["reply", "--help"],
        &["dispatch", "--help"],
        &["clean", "--help"],
        &["archive", "--help"],
        &["unarchive", "--help"],
        &["project", "--help"],
        &["trash", "--help"],
        &["import", "--help"],
        &["backup", "--help"],
        &["restore", "--help"],
        &["setup", "--help"],
        &["update", "--help"],
        &["guide", "--help"],
        &["capture", "--help"],
    ] {
        let output = executable(args);
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        let stdout = String::from_utf8(output.stdout).expect("UTF-8 help");
        assert!(
            stdout.lines().all(|line| line == line.trim_end()),
            "{args:?} contains trailing whitespace:\n{stdout}"
        );
    }
}

/// Help is reference text at 80 columns, and an example copies as one command: a wrapped
/// example would paste as two broken lines.
#[test]
fn help_fits_80_columns_and_examples_never_wrap() {
    for command in [
        "",
        "add",
        "list",
        "status",
        "edit",
        "steps",
        "reply",
        "dispatch",
        "clean",
        "archive",
        "unarchive",
        "project",
        "trash",
        "backup",
        "restore",
        "setup",
        "update",
        "guide",
        "capture",
    ] {
        let args: Vec<&str> = ["tsk", "help", command]
            .into_iter()
            .filter(|arg| !arg.is_empty())
            .collect();
        let output = help(&args);
        assert_eq!(output.code, 0, "{args:?}");
        for line in output.stdout.lines() {
            assert!(
                line.chars().count() <= 80,
                "{args:?} help is wider than 80 columns: {line:?}"
            );
        }
        let examples = output
            .stdout
            .lines()
            .skip_while(|line| *line != "Examples:")
            .skip(1)
            .take_while(|line| !line.is_empty());
        for line in examples {
            assert!(
                line.starts_with("  tsk ") || line.starts_with("  cat "),
                "{args:?} example wraps mid-command: {line:?}"
            );
        }
    }
}

#[test]
fn every_command_help_purpose_is_a_capitalized_sentence() {
    for args in [
        &["add", "--help"][..],
        &["list", "--help"],
        &["status", "--help"],
        &["edit", "--help"],
        &["steps", "--help"],
        &["reply", "--help"],
        &["dispatch", "--help"],
        &["clean", "--help"],
        &["archive", "--help"],
        &["unarchive", "--help"],
        &["project", "--help"],
        &["trash", "--help"],
        &["import", "--help"],
        &["backup", "--help"],
        &["restore", "--help"],
        &["setup", "--help"],
        &["update", "--help"],
        &["guide", "--help"],
        &["capture", "--help"],
    ] {
        let output = help(&["tsk", args[0], "--help"]);
        let purpose = output
            .stdout
            .split("\n\n")
            .nth(1)
            .expect("purpose after usage");
        assert!(
            purpose.chars().next().is_some_and(char::is_uppercase) && purpose.ends_with('.'),
            "{args:?} purpose should be a capitalized sentence: {purpose:?}"
        );
    }
}

#[test]
fn help_operand_matches_verb_help_byte_for_byte() {
    let via_help = executable(&["help", "add"]);
    let direct = executable(&["add", "--help"]);
    assert_eq!(via_help.status.code(), Some(0));
    assert!(via_help.stderr.is_empty());
    assert_eq!(via_help.stdout, direct.stdout);
}

#[test]
fn capture_help_prints_without_opening_the_popup() {
    let via_help = executable(&["help", "capture"]);
    let direct = executable(&["capture", "--help"]);
    assert_eq!(via_help.status.code(), Some(0));
    assert!(via_help.stderr.is_empty());
    assert!(String::from_utf8_lossy(&via_help.stdout)
        .starts_with("usage: tsk capture [--state-dir <dir>]"));
    assert_eq!(via_help.stdout, direct.stdout);
    assert_eq!(direct.status.code(), Some(0));
}

#[test]
fn capture_refuses_another_argument_and_says_why() {
    let output = executable(&["capture", "--bogus"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "tsk capture: unknown argument --bogus\nusage: tsk capture [--state-dir <dir>]\n"
    );
}

#[test]
fn unknown_help_operand_is_a_usage_error() {
    let output = executable(&["help", "nope"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).expect("UTF-8 usage error"),
        "tsk help: unknown command nope\nusage: tsk help [<command>]\n"
    );
}

#[test]
fn add_version_is_not_a_global_flag_after_the_verb() {
    let output = help(&["tsk", "add", "--version"]);
    assert_eq!(output.code, 2);
    assert!(output.stdout.is_empty());
    assert!(output.stderr.starts_with("tsk add:"));
}

#[test]
fn version_flags_print_the_package_version() {
    for flag in ["--version", "-V"] {
        let output = Command::new(env!("CARGO_BIN_EXE_tsk"))
            .arg(flag)
            .output()
            .expect("run version flag");
        assert_eq!(output.status.code(), Some(0));
        assert!(output.stderr.is_empty());
        assert_eq!(
            String::from_utf8(output.stdout).expect("UTF-8 version"),
            format!("tsk {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
}

/// The JSON block under a help heading, as one string with the indent removed.
fn help_json(output: &str, heading: &str) -> String {
    output
        .lines()
        .skip_while(|line| !line.starts_with(heading))
        .skip(1)
        .take_while(|line| {
            line.starts_with("  [") || line.starts_with("  {") || line.starts_with("   ")
        })
        .map(|line| &line[2..])
        .collect::<Vec<_>>()
        .join("\n")
}

static HELP_TEMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn help_temp_dir(label: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let seq = HELP_TEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsk-cli-help-{label}-{nanos}-{seq}"));
    std::fs::create_dir_all(&dir).expect("create state directory");
    dir
}

fn run_stdin(args: &[&str], stdin: String) -> tsk_tui::cli::CliOutput {
    run_with(args, Cursor::new(stdin.into_bytes()), false)
}

/// A first user copies the JSON shapes from help: each one must be accepted as shown.
#[test]
fn help_json_shapes_are_accepted_as_written() {
    let dir = help_temp_dir("json-shapes");
    let state = dir.to_string_lossy().into_owned();

    let plan = help_json(&help(&["tsk", "help", "add"]).stdout, "JSON plan");
    let items: serde_json::Value = serde_json::from_str(&plan).expect("plan example is JSON");
    assert_eq!(items.as_array().map(Vec::len), Some(2), "{plan}");
    // Only the canonical project and task the example names are missing on an empty board.
    let added = run_stdin(&["tsk", "add", "--state-dir", &state], plan);
    let result: serde_json::Value = serde_json::from_str(&added.stdout).expect("plan result");
    let codes: Vec<&str> = result["failed"]
        .as_array()
        .expect("failed array")
        .iter()
        .map(|item| item["code"].as_str().expect("code"))
        .collect();
    assert_eq!(
        codes,
        ["unknown-project", "invalid-after"],
        "{}",
        added.stdout
    );

    let questions = help(&["tsk", "help", "ask"]).stdout;
    for heading in ["ask JSON", "answer JSON", "route JSON"] {
        let block = questions
            .lines()
            .skip_while(|line| !line.starts_with(heading))
            .take_while(|line| !line.is_empty());
        for line in block {
            assert!(
                line.chars().count() <= 80,
                "wider than 80 columns: {line:?}"
            );
        }
    }
    let created = run_with(
        [
            "tsk",
            "add",
            "-t",
            "Cache work",
            "--desk",
            "--state-dir",
            &state,
        ],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(created.code, 0, "{}", created.stderr);
    let id = run_with(
        ["tsk", "list", "T1", "--field", "id", "--state-dir", &state],
        Cursor::new(Vec::<u8>::new()),
        true,
    )
    .stdout
    .trim()
    .to_owned();
    let ask = help_json(&questions, "ask JSON").replace("<task UUID>", &id);
    let asked = run_stdin(&["tsk", "ask", "--file", "-", "--state-dir", &state], ask);
    assert_eq!(asked.code, 0, "{}", asked.stderr);

    let revision = |output: &str| {
        let value: serde_json::Value = serde_json::from_str(output).expect("question JSON");
        value["saved"]["record"]["reference"]["revision"]
            .as_str()
            .expect("revision")
            .to_owned()
    };
    let route =
        help_json(&questions, "route JSON").replace("<revision UUID>", &revision(&asked.stdout));
    let routed = run_stdin(
        &["tsk", "route", "Q1", "--file", "-", "--state-dir", &state],
        route,
    );
    assert_eq!(routed.code, 0, "{}", routed.stderr);
    let answer =
        help_json(&questions, "answer JSON").replace("<revision UUID>", &revision(&routed.stdout));
    let answered = run_stdin(
        &["tsk", "answer", "Q1", "--file", "-", "--state-dir", &state],
        answer,
    );
    assert_eq!(answered.code, 0, "{}", answered.stderr);

    let _ = std::fs::remove_dir_all(dir);
}
