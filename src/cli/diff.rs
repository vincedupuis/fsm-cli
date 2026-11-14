//! `fsm diff <old.fsm> <new.fsm> [--json]`: reads two versions of a state machine and prints what changed (the
//! comparison itself is the library's `diff` module).

use std::path::Path;

use fsm_cli::diff::diff;
use fsm_cli::model::FsmModel;
use fsm_cli::paths::{absolute, display};
use fsm_cli::xmi::from_xmi;

use crate::Failure;

const USAGE: &str = "Usage: fsm diff <old.fsm> <new.fsm> [--json]

Compares two versions of a state machine (.fsm files) and prints what changed:
the elements added (+), removed (-) and changed (~), matched by their xmi:id,
then how many changed in the layout only (·: moved, resized, rerouted,
recolored or regrouped). An empty or missing file counts as a file without a
machine: everything in the other one is added or removed.

Options:
      --json     Print the differences as one JSON document.
  -h, --help     Show this help.

Exit codes: 0 compared (with or without differences), 1 a file can't be read
or isn't a state machine, 2 bad usage.";

fn usage(message: String) -> Failure {
    Failure::Usage { message, command: "diff" }
}

#[derive(Debug, PartialEq)]
struct Args {
    old: String,
    new: String,
    json: bool,
}

#[derive(Debug, PartialEq)]
enum Command {
    Help,
    Version,
    Run(Args),
}

fn parse_args(argv: &[String]) -> Result<Command, Failure> {
    let mut files = Vec::new();
    let mut json = false;
    for a in argv {
        match a.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-v" | "--version" => return Ok(Command::Version),
            "--json" => json = true,
            _ if a.starts_with('-') && a != "-" => return Err(usage(format!("Unknown option {a}."))),
            _ => files.push(a.clone()),
        }
    }
    match files.len() {
        0 if !json => Ok(Command::Help),
        2 => Ok(Command::Run(Args { old: files.remove(0), new: files.remove(0), json })),
        _ => Err(usage("Give two .fsm files: the old version and the new one.".to_string())),
    }
}

/// A side of the comparison: the file's text, or a file without a machine (missing, or empty).
enum Side {
    Text(String),
    Missing,
    Empty,
}

fn read(file: &str) -> Result<Side, Failure> {
    let path = absolute(file);
    if !path.exists() && std::fs::symlink_metadata(&path).is_err() {
        return Ok(Side::Missing);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|_| Failure::Error(format!("{} does not exist or cannot be read.", display(&path))))?;
    if text.trim_matches(['\u{feff}', ' ', '\t', '\r', '\n']).is_empty() {
        return Ok(Side::Empty);
    }
    Ok(Side::Text(text))
}

fn model(file: &str, side: &Side) -> Result<Option<FsmModel>, Failure> {
    match side {
        Side::Text(text) => from_xmi(text).map(Some).map_err(|e| {
            Failure::Error(format!("{} is not a valid state machine file: {e}", display(&absolute(file))))
        }),
        _ => Ok(None),
    }
}

pub fn main(argv: &[String]) -> Result<(), Failure> {
    match parse_args(argv)? {
        Command::Help => {
            println!("{USAGE}");
            Ok(())
        }
        Command::Version => {
            crate::version();
            Ok(())
        }
        Command::Run(args) => run(args),
    }
}

fn run(args: Args) -> Result<(), Failure> {
    let (old, new) = (read(&args.old)?, read(&args.new)?);
    // One missing file is a machine added or deleted; both missing is a mistake.
    if let (Side::Missing | Side::Empty, Side::Missing | Side::Empty) = (&old, &new) {
        for (file, side) in [(&args.old, &old), (&args.new, &new)] {
            if let Side::Missing = side {
                return Err(Failure::Error(format!("{} does not exist or cannot be read.", display(&absolute(file)))));
            }
        }
    }
    let (old_model, new_model) = (model(&args.old, &old)?, model(&args.new, &new)?);
    let result = diff(old_model.as_ref(), new_model.as_ref());
    if args.json {
        print!("{}", result.to_json());
    } else {
        let shown = if matches!(new, Side::Text(_)) { &args.new } else { &args.old };
        print!("{}", result.to_text(&display(Path::new(&absolute(shown)))));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn arguments() {
        assert_eq!(parse_args(&args(&[])), Ok(Command::Help));
        assert_eq!(parse_args(&args(&["a.fsm", "--help"])), Ok(Command::Help));
        assert_eq!(parse_args(&args(&["-v"])), Ok(Command::Version));
        assert_eq!(
            parse_args(&args(&["a.fsm", "--json", "b.fsm"])),
            Ok(Command::Run(Args { old: "a.fsm".into(), new: "b.fsm".into(), json: true }))
        );
        let two = Err(usage("Give two .fsm files: the old version and the new one.".to_string()));
        assert_eq!(parse_args(&args(&["a.fsm"])), two);
        assert_eq!(parse_args(&args(&["--json"])), two);
        assert_eq!(parse_args(&args(&["a.fsm", "b.fsm", "c.fsm"])), two);
        assert_eq!(parse_args(&args(&["a.fsm", "b.fsm", "-x"])), Err(usage("Unknown option -x.".to_string())));
    }
}
