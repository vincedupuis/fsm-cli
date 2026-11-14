//! `fsm diff`: what changed between two versions of a state machine.
//!
//! Elements (the machine, its vertices, its transitions) are paired by xmi:id (`pair`). Each element's properties are
//! named values: its model properties (`machine_properties`, `vertex_properties`, `transition_properties`) and its
//! drawing properties (`vertex_layout`, `transition_layout`), positions relative to the owning state and bend points
//! relative to the source, rounded to 0.1 px as files store them. An element whose model properties differ is
//! changed, one whose drawing properties only differ is changed in the layout only.
//!
//! The labels and sentences are shown to people, here and by other tools: README.md ("fsm diff") documents them, and
//! they must stay stable.

use std::collections::HashMap;

use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

use crate::geometry::n1;
use crate::model::{FsmModel, ModelIndex, PointKind, Region, RegionLayout, TransitionKind, Vertex, VertexType};

/// A property's value, as JSON (objects keep their keys' order).
#[derive(Debug, Clone)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Text(String),
    List(Vec<Value>),
    Object(Vec<(&'static str, Value)>),
}

impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a == b || (a.is_nan() && b.is_nan()),
            (Value::Text(a), Value::Text(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Object(a), Value::Object(b)) => a == b,
            _ => false,
        }
    }
}

impl Value {
    pub(crate) fn text(s: &str) -> Value {
        Value::Text(s.to_string())
    }

    fn texts(list: &[String]) -> Value {
        Value::List(list.iter().map(|s| Value::text(s)).collect())
    }

    /// A coordinate, rounded to 0.1 as in files.
    pub(crate) fn coordinate(n: f64) -> Value {
        Value::Number(n1(n))
    }

    pub(crate) fn point(x: f64, y: f64) -> Value {
        Value::Object(vec![("x", Value::coordinate(x)), ("y", Value::coordinate(y))])
    }

    pub(crate) fn as_str(&self) -> &str {
        match self {
            Value::Text(s) => s,
            _ => "",
        }
    }

    pub(crate) fn as_list(&self) -> &[Value] {
        match self {
            Value::List(l) => l,
            _ => &[],
        }
    }

    pub(crate) fn field(&self, key: &str) -> &str {
        match self {
            Value::Object(fields) => fields.iter().find(|(k, _)| *k == key).map_or("", |(_, v)| v.as_str()),
            _ => "",
        }
    }
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Value::Null => s.serialize_unit(),
            Value::Bool(b) => s.serialize_bool(*b),
            // Whole numbers without a fraction, as JavaScript writes them; NaN and infinities as null.
            Value::Number(n) if !n.is_finite() => s.serialize_unit(),
            Value::Number(n) if n.fract() == 0.0 && n.abs() < 1e15 => s.serialize_i64(*n as i64),
            Value::Number(n) => s.serialize_f64(*n),
            Value::Text(t) => s.serialize_str(t),
            Value::List(list) => {
                let mut seq = s.serialize_seq(Some(list.len()))?;
                for v in list {
                    seq.serialize_element(v)?;
                }
                seq.end()
            }
            Value::Object(fields) => {
                let mut map = s.serialize_map(Some(fields.len()))?;
                for (k, v) in fields {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

/// Elements of two lists paired by id: `(old index, new index)`, None on the side that lacks it. The k-th element
/// with an id on one side goes with the k-th with that id on the other. The pairs follow the new list, then come the
/// elements only the old one has, in its order.
pub fn pair<T>(old: &[T], new: &[T], id: impl Fn(&T) -> &str) -> Vec<(Option<usize>, Option<usize>)> {
    let mut by_id: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, e) in old.iter().enumerate().rev() {
        by_id.entry(id(e)).or_default().push(i);
    }
    let mut taken = vec![false; old.len()];
    let mut out = Vec::new();
    for (j, e) in new.iter().enumerate() {
        let i = by_id.get_mut(id(e)).and_then(Vec::pop);
        if let Some(i) = i {
            taken[i] = true;
        }
        out.push((i, Some(j)));
    }
    out.extend((0..old.len()).filter(|&i| !taken[i]).map(|i| (Some(i), None)));
    out
}

fn regions(list: &[Region]) -> Value {
    Value::List(
        list.iter().map(|r| Value::Object(vec![("id", Value::text(&r.id)), ("name", Value::text(&r.name))])).collect(),
    )
}

/// The model properties of the machine.
pub fn machine_properties(m: &FsmModel) -> Vec<(&'static str, Value)> {
    vec![
        ("name", Value::text(&m.name)),
        ("kind", Value::text(m.kind.as_str())),
        ("context", Value::text(&m.context)),
        ("documentation", Value::text(&m.documentation)),
        ("regions", regions(&m.regions)),
    ]
}

/// The model properties of a vertex (empty for the kinds of vertices that have none).
pub fn vertex_properties(v: &Vertex) -> Vec<(&'static str, Value)> {
    let point = match v.vtype {
        VertexType::ConnectionPointRef => {
            Value::Object(vec![("kind", Value::text(v.point_kind.as_str())), ("ref", Value::text(&v.point_ref))])
        }
        _ => Value::Null,
    };
    vec![
        ("type", Value::text(v.vtype.as_str())),
        ("name", Value::text(&v.name)),
        ("owner", Value::text(&v.parent)),
        ("regions", regions(&v.regions)),
        ("entry", Value::text(&v.entry)),
        ("exit", Value::text(&v.exit)),
        ("do", Value::text(&v.do_activity)),
        ("deferred", Value::texts(&v.deferrable)),
        ("invariant", Value::text(&v.invariant)),
        ("submachine", Value::text(v.submachine.as_deref().unwrap_or(""))),
        ("stereotype", Value::text(&v.stereotype)),
        ("point", point),
        ("text", Value::text(&v.text)),
        ("anchors", Value::texts(&v.anchors)),
    ]
}

fn kind_str(k: TransitionKind) -> &'static str {
    match k {
        TransitionKind::External => "external",
        TransitionKind::Internal => "internal",
        TransitionKind::Local => "local",
    }
}

/// The model properties of a transition.
pub fn transition_properties(m: &FsmModel, t: usize) -> Vec<(&'static str, Value)> {
    let t = &m.transitions[t];
    vec![
        ("source", Value::text(&t.source)),
        ("target", Value::text(&t.target)),
        ("kind", Value::text(kind_str(t.kind))),
        ("triggers", Value::texts(&t.triggers)),
        ("guard", Value::text(&t.guard)),
        ("effect", Value::text(&t.effect)),
        ("precondition", Value::text(&t.precondition)),
        ("postcondition", Value::text(&t.postcondition)),
    ]
}

pub(crate) fn colors(c: &crate::model::Colors) -> [(&'static str, Value); 2] {
    let items = c.items.iter().map(|(k, v)| (*k, Value::text(v))).collect();
    [("color", Value::text(&c.color)), ("itemColors", Value::Object(items))]
}

/// The drawing properties of a vertex: its position relative to the state that owns it (absolute at the top
/// level), its size, its regions' layout and sizes, its colors and its group.
pub fn vertex_layout(index: &ModelIndex, v: usize) -> Vec<(&'static str, Value)> {
    let vx = index.v(v);
    let (ox, oy) = index.owner_state(v).map_or((0.0, 0.0), |o| (index.v(o).x, index.v(o).y));
    let mut out = vec![
        ("position", Value::point(vx.x - ox, vx.y - oy)),
        ("size", Value::Object(vec![("width", Value::coordinate(vx.w)), ("height", Value::coordinate(vx.h))])),
    ];
    if vx.regions.len() >= 2 {
        let horizontal = vx.region_layout == Some(RegionLayout::Horizontal);
        out.push(("regionLayout", Value::text(if horizontal { "horizontal" } else { "vertical" })));
    }
    if !vx.regions.is_empty() {
        let sizes = vx.regions.iter().map(|r| r.size.map_or(Value::Null, Value::coordinate)).collect();
        out.push(("regionSizes", Value::List(sizes)));
    }
    out.extend(colors(&vx.colors));
    out.push(("group", Value::text(&vx.group)));
    out
}

/// The drawing properties of a transition: its bend points relative to its source, its label's offset, its colors.
pub fn transition_layout(index: &ModelIndex, t: usize) -> Vec<(&'static str, Value)> {
    let tr = &index.model.transitions[t];
    let (sx, sy) = index.vertex(&tr.source).map_or((0.0, 0.0), |s| (index.v(s).x, index.v(s).y));
    let mut out = vec![
        ("points", Value::List(tr.points.iter().map(|p| Value::point(p.x - sx, p.y - sy)).collect())),
        ("labelOffset", tr.label_offset.map_or(Value::Null, |p| Value::point(p.x, p.y))),
    ];
    out.extend(colors(&tr.colors));
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Element {
    Machine,
    Vertex,
    Transition,
}

impl Element {
    pub fn as_str(self) -> &'static str {
        match self {
            Element::Machine => "machine",
            Element::Vertex => "vertex",
            Element::Transition => "transition",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Added,
    Removed,
    Changed,
    Layout,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Added => "added",
            Status::Removed => "removed",
            Status::Changed => "changed",
            Status::Layout => "layout",
        }
    }
}

/// A property that differs.
#[derive(Debug, Clone)]
pub struct Change {
    pub property: &'static str,
    pub before: Value,
    pub after: Value,
    /// The sentence describing a model change (None for a drawing property).
    pub sentence: Option<String>,
}

/// An element that differs.
#[derive(Debug, Clone)]
pub struct ElementDiff {
    pub id: String,
    pub element: Element,
    pub status: Status,
    /// The element as people find it on the diagram: as it was, or as it is for an added element.
    pub label: String,
    /// Whether a drawing property differs.
    pub moved: bool,
    /// The model properties that differ, then the drawing ones (none for added and removed elements).
    pub changes: Vec<Change>,
}

/// The differences between two versions of a machine.
#[derive(Debug, Clone, Default)]
pub struct Diff {
    /// Changed elements, added ones, removed ones, then those changed in the layout only; within each, the machine,
    /// then the vertices and the transitions in file order (the new file's, the old one's for removed elements).
    pub elements: Vec<ElementDiff>,
}

impl Diff {
    pub fn count(&self, status: Status) -> usize {
        self.elements.iter().filter(|e| e.status == status).count()
    }
}

// ------------------------------------------------------------------ labels

fn kind_word(t: VertexType) -> &'static str {
    match t {
        VertexType::State => "State",
        VertexType::Final => "Final state",
        VertexType::Initial => "Initial",
        VertexType::ShallowHistory => "Shallow history",
        VertexType::DeepHistory => "Deep history",
        VertexType::Choice => "Choice",
        VertexType::Junction => "Junction",
        VertexType::Fork => "Fork",
        VertexType::Join => "Join",
        VertexType::EntryPoint => "Entry point",
        VertexType::ExitPoint => "Exit point",
        VertexType::Terminate => "Terminate",
        VertexType::ConnectionPointRef => "Connection point reference",
        VertexType::Comment => "Comment",
    }
}

/// Text on one line, cut after `max` characters.
fn short_text(s: &str, max: usize) -> String {
    let line = one_line(s);
    match line.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line,
    }
}

