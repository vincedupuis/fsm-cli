//! Parity with the editors: the fixtures in `tests/fixtures`, which the umbrella repo's `parity/fixtures.mjs` makes from
//! fsm-editor-vscode's `src/xmi.ts` and `src/validation.ts`, as it does for the Visual Studio and JetBrains editions
//! (`tests/fixtures/README.md`). The validator must report what the editors' validator reports (element, severity and
//! message, in order), and the writer must write each case's round trip, which must save to itself when its ids are
//! unique.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::model::{SubmachineInfo, VertexType, machine_connection_points};
use crate::paths::decode_uri;
use crate::validation::{Issue, validate};
use crate::writer::to_xmi;
use crate::xmi::from_xmi;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).replace("\r\n", "\n")
}

fn expected() -> Value {
    serde_json::from_str(&read(&fixtures().join("expected.json"))).unwrap()
}

fn format(issues: &[Issue]) -> Vec<String> {
    issues.iter().map(|i| format!("{}|{}|{}", i.severity.as_str(), i.id, i.message)).collect()
}

fn format_expected(issues: &Value) -> Vec<String> {
    let s = |i: &Value, k: &str| i[k].as_str().unwrap_or("").to_string();
    issues
        .as_array()
        .unwrap()
        .iter()
        .map(|i| format!("{}|{}|{}", s(i, "severity"), s(i, "id"), s(i, "message")))
        .collect()
}

/// The differences between the issues expected and found, for a message listing every case at once.
fn compare(name: &str, expected: Vec<String>, found: Vec<String>, failures: &mut Vec<String>) {
    if expected != found {
        let missing: Vec<&String> = expected.iter().filter(|i| !found.contains(i)).collect();
        let extra: Vec<&String> = found.iter().filter(|i| !expected.contains(i)).collect();
        failures
            .push(format!("{name}:\n  expected only: {missing:#?}\n  found only: {extra:#?}\n  (or in another order)"));
    }
}

#[test]
fn cases_validate_as_the_editors() {
    // Validated without submachines, as the fixtures are.
    let mut failures = Vec::new();
    for (name, case) in expected()["cases"].as_object().unwrap() {
        let model = from_xmi(&read(&fixtures().join(format!("{name}.fsm")))).unwrap();
        compare(name, format_expected(&case["issues"]), format(&validate(&model, &HashMap::new())), &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn examples_validate_as_the_editors() {
    // Validated with their submachines resolved, as the editors (and the fixtures) do.
    let dir = fixtures().join("examples");
    let mut failures = Vec::new();
    for (file, example) in expected()["examples"].as_object().unwrap() {
        let model = from_xmi(&read(&dir.join(file))).unwrap();
        let mut info: HashMap<String, SubmachineInfo> = HashMap::new();
        for v in model.vertices.iter().filter(|v| v.vtype == VertexType::State) {
            let Some(href) = v.submachine.as_deref().filter(|h| !h.is_empty() && !info.contains_key(*h)) else {
                continue;
            };
            let (path, id) = href.split_once('#').unwrap_or((href, ""));
            let sub = std::fs::read_to_string(dir.join(decode_uri(path))).ok().and_then(|t| from_xmi(&t).ok());
            let found = match sub {
                Some(sub) if sub.id == id => {
                    SubmachineInfo { found: true, name: sub.name.clone(), points: machine_connection_points(&sub) }
                }
                _ => SubmachineInfo::default(),
            };
            info.insert(href.to_string(), found);
        }
        compare(file, format_expected(&example["issues"]), format(&validate(&model, &info)), &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn writes_the_round_trips_as_the_editors() {
    let mut failures = Vec::new();
    for name in expected()["cases"].as_object().unwrap().keys() {
        let text = read(&fixtures().join(format!("{name}.fsm")));
        let round_trip = read(&fixtures().join(format!("{name}.roundtrip.xmi")));
        if to_xmi(&from_xmi(&text).unwrap()).unwrap() != round_trip {
            failures.push(format!("{name}: write(read({name}.fsm)) isn't {name}.roundtrip.xmi"));
        }
        // Settled in one save when the ids are unique; with ids used twice (reported by the validator), saved again
        // without an element written twice.
        let again = to_xmi(&from_xmi(&round_trip).unwrap()).unwrap();
        let shares_ids = expected()["cases"][name]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["message"].as_str().unwrap_or("").starts_with("Duplicate id"));
        let ids = |s: &str| s.matches("xmi:id=").count();
        if !shares_ids && again != round_trip {
            failures.push(format!("{name}: {name}.roundtrip.xmi doesn't save to itself"));
        }
        if shares_ids
            && (ids(&again) != ids(&round_trip) || ids(&to_xmi(&from_xmi(&again).unwrap()).unwrap()) != ids(&again))
        {
            failures.push(format!("{name}: saving {name}.roundtrip.xmi again changes its number of elements"));
        }
    }
    for (file, example) in expected()["examples"].as_object().unwrap() {
        let text = read(&fixtures().join("examples").join(file));
        let same = to_xmi(&from_xmi(&text).unwrap()).unwrap() == text;
        if Some(same) != example["roundTripSame"].as_bool() {
            failures.push(format!("{file}: saved unchanged {same}, the editors {}", example["roundTripSame"]));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
