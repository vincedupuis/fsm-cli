//! The `fsm` program as a process: commands, exit codes, `fsm write` giving the files the library gives, and
//! `fsm diff`'s output.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MACHINE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xmi:XMI xmi:version="20131001" xmlns:xmi="http://www.omg.org/spec/XMI/20131001" xmlns:uml="http://www.omg.org/spec/UML/20161101">
  <uml:Model xmi:id="m" name="Lamp">
    <packagedElement xmi:type="uml:StateMachine" xmi:id="sm" name="Lamp">
      <region xmi:type="uml:Region" xmi:id="r">
        <subvertex xmi:type="uml:Pseudostate" xmi:id="i" kind="initial"/>
        <subvertex xmi:type="uml:State" xmi:id="Off" name="Off"/>
        <subvertex xmi:type="uml:State" xmi:id="On" name="On"/>
        <transition xmi:type="uml:Transition" xmi:id="t0" source="i" target="Off"/>
        <transition xmi:type="uml:Transition" xmi:id="t1" source="Off" target="On"><trigger xmi:id="g1" name="press"/></transition>
        <transition xmi:type="uml:Transition" xmi:id="t2" source="On" target="Off"><trigger xmi:id="g2" name="press"/></transition>
      </region>
    </packagedElement>
  </uml:Model>
