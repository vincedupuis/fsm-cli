//! The test cases of the format (`tests/cases`): `.fsm` files with the problems `fsm` reports for them, in
//! `expected.json` beside them, which `fsm` itself writes: `FSM_UPDATE_CASES=1 cargo test --test cases`.
//!
//! Each folder is checked as a whole, as `fsm write` would read its files (submachines resolved within it), and the
//! JSON Schemas (`schemas/`) must agree with the reader: every file the reader takes passes `fsm.schema.json`, every
//! file whose text or shape it refuses fails it. (A field given twice is the exception: JSON Schema can't see it.)

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fsm_cli::format::read_machine;
use fsm_cli::generate::check;
use serde_json::{Value, json};

fn cases() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases")
}

fn files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file() && p.to_string_lossy().ends_with(ext))
        .collect();
    out.sort();
    out
}

fn folders() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> =
        std::fs::read_dir(cases()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    out.sort();
    out
}

/// The problems of every `.fsm` file of `dir`, by file name.
fn problems(dir: &Path) -> Value {
    let sources = files(dir, ".fsm");
    let read = |p: &Path| std::fs::read_to_string(p).ok();
    // Paths displayed relative to the folder.
    fsm_cli::paths::set_cwd(Some(dir.to_path_buf()));
    let issues = check(&sources, &read).unwrap();
    fsm_cli::paths::set_cwd(None);
    let mut out: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for s in &sources {
        out.insert(s.file_name().unwrap().to_string_lossy().into_owned(), Vec::new());
    }
    for i in issues {
        let name = Path::new(&i.file).file_name().unwrap().to_string_lossy().into_owned();
        out.entry(name).or_default().push(json!({
            "line": i.line,
            "id": i.id,
            "severity": i.severity.as_str(),
            "message": i.message,
        }));
    }
    serde_json::to_value(out).unwrap()
}

#[test]
fn expected_problems() {
    let update = std::env::var("FSM_UPDATE_CASES").is_ok_and(|v| !v.is_empty());
    for dir in folders() {
        let found = problems(&dir);
        let path = dir.join("expected.json");
        if update {
            std::fs::write(&path, serde_json::to_string_pretty(&found).unwrap() + "\n").unwrap();
            continue;
        }
        let expected: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            found,
            expected,
            "{}: run FSM_UPDATE_CASES=1 cargo test --test cases after checking the change",
            dir.display()
        );
    }
}

/// A compiled schema of `schemas/`.
struct Schema {
    schemas: boon::Schemas,
    index: boon::SchemaIndex,
}

impl Schema {
    fn new(name: &str) -> Schema {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas").join(name);
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let url = format!("file:///schemas/{name}");
        let mut schemas = boon::Schemas::new();
        let mut compiler = boon::Compiler::new();
        compiler.add_resource(&url, value).unwrap();
        let index = compiler.compile(&url, &mut schemas).unwrap();
        Schema { schemas, index }
    }

    /// Whether `text` is JSON that the schema accepts.
    fn accepts(&self, text: &str) -> bool {
        match serde_json::from_str::<Value>(text) {
            Ok(v) => self.schemas.validate(&v, self.index).is_ok(),
            Err(_) => false,
        }
    }
}

#[test]
fn the_model_schema_agrees_with_the_reader() {
    let schema = Schema::new("fsm.schema.json");
    let mut counts = (0, 0);
    for dir in folders() {
        for f in files(&dir, ".fsm") {
            let text = std::fs::read_to_string(&f).unwrap();
            let read = read_machine(&text);
            // A field given twice is the one problem a schema can't see: JSON parsers keep one of the values.
            if read.as_ref().is_err_and(|p| p.iter().all(|p| p.message.ends_with("is given twice."))) {
                continue;
            }
            let readable = read.is_ok();
            assert_eq!(
                schema.accepts(&text),
                readable,
                "{}: the reader {}",
                f.display(),
                if readable { "takes it" } else { "refuses it" }
            );
            if readable {
                counts.0 += 1;
            } else {
                counts.1 += 1;
            }
        }
    }
    assert!(counts.0 >= 10 && counts.1 >= 10, "{counts:?}");
}
