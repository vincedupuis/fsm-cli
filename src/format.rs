//! Reading `.fsm` files: a JSON document holding one state machine, without its layout (that is the
//! `.fsm.layout` file beside it, which `fsm` never reads). The format is described in README.md ("The .fsm file
//! format") and by `schemas/fsm.schema.json`; FSM Editor's reader (`src/format.ts` of fsm-editor-vscode) follows this
//! one: the same problems, with the same messages, at the same lines.
//!
//! Reading is strict: a document that isn't JSON, or whose shape is wrong (a missing or unknown field, a value of the
//! wrong kind, an unknown type), gives problems with their lines and no machine. A document with the right shape gives
//! the machine, whatever it means: names used twice and references that name nothing are the validator's to report.
//!
//! The model is flat (model.rs), and its ids are derived from the names:
//! - an element: its name; a region of `regions`: its name;
//! - the region of a state with one region: `<state>.`; the machine's only region: `<machine>.`;
//! - an initial, history or deep history pseudostate: `<owner>.initial`, `<owner>.history`, `<owner>.deepHistory`,
//!   the owner being the state, the region when the state (or machine) has several, or the machine;
//! - a point: `<state>.<point>`, or `<machine>.<point>` for the machine's own;
//! - a transition: `<source>#<n>`, n counting the source's transitions from 1;
//! - an id already taken (a name used twice) gets `~2`, `~3`, ... appended; an empty name gives `~`.

use std::collections::{HashMap, HashSet};

use crate::json::{self, Field, Json, Node};
use crate::model::{
    FsmModel, MachineKind, PointKind, Region, Transition, TransitionKind, UNRESOLVED, Vertex, VertexType, js_trim,
};
use crate::sha256::sha256;

/// A problem of the file's text or shape, with its line.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub line: usize,
    pub message: String,
}

/// A machine read from its file.
pub struct ReadMachine {
    pub model: FsmModel,
    /// Line of each element, region, point and transition, by id (the machine itself is on line 1).
    pub lines: HashMap<String, usize>,
    /// The model hash: the first 16 hex digits of the SHA-256 of the document's content (`json::canonical`), without
    /// `$schema`.
    pub hash: String,
}

/// Whether a text is an old `.fsm` file (XMI), which `fsm` doesn't read any more.
pub fn is_old_format(text: &str) -> bool {
    text.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']).starts_with('<')
}

// ------------------------------------------------------------------ shape

const ELEMENT_TYPES: [&str; 8] = ["state", "choice", "junction", "fork", "join", "final", "terminate", "comment"];
const REGION_FIELDS: [&str; 4] = ["initial", "history", "deepHistory", "elements"];

/// `"a", "b" or "c"`.
fn one_of(values: &[&str]) -> String {
    let q: Vec<String> = values.iter().map(|v| format!("\"{v}\"")).collect();
    match q.len() {
        1 => q[0].clone(),
        n => format!("{} or {}", q[..n - 1].join(", "), q[n - 1]),
    }
}

enum Kind {
    Text,
    Object,
    List,
}

struct Shape {
    problems: Vec<Problem>,
}

impl Shape {
    fn add(&mut self, line: usize, message: String) {
        self.problems.push(Problem { line, message });
    }

    /// Checks the kind of a field's value; false (and a problem) when it is wrong.
    fn kind(&mut self, f: &Field, kind: Kind) -> bool {
        let ok = match kind {
            Kind::Text => matches!(f.value.value, Json::String(_)),
            Kind::Object => matches!(f.value.value, Json::Object(_)),
            Kind::List => matches!(f.value.value, Json::Array(_)),
        };
        if !ok {
            let what = match kind {
                Kind::Text => "a text",
                Kind::Object => "an object",
                Kind::List => "a list",
            };
            self.add(f.value.line, format!("Field '{}' must be {what}.", f.key));
        }
        ok
    }

    /// Checks an object's fields against `allowed` (each with its check) and `required`.
    fn object(
        &mut self,
        node: &Node,
        what: &str,
        allowed: &[&str],
        required: &[&str],
        check: &mut dyn FnMut(&mut Shape, &Field),
    ) {
        let fields = node.fields().unwrap_or(&[]);
        for f in fields {
            if allowed.contains(&f.key.as_str()) {
                check(self, f);
            } else {
                self.add(f.line, format!("Unknown field '{}' in {what}.", f.key));
            }
        }
        for r in required {
            if !fields.iter().any(|f| f.key == *r) {
                self.add(node.line, format!("Missing field '{r}' in {what}."));
            }
        }
    }

