//! The `fsm` program as a process: commands, exit codes, `fsm write` giving the files the library gives, the refusal
//! of old files, problems reported with their file and line, and the model hash.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MACHINE: &str = r#"{
  "format": 1,
  "name": "Lamp",
  "type": "behavioral",
  "initial": { "target": "Off" },
  "elements": [
    {
      "name": "Off",
      "type": "state",
      "transitions": [
        { "type": "external", "trigger": "press", "target": "On" }
      ]
    },
    {
      "name": "On",
      "type": "state",
      "transitions": [
        { "type": "external", "trigger": "press", "target": "Off" }
      ]
    }
  ]
}
"#;

fn fsm(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fsm")).args(args).current_dir(dir).output().unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A new, empty folder for one test.
fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fsm-cli-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn commands() {
    let dir = folder("commands");
    let o = fsm(&dir, &[]);
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(stderr(&o), "fsm: No command given.\nRun 'fsm --help' for usage.\n");

    let o = fsm(&dir, &["Lamp.fsm", "-t", "ts"]);
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(stderr(&o), "fsm: Unknown command 'Lamp.fsm'.\nRun 'fsm --help' for usage.\n");

    for args in [&["-h"][..], &["write", "--help"], &["write"]] {
        let o = fsm(&dir, args);
        assert_eq!(o.status.code(), Some(0));
        assert!(String::from_utf8_lossy(&o.stdout).starts_with("Usage: fsm "));
    }
    let o = fsm(&dir, &["-v"]);
    assert_eq!(String::from_utf8_lossy(&o.stdout), format!("{}\n", env!("CARGO_PKG_VERSION")));

    let o = fsm(&dir, &["write", "Lamp.fsm"]);
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(stderr(&o), "fsm: Missing --template.\nRun 'fsm write --help' for usage.\n");
}

#[test]
fn handlebars_templates_are_refused() {
    let dir = folder("hbs");
    std::fs::write(dir.join("Lamp.fsm"), MACHINE).unwrap();
    std::fs::write(dir.join("ts.hbs"), "{{#file \"a\"}}{{/file}}").unwrap();
    let o = fsm(&dir, &["write", "Lamp.fsm", "-t", "ts.hbs"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).starts_with("fsm: ts.hbs is a Handlebars template: they are no longer supported."));
    assert!(!dir.join("Lamp.ts").exists());
}

#[test]
fn write_and_check() {
    let dir = folder("write");
    std::fs::write(dir.join("Lamp.fsm"), MACHINE).unwrap();
    let o = fsm(&dir, &["write", "*.fsm", "-t", "ts", "-o", "out"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(String::from_utf8_lossy(&o.stdout), "wrote out/LampActions.ts\nwrote out/Lamp.ts\n");

    // The library gives the same files from the same input.
    let template = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/ts.jinja")).unwrap();
    let request = serde_json::json!({
        "cwd": dir.to_string_lossy(),
        "files": { "Lamp.fsm": MACHINE },
        "generate": ["Lamp.fsm"],
        "template": template,
        "templateName": "ts.jinja",
    });
    let response: serde_json::Value =
        serde_json::from_slice(&fsm_cli::run_json(&serde_json::to_vec(&request).unwrap())).unwrap();
    assert_eq!(response["exitCode"], 0);
    let files = response["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    for f in files {
        let written = std::fs::read_to_string(dir.join("out").join(f["path"].as_str().unwrap())).unwrap();
        assert_eq!(written, f["content"].as_str().unwrap());
    }

    let o = fsm(&dir, &["write", "Lamp.fsm", "-t", "ts", "-o", "out", "--check"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    // Indentation, field order and escapes don't change the model hash: the files are up to date.
    let compact = MACHINE
        .replace("\n", "")
        .replace("  ", "")
        .replace(r#""format": 1, "name": "Lamp""#, r#""name": "Lamp", "format": 1"#);
    std::fs::write(dir.join("Lamp.fsm"), compact.replace("Off", "\\u004fff")).unwrap();
    let o = fsm(&dir, &["write", "Lamp.fsm", "-t", "ts", "-o", "out", "--check"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    std::fs::write(dir.join("Lamp.fsm"), MACHINE.replace("\"On\"", "\"Lit\"")).unwrap();
    let o = fsm(&dir, &["write", "Lamp.fsm", "-t", "ts", "-o", "out", "--check"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).ends_with("fsm: 2 generated files are missing or out of date.\n"), "{}", stderr(&o));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn old_files_are_refused() {
    let dir = folder("old");
    let old = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xmi:XMI xmi:version=\"20131001\"/>\n";
    std::fs::write(dir.join("Old.fsm"), old).unwrap();
    let o = fsm(&dir, &["write", "Old.fsm", "-t", "ts", "-o", "out"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(stderr(&o), "fsm: Old.fsm is in the old XMI format: fsm reads the JSON format only.\n");
    // A machine using a submachine still in the old format: that file is named.
    std::fs::write(
        dir.join("Main.fsm"),
        MACHINE.replace(r#""type": "state","#, r#""type": "state", "submachine": "Old.fsm","#),
    )
    .unwrap();
    let o = fsm(&dir, &["write", "Main.fsm", "-t", "ts", "-o", "out"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(stderr(&o), "fsm: Old.fsm is in the old XMI format: fsm reads the JSON format only.\n");
    assert!(!dir.join("out").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn problems_with_file_and_line() {
    let dir = folder("problems");
    // A wrong shape: every problem, nothing generated.
    std::fs::write(dir.join("Bad.fsm"), MACHINE.replace(r#""trigger": "press", "target": "On""#, r#""gaurd": "x()""#))
        .unwrap();
    let o = fsm(&dir, &["write", "Bad.fsm", "-t", "ts", "-o", "out"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(
        stderr(&o),
        "Bad.fsm:11: error: Unknown field 'gaurd' in a transition.\n\
         Bad.fsm:11: error: Missing field 'target' in a transition.\n\
         fsm: 2 errors in the state machine; nothing was generated.\n"
    );
    // Not JSON.
    std::fs::write(dir.join("Bad.fsm"), "{\n  \"format\": 1,\n}\n").unwrap();
    let o = fsm(&dir, &["write", "Bad.fsm", "-t", "ts", "-o", "out"]);
    assert_eq!(
        stderr(&o),
        "Bad.fsm:3: error: Unexpected character '}'.\nfsm: 1 error in the state machine; nothing was generated.\n"
    );
    // The right shape, a reference that names nothing: a validation error at its line.
    std::fs::write(
        dir.join("Bad.fsm"),
        MACHINE.replace(
            r#""target": "Off" }
      ]"#,
            r#""target": "Of" }
      ]"#,
        ),
    )
    .unwrap();
    let o = fsm(&dir, &["write", "Bad.fsm", "-t", "ts", "-o", "out"]);
    assert_eq!(
        stderr(&o),
        "Bad.fsm:18: error: Transition target 'Of' does not exist.\nfsm: 1 error in the state machine; nothing was generated.\n"
    );
    assert!(!dir.join("out").exists());
    let _ = std::fs::remove_dir_all(&dir);
}
