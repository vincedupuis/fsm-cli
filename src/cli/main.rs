//! `fsm`, the command-line tool of FSM Editor: `fsm <command> [options]`. One module per command.

mod write;

use std::process::ExitCode;

const USAGE: &str = "Usage: fsm <command> [options]

The command-line tool of FSM Editor, for UML state machines (.fsm files).

Commands:
  write    Generates code from state machines with a Jinja template.

Run 'fsm <command> --help' for the options of a command.

Options:
  -h, --help     Show this help.
  -v, --version  Show the version.";

/// Why the program stops.
#[derive(Debug, PartialEq)]
pub enum Failure {
    /// Bad usage: exit code 2. `command` names the help that explains it (`write`, or "" for `fsm --help`).
    Usage { message: String, command: &'static str },
    /// Exit code 1.
    Error(String),
}

fn version() {
    println!("{}", env!("CARGO_PKG_VERSION"));
}

/// Runs the command line (without the program's name).
fn run(argv: &[String]) -> Result<(), Failure> {
    let usage = |message: String| Failure::Usage { message, command: "" };
    let Some(command) = argv.first() else {
        return Err(usage("No command given.".to_string()));
    };
    let args = &argv[1..];
    match command.as_str() {
        "write" => write::main(args),
        "-h" | "--help" => {
            println!("{USAGE}");
            Ok(())
        }
        "-v" | "--version" => {
            version();
            Ok(())
        }
        a if a.starts_with('-') => Err(usage(format!("Unknown option {a}."))),
        a => Err(usage(format!("Unknown command '{a}'."))),
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match run(&argv) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage { message, command }) => {
            let help = if command.is_empty() { "fsm --help".to_string() } else { format!("fsm {command} --help") };
            eprintln!("fsm: {message}\nRun '{help}' for usage.");
            ExitCode::from(2)
        }
        Err(Failure::Error(message)) => {
            eprintln!("fsm: {message}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn commands() {
        let usage = |m: &str| Err(Failure::Usage { message: m.to_string(), command: "" });
        assert_eq!(run(&args(&[])), usage("No command given."));
        assert_eq!(run(&args(&["generate"])), usage("Unknown command 'generate'."));
        assert_eq!(run(&args(&["x.fsm", "-t", "ts"])), usage("Unknown command 'x.fsm'."));
        assert_eq!(run(&args(&["--template", "ts"])), usage("Unknown option --template."));
    }
}