    /// A list of objects, each checked by `item`.
    fn list(&mut self, f: &Field, item: &mut dyn FnMut(&mut Shape, &Node)) {
        if !self.kind(f, Kind::List) {
            return;
        }
        for x in f.value.items().unwrap_or(&[]) {
            if x.fields().is_none() {
                self.add(x.line, format!("Items of '{}' must be objects.", f.key));
            } else {
                item(self, x);
            }
        }
    }

    fn enumeration(&mut self, f: &Field, values: &[&str]) -> bool {
        if !self.kind(f, Kind::Text) {
            return false;
        }
        let v = f.value.str().unwrap_or("");
        if values.contains(&v) {
            return true;
        }
        self.add(f.value.line, format!("Field '{}' must be {}.", f.key, one_of(values)));
        false
    }

    fn machine(&mut self, node: &Node) {
        let Some(fields) = node.fields() else {
            self.add(node.line, "The file must hold a JSON object.".to_string());
            return;
        };
        let mut allowed = vec!["$schema", "format", "name", "type", "documentation", "points", "regions"];
        allowed.extend(REGION_FIELDS);
        self.object(node, "the machine", &allowed, &["format", "name", "type"], &mut |s, f| match f.key.as_str() {
            "format" => match f.value.number() {
                Some(1.0) => {}
                Some(n) if n > 1.0 && n.fract() == 0.0 => s.add(
                    f.value.line,
                    format!("This file is in format {}: it needs a newer version of FSM Editor.", json::number(n)),
                ),
                _ => s.add(f.value.line, "Field 'format' must be 1.".to_string()),
            },
            "type" => {
                s.enumeration(f, &["behavioral", "protocol"]);
            }
            "points" => s.list(f, &mut |s, x| s.point(x)),
            "$schema" | "name" | "documentation" => {
                s.kind(f, Kind::Text);
            }
            _ => s.region_field(f),
        });
        self.regions_alone(fields);
    }

    /// `regions`, when present, without region fields beside it.
    fn regions_alone(&mut self, fields: &[Field]) {
        if fields.iter().any(|f| f.key == "regions") {
            for f in fields.iter().filter(|f| REGION_FIELDS.contains(&f.key.as_str())) {
                self.add(f.line, format!("Field '{}' cannot be used with 'regions': each region has its own.", f.key));
            }
        }
    }

    /// `initial`, `history`, `deepHistory`, `elements` or `regions`.
    fn region_field(&mut self, f: &Field) {
        match f.key.as_str() {
            "initial" | "history" | "deepHistory" => {
                if self.kind(f, Kind::Object) {
                    let what = format!("'{}'", f.key);
                    self.object(&f.value, &what, &["target", "action"], &[], &mut |s, g| {
                        s.kind(g, Kind::Text);
                    });
                }
            }
            "elements" => self.list(f, &mut |s, x| s.element(x)),
            _ => {
                // regions
                if !self.kind(f, Kind::List) {
                    return;
                }
                let items = f.value.items().unwrap_or(&[]);
                if items.len() < 2 {
                    self.add(
                        f.value.line,
                        "Field 'regions' needs two regions or more: the fields of a single region go directly in its owner."
                            .to_string(),
                    );
                }
                for x in items {
                    if x.fields().is_none() {
                        self.add(x.line, "Items of 'regions' must be objects.".to_string());
                        continue;
                    }
                    let mut allowed = vec!["name"];
                    allowed.extend(REGION_FIELDS);
                    self.object(x, "a region", &allowed, &["name"], &mut |s, g| {
                        if g.key == "name" {
                            s.kind(g, Kind::Text);
                        } else {
                            s.region_field(g);
                        }
                    });
                }
            }
        }
    }