/// Line breaks (and the white space around them) as one space.
fn one_line(s: &str) -> String {
    if !s.contains(['\n', '\r']) {
        return s.to_string();
    }
    s.split(['\n', '\r']).map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ")
}

/// Names elements in one version of the machine.
pub(crate) struct Namer<'a> {
    pub(crate) index: ModelIndex<'a>,
}

impl<'a> Namer<'a> {
    pub(crate) fn new(m: &'a FsmModel) -> Namer<'a> {
        Namer { index: ModelIndex::new(m) }
    }

    fn m(&self) -> &'a FsmModel {
        self.index.model
    }

    /// A vertex in another element's label: its name, the point it references, or its kind; an unknown id as is.
    pub(crate) fn short(&self, id: &str) -> String {
        let Some(i) = self.index.vertex(id) else { return id.to_string() };
        let v = self.index.v(i);
        match v.vtype {
            _ if !v.name.is_empty() => v.name.clone(),
            VertexType::ConnectionPointRef if !v.point_ref.is_empty() => v.point_ref.clone(),
            VertexType::Comment => "comment".to_string(),
            t => kind_word(t).to_string(),
        }
    }

    pub(crate) fn vertex_label(&self, v: usize) -> String {
        let vx = self.index.v(v);
        let owner = self.index.owner_state(v).map(|o| self.short(&self.index.v(o).id));
        match vx.vtype {
            VertexType::Comment => {
                let text = short_text(vx.text.trim(), 30);
                match (text.is_empty(), owner) {
                    (false, _) => format!("Comment \"{text}\""),
                    (true, Some(o)) => format!("Comment in {o}"),
                    (true, None) => "Comment".to_string(),
                }
            }
            VertexType::ConnectionPointRef => {
                let kind = if vx.point_kind == PointKind::Exit { "Exit" } else { "Entry" };
                let mut s = format!("{kind} point reference");
                if !vx.point_ref.is_empty() {
                    s += &format!(" {}", vx.point_ref);
                }
                if let Some(o) = owner {
                    s += &format!(" on {o}");
                }
                s
            }
            t if !vx.name.is_empty() => format!("{} {}", kind_word(t), vx.name),
            t => {
                // Unnamed: by its kind and neighbours.
                let mut s = kind_word(t).to_string();
                let m = self.m();
                if let Some(tr) = m.transitions.iter().find(|tr| tr.target == vx.id && tr.source != vx.id) {
                    s += &format!(" from {}", self.short(&tr.source));
                }
                if let Some(tr) = m.transitions.iter().find(|tr| tr.source == vx.id && tr.target != vx.id) {
                    s += &format!(" to {}", self.short(&tr.target));
                }
                if let Some(o) = owner {
                    s += &format!(" in {o}");
                }
                s
            }
        }
    }

