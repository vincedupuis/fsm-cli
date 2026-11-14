//! `fsm merge <base.fsm> <ours.fsm> <theirs.fsm> -o <merged.fsm> [--json]`: a three-way merge of a state machine
//! (the merge itself is the library's `merge` module). Every input is read before the output is written, in one go,
//! so the output can be one of them, as a Git merge driver has it (`fsm merge %O %A %B -o %A`).

use std::path::Path;

use fsm_cli::merge::merge;
use fsm_cli::model::FsmModel;
use fsm_cli::paths::{absolute, display};
use fsm_cli::xmi::from_xmi;

use crate::Failure;

const USAGE: &str = "Usage: fsm merge <base.fsm> <ours.fsm> <theirs.fsm> -o <merged.fsm> [--json]

Merges two versions of a state machine (.fsm files), ours and theirs, made
from a common one, the base: elements are matched by their xmi:id and merged
property by property. What only one side changed is taken, elements both
sides added are all kept, and the layout takes ours when both sides moved
something. Where both sides changed a property differently, or one side
deleted what the other changed, ours is kept and the conflict is reported.
The merged file is always a valid .fsm file, written as the editors write it,
without conflict markers; it may be ours or theirs. An empty or missing base
counts as a file without a machine. The report goes to stderr: the conflicts
(✕), the layout kept from ours (·), and the validation issues the merge
caused (!), whose errors make it fail like a conflict.

As a Git merge driver: fsm merge %O %A %B -o %A

Options:
  -o, --out <file>  Where to write the merged machine.
      --json        Print the report as one JSON document on stdout instead.
  -h, --help        Show this help.

Exit codes: 0 merged, 1 conflicts remain (ours kept) or the merge caused
validation errors (the file is written either way), or a file can't be read,
isn't a state machine or the files can't be merged (nothing written), 2 bad
usage.";

fn usage(message: String) -> Failure {
    Failure::Usage { message, command: "merge" }
}

#[derive(Debug, PartialEq)]
struct Args {
    base: String,
    ours: String,
    theirs: String,
    out: String,
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
    let mut out = None;
    let mut json = false;
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-v" | "--version" => return Ok(Command::Version),
            "--json" => json = true,
            "-o" | "--out" => match it.next() {
                Some(f) => out = Some(f.clone()),
                None => return Err(usage(format!("{a} needs a file."))),
            },
            _ if a.starts_with("--out=") => out = Some(a["--out=".len()..].to_string()),
            _ if a.starts_with('-') && a != "-" => return Err(usage(format!("Unknown option {a}."))),
            _ => files.push(a.clone()),
        }
    }
    if files.is_empty() && out.is_none() && !json {
        return Ok(Command::Help);
    }
    if files.len() != 3 {
        return Err(usage("Give three .fsm files: the base, ours and theirs.".to_string()));
    }
    let Some(out) = out else {
        return Err(usage("Give the file to write with -o.".to_string()));
    };
    Ok(Command::Run(Args { base: files.remove(0), ours: files.remove(0), theirs: files.remove(0), out, json }))
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

/// A file's text; None for a base that is missing or empty (white space or a BOM only).
fn read(file: &str, optional: bool) -> Result<Option<String>, Failure> {
    let path = absolute(file);
    let unreadable = || Failure::Error(format!("{} does not exist or cannot be read.", display(&path)));
    if optional && !path.exists() && std::fs::symlink_metadata(&path).is_err() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|_| unreadable())?;
    if text.trim_matches(['\u{feff}', ' ', '\t', '\r', '\n']).is_empty() {
        if optional {
            return Ok(None);
        }
        return Err(Failure::Error(format!("{} is empty: it holds no state machine to merge.", display(&path))));
    }
    Ok(Some(text))
}

fn model(file: &str, text: &str) -> Result<FsmModel, Failure> {
    from_xmi(text)
        .map_err(|e| Failure::Error(format!("{} is not a valid state machine file: {e}", display(&absolute(file)))))
}

fn run(args: Args) -> Result<(), Failure> {
    // Everything is read before anything is written: the output may be one of the inputs.
    let base = read(&args.base, true)?;
    let ours = read(&args.ours, false)?.unwrap_or_default();
    let theirs = read(&args.theirs, false)?.unwrap_or_default();
    let base = base.map(|t| model(&args.base, &t)).transpose()?;
    let (ours, theirs) = (model(&args.ours, &ours)?, model(&args.theirs, &theirs)?);
    let result = merge(base.as_ref(), &ours, &theirs).map_err(Failure::Error)?;
    let out = absolute(&args.out);
    std::fs::write(&out, &result.text).map_err(|e| Failure::Error(format!("Cannot write {}: {e}", display(&out))))?;
    if args.json {
        print!("{}", result.to_json());
    } else {
        eprint!("{}", result.to_text(&display(Path::new(&out))));
    }
    // Conflicts, and validation errors the merge caused (a machine that no longer generates code), need a review.
    if result.succeeded() { Ok(()) } else { Err(Failure::Conflicts) }
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
        assert_eq!(parse_args(&args(&["a", "b", "c", "-h"])), Ok(Command::Help));
        assert_eq!(parse_args(&args(&["-v"])), Ok(Command::Version));
        let run = |out: &str, json| {
            Ok(Command::Run(Args {
                base: "b.fsm".into(),
                ours: "o.fsm".into(),
                theirs: "t.fsm".into(),
                out: out.into(),
                json,
            }))
        };
        assert_eq!(parse_args(&args(&["b.fsm", "o.fsm", "t.fsm", "-o", "o.fsm"])), run("o.fsm", false));
        assert_eq!(parse_args(&args(&["--json", "b.fsm", "--out", "m.fsm", "o.fsm", "t.fsm"])), run("m.fsm", true));
        assert_eq!(parse_args(&args(&["b.fsm", "o.fsm", "t.fsm", "--out=m.fsm"])), run("m.fsm", false));
        let three = Err(usage("Give three .fsm files: the base, ours and theirs.".to_string()));
        assert_eq!(parse_args(&args(&["b.fsm", "o.fsm", "-o", "m.fsm"])), three);
        assert_eq!(parse_args(&args(&["--json"])), three);
        assert_eq!(
            parse_args(&args(&["b.fsm", "o.fsm", "t.fsm"])),
            Err(usage("Give the file to write with -o.".to_string()))
        );
        assert_eq!(parse_args(&args(&["b.fsm", "o.fsm", "t.fsm", "-o"])), Err(usage("-o needs a file.".to_string())));
        assert_eq!(parse_args(&args(&["b.fsm", "o.fsm", "t.fsm", "-x"])), Err(usage("Unknown option -x.".to_string())));
    }
}