    fn element(&mut self, node: &Node) {
        let t = node.get("type");
        let valid = t.is_some_and(|t| t.value.str().is_some_and(|v| ELEMENT_TYPES.contains(&v)));
        if !valid {
            // Without a known type, only the name and the type are checked.
            let ok = |s: &mut Shape, f: &Field| {
                if f.key == "name" {
                    s.kind(f, Kind::Text);
                } else if f.key == "type" {
                    s.enumeration(f, &ELEMENT_TYPES);
                }
            };
            for f in node.fields().unwrap_or(&[]) {
                ok(self, f);
            }
            for r in ["name", "type"] {
                if node.get(r).is_none() {
                    self.add(node.line, format!("Missing field '{r}' in an element."));
                }
            }
            return;
        }
        let ty = t.unwrap().value.str().unwrap();
        let (what, extra): (&str, &[&str]) = match ty {
            "state" => (
                "a state",
                &[
                    "stereotype",
                    "submachine",
                    "entry",
                    "do",
                    "exit",
                    "defer",
                    "invariant",
                    "transitions",
                    "points",
                    "initial",
                    "history",
                    "deepHistory",
                    "elements",
                    "regions",
                ],
            ),
            "choice" => ("a choice", &["transitions"]),
            "junction" => ("a junction", &["transitions"]),
            "fork" => ("a fork", &["transitions"]),
            "join" => ("a join", &["transitions"]),
            "final" => ("a final state", &[]),
            "terminate" => ("a terminate pseudostate", &[]),
            _ => ("a comment", &["text", "anchors"]),
        };
        let mut allowed = vec!["name", "type"];
        allowed.extend(extra);
        self.object(node, what, &allowed, &["name", "type"], &mut |s, f| match f.key.as_str() {
            "type" => {}
            "transitions" => s.list(f, &mut |s, x| s.transition(x)),
            "points" => s.list(f, &mut |s, x| s.point(x)),
            "anchors" => {
                if s.kind(f, Kind::List) {
                    for x in f.value.items().unwrap_or(&[]) {
                        match &x.value {
                            Json::String(_) => {}
                            Json::Object(_) => s.object(
                                x,
                                "a comment anchor",
                                &["source", "trigger", "target"],
                                &["source"],
                                &mut |s, g| {
                                    s.kind(g, Kind::Text);
                                },
                            ),
                            _ => s.add(x.line, "Items of 'anchors' must be texts or objects.".to_string()),
                        }
                    }
                }
            }
            "initial" | "history" | "deepHistory" | "elements" | "regions" => s.region_field(f),
            _ => {
                s.kind(f, Kind::Text);
            }
        });
        if ty == "state" {
            self.regions_alone(node.fields().unwrap_or(&[]));
        }
    }

    fn point(&mut self, node: &Node) {
        self.object(node, "a point", &["name", "type", "transitions"], &["name", "type"], &mut |s, f| match f
            .key
            .as_str()
        {
            "type" => {
                s.enumeration(f, &["entryPoint", "exitPoint"]);
            }
            "transitions" => s.list(f, &mut |s, x| s.transition(x)),
            _ => {
                s.kind(f, Kind::Text);
            }
        });
    }

    fn transition(&mut self, node: &Node) {
        let allowed = ["type", "trigger", "guard", "action", "precondition", "postcondition", "target"];
        self.object(node, "a transition", &allowed, &["type"], &mut |s, f| {
            if f.key == "type" {
                s.enumeration(f, &["external", "internal", "local"]);
            } else {
                s.kind(f, Kind::Text);
            }
        });
        match node.get("type").and_then(|t| t.value.str()) {
            Some("internal") => {
                if let Some(f) = node.get("target") {
                    self.add(f.line, "An internal transition has no 'target'.".to_string());
                }
            }
            Some("external" | "local") if node.get("target").is_none() => {
                self.add(node.line, "Missing field 'target' in a transition.".to_string());
            }
            _ => {}
        }
    }
}

/// The problems of a document's shape (none when it is a machine file).
pub fn shape_problems(node: &Node) -> Vec<Problem> {
    let mut s = Shape { problems: Vec::new() };
    s.machine(node);
    s.problems
}

// ------------------------------------------------------------------ model

fn text(node: &Node, key: &str) -> String {
    node.get(key).and_then(|f| f.value.str()).unwrap_or("").to_string()
}

fn list<'a>(node: &'a Node, key: &str) -> &'a [Node] {
    node.get(key).and_then(|f| f.value.items()).unwrap_or(&[])
}

/// Splits triggers or deferred events written `a, b, c`.
pub fn split_events(s: &str) -> Vec<String> {
    s.split(',').map(|x| js_trim(x).to_string()).filter(|x| !x.is_empty()).collect()
}

/// A region's fields, where they come from: the machine, a state, or an item of `regions`.
struct RegionSource<'a> {
    node: &'a Node,
    id: String,
    /// The owner in references to its history pseudostates.
    owner: String,
}

struct Builder<'a> {
    model: FsmModel,
    lines: HashMap<String, usize>,
    taken: HashSet<String>,
    /// Name → id of the first element (or region of `regions`) with that name.
    by_name: HashMap<String, String>,
    /// Region or state or machine name → its regions' history pseudostates (`owner.history` → id).
    pseudo: HashMap<String, String>,
    /// Point references (`owner.point`) → id.
    points: HashMap<String, String>,
    /// Transitions to build once every vertex is known: source id, the transition's node.
    pending: Vec<(String, &'a Node, bool)>,
}