    pub(crate) fn transition_label(&self, t: usize) -> String {
        let tr = &self.m().transitions[t];
        let on = if tr.triggers.is_empty() { String::new() } else { format!(" on {}", tr.triggers.join(", ")) };
        if tr.kind == TransitionKind::Internal {
            return format!("Internal transition in {}{on}", self.short(&tr.source));
        }
        format!("Transition {} → {}{on}", self.short(&tr.source), self.short(&tr.target))
    }

    pub(crate) fn machine_label(&self) -> String {
        let name = &self.m().name;
        if name.is_empty() { "Machine".to_string() } else { format!("Machine {name}") }
    }

    /// Where a vertex is: a region (of a state or of the machine), or a state for a vertex on its border.
    pub(crate) fn place(&self, id: &str) -> String {
        let m = self.m();
        let region = |list: &[Region], of: String| {
            let at = list.iter().position(|r| r.id == id).unwrap_or(0);
            match (list.len(), list[at].name.as_str()) {
                (1, _) => of,
                (_, "") => format!("region {} of {of}", at + 1),
                (_, name) => format!("region {name} of {of}"),
            }
        };
        match self.index.region_owner(id) {
            Some(None) => region(&m.regions, "the machine".to_string()),
            Some(Some(s)) => region(&self.index.v(s).regions, self.short(&self.index.v(s).id)),
            None if self.index.has_vertex(id) => self.short(id),
            None => id.to_string(),
        }
    }