</xmi:XMI>
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
    std::fs::write(dir.join("Lamp.fsm"), MACHINE.replace("name=\"On\"", "name=\"Lit\"")).unwrap();
    let o = fsm(&dir, &["write", "Lamp.fsm", "-t", "ts", "-o", "out", "--check"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).ends_with("fsm: 2 generated files are missing or out of date.\n"), "{}", stderr(&o));
    let _ = std::fs::remove_dir_all(&dir);
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn diff_command_line() {
    let dir = folder("diff-args");
    std::fs::write(dir.join("Lamp.fsm"), MACHINE).unwrap();
    for args in [&["diff"][..], &["diff", "--help"], &["diff", "a", "b", "-h"]] {
        let o = fsm(&dir, args);
        assert_eq!(o.status.code(), Some(0));
        assert!(stdout(&o).starts_with("Usage: fsm diff <old.fsm> <new.fsm> [--json]"));
    }
    assert!(stdout(&fsm(&dir, &["--help"])).contains("\n  diff     Compares two versions of a state machine.\n"));
    for args in [&["diff", "Lamp.fsm"][..], &["diff", "Lamp.fsm", "Lamp.fsm", "Lamp.fsm"], &["diff", "--json"]] {
        let o = fsm(&dir, args);
        assert_eq!(o.status.code(), Some(2));
        assert_eq!(
            stderr(&o),
            "fsm: Give two .fsm files: the old version and the new one.\nRun 'fsm diff --help' for usage.\n"
        );
    }
    let o = fsm(&dir, &["diff", "Lamp.fsm", "Lamp.fsm", "--color"]);
    assert_eq!(
        (o.status.code(), stderr(&o)),
        (Some(2), "fsm: Unknown option --color.\nRun 'fsm diff --help' for usage.\n".to_string())
    );

    // Files that can't be compared: exit 1, nothing on stdout.
    std::fs::write(dir.join("notes.txt"), "not xml").unwrap();
    std::fs::create_dir_all(dir.join("folder.fsm")).unwrap();
    for (args, message) in [
        (&["diff", "Lamp.fsm", "notes.txt"][..], "fsm: notes.txt is not a valid state machine file: "),
        (&["diff", "folder.fsm", "Lamp.fsm"], "fsm: folder.fsm does not exist or cannot be read.\n"),
        (&["diff", "gone.fsm", "lost.fsm"], "fsm: gone.fsm does not exist or cannot be read.\n"),
    ] {
        let o = fsm(&dir, args);
        assert_eq!(o.status.code(), Some(1), "{args:?}");
        assert!(stderr(&o).starts_with(message), "{}", stderr(&o));
        assert_eq!(stdout(&o), "");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn diff_output() {
    let dir = folder("diff-out");
    std::fs::write(dir.join("Lamp.fsm"), MACHINE).unwrap();
    let lit = MACHINE
        .replace("name=\"On\"", "name=\"Lit\"")
        .replace("<trigger xmi:id=\"g1\" name=\"press\"/>", "<trigger xmi:id=\"g1\" name=\"press\"/><guard xmi:id=\"g\"><specification><body>ready()</body></specification></guard>")
        .replace("<transition xmi:type=\"uml:Transition\" xmi:id=\"t2\"", "<subvertex xmi:type=\"uml:FinalState\" xmi:id=\"F\"/><transition xmi:type=\"uml:Transition\" xmi:id=\"t2\"");
    std::fs::write(dir.join("Lit.fsm"), &lit).unwrap();
    std::fs::write(dir.join("Empty.fsm"), "").unwrap();

    let o = fsm(&dir, &["diff", "Lamp.fsm", "Lit.fsm"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(
        stdout(&o),
        "Lit.fsm\n  ~ state On renamed to Lit\n  ~ transition Off → On on press: guard added [ready()]\n  + final state\n"
    );
    let o = fsm(&dir, &["diff", "Lamp.fsm", "Lamp.fsm"]);
    assert_eq!((o.status.code(), stdout(&o)), (Some(0), "Lamp.fsm\n  no differences\n".to_string()));

    // A new file: everything added. A deleted one: everything removed.
    let o = fsm(&dir, &["diff", "Empty.fsm", "Lamp.fsm"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(
        stdout(&o).starts_with("Lamp.fsm\n  + machine Lamp\n  + initial to Off\n  + state Off\n"),
        "{}",
        stdout(&o)
    );
    let o = fsm(&dir, &["diff", "Lamp.fsm", "Deleted.fsm"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(stdout(&o).starts_with("Lamp.fsm\n  - machine Lamp\n"), "{}", stdout(&o));

    // JSON: one document, keys in a fixed order.
    let o = fsm(&dir, &["diff", "--json", "Lamp.fsm", "Lit.fsm"]);
    assert_eq!(o.status.code(), Some(0));
    let text = stdout(&o);
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(doc["counts"], serde_json::json!({ "added": 1, "removed": 0, "changed": 2, "layout": 0 }));
    let elements = doc["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 3);
    assert_eq!(
        elements[1],
        serde_json::json!({ "id": "t1", "element": "transition", "status": "changed", "label": "Transition Off → On on press",
            "moved": false, "changes": [{ "property": "guard", "before": "", "after": "ready()" }] })
    );
    // The keys, in the order they first appear.
    let mut keys: Vec<&str> = Vec::new();
    for part in text.split("\":").map(|p| p.rsplit('"').next().unwrap()).take(text.matches("\":").count()) {
        if !keys.contains(&part) {
            keys.push(part);
        }
    }
    assert_eq!(
        keys,
        [
            "elements", "id", "element", "status", "label", "moved", "changes", "property", "before", "after",
            "counts", "added", "removed", "changed", "layout"
        ]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// MACHINE with each `(from, to)` replaced.
fn edited(edits: &[(&str, &str)]) -> String {
    let mut text = MACHINE.to_string();
    for (a, b) in edits {
        assert!(text.contains(a), "{a}");
        text = text.replacen(a, b, 1);
    }
    text
}

const GUARD_A: (&str, &str) = (
    "<trigger xmi:id=\"g1\" name=\"press\"/>",
    "<trigger xmi:id=\"g1\" name=\"press\"/><guard xmi:id=\"g\"><specification><body>a()</body></specification></guard>",
);
const GUARD_B: (&str, &str) = (
    "<trigger xmi:id=\"g1\" name=\"press\"/>",
    "<trigger xmi:id=\"g1\" name=\"press\"/><guard xmi:id=\"g\"><specification><body>b()</body></specification></guard>",
);
const RENAME: (&str, &str) = ("name=\"On\"", "name=\"Lit\"");

#[test]
fn merge_command_line() {
    let dir = folder("merge-args");
    std::fs::write(dir.join("Lamp.fsm"), MACHINE).unwrap();
    for args in [&["merge"][..], &["merge", "--help"]] {
        let o = fsm(&dir, args);
        assert_eq!(o.status.code(), Some(0));
        assert!(stdout(&o).starts_with("Usage: fsm merge <base.fsm> <ours.fsm> <theirs.fsm> -o <merged.fsm>"));
    }
    for (args, message) in [
        (&["merge", "Lamp.fsm", "Lamp.fsm", "-o", "M.fsm"][..], "Give three .fsm files: the base, ours and theirs."),
        (&["merge", "Lamp.fsm", "Lamp.fsm", "Lamp.fsm"], "Give the file to write with -o."),
        (&["merge", "Lamp.fsm", "Lamp.fsm", "Lamp.fsm", "-o"], "-o needs a file."),
        (&["merge", "Lamp.fsm", "Lamp.fsm", "Lamp.fsm", "-o", "M.fsm", "--theirs"], "Unknown option --theirs."),
    ] {
        let o = fsm(&dir, args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        assert_eq!(stderr(&o), format!("fsm: {message}\nRun 'fsm merge --help' for usage.\n"));
    }
    assert!(!dir.join("M.fsm").exists());

    // Files that can't be merged: exit 1, and the output (here ours, which the writer would format otherwise) is left
    // as it was.
    let other = MACHINE.replace("xmi:id=\"sm\"", "xmi:id=\"sm2\"");
    std::fs::write(dir.join("Other.fsm"), &other).unwrap();
    std::fs::write(dir.join("notes.txt"), "not xml").unwrap();
    std::fs::write(dir.join("Empty.fsm"), "\u{feff}\n").unwrap();
    for (args, message) in [
        (
            &["merge", "Lamp.fsm", "Ours.fsm", "Other.fsm", "-o", "Ours.fsm"][..],
            "fsm: ours and theirs are two different state machines (ids 'sm' and 'sm2'): they can't be merged.\n",
        ),
        (
            &["merge", "Lamp.fsm", "Ours.fsm", "notes.txt", "-o", "Ours.fsm"],
            "fsm: notes.txt is not a valid state machine file: ",
        ),
        (
            &["merge", "Lamp.fsm", "Ours.fsm", "Gone.fsm", "-o", "Ours.fsm"],
            "fsm: Gone.fsm does not exist or cannot be read.\n",
        ),
        (
            &["merge", "Lamp.fsm", "Ours.fsm", "Empty.fsm", "-o", "Ours.fsm"],
            "fsm: Empty.fsm is empty: it holds no state machine to merge.\n",
        ),
    ] {
        std::fs::write(dir.join("Ours.fsm"), MACHINE).unwrap();
        let o = fsm(&dir, args);
        assert_eq!(o.status.code(), Some(1), "{args:?}");
        assert!(stderr(&o).starts_with(message), "{}", stderr(&o));
        assert_eq!(std::fs::read_to_string(dir.join("Ours.fsm")).unwrap(), MACHINE);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn merge_over_ours_as_git_calls_it() {
    let dir = folder("merge-git");
    let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
    let saved = |text: &str| fsm_cli::writer::to_xmi(&fsm_cli::xmi::from_xmi(text).unwrap()).unwrap();

    // Merged: exit 0, the file written over ours, the report on stderr.
    write("Base.fsm", MACHINE);
    write("Ours.fsm", &edited(&[GUARD_A]));
    write("Theirs.fsm", &edited(&[RENAME]));
    let o = fsm(&dir, &["merge", "Base.fsm", "Ours.fsm", "Theirs.fsm", "-o", "Ours.fsm"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!((stdout(&o), stderr(&o)), (String::new(), "Ours.fsm: 2 changes merged, 0 conflicts\n".to_string()));
    assert_eq!(read("Ours.fsm"), saved(&edited(&[GUARD_A, RENAME])));

    // --name: the header shows the machine's path (Git's %P) instead of the output file.
    write("Ours.fsm", &edited(&[GUARD_A]));
    let o = fsm(&dir, &["merge", "Base.fsm", "Ours.fsm", "Theirs.fsm", "-o", "Ours.fsm", "--name", "models/Lamp.fsm"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(stderr(&o), "models/Lamp.fsm: 2 changes merged, 0 conflicts\n");
    assert_eq!(read("Ours.fsm"), saved(&edited(&[GUARD_A, RENAME])));

    // Conflicts: exit 1, ours kept, the file still written (valid, without conflict markers).
    write("Ours.fsm", &edited(&[GUARD_A]));
    write("Theirs.fsm", &edited(&[GUARD_B, RENAME]));
    let o = fsm(&dir, &["merge", "Base.fsm", "Ours.fsm", "Theirs.fsm", "-o", "Ours.fsm"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(
        stderr(&o),
        "Ours.fsm: 1 change merged, 1 conflict\n  ✕ transition Off → On on press: guard\n      ours    [a()]\n      theirs  [b()]\n"
    );
    assert_eq!(read("Ours.fsm"), saved(&edited(&[GUARD_A, RENAME])));

    // No common ancestor: Git passes an empty base.
    write("Empty.fsm", "");
    write("Ours.fsm", MACHINE);
    let o = fsm(&dir, &["merge", "Empty.fsm", "Ours.fsm", "Base.fsm", "-o", "M.fsm"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(read("M.fsm"), saved(MACHINE));
    let o = fsm(&dir, &["merge", "Missing.fsm", "Ours.fsm", "Base.fsm", "-o", "M.fsm"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));

    // JSON: one document on stdout, nothing on stderr.
    write("Ours.fsm", &edited(&[GUARD_A]));
    let o = fsm(&dir, &["merge", "--json", "Base.fsm", "Ours.fsm", "Theirs.fsm", "-o", "M.fsm"]);
    assert_eq!((o.status.code(), stderr(&o)), (Some(1), String::new()));
    let text = stdout(&o);
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        doc["counts"],
        serde_json::json!({ "merged": 1, "conflicts": 1, "layout": 0, "issues": 0, "errors": 0 })
    );
    assert_eq!(
        doc["conflicts"][0],
        serde_json::json!({ "id": "t1", "element": "transition", "label": "Transition Off → On on press", "conflict": "property",
            "property": "guard", "base": "", "ours": "a()", "theirs": "b()", "deletedOn": null, "region": null, "changes": [],
            "kept": true })
    );
    assert_eq!(
        doc["merged"][0],
        serde_json::json!({ "id": "On", "element": "vertex", "status": "changed", "label": "State On", "from": "theirs" })
    );
    let mut keys: Vec<&str> = Vec::new();
    for part in text.split("\":").map(|p| p.rsplit('"').next().unwrap()).take(text.matches("\":").count()) {
        if !keys.contains(&part) {
            keys.push(part);
        }
    }
    assert_eq!(
        keys,
        [
            "merged",
            "id",
            "element",
            "status",
            "label",
            "from",
            "conflicts",
            "conflict",
            "property",
            "base",
            "ours",
            "theirs",
            "deletedOn",
            "region",
            "changes",
            "kept",
            "layout",
            "issues",
            "counts",
            "errors"
        ]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn merge_fails_when_it_makes_the_machine_invalid() {
    let dir = folder("merge-invalid");
    let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
    // The initial pseudostate has no transition in the base; ours gives it one to Off, theirs one to On. Each side is
    // valid on that point, the merge has both: an error that neither side has.
    let t0 = "<transition xmi:type=\"uml:Transition\" xmi:id=\"t0\" source=\"i\" target=\"Off\"/>";
    let t5 = "<transition xmi:type=\"uml:Transition\" xmi:id=\"t5\" source=\"i\" target=\"On\"/>";
    let base = edited(&[(t0, "")]);
    write("Base.fsm", &base);
    write("Ours.fsm", MACHINE);
    write("Theirs.fsm", &edited(&[(t0, t5)]));
    let o = fsm(&dir, &["merge", "Base.fsm", "Ours.fsm", "Theirs.fsm", "-o", "Ours.fsm"]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert_eq!(
        stderr(&o),
        "Ours.fsm: 2 changes merged, 0 conflicts, 1 error\n  ! initial to Off: error: An initial pseudostate must have exactly one outgoing transition.\n"
    );
    // The merged file is written all the same, with both transitions.
    let merged = fsm_cli::xmi::from_xmi(&read("Ours.fsm")).unwrap();
    assert_eq!(merged.transitions.iter().filter(|t| t.source == "i").count(), 2);
    // JSON: counts.errors says why the exit code is 1.
    write("Ours.fsm", MACHINE);
    let o = fsm(&dir, &["merge", "--json", "Base.fsm", "Ours.fsm", "Theirs.fsm", "-o", "M.fsm"]);
    assert_eq!(o.status.code(), Some(1));
    let doc: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(
        doc["counts"],
        serde_json::json!({ "merged": 2, "conflicts": 0, "layout": 0, "issues": 1, "errors": 1 })
    );
    assert_eq!(doc["issues"][0]["severity"], "error");
    // A side alone merges cleanly.
    let o = fsm(&dir, &["merge", "Base.fsm", "Base.fsm", "Theirs.fsm", "-o", "M.fsm"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let _ = std::fs::remove_dir_all(&dir);
}