impl<'a> Builder<'a> {
    fn alloc(&mut self, base: &str) -> String {
        let mut id = base.to_string();
        let mut n = 2;
        while self.taken.contains(&id) {
            id = format!("{base}~{n}");
            n += 1;
        }
        self.taken.insert(id.clone());
        id
    }

    /// The id of a named element or region: its name, unless an earlier one took it (an empty name, which the
    /// validator reports, gets `~`: '' is the machine's).
    fn named(&mut self, name: &str) -> String {
        let id = self.alloc(if name.is_empty() { "~" } else { name });
        self.by_name.entry(name.to_string()).or_insert_with(|| id.clone());
        id
    }

    fn region(&mut self, src: RegionSource<'a>) {
        let RegionSource { node, id, owner } = src;
        for (key, vtype) in [
            ("initial", VertexType::Initial),
            ("history", VertexType::ShallowHistory),
            ("deepHistory", VertexType::DeepHistory),
        ] {
            let Some(f) = node.get(key) else { continue };
            let vid = self.alloc(&format!("{owner}.{key}"));
            if key != "initial" {
                self.pseudo.entry(format!("{owner}.{key}")).or_insert_with(|| vid.clone());
            }
            self.lines.insert(vid.clone(), f.line);
            self.model.vertices.push(Vertex::new(&vid, vtype, "", &id));
            // The default transition, when there is one: a target or an action (an action without a target is reported).
            let x = &f.value;
            if x.get("target").is_some() || x.get("action").is_some() {
                self.pending.push((vid, x, true));
            }
        }
        for e in list(node, "elements") {
            self.element(e, &id);
        }
    }

    fn element(&mut self, e: &'a Node, parent: &str) {
        let name = text(e, "name");
        let id = self.named(&name);
        self.lines.insert(id.clone(), e.line);
        let ty = text(e, "type");
        let vtype = match ty.as_str() {
            "state" => VertexType::State,
            "choice" => VertexType::Choice,
            "junction" => VertexType::Junction,
            "fork" => VertexType::Fork,
            "join" => VertexType::Join,
            "final" => VertexType::Final,
            "terminate" => VertexType::Terminate,
            _ => VertexType::Comment,
        };
        let mut v = Vertex::new(&id, vtype, &name, parent);
        if vtype == VertexType::Comment {
            v.text = text(e, "text");
            v.anchors = list(e, "anchors")
                .iter()
                .map(|a| {
                    a.str().map(str::to_string).unwrap_or_else(|| {
                        let mut s = String::new();
                        json::canonical(a, &mut s);
                        s
                    })
                })
                .collect();
        }
        if vtype != VertexType::State {
            self.model.vertices.push(v);
            for t in list(e, "transitions") {
                self.pending.push((id.clone(), t, false));
            }
            return;
        }
        v.stereotype = text(e, "stereotype");
        v.entry = text(e, "entry");
        v.exit = text(e, "exit");
        v.do_activity = text(e, "do");
        v.invariant = text(e, "invariant");
        v.deferrable = split_events(&text(e, "defer"));
        v.submachine = e.get("submachine").and_then(|f| f.value.str()).map(str::to_string);
        let index = self.model.vertices.len();
        self.model.vertices.push(v);
        for t in list(e, "transitions") {
            self.pending.push((id.clone(), t, false));
        }
        self.points(e, &id, &name, &id, true);
        // Regions
        let mut sources = Vec::new();
        if let Some(f) = e.get("regions") {
            for r in f.value.items().unwrap_or(&[]) {
                let rname = text(r, "name");
                let rid = self.named(&rname);
                self.lines.insert(rid.clone(), r.line);
                sources.push((Region::new(&rid, &rname), RegionSource { node: r, id: rid, owner: rname }));
            }
        } else if REGION_FIELDS.iter().any(|k| e.get(k).is_some()) {
            let rid = self.alloc(&format!("{name}."));
            self.lines.insert(rid.clone(), e.line);
            sources.push((Region::new(&rid, ""), RegionSource { node: e, id: rid, owner: name.clone() }));
        }
        for (r, src) in sources {
            self.model.vertices[index].regions.push(r);
            self.region(src);
        }
    }