    /// An element a comment is attached to.
    pub(crate) fn anchor(&self, id: &str) -> String {
        if self.index.has_vertex(id) {
            return self.short(id);
        }
        match self.m().transitions.iter().position(|t| t.id == id) {
            Some(t) => {
                let label = self.transition_label(t);
                let mut c = label.chars();
                c.next().map(|f| f.to_lowercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }
            None => id.to_string(),
        }
    }
}

// ------------------------------------------------------------------ sentences

fn list_words(property: &str) -> (&'static str, &'static str) {
    match property {
        "triggers" => ("trigger", "triggers"),
        "deferred" => ("deferred event", "deferred events"),
        _ => ("anchor", "anchors"),
    }
}

/// The items of `a` that `b` lacks, counting repeats.
fn minus(a: &[String], b: &[String]) -> Vec<String> {
    let mut rest: Vec<&String> = b.iter().collect();
    a.iter()
        .filter(|x| match rest.iter().position(|y| y == x) {
            Some(i) => {
                rest.remove(i);
                false
            }
            None => true,
        })
        .cloned()
        .collect()
}

fn list_sentence(property: &str, before: Vec<String>, after: Vec<String>) -> String {
    let (one, many) = list_words(property);
    let word = |n: usize| if n == 1 { one } else { many };
    let added = minus(&after, &before);
    let removed = minus(&before, &after);
    let show = |l: &[String]| if l.is_empty() { "none".to_string() } else { l.join(", ") };
    match (added.is_empty(), removed.is_empty()) {
        (false, true) => format!("{} {} added", word(added.len()), added.join(", ")),
        (true, false) => format!("{} {} removed", word(removed.len()), removed.join(", ")),
        _ => format!("{many} changed from {} to {}", show(&before), show(&after)),
    }
}

fn regions_sentence(before: &Value, after: &Value) -> String {
    let read = |v: &Value| -> Vec<(String, String)> {
        v.as_list().iter().map(|r| (r.field("id").to_string(), r.field("name").to_string())).collect()
    };
    let (old, new) = (read(before), read(after));
    let desc = |name: &str| if name.is_empty() { "an unnamed region".to_string() } else { format!("region {name}") };
    let mut parts = Vec::new();
    for (id, name) in &new {
        match old.iter().find(|(i, _)| i == id) {
            None => parts.push(format!("{} added", desc(name))),
            Some((_, was)) if was != name => parts.push(match (was.is_empty(), name.is_empty()) {
                (true, _) => format!("an unnamed region named {name}"),
                (_, true) => format!("region {was} unnamed"),
                _ => format!("region {was} renamed to {name}"),
            }),
            _ => {}
        }
    }
    for (id, name) in &old {
        if !new.iter().any(|(i, _)| i == id) {
            parts.push(format!("{} removed", desc(name)));
        }
    }
    let kept_old: Vec<&String> = old.iter().map(|(i, _)| i).filter(|i| new.iter().any(|(j, _)| j == *i)).collect();
    let kept_new: Vec<&String> = new.iter().map(|(i, _)| i).filter(|i| old.iter().any(|(j, _)| j == *i)).collect();
    if kept_old != kept_new {
        parts.push("regions reordered".to_string());
    }
    parts.join(", ")
}

/// A text property's value, as shown in sentences.
pub(crate) fn shown(property: &str, v: &str) -> String {
    match property {
        "guard" | "invariant" | "precondition" | "postcondition" => format!("[{}]", one_line(v)),
        "stereotype" => format!("«{v}»"),
        "text" | "documentation" => format!("\"{}\"", short_text(v, 40)),
        _ => one_line(v),
    }
}

fn text_sentence(property: &str, before: &str, after: &str) -> String {
    let word = match property {
        "do" => "do activity",
        p => p,
    };
    if property == "name" {
        return match (before.is_empty(), after.is_empty()) {
            (true, _) => format!("named {after}"),
            (_, true) => format!("name {before} removed"),
            _ => format!("renamed to {after}"),
        };
    }
    match (before.is_empty(), after.is_empty()) {
        (true, _) => format!("{word} added {}", shown(property, after)),
        (_, true) => format!("{word} removed {}", shown(property, before)),
        _ => format!("{word} changed from {} to {}", shown(property, before), shown(property, after)),
    }
}

pub(crate) fn type_word(v: &Value) -> String {
    let t = v.as_str();
    let all = [
        VertexType::State,
        VertexType::Final,
        VertexType::Initial,
        VertexType::ShallowHistory,
        VertexType::DeepHistory,
        VertexType::Choice,
        VertexType::Junction,
        VertexType::Fork,
        VertexType::Join,
        VertexType::EntryPoint,
        VertexType::ExitPoint,
        VertexType::Terminate,
        VertexType::ConnectionPointRef,
        VertexType::Comment,
    ];
    all.iter().find(|k| k.as_str() == t).map_or(t.to_string(), |k| kind_word(*k).to_lowercase())
}

pub(crate) fn point_shown(v: &Value) -> String {
    match v {
        Value::Object(_) => format!("{} point {}", v.field("kind"), v.field("ref")),
        _ => "none".to_string(),
    }
}

/// `<word> changed from <a> to <b>` for references, the ids added when both are shown alike (a state deleted and
/// drawn again under the same name).
fn reference_sentence(word: &str, (a, a_id): (String, &str), (b, b_id): (String, &str)) -> String {
    if a == b {
        return format!("{word} changed from {a} ({a_id}) to {b} ({b_id})");
    }
    format!("{word} changed from {a} to {b}")
}

/// The sentence for a model property that differs; `old` and `new` name the elements it refers to.
fn sentence(property: &str, before: &Value, after: &Value, old: &Namer, new: &Namer) -> String {
    let texts = |v: &Value| -> Vec<String> { v.as_list().iter().map(|x| x.as_str().to_string()).collect() };
    match property {
        "type" => format!("type changed from {} to {}", type_word(before), type_word(after)),
        "owner" => reference_sentence(
            "owner",
            (old.place(before.as_str()), before.as_str()),
            (new.place(after.as_str()), after.as_str()),
        ),
        "source" | "target" => reference_sentence(
            property,
            (old.short(before.as_str()), before.as_str()),
            (new.short(after.as_str()), after.as_str()),
        ),
        "kind" => format!("kind changed from {} to {}", before.as_str(), after.as_str()),
        "regions" => regions_sentence(before, after),
        "point" => format!("point changed from {} to {}", point_shown(before), point_shown(after)),
        "triggers" | "deferred" => list_sentence(property, texts(before), texts(after)),
        "anchors" => {
            let names =
                |v: &Value, n: &Namer| -> Vec<String> { v.as_list().iter().map(|x| n.anchor(x.as_str())).collect() };
            list_sentence(property, names(before, old), names(after, new))
        }
        _ => text_sentence(property, before.as_str(), after.as_str()),
    }
}

// ------------------------------------------------------------------ comparison

fn differences(
    before: Vec<(&'static str, Value)>,
    after: Vec<(&'static str, Value)>,
) -> Vec<(&'static str, Value, Value)> {
    let mut out = Vec::new();
    for (k, a) in &after {
        let b = before.iter().find(|(p, _)| p == k).map_or(Value::Null, |(_, v)| v.clone());
        if b != *a {
            out.push((*k, b, a.clone()));
        }
    }
    for (k, b) in before {
        if !after.iter().any(|(p, _)| *p == k) {
            out.push((k, b, Value::Null));
        }
    }
    out
}

/// The differences between `old` and `new` (None: a file without a machine, whose elements are all added or removed).
pub fn diff(old: Option<&FsmModel>, new: Option<&FsmModel>) -> Diff {
    let empty = FsmModel {
        id: String::new(),
        name: String::new(),
        kind: crate::model::MachineKind::Behavioral,
        context: String::new(),
        documentation: String::new(),
        regions: Vec::new(),
        vertices: Vec::new(),
        transitions: Vec::new(),
    };
    let (o, n) = (old.unwrap_or(&empty), new.unwrap_or(&empty));
    let (on, nn) = (Namer::new(o), Namer::new(n));

    let mut changed = Vec::new();
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut layout = Vec::new();
    let element = |id: &str, element, status, label, changes: Vec<Change>| ElementDiff {
        id: id.to_string(),
        element,
        status,
        label,
        moved: changes.iter().any(|c| c.sentence.is_none()),
        changes,
    };
    let model_changes = |list: Vec<(&'static str, Value, Value)>| -> Vec<Change> {
        list.into_iter()
            .map(|(property, before, after)| {
                let s = sentence(property, &before, &after, &on, &nn);
                Change { property, before, after, sentence: Some(s) }
            })
            .collect()
    };
    let layout_changes = |list: Vec<(&'static str, Value, Value)>| -> Vec<Change> {
        list.into_iter().map(|(property, before, after)| Change { property, before, after, sentence: None }).collect()
    };

    // The machine.
    match (old.is_some(), new.is_some(), o.id == n.id) {
        (true, true, true) => {
            let changes = model_changes(differences(machine_properties(o), machine_properties(n)));
            if !changes.is_empty() {
                changed.push(element(&n.id, Element::Machine, Status::Changed, on.machine_label(), changes));
            }
        }
        (has_old, has_new, _) => {
            if has_new {
                added.push(element(&n.id, Element::Machine, Status::Added, nn.machine_label(), Vec::new()));
            }
            if has_old {
                removed.push(element(&o.id, Element::Machine, Status::Removed, on.machine_label(), Vec::new()));
            }
        }
    }

    let mut sort = |id: &str, kind: Element, label_old: String, mc: Vec<Change>, lc: Vec<Change>| {
        if !mc.is_empty() {
            let mut all = mc;
            all.extend(lc);
            changed.push(element(id, kind, Status::Changed, label_old, all));
        } else if !lc.is_empty() {
            layout.push(element(id, kind, Status::Layout, label_old, lc));
        }
    };
    let mut vertex_removed = Vec::new();
    for (i, j) in pair(&o.vertices, &n.vertices, |v| &v.id) {
        match (i, j) {
            (Some(i), Some(j)) => {
                let mc =
                    model_changes(differences(vertex_properties(&o.vertices[i]), vertex_properties(&n.vertices[j])));
                let mut layout = differences(vertex_layout(&on.index, i), vertex_layout(&nn.index, j));
                // Regions added or removed are a model change: their layout follows.
                let ids = |v: &Vertex| v.regions.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
                if ids(&o.vertices[i]) != ids(&n.vertices[j]) {
                    layout.retain(|(p, _, _)| !matches!(*p, "regionLayout" | "regionSizes"));
                }
                let lc = layout_changes(layout);
                sort(&n.vertices[j].id, Element::Vertex, on.vertex_label(i), mc, lc);
            }
            (None, Some(j)) => {
                added.push(element(&n.vertices[j].id, Element::Vertex, Status::Added, nn.vertex_label(j), Vec::new()))
            }
            (Some(i), None) => vertex_removed.push(element(
                &o.vertices[i].id,
                Element::Vertex,
                Status::Removed,
                on.vertex_label(i),
                Vec::new(),
            )),
            (None, None) => {}
        }
    }
    removed.extend(vertex_removed);
    for (i, j) in pair(&o.transitions, &n.transitions, |t| &t.id) {
        match (i, j) {
            (Some(i), Some(j)) => {
                let mc = model_changes(differences(transition_properties(o, i), transition_properties(n, j)));
                let lc = layout_changes(differences(transition_layout(&on.index, i), transition_layout(&nn.index, j)));
                sort(&n.transitions[j].id, Element::Transition, on.transition_label(i), mc, lc);
            }
            (None, Some(j)) => added.push(element(
                &n.transitions[j].id,
                Element::Transition,
                Status::Added,
                nn.transition_label(j),
                Vec::new(),
            )),
            (Some(i), None) => removed.push(element(
                &o.transitions[i].id,
                Element::Transition,
                Status::Removed,
                on.transition_label(i),
                Vec::new(),
            )),
            (None, None) => {}
        }
    }
    let mut elements = changed;
    elements.extend(added);
    elements.extend(removed);
    elements.extend(layout);
    Diff { elements }
}

// ------------------------------------------------------------------ output

/// A label inside a line: its first letter in lowercase (`state Playing`).
pub(crate) fn in_line(label: &str) -> String {
    let mut c = label.chars();
    c.next().map(|f| f.to_lowercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

impl Diff {
    /// The text output: `header` (the file), then a line per model change (`~`), added element (`+`) and removed
    /// element (`-`), and one line counting the elements changed in the layout only (`·`).
    pub fn to_text(&self, header: &str) -> String {
        let mut out = format!("{header}\n");
        for e in &self.elements {
            let label = in_line(&e.label);
            match e.status {
                Status::Added => out += &format!("  + {label}\n"),
                Status::Removed => out += &format!("  - {label}\n"),
                Status::Changed => {
                    let also = if e.moved { " (also moved)" } else { "" };
                    for (k, s) in e.changes.iter().filter_map(|c| c.sentence.as_ref()).enumerate() {
                        let sep = if s.starts_with("renamed ") || s.starts_with("named ") || s.starts_with("name ") {
                            " "
                        } else {
                            ": "
                        };
                        out += &format!("  ~ {label}{sep}{s}{}\n", if k == 0 { also } else { "" });
                    }
                }
                Status::Layout => {}
            }
        }
        match self.count(Status::Layout) {
            0 => {}
            1 => out += "  · 1 shape moved (layout only)\n",
            n => out += &format!("  · {n} shapes moved (layout only)\n"),
        }
        if self.elements.is_empty() {
            out += "  no differences\n";
        }
        out
    }

    /// The JSON output: `{ "elements": [...], "counts": {...} }`, pretty-printed, keys in a fixed order.
    pub fn to_json(&self) -> String {
        struct Changes<'a>(&'a [Change]);
        impl Serialize for Changes<'_> {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let mut seq = s.serialize_seq(Some(self.0.len()))?;
                for c in self.0 {
                    seq.serialize_element(&Value::Object(vec![
                        ("property", Value::text(c.property)),
                        ("before", c.before.clone()),
                        ("after", c.after.clone()),
                    ]))?;
                }
                seq.end()
            }
        }
        struct Doc<'a>(&'a Diff);
        impl Serialize for Doc<'_> {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                struct Elements<'a>(&'a [ElementDiff]);
                impl Serialize for Elements<'_> {
                    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                        let mut seq = s.serialize_seq(Some(self.0.len()))?;
                        for e in self.0 {
                            seq.serialize_element(&ElementJson(e))?;
                        }
                        seq.end()
                    }
                }
                struct ElementJson<'a>(&'a ElementDiff);
                impl Serialize for ElementJson<'_> {
                    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                        let e = self.0;
                        let mut map = s.serialize_map(Some(6))?;
                        map.serialize_entry("id", &e.id)?;
                        map.serialize_entry("element", e.element.as_str())?;
                        map.serialize_entry("status", e.status.as_str())?;
                        map.serialize_entry("label", &e.label)?;
                        map.serialize_entry("moved", &e.moved)?;
                        map.serialize_entry("changes", &Changes(&e.changes))?;
                        map.end()
                    }
                }
                let d = self.0;
                let counts = Value::Object(
                    [Status::Added, Status::Removed, Status::Changed, Status::Layout]
                        .iter()
                        .map(|&st| (st.as_str(), Value::Number(d.count(st) as f64)))
                        .collect(),
                );
                let mut map = s.serialize_map(Some(2))?;
                map.serialize_entry("elements", &Elements(&d.elements))?;
                map.serialize_entry("counts", &counts)?;
                map.end()
            }
        }
        serde_json::to_string_pretty(&Doc(self)).unwrap_or_default() + "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xmi::from_xmi;

    /// Lamp: Off and the composite On (Dim and a choice inside), an internal transition, a comment, a diagram.
    const LAMP: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xmi:XMI xmi:version="20131001" xmlns:xmi="http://www.omg.org/spec/XMI/20131001" xmlns:uml="http://www.omg.org/spec/UML/20161101" xmlns:umldi="http://www.omg.org/spec/UML/20161101/UMLDI" xmlns:dc="http://www.omg.org/spec/DD/20100524/DC">
  <uml:Model xmi:id="sm_model" name="Lamp">
    <packagedElement xmi:type="uml:StateMachine" xmi:id="sm" name="Lamp">
      <region xmi:type="uml:Region" xmi:id="r">
        <ownedComment xmi:type="uml:Comment" xmi:id="note" annotatedElement="Off"><body>Starts off</body></ownedComment>
        <subvertex xmi:type="uml:Pseudostate" xmi:id="i" kind="initial"/>
        <subvertex xmi:type="uml:State" xmi:id="Off" name="Off"/>
        <subvertex xmi:type="uml:State" xmi:id="On" name="On">
          <region xmi:type="uml:Region" xmi:id="rOn" name="main">
            <subvertex xmi:type="uml:State" xmi:id="Dim" name="Dim"/>
            <subvertex xmi:type="uml:Pseudostate" xmi:id="c" kind="choice"/>
            <transition xmi:type="uml:Transition" xmi:id="t3" source="Dim" target="c"><trigger xmi:id="t3_g" name="tick"/></transition>
            <transition xmi:type="uml:Transition" xmi:id="t4" source="c" target="Dim"/>
          </region>
        </subvertex>
        <transition xmi:type="uml:Transition" xmi:id="t0" source="i" target="Off"/>
        <transition xmi:type="uml:Transition" xmi:id="t1" source="Off" target="On"><trigger xmi:id="t1_g" name="press"/></transition>
        <transition xmi:type="uml:Transition" xmi:id="t2" kind="internal" source="On" target="On"><trigger xmi:id="t2_g" name="dim"/></transition>
      </region>
    </packagedElement>
  </uml:Model>
  <umldi:UMLStateMachineDiagram xmi:id="d" modelElement="sm">
    <ownedElement xmi:type="umldi:UMLShape" xmi:id="di_note" modelElement="note"><bounds x="10" y="200" width="160" height="64"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLShape" xmi:id="di_i" modelElement="i"><bounds x="20" y="40" width="20" height="20"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLShape" xmi:id="di_Off" modelElement="Off"><bounds x="80" y="20" width="140" height="60"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLShape" xmi:id="di_On" modelElement="On"><bounds x="300" y="20" width="300" height="200"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLShape" xmi:id="di_rOn" modelElement="rOn"><bounds x="300" y="50" width="300" height="170"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLShape" xmi:id="di_Dim" modelElement="Dim"><bounds x="320" y="80" width="100" height="40"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLShape" xmi:id="di_c" modelElement="c"><bounds x="500" y="100" width="28" height="28"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLEdge" xmi:id="di_t1" modelElement="t1"><waypoint x="150" y="50"/><waypoint x="260" y="50"/><waypoint x="450" y="120"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLEdge" xmi:id="di_t3" modelElement="t3"><waypoint x="370" y="100"/><waypoint x="450" y="150"/><waypoint x="514" y="114"/></ownedElement>
  </umldi:UMLStateMachineDiagram>
