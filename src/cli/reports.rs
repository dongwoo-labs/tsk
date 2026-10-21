//! Register and inspect independent Markdown completion reports.
use std::io::Read;
use std::path::PathBuf;

use super::parser::{parse_task_address, TaskAddress};
use super::{presenter, CliOutput};
use crate::store::{default_state_dir, ReportError, ReportStore, TrashTarget};

struct Input {
    list: bool,
    task: Option<TrashTarget>,
    file: Option<PathBuf>,
    state_dir: Option<PathBuf>,
    json: bool,
    unread: bool,
}

pub fn run(args: Vec<String>, stdin: &mut dyn Read) -> CliOutput {
    if args.len() == 3 && args[2] == "--help" {
        return presenter::reports_help();
    }
    match parse(&args).and_then(|input| execute(input, stdin)) {
        Ok(output) => output,
        Err((message, code)) => presenter::reports_rejected(&message, code),
    }
}

fn parse(args: &[String]) -> Result<Input, (String, u8)> {
    let mut input = Input {
        list: args.get(1).is_some_and(|verb| verb == "reports"),
        task: None,
        file: None,
        state_dir: None,
        json: false,
        unread: false,
    };
    let mut index = 2;
    while let Some(argument) = args.get(index) {
        match argument.as_str() {
            "--json" => input.json = true,
            "--unread" => input.unread = true,
            "--file" | "--state-dir" => {
                index += 1;
                let value = args
                    .get(index)
                    .filter(|value| {
                        !value.is_empty()
                            && (!value.starts_with('-') || (argument == "--file" && *value == "-"))
                    })
                    .ok_or_else(|| (format!("missing value for {argument}"), 2))?;
                set_path(&mut input, argument, value)?;
            }
            value if value.starts_with("--file=") => set_path(&mut input, "--file", &value[7..])?,
            value if value.starts_with("--state-dir=") => {
                set_path(&mut input, "--state-dir", &value[12..])?
            }
            value if value.starts_with('-') => {
                return Err((format!("unknown argument {value}"), 2))
            }
            value => {
                if input.task.is_some() {
                    return Err(("too many operands".into(), 2));
                }
                input.task = Some(
                    match parse_task_address(value).map_err(|message| (message, 2))? {
                        TaskAddress::Number(number) => TrashTarget::Number(number),
                        TaskAddress::Id(id) => TrashTarget::Id(id),
                    },
                );
            }
        }
        index += 1;
    }
    if input.list {
        if input.task.is_some() || input.file.is_some() {
            return Err(("reports accepts neither a task nor --file".into(), 2));
        }
    } else if input.task.is_none() || input.unread {
        return Err((
            "report requires one task; --unread is a reports filter".into(),
            2,
        ));
    }
    Ok(input)
}

fn set_path(input: &mut Input, flag: &str, value: &str) -> Result<(), (String, u8)> {
    if value.is_empty() {
        return Err((format!("missing value for {flag}"), 2));
    }
    let slot = if flag == "--file" {
        &mut input.file
    } else {
        &mut input.state_dir
    };
    if slot.replace(PathBuf::from(value)).is_some() {
        return Err((format!("duplicate {flag}"), 2));
    }
    Ok(())
}

fn error(error: ReportError) -> (String, u8) {
    let code = match error {
        ReportError::InvalidInput => 2,
        ReportError::Io => 3,
        _ => 1,
    };
    (error.to_string(), code)
}

fn execute(input: Input, stdin: &mut dyn Read) -> Result<CliOutput, (String, u8)> {
    let store = ReportStore::new(input.state_dir.unwrap_or_else(default_state_dir));
    if input.list {
        let mut records = store.list().map_err(error)?;
        if input.unread {
            records.retain(|record| record.unread());
        }
        return Ok(presenter::reports(&records, input.json));
    }
    let task = input.task.expect("parser requires a task");
    let registration = input.file.is_some();
    let record = if let Some(path) = input.file {
        let mut bytes = Vec::new();
        if path.as_os_str() == "-" {
            stdin
                .read_to_end(&mut bytes)
                .map_err(|_| ("cannot read report input".into(), 3))?;
        } else {
            bytes = std::fs::read(path).map_err(|_| ("cannot read report file".into(), 3))?;
        }
        let markdown =
            String::from_utf8(bytes).map_err(|_| (ReportError::InvalidInput.to_string(), 2))?;
        // Match add's JSON input: remove one initial UTF-8 BOM, preserve all other text.
        let markdown = markdown
            .strip_prefix('\u{feff}')
            .unwrap_or(&markdown)
            .to_owned();
        store.register(task, markdown).map_err(error)?
    } else {
        store.read_address(task).map_err(error)?
    };
    Ok(presenter::report(&record, input.json, registration))
}