    /// The points of a state (or of the machine): entry and exit points, or connection point references for a
    /// submachine state.
    fn points(&mut self, node: &'a Node, owner_id: &str, owner_name: &str, parent: &str, state: bool) {
        let refs = state && node.get("submachine").is_some();
        for p in list(node, "points") {
            let name = text(p, "name");
            let entry = text(p, "type") == "entryPoint";
            let key = format!("{owner_name}.{name}");
            let id = self.alloc(&key);
            self.points.entry(key).or_insert_with(|| id.clone());
            self.lines.insert(id.clone(), p.line);
            let v = if refs {
                let mut v = Vertex::new(&id, VertexType::ConnectionPointRef, "", owner_id);
                v.point_ref = name;
                v.point_kind = if entry { PointKind::Entry } else { PointKind::Exit };
                v
            } else {
                Vertex::new(&id, if entry { VertexType::EntryPoint } else { VertexType::ExitPoint }, &name, parent)
            };
            self.model.vertices.push(v);
            for t in list(p, "transitions") {
                self.pending.push((id.clone(), t, false));
            }
        }
    }

    /// The id a reference names, or UNRESOLVED followed by the reference.
    fn resolve(&self, reference: &str) -> String {
        let found = match reference.split_once('.') {
            None => self.by_name.get(reference),
            Some((_, rest)) if rest == "history" || rest == "deepHistory" => self.pseudo.get(reference),
            Some(_) => self.points.get(reference),
        };
        match found {
            Some(id) => id.clone(),
            None => format!("{UNRESOLVED}{reference}"),
        }
    }
}

/// Reads a machine file.
pub fn read_machine(text: &str) -> Result<ReadMachine, Vec<Problem>> {
    let root = json::parse(text).map_err(|e| vec![Problem { line: e.line, message: e.message }])?;
    let problems = shape_problems(&root);
    if !problems.is_empty() {
        return Err(problems);
    }
    let name = self::text(&root, "name");
    let kind = if self::text(&root, "type") == "protocol" { MachineKind::Protocol } else { MachineKind::Behavioral };
    let mut b = Builder {
        model: FsmModel {
            name: name.clone(),
            kind,
            documentation: self::text(&root, "documentation"),
            regions: Vec::new(),
            vertices: Vec::new(),
            transitions: Vec::new(),
        },
        lines: HashMap::new(),
        taken: HashSet::new(),
        by_name: HashMap::new(),
        pseudo: HashMap::new(),
        points: HashMap::new(),
        pending: Vec::new(),
    };
    let mut sources = Vec::new();
    if let Some(f) = root.get("regions") {
        for r in f.value.items().unwrap_or(&[]) {
            let rname = self::text(r, "name");
            let rid = b.named(&rname);
            b.lines.insert(rid.clone(), r.line);
            b.model.regions.push(Region::new(&rid, &rname));
            sources.push(RegionSource { node: r, id: rid, owner: rname });
        }
    } else {
        let rid = b.alloc(&format!("{name}."));
        b.model.regions.push(Region::new(&rid, ""));
        sources.push(RegionSource { node: &root, id: rid, owner: name.clone() });
    }
    for src in sources {
        b.region(src);
    }
    let top = b.model.regions[0].id.clone();
    b.points(&root, "", &name, &top, false);

    // Transitions, now that every vertex is known.
    let mut count: HashMap<String, usize> = HashMap::new();
    let pending = std::mem::take(&mut b.pending);
    for (source, node, default) in pending {
        let n = count.entry(source.clone()).or_insert(0);
        *n += 1;
        let id = b.alloc(&format!("{source}#{n}"));
        b.lines.insert(id.clone(), node.line);
        let kind = match self::text(node, "type").as_str() {
            "internal" => TransitionKind::Internal,
            "local" => TransitionKind::Local,
            _ => TransitionKind::External,
        };
        let target = match node.get("target").and_then(|f| f.value.str()) {
            Some(r) => b.resolve(r),
            None if kind == TransitionKind::Internal && !default => source.clone(),
            None => UNRESOLVED.to_string(),
        };
        let mut t = Transition::new(&id, &source, &target, kind);
        t.triggers = split_events(&self::text(node, "trigger"));
        t.guard = self::text(node, "guard");
        t.action = self::text(node, "action");
        t.precondition = self::text(node, "precondition");
        t.postcondition = self::text(node, "postcondition");
        b.model.transitions.push(t);
    }

    let mut canon = String::new();
    let mut content = root.clone();
    if let Json::Object(fields) = &mut content.value {
        fields.retain(|f| f.key != "$schema");
    }
    json::canonical(&content, &mut canon);
    let hash = sha256(&canon)[..16].to_string();
    Ok(ReadMachine { model: b.model, lines: b.lines, hash })
}