</xmi:XMI>
"#;

    /// LAMP with each `(from, to)` replaced (each `from` must be there).
    fn lamp(edits: &[(&str, &str)]) -> FsmModel {
        let mut text = LAMP.to_string();
        for (a, b) in edits {
            assert!(text.contains(a), "{a}");
            text = text.replacen(a, b, 1);
        }
        from_xmi(&text).unwrap()
    }

    fn compare(edits: &[(&str, &str)]) -> Diff {
        diff(Some(&lamp(&[])), Some(&lamp(edits)))
    }

    fn text(edits: &[(&str, &str)]) -> String {
        compare(edits).to_text("Lamp.fsm")
    }

    /// The text output's lines, without the header.
    fn lines(edits: &[(&str, &str)]) -> Vec<String> {
        text(edits).lines().skip(1).map(|l| l.trim().to_string()).collect()
    }

    const OFF: &str = r#"<subvertex xmi:type="uml:State" xmi:id="Off" name="Off"/>"#;
    const T1: &str = r#"<trigger xmi:id="t1_g" name="press"/></transition>"#;

    #[test]
    fn identical_files() {
        let d = compare(&[]);
        assert!(d.elements.is_empty());
        assert_eq!(d.to_text("Lamp.fsm"), "Lamp.fsm\n  no differences\n");
    }

    #[test]
    fn each_status() {
        let d = compare(&[
            (r#"name="Off"/>"#, r#"name="Dark"/>"#),
            (r#"kind="initial"/>"#, r#"kind="initial"/><subvertex xmi:type="uml:State" xmi:id="Hot" name="Hot"/>"#),
            (r#"<transition xmi:type="uml:Transition" xmi:id="t4" source="c" target="Dim"/>"#, ""),
            (r#"<bounds x="500" y="100""#, r#"<bounds x="510" y="100""#),
        ]);
        let got: Vec<(&str, &str, &str, bool)> =
            d.elements.iter().map(|e| (e.status.as_str(), e.id.as_str(), e.label.as_str(), e.moved)).collect();
        assert_eq!(
            got,
            [
                ("changed", "Off", "State Off", false),
                ("added", "Hot", "State Hot", false),
                ("removed", "t4", "Transition Choice → Dim", false),
                ("layout", "c", "Choice from Dim to Dim in On", true),
            ]
        );
        assert_eq!((d.count(Status::Added), d.count(Status::Removed)), (1, 1));
        assert_eq!(
            d.to_text("Lamp.fsm"),
            "Lamp.fsm\n  ~ state Off renamed to Dark\n  + state Hot\n  - transition Choice → Dim\n  · 1 shape moved (layout only)\n"
        );
        let c = &d.elements[0].changes[0];
        assert_eq!((c.property, &c.before, &c.after), ("name", &Value::text("Off"), &Value::text("Dark")));
    }

    #[test]
    fn sentences_for_texts() {
        let state = |inner: &str| {
            (OFF, format!(r#"<subvertex xmi:type="uml:State" xmi:id="Off" name="Off">{inner}</subvertex>"#))
        };
        let entry = |b: &str| format!(r#"<entry xmi:type="uml:OpaqueBehavior" xmi:id="e"><body>{b}</body></entry>"#);
        let (a, b) = state(&entry("a()"));
        let old = lamp(&[(a, &b)]);
        let (a, b) = state(&entry("a(); b()"));
        let new = lamp(&[(a, &b)]);
        assert_eq!(
            diff(Some(&old), Some(&new)).to_text("x").lines().nth(1),
            Some("  ~ state Off: entry changed from a() to a(); b()")
        );
        let (a, b) = state(
            &(entry("go()")
                + r#"<doActivity xmi:id="d"><body>spin()</body></doActivity><stateInvariant xmi:id="v"><specification><body>ok()</body></specification></stateInvariant>"#),
        );
        assert_eq!(
            lines(&[(a, &b)]),
            [
                "~ state Off: entry added go()",
                "~ state Off: do activity added spin()",
                "~ state Off: invariant added [ok()]"
            ]
        );
        assert_eq!(lines(&[(r#"name="Off"/>"#, r#"name=""/>"#)]), ["~ state Off name Off removed"]);
        assert_eq!(
            lines(&[(r#"xmi:id="sm" name="Lamp">"#, r#"xmi:id="sm" name="Light">"#)]),
            ["~ machine Lamp renamed to Light"]
        );
        let guard =
            |g: &str| format!(r#"<guard xmi:id="g"><specification><body>{g}</body></specification></guard>{T1}"#);
        assert_eq!(lines(&[(T1, &guard("canPress()"))]), ["~ transition Off → On on press: guard added [canPress()]"]);
        let old = lamp(&[(T1, &guard("a()"))]);
        assert_eq!(
            diff(Some(&old), Some(&lamp(&[]))).to_text("x").lines().nth(1),
            Some("  ~ transition Off → On on press: guard removed [a()]")
        );
        let effect =
            r#"<trigger xmi:id="t1_g" name="press"/><effect xmi:id="f"><body>light()</body></effect></transition>"#;
        assert_eq!(lines(&[(T1, effect)]), ["~ transition Off → On on press: effect added light()"]);
    }

    #[test]
    fn sentences_for_lists_and_references() {
        let two = r#"<trigger xmi:id="t1_g" name="press"/><trigger xmi:id="t1_h" name="clap"/></transition>"#;
        assert_eq!(lines(&[(T1, two)]), ["~ transition Off → On on press: trigger clap added"]);
        assert_eq!(
            lines(&[(T1, r#"<trigger xmi:id="t1_g" name="push"/></transition>"#)]),
            ["~ transition Off → On on press: triggers changed from press to push"]
        );
        assert_eq!(lines(&[(T1, "</transition>")]), ["~ transition Off → On on press: trigger press removed"]);
        let deferred = r#"<subvertex xmi:type="uml:State" xmi:id="Off" name="Off"><deferrableTrigger xmi:id="dt" name="press"/></subvertex>"#;
        assert_eq!(lines(&[(OFF, deferred)]), ["~ state Off: deferred event press added"]);
        assert_eq!(
            lines(&[(r#"source="Off" target="On""#, r#"source="Off" target="Dim""#)]),
            ["~ transition Off → On on press: target changed from On to Dim"]
        );
        assert_eq!(
            lines(&[(r#"kind="internal" source="On""#, r#"kind="local" source="On""#)]),
            ["~ internal transition in On on dim: kind changed from internal to local"]
        );
        // A vertex moved into another region; a comment attached elsewhere, and its text.
        let moved = lamp(&[]);
        let mut new = moved.clone();
        new.vertices.iter_mut().find(|v| v.id == "Off").unwrap().parent = "rOn".to_string();
        let note = new.vertices.iter_mut().find(|v| v.id == "note").unwrap();
        note.anchors = vec!["t1".to_string(), "Dim".to_string()];
        note.text = "Starts dark".to_string();
        let got = diff(Some(&moved), Some(&new)).to_text("x");
        // Its position is now relative to On.
        assert!(got.contains("~ state Off: owner changed from the machine to On (also moved)\n"), "{got}");
        assert!(
            got.contains("~ comment \"Starts off\": text changed from \"Starts off\" to \"Starts dark\"\n"),
            "{got}"
        );
        assert!(
            got.contains("~ comment \"Starts off\": anchors changed from Off to transition Off → On on press, Dim\n"),
            "{got}"
        );
    }

    #[test]
    fn sentences_for_types_regions_and_points() {
        assert_eq!(
            lines(&[(r#"kind="choice""#, r#"kind="junction""#)]),
            ["~ choice from Dim to Dim in On: type changed from choice to junction"]
        );
        assert_eq!(
            lines(&[(r#"xmi:id="rOn" name="main""#, r#"xmi:id="rOn" name="core""#)]),
            ["~ state On: region main renamed to core"]
        );
        let second = r#"</region>
        </subvertex>
        <transition xmi:type="uml:Transition" xmi:id="t0""#;
        let with_second = r#"</region><region xmi:type="uml:Region" xmi:id="rOn2" name="aux"/>
        </subvertex>
        <transition xmi:type="uml:Transition" xmi:id="t0""#;
        assert_eq!(lines(&[(second, with_second)]), ["~ state On: region aux added"]);
        let sub = r#"<subvertex xmi:type="uml:State" xmi:id="Off" name="Off"><submachine href="Dark.fsm#sm"/><connection xmi:id="cp" ><entry href="Dark.fsm#start"/></connection></subvertex>"#;
        let old = lamp(&[(OFF, sub)]);
        let new = lamp(&[(
            OFF,
            &sub.replace("<entry href=\"Dark.fsm#start\"/>", "<exit href=\"Dark.fsm#done\"/>")
                .replace("Dark.fsm#sm", "Night.fsm#sm"),
        )]);
        assert_eq!(
            diff(Some(&old), Some(&new)).to_text("x").lines().skip(1).collect::<Vec<_>>(),
            [
                "  ~ state Off: submachine changed from Dark.fsm#sm to Night.fsm#sm",
                "  ~ entry point reference start on Off: point changed from entry point start to exit point done",
            ]
        );
        let profiled = |s: &str| {
            format!(
                r#"<FsmStereotypes:{s} xmlns:FsmStereotypes="https://github.com/vincedupuis/fsm-editor/profiles/FsmStereotypes" base_State="Off"/></xmi:XMI>"#
            )
        };
        let old = lamp(&[("</xmi:XMI>", &profiled("device"))]);
        assert_eq!(
            diff(Some(&old), Some(&lamp(&[("</xmi:XMI>", &profiled("sensor"))]))).to_text("x").lines().nth(1),
            Some("  ~ state Off: stereotype changed from «device» to «sensor»")
        );
    }

    #[test]
    fn changed_wins_over_layout() {
        let d = compare(&[
            (r#"name="Off"/>"#, r#"name="Dark"/>"#),
            (r#"<bounds x="80" y="20""#, r#"<bounds x="90" y="20""#),
        ]);
        let e = &d.elements[0];
        assert_eq!((e.status, e.moved), (Status::Changed, true));
        let props: Vec<&str> = e.changes.iter().map(|c| c.property).collect();
        assert_eq!(props, ["name", "position"]);
        assert_eq!(d.to_text("x"), "x\n  ~ state Off renamed to Dark (also moved)\n  · 1 shape moved (layout only)\n");
        // The source of t1 moved without its bend point: t1's route changed.
        assert_eq!((d.elements[1].id.as_str(), d.elements[1].status), ("t1", Status::Layout));
    }

    #[test]
    fn positions_are_relative_to_the_owner_and_bend_points_to_the_source() {
        // On dragged by (30, 10) with what it holds and the bend points of t3, inside it: only On moved.
        let d = compare(&[
            (r#"<bounds x="300" y="20""#, r#"<bounds x="330" y="30""#),
            (r#"<bounds x="300" y="50""#, r#"<bounds x="330" y="60""#),
            (r#"<bounds x="320" y="80""#, r#"<bounds x="350" y="90""#),
            (r#"<bounds x="500" y="100""#, r#"<bounds x="530" y="110""#),
            (r#"<waypoint x="450" y="150"/>"#, r#"<waypoint x="480" y="160"/>"#),
        ]);
        let ids: Vec<&str> = d.elements.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["On"]);
        let c = &d.elements[0].changes[0];
        assert_eq!(c.before, Value::point(300.0, 20.0));
        assert_eq!(c.after, Value::point(330.0, 30.0));
        // Off moved with t1's bend point: Off only.
        let d = compare(&[
            (r#"<bounds x="80" y="20""#, r#"<bounds x="70" y="20""#),
            (r#"<waypoint x="260" y="50"/>"#, r#"<waypoint x="250" y="50"/>"#),
        ]);
        assert_eq!(d.elements.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["Off"]);
        // A bend point moved alone.
        let d = compare(&[(r#"<waypoint x="260" y="50"/>"#, r#"<waypoint x="260" y="60"/>"#)]);
        assert_eq!(d.elements.iter().map(|e| (e.id.as_str(), e.status)).collect::<Vec<_>>(), [("t1", Status::Layout)]);
    }

    #[test]
    fn differences_below_a_tenth_of_a_pixel_are_ignored() {
        assert!(compare(&[(r#"<bounds x="10" y="200""#, r#"<bounds x="10.04" y="199.96""#)]).elements.is_empty());
        assert_eq!(compare(&[(r#"<bounds x="10" y="200""#, r#"<bounds x="10.1" y="200""#)]).elements.len(), 1);
    }

    #[test]
    fn drawing_properties() {
        let ext = r##"<xmi:Extension extender="FSM Editor"><style element="Off" color="#AA0000" entry="#00aa00"/><style element="t1" color="#0000aa"/><group id="g" members="Off i"/></xmi:Extension></umldi:UMLStateMachineDiagram>"##;
        let label = r#"<waypoint x="450" y="120"/><ownedElement xmi:type="umldi:UMLLabel" xmi:id="l" modelElement="t1"><bounds x="200" y="60" width="100" height="16"/></ownedElement>"#;
        let d = compare(&[
            ("</umldi:UMLStateMachineDiagram>", ext),
            (r#"<waypoint x="450" y="120"/>"#, label),
            (
                r#"<bounds x="300" y="50" width="300" height="170"/>"#,
                r#"<bounds x="300" y="50" width="300" height="150"/>"#,
            ),
        ]);
        let got: Vec<(&str, Vec<&str>)> =
            d.elements.iter().map(|e| (e.id.as_str(), e.changes.iter().map(|c| c.property).collect())).collect();
        assert_eq!(
            got,
            [
                ("i", vec!["group"]),
                ("Off", vec!["color", "itemColors", "group"]),
                ("On", vec!["regionSizes"]),
                ("t1", vec!["labelOffset", "color"]),
            ]
        );
        assert!(d.elements.iter().all(|e| e.status == Status::Layout && e.moved));
        let off = &d.elements[1].changes;
        assert_eq!(off[0].after, Value::text("#aa0000"));
        assert_eq!(off[1].after, Value::Object(vec![("entry", Value::text("#00aa00"))]));
        // The label's center (250, 68), from the middle of the line (303.4, 66).
        assert_eq!(d.elements[3].changes[0].after, Value::point(-53.4, 2.0));
    }

    #[test]
    fn internal_transitions_are_elements_of_their_own() {
        let d = compare(&[(
            r#"<trigger xmi:id="t2_g" name="dim"/>"#,
            r#"<trigger xmi:id="t2_g" name="dim"/><effect xmi:id="e2"><body>lower()</body></effect>"#,
        )]);
        assert_eq!(d.elements.len(), 1);
        assert_eq!((d.elements[0].id.as_str(), d.elements[0].element), ("t2", Element::Transition));
        assert_eq!(d.to_text("x").lines().nth(1), Some("  ~ internal transition in On on dim: effect added lower()"));
    }

    #[test]
    fn an_element_drawn_again_is_removed_and_added() {
        let d = compare(&[
            (r#"xmi:id="Dim" name="Dim""#, r#"xmi:id="Dim2" name="Dim""#),
            (r#"modelElement="Dim""#, r#"modelElement="Dim2""#),
            (r#"source="Dim""#, r#"source="Dim2""#),
            (r#"target="Dim""#, r#"target="Dim2""#),
        ]);
        let got: Vec<(&str, &str)> = d.elements.iter().map(|e| (e.status.as_str(), e.id.as_str())).collect();
        // t3 and t4 lost their end: changed, with ids to tell the two Dims apart.
        assert_eq!(got, [("changed", "t3"), ("changed", "t4"), ("added", "Dim2"), ("removed", "Dim")]);
        assert_eq!(
            d.to_text("x").lines().nth(1),
            Some("  ~ transition Dim → Choice on tick: source changed from Dim (Dim) to Dim (Dim2)")
        );
    }

    #[test]
    fn missing_side_and_unrelated_machines() {
        let lamp = lamp(&[]);
        let d = diff(None, Some(&lamp));
        assert_eq!(d.elements.len(), 1 + lamp.vertices.len() + lamp.transitions.len());
        assert!(d.elements.iter().all(|e| e.status == Status::Added && e.changes.is_empty()));
        assert_eq!((d.elements[0].element, d.elements[0].label.as_str()), (Element::Machine, "Machine Lamp"));
        let d = diff(Some(&lamp), None);
        assert_eq!(d.count(Status::Removed), d.elements.len());
        assert!(diff(None, None).elements.is_empty());
        // Another machine id: the machine is removed and added, as any element would be.
        let mut other = lamp.clone();
        other.id = "sm2".to_string();
        let d = diff(Some(&lamp), Some(&other));
        let got: Vec<(&str, Element, &str)> =
            d.elements.iter().map(|e| (e.status.as_str(), e.element, e.id.as_str())).collect();
        assert_eq!(got, [("added", Element::Machine, "sm2"), ("removed", Element::Machine, "sm")]);
    }

    #[test]
    fn repeated_ids_are_paired_in_order() {
        assert_eq!(
            pair(&["a", "b", "a"], &["a", "a", "c"], |s| s),
            [(Some(0), Some(0)), (Some(2), Some(1)), (None, Some(2)), (Some(1), None)]
        );
    }

    #[test]
    fn labels_of_unnamed_elements() {
        let m = lamp(&[]);
        let n = Namer::new(&m);
        let label = |id: &str| n.vertex_label(m.vertices.iter().position(|v| v.id == id).unwrap());
        assert_eq!(label("i"), "Initial to Off");
        assert_eq!(label("c"), "Choice from Dim to Dim in On");
        assert_eq!(label("note"), "Comment \"Starts off\"");
        assert_eq!(n.transition_label(0), "Transition Dim → Choice on tick");
        assert_eq!(n.transition_label(2), "Transition Initial → Off");
        assert_eq!(n.transition_label(4), "Internal transition in On on dim");
        assert_eq!(short_text("a long comment\nspread over lines", 20), "a long comment sprea…");
    }

    #[test]
    fn json_document() {
        let d = compare(&[
            (r#"name="Off"/>"#, r#"name="Dark"/>"#),
            (r#"<bounds x="500" y="100""#, r#"<bounds x="510.5" y="100""#),
        ]);
        let expected = r#"{
  "elements": [
    {
      "id": "Off",
      "element": "vertex",
      "status": "changed",
      "label": "State Off",
      "moved": false,
      "changes": [
        {
          "property": "name",
          "before": "Off",
          "after": "Dark"
        }
      ]
    },
    {
      "id": "c",
      "element": "vertex",
      "status": "layout",
      "label": "Choice from Dim to Dim in On",
      "moved": true,
      "changes": [
        {
          "property": "position",
          "before": {
            "x": 200,
            "y": 80
          },
          "after": {
            "x": 210.5,
            "y": 80
          }
        }
      ]
    }
  ],
  "counts": {
    "added": 0,
    "removed": 0,
    "changed": 1,
    "layout": 1
  }
}
"#;
        assert_eq!(d.to_json(), expected);
    }
}
