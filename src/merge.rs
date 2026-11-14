//! `fsm merge`: a three-way merge of state machines, by xmi:id, property by property.
//!
//! Elements (the machine, its regions, vertices and transitions) are matched by id across the common ancestor
//! (`base`), `ours` and `theirs`, the k-th element with an id on one side going with the k-th on the others, as
//! `diff::pair` does. Their model and drawing properties are the named values of `diff` (`*_properties`, `*_layout`).
//!
//! - A property changed on one side only, or the same way on both, is taken. Lists of triggers, deferred events and
//!   comment anchors changed on both sides become the union (ours' list without what theirs removed, then what theirs
//!   added); regions are merged the same way, by id.
//! - Elements added on both sides are all kept: the order of the side that changed it (ours' when both did), then the
//!   other side's additions, so in each region the base order, then ours' additions, then theirs'.
//! - Layout never conflicts: when both sides changed a drawing property, ours' value is kept and reported. Positions
//!   are merged relative to the owning state and bend points relative to the source, then made absolute again from
//!   the merged owner: a child moved on one side inside a state moved on the other lands inside the moved state.
//!   Bend points that only one side moved on the canvas are taken as that side has them: dragging a state leaves the
//!   bend points of its transitions to and from other vertices where they are, so a side that only moved a state has
//!   not changed them (as in `fsm diff`, they changed when they moved both on the canvas and relative to the source).
//! - Conflicts keep ours' value: a model property set to two different values; an element deleted on one side and
//!   changed, or given new contents or transitions, on the other (kept deleted when ours deleted it, kept with what it
//!   holds when theirs did); two states moved into each other (ours' owners are kept).
//! - A deleted element takes its contents and its transitions with it, and comments stop pointing to it.
//!
//! The result is written as the editors write it (`writer::to_xmi`), read back and validated: the issues that neither
//! `ours` nor `theirs` has on its own are reported as caused by the merge. The report's wording is documented in
//! README.md ("fsm merge") and reuses the labels of `fsm diff`.

use std::collections::{HashMap, HashSet};

use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

use crate::diff::{
    Element, Namer, Value, colors, diff, in_line, machine_properties, point_shown, shown, transition_properties,
    type_word, vertex_properties,
};
use crate::model::{FsmModel, MachineKind, ModelIndex, Region, RegionLayout, Transition, Vertex};
use crate::validation::validate;
use crate::writer::{document_order, to_xmi};
use crate::xmi::from_xmi;

/// The two sides of a merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Ours,
    Theirs,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Ours => "ours",
            Side::Theirs => "theirs",
        }
    }

    fn other(self) -> Side {
        match self {
            Side::Ours => Side::Theirs,
            Side::Theirs => Side::Ours,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// A model property set to two different values.
    Property,
    /// An element deleted on one side and changed, or given new contents or transitions, on the other.
    Deleted,
    /// Two states moved into each other.
    Cycle,
}

impl ConflictKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ConflictKind::Property => "property",
            ConflictKind::Deleted => "deleted",
            ConflictKind::Cycle => "cycle",
        }
    }
}

/// A conflict: what the merged file holds is ours' version.
#[derive(Debug, Clone)]
pub struct Conflict {
    pub id: String,
    pub element: Element,
    /// The element as named in the base (in ours or theirs for an element they added).
    pub label: String,
    pub kind: ConflictKind,
    /// The property (Property and Cycle), as `fsm diff` names it.
    pub property: Option<&'static str>,
    /// Its raw values, as `fsm diff` gives them (Null where the side has no such element).
    pub base: Value,
    pub ours: Value,
    pub theirs: Value,
    /// The values as the report shows them.
    pub ours_shown: String,
    pub theirs_shown: String,
    /// Deleted: the side that deleted it, and what the other side did to it (sentences).
    pub deleted_on: Option<Side>,
    /// Deleted: a region of the element (`region main`) rather than the element itself.
    pub region: Option<String>,
    pub changes: Vec<String>,
    /// Whether the element is in the merged file.
    pub kept: bool,
}

/// A drawing property changed on both sides: ours' value is kept.
#[derive(Debug, Clone)]
pub struct LayoutNote {
    pub id: String,
    pub element: Element,
    pub label: String,
    pub properties: Vec<&'static str>,
}

/// An element changed on one side or both and merged without a conflict.
#[derive(Debug, Clone)]
pub struct Merged {
    pub id: String,
    pub element: Element,
    /// Its status on the side that changed it (ours' when both did), as `fsm diff` gives it.
    pub status: &'static str,
    pub label: String,
    pub from: &'static str,
}

/// A validation issue of the merged machine that neither `ours` nor `theirs` has.
#[derive(Debug, Clone)]
pub struct NewIssue {
    pub id: String,
    pub label: String,
    pub severity: &'static str,
    pub message: String,
}

/// The result of a merge.
#[derive(Debug, Clone)]
pub struct Merge {
    /// The merged file, as the editors write it.
    pub text: String,
    pub merged: Vec<Merged>,
    pub conflicts: Vec<Conflict>,
    pub layout: Vec<LayoutNote>,
    pub issues: Vec<NewIssue>,
}

// ------------------------------------------------------------------ the three versions

/// An element's identity: its id and how many elements before it have that id.
type Key = (String, usize);

fn keys<'a, T>(list: &'a [T], id: impl Fn(&'a T) -> &'a str) -> Vec<Key> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    list.iter()
        .map(|e| {
            let n = seen.entry(id(e)).or_default();
            *n += 1;
            (id(e).to_string(), *n - 1)
        })
        .collect()
}

/// Which version an element's value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Src {
    Base,
    Ours,
    Theirs,
}

struct Version<'a> {
    m: &'a FsmModel,
    namer: Namer<'a>,
    vkeys: Vec<Key>,
    vat: HashMap<Key, usize>,
    tkeys: Vec<Key>,
    tat: HashMap<Key, usize>,
    /// Every region id, of the machine and of its states.
    regions: HashSet<&'a str>,
    /// Vertex and transition ids.
    alive: HashSet<&'a str>,
}

impl<'a> Version<'a> {
    fn new(m: &'a FsmModel) -> Version<'a> {
        let vkeys = keys(&m.vertices, |v| &v.id);
        let tkeys = keys(&m.transitions, |t| &t.id);
        Version {
            m,
            namer: Namer::new(m),
            vat: vkeys.iter().cloned().enumerate().map(|(i, k)| (k, i)).collect(),
            tat: tkeys.iter().cloned().enumerate().map(|(i, k)| (k, i)).collect(),
            vkeys,
            tkeys,
            regions: m
                .regions
                .iter()
                .chain(m.vertices.iter().flat_map(|v| &v.regions))
                .map(|r| r.id.as_str())
                .collect(),
            alive: m
                .vertices
                .iter()
                .map(|v| v.id.as_str())
                .chain(m.transitions.iter().map(|t| t.id.as_str()))
                .collect(),
        }
    }

    fn ix(&self) -> &ModelIndex<'a> {
        &self.namer.index
    }

    fn has_region_or_vertex(&self, id: &str) -> bool {
        self.regions.contains(id) || self.ix().has_vertex(id)
    }

    /// The top-left corner of the state owning vertex `i` (None at the top level).
    fn owner_at(&self, i: usize) -> Option<(f64, f64)> {
        self.ix().owner_state(i).map(|o| (self.m.vertices[o].x, self.m.vertices[o].y))
    }

    fn vertex_label(&self, i: usize) -> String {
        self.namer.vertex_label(i)
    }

    fn transition_label(&self, i: usize) -> String {
        self.namer.transition_label(i)
    }
}

/// A node of the containment tree: a vertex, or a region.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Node {
    V(Key),
    R(String),
}

/// Which side a value is taken from, and whether both sides changed it differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    Ours,
    Theirs,
    Conflict,
}

/// The three-way choice for a value: ours' when both agree or only ours changed it, theirs' when only theirs did.
fn decide<T: PartialEq>(b: Option<&T>, o: Option<&T>, t: Option<&T>) -> Pick {
    match (o, t) {
        (Some(o), Some(t)) if o == t => Pick::Ours,
        (Some(o), Some(_)) if b == Some(o) => Pick::Theirs,
        (Some(_), Some(t)) if b == Some(t) => Pick::Ours,
        (Some(_), Some(_)) => Pick::Conflict,
        (None, Some(_)) => Pick::Theirs,
        _ => Pick::Ours,
    }
}

/// The choice for a drawing property: on the exact values (so that a value only one side changed is taken as it is),
/// and when both sides changed it, a conflict only if their values differ once rounded to 0.1 as files store them.
fn decide_layout(exact: [Option<&Value>; 3], rounded: [Option<&Value>; 3]) -> Pick {
    match decide(exact[0], exact[1], exact[2]) {
        Pick::Conflict => decide(rounded[0], rounded[1], rounded[2]),
        p => p,
    }
}

fn num(n: f64, round: bool) -> Value {
    if round { Value::coordinate(n) } else { Value::Number(n) }
}

/// A drawing property of one version's element, exact or rounded to 0.1.
type LayoutValue<'f> = &'f dyn Fn(&Merger, (Src, usize), bool) -> Value;

fn pt(x: f64, y: f64, round: bool) -> Value {
    Value::Object(vec![("x", num(x, round)), ("y", num(y, round))])
}

/// Lists merged as sets: one side's list when only it changed, else ours' without what theirs removed, then what
/// theirs added.
fn merge_list(b: Option<&[String]>, o: &[String], t: &[String]) -> Vec<String> {
    if o == t || b == Some(t) {
        return o.to_vec();
    }
    if b == Some(o) {
        return t.to_vec();
    }
    let b = b.unwrap_or(&[]);
    let mut out: Vec<String> = o.iter().filter(|x| !(b.contains(x) && !t.contains(x))).cloned().collect();
    for x in t {
        if !b.contains(x) && !out.contains(x) {
            out.push(x.clone());
        }
    }
    out
}

fn prop<'v>(list: &'v [(&'static str, Value)], name: &str) -> Option<&'v Value> {
    list.iter().find(|(k, _)| *k == name).map(|(_, v)| v)
}

fn property_word(p: &str) -> &str {
    match p {
        "do" => "do activity",
        "deferred" => "deferred events",
        p => p,
    }
}

/// A merged vertex: its model, and the versions holding it (for its layout and its references).
struct MergedVertex {
    v: Vertex,
    b: Option<usize>,
    o: Option<(Src, usize)>,
    t: Option<(Src, usize)>,
    /// Where its position comes from: relative to its owner there.
    pos: (Src, usize),
}

struct MergedTransition {
    t: Transition,
    b: Option<usize>,
    o: Option<(Src, usize)>,
    tt: Option<(Src, usize)>,
    /// Where its bend points come from: relative to its source there, or as they are when `absolute`.
    points: (Src, usize),
    absolute: bool,
}

struct Merger<'a> {
    base: Option<Version<'a>>,
    empty: Version<'a>,
    ours: Version<'a>,
    theirs: Version<'a>,
    conflicts: Vec<Conflict>,
    layout: Vec<LayoutNote>,
    /// Elements deleted on theirs and kept, because ours changed them or added to them.
    keep_nodes: HashSet<Node>,
    keep_transitions: HashSet<Key>,
}

impl<'a> Merger<'a> {
    fn b(&self) -> &Version<'a> {
        self.base.as_ref().unwrap_or(&self.empty)
    }

    fn version(&self, s: Src) -> &Version<'a> {
        match s {
            Src::Base => self.b(),
            Src::Ours => &self.ours,
            Src::Theirs => &self.theirs,
        }
    }

    fn side(&self, s: Side) -> &Version<'a> {
        match s {
            Side::Ours => &self.ours,
            Side::Theirs => &self.theirs,
        }
    }

    fn vertex(&self, (s, i): (Src, usize)) -> &'a Vertex {
        &self.version(s).m.vertices[i]
    }

    fn transition(&self, (s, i): (Src, usize)) -> &'a Transition {
        &self.version(s).m.transitions[i]
    }

    // -------------------------------------------------------------- deletions

    fn base_parent(&self, n: &Node) -> Option<Node> {
        let b = self.b();
        match n {
            Node::V(k) => {
                let i = b.vat[k];
                let v = &b.m.vertices[i];
                if b.ix().is_border(i) {
                    b.ix().vertex(&v.parent).map(|j| Node::V(b.vkeys[j].clone()))
                } else if b.regions.contains(v.parent.as_str()) {
                    Some(Node::R(v.parent.clone()))
                } else {
                    None
                }
            }
            Node::R(r) => b.ix().region_owner(r).flatten().map(|j| Node::V(b.vkeys[j].clone())),
        }
    }

    fn deleted_on(&self, n: &Node, d: Side) -> bool {
        let v = self.side(d);
        match n {
            Node::V(k) => !v.vat.contains_key(k),
            Node::R(r) => !v.regions.contains(r.as_str()),
        }
    }

    /// The elements each side deleted, grouped under the topmost one (a deleted state with its contents, a deleted
    /// region with its vertices, and their transitions), checked against what the other side did there.
    fn deletions(&mut self) {
        let Some(b) = &self.base else { return };
        let mut nodes: Vec<Node> = b.vkeys.iter().cloned().map(Node::V).collect();
        let mut seen_regions = HashSet::new();
        for r in b.m.regions.iter().chain(b.m.vertices.iter().flat_map(|v| &v.regions)) {
            if seen_regions.insert(r.id.clone()) {
                nodes.push(Node::R(r.id.clone()));
            }
        }
        for d in [Side::Ours, Side::Theirs] {
            let deleted: Vec<&Node> = nodes.iter().filter(|n| self.deleted_on(n, d)).collect();
            // The topmost deleted ancestor of each deleted node.
            let mut roots: Vec<Node> = Vec::new();
            let mut members: HashMap<Node, Vec<Node>> = HashMap::new();
            for n in deleted {
                let mut root = n.clone();
                let mut seen = HashSet::new();
                while let Some(p) = self.base_parent(&root) {
                    if !self.deleted_on(&p, d) || !seen.insert(p.clone()) {
                        break;
                    }
                    root = p;
                }
                if !members.contains_key(&root) {
                    roots.push(root.clone());
                }
                members.entry(root).or_default().push(n.clone());
            }
            let mut grouped: HashSet<Key> = HashSet::new();
            for root in roots {
                let subtree = &members[&root];
                let transitions = self.attached_transitions(subtree, d);
                grouped.extend(transitions.iter().cloned());
                self.check_deletion(&root, subtree, &transitions, d);
            }
            // Transitions deleted on their own.
            let b = self.b();
            let lone: Vec<Key> = b
                .tkeys
                .iter()
                .filter(|k| !self.side(d).tat.contains_key(*k) && !grouped.contains(*k))
                .cloned()
                .collect();
            for k in lone {
                self.check_transition_deletion(&k, d);
            }
        }
    }

    /// Base transitions deleted on `d` that touch a vertex of `subtree`.
    fn attached_transitions(&self, subtree: &[Node], d: Side) -> Vec<Key> {
        let b = self.b();
        let ids: HashSet<&str> = subtree
            .iter()
            .filter_map(|n| match n {
                Node::V(k) => Some(k.0.as_str()),
                Node::R(_) => None,
            })
            .collect();
        b.tkeys
            .iter()
            .enumerate()
            .filter(|(i, k)| {
                let t = &b.m.transitions[*i];
                !self.side(d).tat.contains_key(*k)
                    && (ids.contains(t.source.as_str()) || ids.contains(t.target.as_str()))
            })
            .map(|(_, k)| k.clone())
            .collect()
    }

    fn check_deletion(&mut self, root: &Node, subtree: &[Node], transitions: &[Key], d: Side) {
        let b = self.b();
        let o = self.side(d.other());
        let mut changes = Vec::new();
        for n in subtree {
            let Node::V(k) = n else { continue };
            let Some(&j) = o.vat.get(k) else { continue };
            let bp = vertex_properties(&b.m.vertices[b.vat[k]]);
            let op = vertex_properties(&o.m.vertices[j]);
            let differing: Vec<&str> = op.iter().filter(|(p, v)| prop(&bp, p) != Some(v)).map(|(p, _)| *p).collect();
            if differing.is_empty() {
                continue;
            }
            if n == root {
                changes.extend(differing.iter().map(|p| format!("{} changed", property_word(p))));
            } else {
                changes.push(format!("{} changed", in_line(&b.vertex_label(b.vat[k]))));
            }
        }
        for k in transitions {
            let Some(&j) = o.tat.get(k) else { continue };
            if transition_properties(b.m, b.tat[k]) != transition_properties(o.m, j) {
                changes.push(format!("{} changed", in_line(&b.transition_label(b.tat[k]))));
            }
        }
        // What the other side added inside, or moved in, and its new transitions to and from it.
        let mut inside: HashSet<&str> = HashSet::new();
        let mut vertices: HashSet<&str> = HashSet::new();
        for n in subtree {
            match n {
                Node::V(k) => {
                    inside.insert(&k.0);
                    vertices.insert(&k.0);
                }
                Node::R(r) => {
                    inside.insert(r);
                }
            }
        }
        for (j, v) in o.m.vertices.iter().enumerate() {
            let in_base = b.vat.get(&o.vkeys[j]).map(|&i| &b.m.vertices[i]);
            let new_here = in_base.is_none_or(|bv| bv.parent != v.parent);
            if new_here && inside.contains(v.parent.as_str()) && !vertices.contains(v.id.as_str()) {
                changes.push(format!("{} added", in_line(&o.vertex_label(j))));
            }
        }
        for (j, t) in o.m.transitions.iter().enumerate() {
            let in_base = b.tat.get(&o.tkeys[j]).map(|&i| &b.m.transitions[i]);
            let new_here = in_base.is_none_or(|bt| bt.source != t.source || bt.target != t.target);
            if new_here && (vertices.contains(t.source.as_str()) || vertices.contains(t.target.as_str())) {
                changes.push(format!("{} added", in_line(&o.transition_label(j))));
            }
        }
        if changes.is_empty() {
            return;
        }
        let (id, label, region) = match root {
            Node::V(k) => (k.0.clone(), b.vertex_label(b.vat[k]), None),
            Node::R(r) => {
                let owner = b.ix().region_owner(r).flatten();
                let name = owner
                    .and_then(|s| b.m.vertices[s].regions.iter().find(|x| &x.id == r))
                    .map_or(String::new(), |x| x.name.clone());
                let region = if name.is_empty() { "a region".to_string() } else { format!("region {name}") };
                match owner {
                    Some(s) => (b.m.vertices[s].id.clone(), b.vertex_label(s), Some(region)),
                    None => (b.m.id.clone(), b.namer.machine_label(), Some(region)),
                }
            }
        };
        let element = if matches!(root, Node::R(_))
            && b.ix().region_owner(match root {
                Node::R(r) => r,
                _ => "",
            }) == Some(None)
        {
            Element::Machine
        } else {
            Element::Vertex
        };
        if d == Side::Theirs {
            self.keep_nodes.extend(subtree.iter().cloned());
            self.keep_transitions.extend(transitions.iter().cloned());
        }
        self.conflicts.push(Conflict {
            id,
            element,
            label,
            kind: ConflictKind::Deleted,
            property: None,
            base: Value::Null,
            ours: Value::Null,
            theirs: Value::Null,
            ours_shown: String::new(),
            theirs_shown: String::new(),
            deleted_on: Some(d),
            region,
            changes,
            kept: d == Side::Theirs,
        });
    }

    fn check_transition_deletion(&mut self, k: &Key, d: Side) {
        let b = self.b();
        let o = self.side(d.other());
        let Some(&j) = o.tat.get(k) else { return };
        let bp = transition_properties(b.m, b.tat[k]);
        let op = transition_properties(o.m, j);
        let changes: Vec<String> = op
            .iter()
            .filter(|(p, v)| prop(&bp, p) != Some(v))
            .map(|(p, _)| format!("{} changed", property_word(p)))
            .collect();
        if changes.is_empty() {
            return;
        }
        let label = b.transition_label(b.tat[k]);
        if d == Side::Theirs {
            self.keep_transitions.insert(k.clone());
        }
        self.conflicts.push(Conflict {
            id: k.0.clone(),
            element: Element::Transition,
            label,
            kind: ConflictKind::Deleted,
            property: None,
            base: Value::Null,
            ours: Value::Null,
            theirs: Value::Null,
            ours_shown: String::new(),
            theirs_shown: String::new(),
            deleted_on: Some(d),
            region: None,
            changes,
            kept: d == Side::Theirs,
        });
    }

    // -------------------------------------------------------------- properties

    /// The label of an element for the report: as in the base, else as ours or theirs added it.
    fn label_of(&self, element: Element, b: Option<usize>, o: Option<(Src, usize)>, t: Option<(Src, usize)>) -> String {
        let (src, i) = match (b, o, t) {
            (Some(i), _, _) => (Src::Base, i),
            (None, Some(x), _) | (None, None, Some(x)) => x,
            _ => return String::new(),
        };
        let v = self.version(src);
        match element {
            Element::Vertex => v.vertex_label(i),
            Element::Transition => v.transition_label(i),
            Element::Machine => v.namer.machine_label(),
        }
    }

    /// A property value as the report shows it, naming the elements it refers to in `src`.
    fn show(&self, property: &str, value: &Value, src: Src) -> String {
        let namer = &self.version(src).namer;
        let none = |s: String| if s.is_empty() { "none".to_string() } else { s };
        let list = |f: &dyn Fn(&Value) -> String| none(value.as_list().iter().map(f).collect::<Vec<_>>().join(", "));
        match property {
            "type" => type_word(value),
            "owner" => namer.place(value.as_str()),
            "source" | "target" => namer.short(value.as_str()),
            "kind" | "submachine" | "name" | "context" => none(value.as_str().to_string()),
            "point" => point_shown(value),
            "triggers" | "deferred" => list(&|x| x.as_str().to_string()),
            "anchors" => list(&|x| namer.anchor(x.as_str())),
            "regions" => list(&|r| {
                let name = r.field("name");
                if name.is_empty() { "unnamed".to_string() } else { name.to_string() }
            }),
            _ if value.as_str().is_empty() => "none".to_string(),
            p => shown(p, value.as_str()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn property_conflict(
        &mut self,
        kind: ConflictKind,
        id: &str,
        element: Element,
        label: String,
        property: &'static str,
        values: [Option<&Value>; 3],
        srcs: (Src, Src),
    ) {
        let [b, o, t] = values.map(|v| v.cloned().unwrap_or(Value::Null));
        let ours_shown = self.show(property, &o, srcs.0);
        let theirs_shown = self.show(property, &t, srcs.1);
        self.conflicts.push(Conflict {
            id: id.to_string(),
            element,
            label,
            kind,
            property: Some(property),
            base: b,
            ours: o,
            theirs: t,
            ours_shown,
            theirs_shown,
            deleted_on: None,
            region: None,
            changes: Vec::new(),
            kept: true,
        });
    }

    fn note(&mut self, id: &str, element: Element, label: String, properties: Vec<&'static str>) {
        if !properties.is_empty() {
            self.layout.push(LayoutNote { id: id.to_string(), element, label, properties });
        }
    }

    /// Regions merged by id (lists of regions, with the ones theirs deleted but must keep put back), their names
    /// property by property. Sizes are layout: taken later.
    fn merge_regions(&self, b: Option<&[Region]>, o: Option<&[Region]>, t: Option<&[Region]>) -> (Vec<Region>, bool) {
        let ids = |l: &[Region]| l.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
        let (Some(o), Some(t)) = (o, t) else {
            return (o.or(t).unwrap_or(&[]).to_vec(), false);
        };
        let bi = b.map(ids);
        let mut ti = ids(t);
        if let Some(b) = b {
            for (at, r) in b.iter().enumerate() {
                if !ti.contains(&r.id) && self.keep_nodes.contains(&Node::R(r.id.clone())) {
                    ti.insert(at.min(ti.len()), r.id.clone());
                }
            }
        }
        let merged = merge_list(bi.as_deref(), &ids(o), &ti);
        fn find<'r>(l: Option<&'r [Region]>, id: &str) -> Option<&'r Region> {
            l.and_then(|l| l.iter().find(|r| r.id == id))
        }
        let mut conflict = false;
        let out = merged
            .iter()
            .map(|id| {
                let (rb, ro, rt) = (find(b, id), find(Some(o), id), find(Some(t), id).or(find(b, id)));
                let name = match decide(rb.map(|r| &r.name), ro.map(|r| &r.name), rt.map(|r| &r.name)) {
                    Pick::Theirs => rt,
                    Pick::Conflict => {
                        conflict = true;
                        ro
                    }
                    Pick::Ours => ro.or(rt),
                };
                let r = ro.or(rt).or(rb).cloned().unwrap_or_else(|| Region::new(id, ""));
                Region { name: name.map_or(r.name.clone(), |n| n.name.clone()), ..r }
            })
            .collect();
        (out, conflict)
    }

    fn merge_vertex(&mut self, b: Option<usize>, o: Option<(Src, usize)>, t: Option<(Src, usize)>) -> MergedVertex {
        let bv = b.map(|i| &self.b().m.vertices[i]);
        let (ov, tv) = (o.map(|x| self.vertex(x)), t.map(|x| self.vertex(x)));
        let first = ov.or(tv).expect("a side has the vertex");
        let mut v = first.clone();
        let id = v.id.clone();
        let label = self.label_of(Element::Vertex, b, o, t);
        let (os, ts) = (o.map_or(Src::Theirs, |x| x.0), t.map_or(Src::Ours, |x| x.0));
        if let (Some(ov), Some(tv)) = (ov, tv) {
            let bp = bv.map(vertex_properties);
            let (op, tp) = (vertex_properties(ov), vertex_properties(tv));
            let val = |l: &Option<Vec<(&'static str, Value)>>, p: &str| l.as_ref().and_then(|l| prop(l, p).cloned());
            let changed =
                |side: &[(&'static str, Value)], p: &str| bp.as_ref().is_none_or(|b| prop(b, p) != prop(side, p));
            let others = |side: &[(&'static str, Value)]| side.iter().any(|(p, _)| *p != "type" && changed(side, p));
            // A kind of vertex changed on one side while the other changed it too: ours' whole vertex.
            let type_pick = decide(val(&bp, "type").as_ref(), prop(&op, "type"), prop(&tp, "type"));
            let retyped = (changed(&op, "type") && others(&tp)) || (changed(&tp, "type") && others(&op));
            if type_pick == Pick::Conflict || (bp.is_some() && retyped && changed(&op, "type") != changed(&tp, "type"))
            {
                let values = [bp.as_ref().and_then(|l| prop(l, "type")), prop(&op, "type"), prop(&tp, "type")];
                self.property_conflict(
                    ConflictKind::Property,
                    &id,
                    Element::Vertex,
                    label.clone(),
                    "type",
                    values,
                    (os, ts),
                );
                v = ov.clone();
            } else {
                for (p, _) in &op {
                    let p: &'static str = p;
                    let (bvl, ovl, tvl) = (val(&bp, p), prop(&op, p), prop(&tp, p));
                    match p {
                        "regions" => {
                            let (regions, conflict) =
                                self.merge_regions(bv.map(|x| &x.regions[..]), Some(&ov.regions), Some(&tv.regions));
                            v.regions = regions;
                            if conflict {
                                let values = [bvl.as_ref(), ovl, tvl];
                                self.property_conflict(
                                    ConflictKind::Property,
                                    &id,
                                    Element::Vertex,
                                    label.clone(),
                                    p,
                                    values,
                                    (os, ts),
                                );
                            }
                            continue;
                        }
                        "deferred" => {
                            v.deferrable = merge_list(bv.map(|x| &x.deferrable[..]), &ov.deferrable, &tv.deferrable);
                            continue;
                        }
                        "anchors" => {
                            v.anchors = merge_list(bv.map(|x| &x.anchors[..]), &ov.anchors, &tv.anchors);
                            continue;
                        }
                        _ => {}
                    }
                    match decide(bvl.as_ref(), ovl, tvl) {
                        Pick::Ours => {}
                        Pick::Theirs => copy_vertex_property(&mut v, tv, p),
                        Pick::Conflict => {
                            let values = [bvl.as_ref(), ovl, tvl];
                            self.property_conflict(
                                ConflictKind::Property,
                                &id,
                                Element::Vertex,
                                label.clone(),
                                p,
                                values,
                                (os, ts),
                            );
                        }
                    }
                }
            }
        }
        let pos = self.merge_vertex_layout(&mut v, b, o, t, &id, &label);
        MergedVertex { v, b, o, t, pos }
    }

    /// The drawing properties of a vertex, taken as `decide` says, ours' when both changed them; where its position
    /// comes from.
    fn merge_vertex_layout(
        &mut self,
        v: &mut Vertex,
        b: Option<usize>,
        o: Option<(Src, usize)>,
        t: Option<(Src, usize)>,
        id: &str,
        label: &str,
    ) -> (Src, usize) {
        let (Some(o), Some(t)) = (o, t) else {
            let only = o.or(t).expect("a side has the vertex");
            let src = self.vertex(only);
            (v.w, v.h, v.region_layout, v.colors, v.group) =
                (src.w, src.h, src.region_layout, src.colors.clone(), src.group.clone());
            for r in &mut v.regions {
                if let Some(x) = src.regions.iter().find(|x| x.id == r.id) {
                    r.size = x.size;
                }
            }
            return only;
        };
        let bsrc = b.map(|i| (Src::Base, i));
        let bv = bsrc.map(|x| self.vertex(x));
        let position = |me: &Merger, x: (Src, usize), r: bool| {
            let vx = me.vertex(x);
            let (ox, oy) = me.version(x.0).owner_at(x.1).unwrap_or((0.0, 0.0));
            Value::Object(vec![("owner", Value::text(&vx.parent)), ("at", pt(vx.x - ox, vx.y - oy, r))])
        };
        let size = |me: &Merger, x: (Src, usize), r: bool| {
            let vx = me.vertex(x);
            Value::Object(vec![("width", num(vx.w, r)), ("height", num(vx.h, r))])
        };
        let region_layout = |me: &Merger, x: (Src, usize), _: bool| {
            let vx = me.vertex(x);
            if vx.regions.len() < 2 {
                return Value::Null;
            }
            Value::text(if vx.region_layout == Some(RegionLayout::Horizontal) { "horizontal" } else { "vertical" })
        };
        let color = |me: &Merger, x: (Src, usize), _: bool| colors(&me.vertex(x).colors)[0].1.clone();
        let items = |me: &Merger, x: (Src, usize), _: bool| colors(&me.vertex(x).colors)[1].1.clone();
        let group = |me: &Merger, x: (Src, usize), _: bool| Value::text(&me.vertex(x).group);
        let mut notes = Vec::new();
        let mut take = |me: &Merger, name: &'static str, f: LayoutValue| {
            let values = |r: bool| [bsrc.map(|x| f(me, x, r)), Some(f(me, o, r)), Some(f(me, t, r))];
            let (e, r) = (values(false), values(true));
            match decide_layout(
                [e[0].as_ref(), e[1].as_ref(), e[2].as_ref()],
                [r[0].as_ref(), r[1].as_ref(), r[2].as_ref()],
            ) {
                Pick::Ours => o,
                Pick::Theirs => t,
                Pick::Conflict => {
                    notes.push(name);
                    o
                }
            }
        };
        // The position goes with the owner: from the side that moved the vertex to another region.
        let pos = if bv.is_some_and(|b| b.parent != v.parent)
            || bv.is_none() && self.vertex(o).parent != self.vertex(t).parent
        {
            if self.vertex(o).parent == v.parent { o } else { t }
        } else {
            take(self, "position", &position)
        };
        let s = take(self, "size", &size);
        (v.w, v.h) = (self.vertex(s).w, self.vertex(s).h);
        let s = take(self, "regionLayout", &region_layout);
        v.region_layout = self.vertex(s).region_layout;
        let s = take(self, "color", &color);
        v.colors.color = self.vertex(s).colors.color.clone();
        let s = take(self, "itemColors", &items);
        v.colors.items = self.vertex(s).colors.items.clone();
        let s = take(self, "group", &group);
        v.group = self.vertex(s).group.clone();
        // Each region's size, by region.
        let (ov, tv) = (self.vertex(o), self.vertex(t));
        for r in &mut v.regions {
            let size_of = |x: Option<&Vertex>| {
                x.and_then(|x| x.regions.iter().find(|y| y.id == r.id)).map(|y| {
                    (
                        y.size,
                        y.size.map_or(Value::Null, |n| num(n, false)),
                        y.size.map_or(Value::Null, |n| num(n, true)),
                    )
                })
            };
            let (sb, so, st) = (size_of(bv), size_of(Some(ov)), size_of(Some(tv)));
            let exact = [sb.as_ref().map(|x| &x.1), so.as_ref().map(|x| &x.1), st.as_ref().map(|x| &x.1)];
            let rounded = [sb.as_ref().map(|x| &x.2), so.as_ref().map(|x| &x.2), st.as_ref().map(|x| &x.2)];
            r.size = match decide_layout(exact, rounded) {
                Pick::Theirs => st.map_or(r.size, |x| x.0),
                Pick::Ours => so.or(st).map_or(r.size, |x| x.0),
                Pick::Conflict => {
                    if !notes.contains(&"regionSizes") {
                        notes.push("regionSizes");
                    }
                    so.map_or(r.size, |x| x.0)
                }
            };
        }
        self.note(id, Element::Vertex, label.to_string(), notes);
        pos
    }

    fn merge_transition(
        &mut self,
        b: Option<usize>,
        o: Option<(Src, usize)>,
        t: Option<(Src, usize)>,
    ) -> MergedTransition {
        let bt = b.map(|i| &self.b().m.transitions[i]);
        let (ot, tt) = (o.map(|x| self.transition(x)), t.map(|x| self.transition(x)));
        let mut tr = ot.or(tt).expect("a side has the transition").clone();
        let id = tr.id.clone();
        let label = self.label_of(Element::Transition, b, o, t);
        let (Some(o), Some(t), Some(ot), Some(tt)) = (o, t, ot, tt) else {
            let only = o.or(t).expect("a side has the transition");
            return MergedTransition { t: tr, b, o, tt: t, points: only, absolute: false };
        };
        let bp = b.map(|i| transition_properties(self.b().m, i));
        let op = transition_properties(self.version(o.0).m, o.1);
        let tp = transition_properties(self.version(t.0).m, t.1);
        for (p, _) in &op {
            let p: &'static str = p;
            let (bvl, ovl, tvl) = (bp.as_ref().and_then(|l| prop(l, p)), prop(&op, p), prop(&tp, p));
            if p == "triggers" {
                tr.triggers = merge_list(bt.map(|x| &x.triggers[..]), &ot.triggers, &tt.triggers);
                continue;
            }
            match decide(bvl, ovl, tvl) {
                Pick::Ours => {}
                Pick::Theirs => copy_transition_property(&mut tr, tt, p),
                Pick::Conflict => self.property_conflict(
                    ConflictKind::Property,
                    &id,
                    Element::Transition,
                    label.clone(),
                    p,
                    [bvl, ovl, tvl],
                    (o.0, t.0),
                ),
            }
        }
        // Layout.
        let bsrc = b.map(|i| (Src::Base, i));
        let points = |me: &Merger, x: (Src, usize), r: bool| {
            let ver = me.version(x.0);
            let tr = &ver.m.transitions[x.1];
            let (sx, sy) =
                ver.ix().vertex(&tr.source).map_or((0.0, 0.0), |s| (ver.m.vertices[s].x, ver.m.vertices[s].y));
            Value::List(tr.points.iter().map(|p| pt(p.x - sx, p.y - sy, r)).collect())
        };
        let label_offset = |me: &Merger, x: (Src, usize), r: bool| {
            me.transition(x).label_offset.map_or(Value::Null, |p| pt(p.x, p.y, r))
        };
        let color = |me: &Merger, x: (Src, usize), _: bool| colors(&me.transition(x).colors)[0].1.clone();
        let items = |me: &Merger, x: (Src, usize), _: bool| colors(&me.transition(x).colors)[1].1.clone();
        let mut notes = Vec::new();
        let mut take = |me: &Merger, name: &'static str, f: LayoutValue| {
            let values = |r: bool| [bsrc.map(|x| f(me, x, r)), Some(f(me, o, r)), Some(f(me, t, r))];
            let (e, r) = (values(false), values(true));
            match decide_layout(
                [e[0].as_ref(), e[1].as_ref(), e[2].as_ref()],
                [r[0].as_ref(), r[1].as_ref(), r[2].as_ref()],
            ) {
                Pick::Ours => o,
                Pick::Theirs => t,
                Pick::Conflict => {
                    notes.push(name);
                    o
                }
            }
        };
        // The bend points go with the source: from the side that gave the transition another source.
        let mut absolute = false;
        let pts = if bt.is_some_and(|b| b.source != tr.source) || bt.is_none() && ot.source != tt.source {
            if ot.source == tr.source { o } else { t }
        } else if let Some(bs) = bsrc {
            // A side touched the bend points when they moved on the canvas, and changed them when they also moved
            // relative to the source (dragging a state alone leaves them where they are on the canvas; dragging it
            // with the other end moves them with it).
            let canvas = |x: (Src, usize)| &self.transition(x).points;
            let (touched_o, touched_t) = (canvas(o) != canvas(bs), canvas(t) != canvas(bs));
            if !(touched_o && touched_t) {
                absolute = true;
                if touched_t { t } else { o }
            } else {
                let relative = |x| points(self, x, false);
                let (rb, ro, rt) = (relative(bs), relative(o), relative(t));
                match (ro != rb, rt != rb) {
                    (true, false) => o,
                    (false, true) => t,
                    _ => take(self, "points", &points),
                }
            }
        } else {
            take(self, "points", &points)
        };
        let s = take(self, "labelOffset", &label_offset);
        tr.label_offset = self.transition(s).label_offset;
        let s = take(self, "color", &color);
        tr.colors.color = self.transition(s).colors.color.clone();
        let s = take(self, "itemColors", &items);
        tr.colors.items = self.transition(s).colors.items.clone();
        self.note(&id, Element::Transition, label, notes);
        MergedTransition { t: tr, b, o: Some(o), tt: Some(t), points: pts, absolute }
    }

    // -------------------------------------------------------------- the whole machine

    fn run(&mut self) -> FsmModel {
        self.deletions();
        let (b, o, t) = (self.b(), &self.ours, &self.theirs);
        let has_base = self.base.is_some();

        // The machine.
        let mut m = FsmModel {
            id: o.m.id.clone(),
            name: o.m.name.clone(),
            kind: o.m.kind,
            context: o.m.context.clone(),
            documentation: o.m.documentation.clone(),
            regions: Vec::new(),
            vertices: Vec::new(),
            transitions: Vec::new(),
        };
        let bp = has_base.then(|| machine_properties(b.m));
        let (op, tp) = (machine_properties(o.m), machine_properties(t.m));
        let label = if has_base { b.namer.machine_label() } else { o.namer.machine_label() };
        let (tm, om) = (t.m, o.m);
        for p in ["name", "kind", "context", "documentation"] {
            let values = [bp.as_ref().and_then(|l| prop(l, p)), prop(&op, p), prop(&tp, p)];
            match decide(values[0], values[1], values[2]) {
                Pick::Ours => {}
                Pick::Theirs => match p {
                    "name" => m.name = tm.name.clone(),
                    "kind" => m.kind = tm.kind,
                    "context" => m.context = tm.context.clone(),
                    _ => m.documentation = tm.documentation.clone(),
                },
                Pick::Conflict => {
                    let id = m.id.clone();
                    self.property_conflict(
                        ConflictKind::Property,
                        &id,
                        Element::Machine,
                        label.clone(),
                        p,
                        values,
                        (Src::Ours, Src::Theirs),
                    )
                }
            }
        }
        let base_regions = has_base.then(|| &self.b().m.regions[..]);
        let (regions, conflict) = self.merge_regions(base_regions, Some(&om.regions), Some(&tm.regions));
        m.regions = regions;
        if conflict {
            let values = [bp.as_ref().and_then(|l| prop(l, "regions")), prop(&op, "regions"), prop(&tp, "regions")];
            let id = m.id.clone();
            self.property_conflict(
                ConflictKind::Property,
                &id,
                Element::Machine,
                label,
                "regions",
                values,
                (Src::Ours, Src::Theirs),
            );
        }

        // Vertices, in the order of the side that changed it (ours' when both did), then the other side's additions.
        let (b, o, t) = (self.b(), &self.ours, &self.theirs);
        let (primary, secondary) = if o.vkeys == b.vkeys { (&t.vkeys, &o.vkeys) } else { (&o.vkeys, &t.vkeys) };
        let in_primary: HashSet<&Key> = primary.iter().collect();
        let order: Vec<Key> =
            primary.iter().chain(secondary.iter().filter(|k| !in_primary.contains(k))).cloned().collect();
        let mut vertices = Vec::new();
        for k in order {
            let (bi, oi, ti) =
                (self.b().vat.get(&k).copied(), self.ours.vat.get(&k).copied(), self.theirs.vat.get(&k).copied());
            let slots = match (bi, oi, ti) {
                (Some(_), None, _) => continue, // deleted on ours
                (Some(i), Some(j), None) if self.keep_nodes.contains(&Node::V(k.clone())) => {
                    (Some(i), Some((Src::Ours, j)), Some((Src::Base, i))) // deleted on theirs, kept
                }
                (Some(_), Some(_), None) => continue, // deleted on theirs
                (b, o, t) => (b, o.map(|j| (Src::Ours, j)), t.map(|j| (Src::Theirs, j))),
            };
            vertices.push(self.merge_vertex(slots.0, slots.1, slots.2));
        }

        let (b, o, t) = (self.b(), &self.ours, &self.theirs);
        let (primary, secondary) = if o.tkeys == b.tkeys { (&t.tkeys, &o.tkeys) } else { (&o.tkeys, &t.tkeys) };
        let in_primary: HashSet<&Key> = primary.iter().collect();
        let order: Vec<Key> =
            primary.iter().chain(secondary.iter().filter(|k| !in_primary.contains(k))).cloned().collect();
        let mut transitions = Vec::new();
        for k in order {
            let (bi, oi, ti) =
                (self.b().tat.get(&k).copied(), self.ours.tat.get(&k).copied(), self.theirs.tat.get(&k).copied());
            let slots = match (bi, oi, ti) {
                (Some(_), None, _) => continue,
                (Some(i), Some(j), None) if self.keep_transitions.contains(&k) => {
                    (Some(i), Some((Src::Ours, j)), Some((Src::Base, i)))
                }
                (Some(_), Some(_), None) => continue,
                (b, o, t) => (b, o.map(|j| (Src::Ours, j)), t.map(|j| (Src::Theirs, j))),
            };
            transitions.push(self.merge_transition(slots.0, slots.1, slots.2));
        }

        self.break_cycles(&m, &mut vertices);
        self.cascade(&m, &mut vertices, &mut transitions);
        self.place(&mut m, vertices, transitions);
        m
    }

    /// Two states moved into each other: the owners of ours are kept for the states of the cycle.
    fn break_cycles(&mut self, m: &FsmModel, vertices: &mut [MergedVertex]) {
        for _ in 0..vertices.len().max(1) {
            let model = FsmModel {
                vertices: vertices.iter().map(|x| x.v.clone()).collect(),
                transitions: Vec::new(),
                ..m.clone()
            };
            let ix = ModelIndex::new(&model);
            let mut fixed = false;
            #[allow(clippy::needless_range_loop)] // `vertices[i]` changes in the loop
            for i in 0..vertices.len() {
                // Is i its own ancestor?
                let mut cur = ix.owner_state(i);
                let mut steps = 0;
                let mut cycle = false;
                while let Some(c) = cur {
                    if model.vertices[c].id == model.vertices[i].id {
                        cycle = true;
                        break;
                    }
                    steps += 1;
                    if steps > model.vertices.len() {
                        break;
                    }
                    cur = ix.owner_state(c);
                }
                let mv = &vertices[i];
                let Some(o) = mv.o else { continue };
                let ours_parent = &self.vertex(o).parent;
                if !cycle || &mv.v.parent == ours_parent {
                    continue;
                }
                let theirs_parent = mv.v.parent.clone();
                let ours_parent = ours_parent.clone();
                let label = self.label_of(Element::Vertex, mv.b, mv.o, mv.t);
                let id = mv.v.id.clone();
                let (tsrc, osrc) = (mv.t.map_or(Src::Theirs, |x| x.0), o.0);
                let values = [
                    mv.b.map(|i| Value::text(&self.b().m.vertices[i].parent)),
                    Some(Value::text(&ours_parent)),
                    Some(Value::text(&theirs_parent)),
                ];
                self.property_conflict(
                    ConflictKind::Cycle,
                    &id,
                    Element::Vertex,
                    label,
                    "owner",
                    [values[0].as_ref(), values[1].as_ref(), values[2].as_ref()],
                    (osrc, tsrc),
                );
                vertices[i].v.parent = ours_parent;
                vertices[i].pos = o;
                fixed = true;
                break;
            }
            if !fixed {
                return;
            }
        }
    }

    /// Takes away what lost its container, source or target in the merge, and the comments' anchors to what is gone:
    /// a reference is broken by the merge when a side holding it had its target and the merged machine doesn't.
    fn cascade(&self, m: &FsmModel, vertices: &mut Vec<MergedVertex>, transitions: &mut Vec<MergedTransition>) {
        loop {
            let regions: HashSet<&str> =
                m.regions.iter().chain(vertices.iter().flat_map(|x| &x.v.regions)).map(|r| r.id.as_str()).collect();
            let ids: HashSet<&str> = vertices.iter().map(|x| x.v.id.as_str()).collect();
            let slots = |b: Option<usize>, o: Option<(Src, usize)>, t: Option<(Src, usize)>| {
                b.map(|i| (Src::Base, i)).into_iter().chain(o).chain(t).collect::<Vec<_>>()
            };
            let broken_parent: Vec<bool> = vertices
                .iter()
                .map(|x| {
                    let p = x.v.parent.as_str();
                    !regions.contains(p)
                        && !ids.contains(p)
                        && slots(x.b, x.o, x.t)
                            .iter()
                            .any(|&s| self.vertex(s).parent == p && self.version(s.0).has_region_or_vertex(p))
                })
                .collect();
            let lost_vertices = broken_parent.iter().any(|&x| x);
            let mut i = 0;
            vertices.retain(|_| {
                i += 1;
                !broken_parent[i - 1]
            });
            let ids: HashSet<String> = vertices.iter().map(|x| x.v.id.clone()).collect();
            let before = transitions.len();
            transitions.retain(|x| {
                let ends = [(&x.t.source, true), (&x.t.target, false)];
                !ends.iter().any(|(end, is_source)| {
                    !ids.contains(end.as_str())
                        && slots(x.b, x.o, x.tt).iter().any(|&s| {
                            let tr = self.transition(s);
                            let held = if *is_source { &tr.source } else { &tr.target };
                            held == *end && self.version(s.0).ix().has_vertex(end)
                        })
                })
            });
            if !lost_vertices && before == transitions.len() {
                break;
            }
        }
        let alive: HashSet<String> =
            vertices.iter().map(|x| x.v.id.clone()).chain(transitions.iter().map(|x| x.t.id.clone())).collect();
        for x in vertices.iter_mut() {
            let sides: Vec<(Src, usize)> = x.b.map(|i| (Src::Base, i)).into_iter().chain(x.o).chain(x.t).collect();
            let keep = |a: &String| {
                alive.contains(a)
                    || !sides
                        .iter()
                        .any(|&s| self.vertex(s).anchors.contains(a) && self.version(s.0).alive.contains(a.as_str()))
            };
            x.v.anchors.retain(keep);
        }
    }

    /// Absolute positions again: each vertex at its position relative to its owner, from the side it was taken from,
    /// inside its merged owner; bend points relative to the source likewise, unless taken as they are on the canvas.
    fn place(&self, m: &mut FsmModel, vertices: Vec<MergedVertex>, transitions: Vec<MergedTransition>) {
        m.vertices = vertices.iter().map(|x| x.v.clone()).collect();
        let ix_model = m.clone();
        let ix = ModelIndex::new(&ix_model);
        let mut at: Vec<Option<(f64, f64)>> = vec![None; vertices.len()];
        fn locate(
            me: &Merger,
            i: usize,
            vertices: &[MergedVertex],
            ix: &ModelIndex,
            at: &mut Vec<Option<(f64, f64)>>,
            depth: usize,
        ) -> (f64, f64) {
            if let Some(p) = at[i] {
                return p;
            }
            let (s, j) = vertices[i].pos;
            let src = me.vertex((s, j));
            let raw = (src.x, src.y);
            let p = match (ix.owner_state(i), me.version(s).owner_at(j)) {
                (Some(o), Some(so)) if depth < vertices.len() => {
                    let mo = locate(me, o, vertices, ix, at, depth + 1);
                    if mo == so { raw } else { (mo.0 + (raw.0 - so.0), mo.1 + (raw.1 - so.1)) }
                }
                _ => raw,
            };
            at[i] = Some(p);
            p
        }
        for i in 0..vertices.len() {
            let (x, y) = locate(self, i, &vertices, &ix, &mut at, 0);
            m.vertices[i].x = x;
            m.vertices[i].y = y;
        }
        m.transitions = transitions
            .into_iter()
            .map(|x| {
                let mut t = x.t;
                let (s, j) = x.points;
                let ver = self.version(s);
                let src_t = &ver.m.transitions[j];
                let from = ver.ix().vertex(&src_t.source).map(|k| (ver.m.vertices[k].x, ver.m.vertices[k].y));
                let to = ix.vertex(&t.source).map(|k| (m.vertices[k].x, m.vertices[k].y));
                t.points = match (from, to) {
                    _ if x.absolute => src_t.points.clone(),
                    (Some(f), Some(to)) if f != to => src_t
                        .points
                        .iter()
                        .map(|p| crate::model::Point { x: to.0 + (p.x - f.0), y: to.1 + (p.y - f.1) })
                        .collect(),
                    _ => src_t.points.clone(),
                };
                t
            })
            .collect();
    }
}

fn copy_vertex_property(v: &mut Vertex, from: &Vertex, p: &str) {
    match p {
        "type" => v.vtype = from.vtype,
        "name" => v.name = from.name.clone(),
        "owner" => v.parent = from.parent.clone(),
        "entry" => v.entry = from.entry.clone(),
        "exit" => v.exit = from.exit.clone(),
        "do" => v.do_activity = from.do_activity.clone(),
        "invariant" => v.invariant = from.invariant.clone(),
        "submachine" => v.submachine = from.submachine.clone(),
        "stereotype" => v.stereotype = from.stereotype.clone(),
        "point" => (v.point_kind, v.point_ref) = (from.point_kind, from.point_ref.clone()),
        "text" => v.text = from.text.clone(),
        _ => {}
    }
}

fn copy_transition_property(t: &mut Transition, from: &Transition, p: &str) {
    match p {
        "source" => t.source = from.source.clone(),
        "target" => t.target = from.target.clone(),
        "kind" => t.kind = from.kind,
        "guard" => t.guard = from.guard.clone(),
        "effect" => t.effect = from.effect.clone(),
        "precondition" => t.precondition = from.precondition.clone(),
        "postcondition" => t.postcondition = from.postcondition.clone(),
        _ => {}
    }
}

fn empty_model(id: &str) -> FsmModel {
    FsmModel {
        id: id.to_string(),
        name: String::new(),
        kind: MachineKind::Behavioral,
        context: String::new(),
        documentation: String::new(),
        regions: Vec::new(),
        vertices: Vec::new(),
        transitions: Vec::new(),
    }
}

/// Merges `ours` and `theirs`, two versions of a machine whose common ancestor is `base` (None: there is none, so
/// both sides added everything). The error says why the files can't be merged at all, or why the result can't be
/// written; nothing is to be written then.
pub fn merge(base: Option<&FsmModel>, ours: &FsmModel, theirs: &FsmModel) -> Result<Merge, String> {
    if ours.id != theirs.id {
        return Err(format!(
            "ours and theirs are two different state machines (ids '{}' and '{}'): they can't be merged.",
            ours.id, theirs.id
        ));
    }
    if let Some(b) = base
        && b.id != ours.id
    {
        return Err(format!(
            "The base is another state machine than ours and theirs (id '{}', not '{}'): they can't be merged.",
            b.id, ours.id
        ));
    }
    let empty = empty_model(&ours.id);
    let mut merger = Merger {
        base: base.map(Version::new),
        empty: Version::new(&empty),
        ours: Version::new(ours),
        theirs: Version::new(theirs),
        conflicts: Vec::new(),
        layout: Vec::new(),
        keep_nodes: HashSet::new(),
        keep_transitions: HashSet::new(),
    };
    let mut model = merger.run();
    // Elements from both sides: in the order the editors will read them back, so that saving the file again keeps it.
    // A merge that gives one side's structure keeps that side's order (a file edited by hand may have another).
    type Structure<'m> = (Vec<(&'m str, &'m str)>, Vec<(&'m str, &'m str, &'m str)>);
    fn structure(m: &FsmModel) -> Structure<'_> {
        let v = m.vertices.iter().map(|v| (v.id.as_str(), v.parent.as_str())).collect();
        let t = m.transitions.iter().map(|t| (t.id.as_str(), t.source.as_str(), t.target.as_str())).collect();
        (v, t)
    }
    if structure(&model) != structure(ours) && structure(&model) != structure(theirs) {
        let (vo, to) = document_order(&model);
        model.vertices = vo.iter().map(|&i| model.vertices[i].clone()).collect();
        model.transitions = to.iter().map(|&i| model.transitions[i].clone()).collect();
    }
    let text = to_xmi(&model).map_err(|e| format!("The merged machine can't be written: {e}"))?;
    // What the editors will read: it must read back.
    let written = from_xmi(&text).map_err(|e| format!("The merged machine doesn't read back: {e}"))?;

    // The elements changed on either side and merged without a conflict: the merged file has them as that side has
    // them (there for an element added or changed, gone for one removed).
    let in_conflict: HashSet<(Element, &str)> = merger.conflicts.iter().map(|c| (c.element, c.id.as_str())).collect();
    let present: HashSet<(Element, &str)> = written
        .vertices
        .iter()
        .map(|v| (Element::Vertex, v.id.as_str()))
        .chain(written.transitions.iter().map(|t| (Element::Transition, t.id.as_str())))
        .chain([(Element::Machine, written.id.as_str())])
        .collect();
    let (d_ours, d_theirs) = (diff(base, Some(ours)), diff(base, Some(theirs)));
    let mut merged: Vec<Merged> = Vec::new();
    for (d, from) in [(&d_ours, "ours"), (&d_theirs, "theirs")] {
        for e in &d.elements {
            let key = (e.element, e.id.as_str());
            if in_conflict.contains(&key) || present.contains(&key) == (e.status == crate::diff::Status::Removed) {
                continue;
            }
            match merged.iter_mut().find(|m| m.element == e.element && m.id == e.id) {
                Some(m) => m.from = "both",
                None => merged.push(Merged {
                    id: e.id.clone(),
                    element: e.element,
                    status: e.status.as_str(),
                    label: e.label.clone(),
                    from,
                }),
            }
        }
    }

    // The issues the merge caused: neither side has them (same element, same message), as read or as written (writing
    // a side alone leaves out what the file format can't hold, as the editors do).
    let none = HashMap::new();
    let mut known: HashSet<(String, String)> = HashSet::new();
    for side in [ours, theirs] {
        let rewritten = to_xmi(side).ok().and_then(|t| from_xmi(&t).ok());
        for i in validate(side, &none).into_iter().chain(rewritten.iter().flat_map(|m| validate(m, &none))) {
            known.insert((i.id, i.message));
        }
    }
    let namer = Namer::new(&written);
    let issues = validate(&written, &none)
        .into_iter()
        .filter(|i| !known.contains(&(i.id.clone(), i.message.clone())))
        .map(|i| {
            let label = if i.id.is_empty() || i.id == written.id {
                namer.machine_label()
            } else if let Some(v) = namer.index.vertex(&i.id) {
                namer.vertex_label(v)
            } else if let Some(t) = written.transitions.iter().position(|t| t.id == i.id) {
                namer.transition_label(t)
            } else {
                i.id.clone()
            };
            NewIssue { id: i.id, label, severity: i.severity.as_str(), message: i.message }
        })
        .collect();
    Ok(Merge { text, merged, conflicts: merger.conflicts, layout: merger.layout, issues })
}

// ------------------------------------------------------------------ output

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [a] => a.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn layout_words(properties: &[&str]) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for p in properties {
        let w = match *p {
            "position" => "moved",
            "size" => "resized",
            "regionLayout" | "regionSizes" => "regions resized",
            "points" => "rerouted",
            "labelOffset" => "label moved",
            "color" | "itemColors" => "recolored",
            _ => "regrouped",
        };
        if !words.iter().any(|x| x == w) {
            words.push(w.to_string());
        }
    }
    words
}

impl Merge {
    /// Whether everything merged without a conflict.
    pub fn clean(&self) -> bool {
        self.conflicts.is_empty()
    }

    /// The validation errors the merge caused: the merged machine no longer generates code.
    pub fn errors(&self) -> usize {
        self.issues.iter().filter(|i| i.severity == "error").count()
    }

    /// Whether the merge needs no review: no conflict, and no validation error caused by the merge (`fsm merge`'s
    /// exit code 0).
    pub fn succeeded(&self) -> bool {
        self.clean() && self.errors() == 0
    }

    /// The report: `<header>: N changes merged, M conflicts` (then `, E errors` when the merge caused validation
    /// errors), a line per conflict (`✕`) with ours' and theirs' values or what the deletion takes, a line per drawing
    /// property changed on both sides (`· layout:`), and the validation issues the merge caused (`!`).
    pub fn to_text(&self, header: &str) -> String {
        let errors = match self.errors() {
            0 => String::new(),
            n => format!(", {}", plural(n, "error", "errors")),
        };
        let mut out = format!(
            "{header}: {} merged, {}{errors}\n",
            plural(self.merged.len(), "change", "changes"),
            plural(self.conflicts.len(), "conflict", "conflicts")
        );
        for c in &self.conflicts {
            let label = in_line(&c.label);
            match c.kind {
                ConflictKind::Property | ConflictKind::Cycle => {
                    let cycle = if c.kind == ConflictKind::Cycle { " (a containment cycle)" } else { "" };
                    out += &format!("  ✕ {label}: {}{cycle}\n", c.property.unwrap_or(""));
                    out += &format!("      ours    {}\n      theirs  {}\n", c.ours_shown, c.theirs_shown);
                }
                ConflictKind::Deleted => {
                    let d = c.deleted_on.unwrap_or(Side::Ours);
                    let what = match &c.region {
                        Some(r) => format!("{r} deleted"),
                        None => "deleted".to_string(),
                    };
                    out += &format!(
                        "  ✕ {label}: {what} on {}, {} on {}\n",
                        d.as_str(),
                        c.changes.join(", "),
                        d.other().as_str()
                    );
                    out += if c.kept {
                        "      kept, as ours has it\n"
                    } else {
                        "      kept deleted: theirs' changes are left out\n"
                    };
                }
            }
        }
        for n in &self.layout {
            out += &format!(
                "  · layout: {} {} on both sides, kept ours\n",
                in_line(&n.label),
                and_list(&layout_words(&n.properties))
            );
        }
        for i in &self.issues {
            out += &format!("  ! {}: {}: {}\n", in_line(&i.label), i.severity, i.message);
        }
        out
    }

    /// The JSON output: `{ "merged", "conflicts", "layout", "issues", "counts" }`, pretty-printed, keys in a fixed order;
    /// `counts.errors` is the number of issues of severity error.
    pub fn to_json(&self) -> String {
        struct Doc<'a>(&'a Merge);
        struct List<'a, T>(&'a [T], fn(&T) -> Value);
        impl<T> Serialize for List<'_, T> {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let mut seq = s.serialize_seq(Some(self.0.len()))?;
                for x in self.0 {
                    seq.serialize_element(&(self.1)(x))?;
                }
                seq.end()
            }
        }
        fn merged(m: &Merged) -> Value {
            Value::Object(vec![
                ("id", Value::Text(m.id.clone())),
                ("element", Value::Text(m.element.as_str().to_string())),
                ("status", Value::Text(m.status.to_string())),
                ("label", Value::Text(m.label.clone())),
                ("from", Value::Text(m.from.to_string())),
            ])
        }
        fn conflict(c: &Conflict) -> Value {
            let opt = |s: Option<&str>| s.map_or(Value::Null, |s| Value::Text(s.to_string()));
            Value::Object(vec![
                ("id", Value::Text(c.id.clone())),
                ("element", Value::Text(c.element.as_str().to_string())),
                ("label", Value::Text(c.label.clone())),
                ("conflict", Value::Text(c.kind.as_str().to_string())),
                ("property", opt(c.property)),
                ("base", c.base.clone()),
                ("ours", c.ours.clone()),
                ("theirs", c.theirs.clone()),
                ("deletedOn", opt(c.deleted_on.map(Side::as_str))),
                ("region", opt(c.region.as_deref())),
                ("changes", Value::List(c.changes.iter().map(|x| Value::Text(x.clone())).collect())),
                ("kept", Value::Bool(c.kept)),
            ])
        }
        fn note(n: &LayoutNote) -> Value {
            Value::Object(vec![
                ("id", Value::Text(n.id.clone())),
                ("element", Value::Text(n.element.as_str().to_string())),
                ("label", Value::Text(n.label.clone())),
                ("properties", Value::List(n.properties.iter().map(|x| Value::Text(x.to_string())).collect())),
            ])
        }
        fn issue(i: &NewIssue) -> Value {
            Value::Object(vec![
                ("id", Value::Text(i.id.clone())),
                ("label", Value::Text(i.label.clone())),
                ("severity", Value::Text(i.severity.to_string())),
                ("message", Value::Text(i.message.clone())),
            ])
        }
        impl Serialize for Doc<'_> {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let m = self.0;
                let counts = Value::Object(vec![
                    ("merged", Value::Number(m.merged.len() as f64)),
                    ("conflicts", Value::Number(m.conflicts.len() as f64)),
                    ("layout", Value::Number(m.layout.len() as f64)),
                    ("issues", Value::Number(m.issues.len() as f64)),
                    ("errors", Value::Number(m.errors() as f64)),
                ]);
                let mut map = s.serialize_map(Some(5))?;
                map.serialize_entry("merged", &List(&m.merged, merged))?;
                map.serialize_entry("conflicts", &List(&m.conflicts, conflict))?;
                map.serialize_entry("layout", &List(&m.layout, note))?;
                map.serialize_entry("issues", &List(&m.issues, issue))?;
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
    use crate::model::{Point, TransitionKind, VertexType};

    /// Lamp: Off, the composite On (Dim and a choice inside), an internal transition, a comment, a diagram; A and B, two
    /// composite states side by side.
    const LAMP: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xmi:XMI xmi:version="20131001" xmlns:xmi="http://www.omg.org/spec/XMI/20131001" xmlns:uml="http://www.omg.org/spec/UML/20161101" xmlns:umldi="http://www.omg.org/spec/UML/20161101/UMLDI" xmlns:dc="http://www.omg.org/spec/DD/20100524/DC">
  <uml:Model xmi:id="sm_model" name="Lamp">
    <packagedElement xmi:type="uml:StateMachine" xmi:id="sm" name="Lamp">
      <region xmi:type="uml:Region" xmi:id="r">
        <ownedComment xmi:type="uml:Comment" xmi:id="note" annotatedElement="Off Dim"><body>Starts off</body></ownedComment>
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
        <subvertex xmi:type="uml:State" xmi:id="A" name="A"><region xmi:type="uml:Region" xmi:id="rA"/></subvertex>
        <subvertex xmi:type="uml:State" xmi:id="B" name="B"><region xmi:type="uml:Region" xmi:id="rB"/></subvertex>
        <transition xmi:type="uml:Transition" xmi:id="t0" source="i" target="Off"/>
        <transition xmi:type="uml:Transition" xmi:id="t1" source="Off" target="On"><trigger xmi:id="t1_g" name="press"/></transition>
        <transition xmi:type="uml:Transition" xmi:id="t2" kind="internal" source="On" target="On"><trigger xmi:id="t2_g" name="dim"/></transition>
      </region>
    </packagedElement>
  </uml:Model>
  <umldi:UMLStateMachineDiagram xmi:id="d" modelElement="sm">
    <ownedElement xmi:type="umldi:UMLShape" modelElement="note"><bounds x="10" y="200" width="160" height="64"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLShape" modelElement="i"><bounds x="20" y="40" width="20" height="20"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLShape" modelElement="Off"><bounds x="80" y="20" width="140" height="60"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLShape" modelElement="On"><bounds x="300" y="20" width="300" height="200"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLShape" modelElement="Dim"><bounds x="320" y="80" width="100" height="40"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLShape" modelElement="c"><bounds x="500" y="100" width="28" height="28"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLShape" modelElement="A"><bounds x="20" y="300" width="200" height="150"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLShape" modelElement="B"><bounds x="300" y="300" width="200" height="150"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLEdge" modelElement="t3"><waypoint x="370" y="100"/><waypoint x="450" y="150"/><waypoint x="514" y="114"/></ownedElement>
  </umldi:UMLStateMachineDiagram>
</xmi:XMI>
"#;

    /// The base, as a file written by the editors.
    fn base() -> FsmModel {
        from_xmi(&to_xmi(&from_xmi(LAMP).unwrap()).unwrap()).unwrap()
    }

    /// The base edited, then written and read back as a side's file.
    fn side(f: impl FnOnce(&mut FsmModel)) -> FsmModel {
        let mut m = base();
        f(&mut m);
        from_xmi(&to_xmi(&m).unwrap()).unwrap()
    }

    fn v<'m>(m: &'m mut FsmModel, id: &str) -> &'m mut Vertex {
        m.vertices.iter_mut().find(|v| v.id == id).unwrap()
    }

    fn t<'m>(m: &'m mut FsmModel, id: &str) -> &'m mut Transition {
        m.transitions.iter_mut().find(|t| t.id == id).unwrap()
    }

    fn new_state(id: &str, parent: &str, x: i32, y: i32) -> Vertex {
        let mut s = Vertex::new(id, VertexType::State, id, parent);
        (s.x, s.y, s.w, s.h) = (f64::from(x), f64::from(y), 80.0, 40.0);
        s
    }

    fn new_transition(id: &str, source: &str, target: &str) -> Transition {
        let mut tr = base().transitions[0].clone();
        (tr.id, tr.source, tr.target, tr.kind, tr.triggers) =
            (id.to_string(), source.to_string(), target.to_string(), TransitionKind::External, Vec::new());
        tr
    }

    /// Merges and reads the result back.
    fn run(ours: &FsmModel, theirs: &FsmModel) -> (Merge, FsmModel) {
        let r = merge(Some(&base()), ours, theirs).unwrap();
        let m = from_xmi(&r.text).unwrap();
        (r, m)
    }

    fn get<'m>(m: &'m FsmModel, id: &str) -> Option<&'m Vertex> {
        m.vertices.iter().find(|v| v.id == id)
    }

    fn tr<'m>(m: &'m FsmModel, id: &str) -> Option<&'m Transition> {
        m.transitions.iter().find(|t| t.id == id)
    }

    #[test]
    fn identical_sides_give_the_file() {
        let b = base();
        let r = merge(Some(&b), &b, &b).unwrap();
        assert_eq!(r.text, to_xmi(&b).unwrap());
        assert!(r.clean() && r.merged.is_empty() && r.layout.is_empty() && r.issues.is_empty());
        assert_eq!(r.to_text("Lamp.fsm"), "Lamp.fsm: 0 changes merged, 0 conflicts\n");
    }

    #[test]
    fn a_change_on_one_side_or_the_same_on_both_is_taken() {
        let ours = side(|m| {
            v(m, "Off").name = "Dark".into();
            t(m, "t1").guard = "ready()".into();
        });
        let theirs = side(|m| {
            v(m, "Dim").entry = "dim()".into();
            t(m, "t1").guard = "ready()".into();
        });
        let (r, m) = run(&ours, &theirs);
        assert!(r.clean());
        assert_eq!(get(&m, "Off").unwrap().name, "Dark");
        assert_eq!(get(&m, "Dim").unwrap().entry, "dim()");
        assert_eq!(tr(&m, "t1").unwrap().guard, "ready()");
        let from: Vec<(&str, &str)> = r.merged.iter().map(|x| (x.id.as_str(), x.from)).collect();
        assert_eq!(from, [("Off", "ours"), ("t1", "both"), ("Dim", "theirs")]);
        assert_eq!(r.to_text("Lamp.fsm"), "Lamp.fsm: 3 changes merged, 0 conflicts\n");
    }

    #[test]
    fn a_property_set_differently_is_a_conflict_that_keeps_ours() {
        let ours = side(|m| t(m, "t1").guard = "a()".into());
        let theirs = side(|m| t(m, "t1").guard = "b()".into());
        let (r, m) = run(&ours, &theirs);
        assert_eq!(tr(&m, "t1").unwrap().guard, "a()");
        assert_eq!(
            r.to_text("Lamp.fsm"),
            "Lamp.fsm: 0 changes merged, 1 conflict\n  ✕ transition Off → On on press: guard\n      ours    [a()]\n      theirs  [b()]\n"
        );
        let c = &r.conflicts[0];
        assert_eq!(
            (c.kind, c.property, &c.base, &c.ours),
            (ConflictKind::Property, Some("guard"), &Value::text(""), &Value::text("a()"))
        );
    }

    #[test]
    fn a_vertex_retyped_on_one_side_and_changed_on_the_other_keeps_ours() {
        let ours = side(|m| v(m, "Dim").entry = "dim()".into());
        let theirs = side(|m| {
            let d = v(m, "Dim");
            d.vtype = VertexType::Junction;
            d.name = "J".into();
        });
        let (r, m) = run(&ours, &theirs);
        let d = get(&m, "Dim").unwrap();
        assert_eq!((d.vtype, d.name.as_str(), d.entry.as_str()), (VertexType::State, "Dim", "dim()"));
        assert_eq!(r.conflicts[0].property, Some("type"));
        assert!(r.to_text("x").contains("✕ state Dim: type\n      ours    state\n      theirs  junction\n"));
    }

    #[test]
    fn triggers_deferred_events_and_anchors_added_on_both_sides_are_united() {
        let ours = side(|m| {
            t(m, "t1").triggers.push("push".into());
            v(m, "Off").deferrable.push("a".into());
            v(m, "note").anchors.retain(|a| a != "Dim");
        });
        let theirs = side(|m| {
            t(m, "t1").triggers = vec!["tap".into()];
            v(m, "Off").deferrable.push("b".into());
            v(m, "note").anchors.push("On".into());
        });
        let (r, m) = run(&ours, &theirs);
        assert!(r.clean());
        assert_eq!(tr(&m, "t1").unwrap().triggers, ["push", "tap"]); // theirs removed press
        assert_eq!(get(&m, "Off").unwrap().deferrable, ["a", "b"]);
        assert_eq!(get(&m, "note").unwrap().anchors, ["Off", "On"]);
    }

    #[test]
    fn elements_added_on_both_sides_are_kept_base_then_ours_then_theirs() {
        let ours = side(|m| {
            m.vertices.push(new_state("O1", "rOn", 330, 150));
            m.transitions.push(new_transition("to1", "Dim", "O1"));
        });
        let theirs = side(|m| {
            m.vertices.push(new_state("T1", "rOn", 430, 150));
            m.transitions.push(new_transition("tt1", "Dim", "T1"));
        });
        let (r, m) = run(&ours, &theirs);
        assert!(r.clean());
        let in_on: Vec<&str> = m.vertices.iter().filter(|v| v.parent == "rOn").map(|v| v.id.as_str()).collect();
        assert_eq!(in_on, ["Dim", "c", "O1", "T1"]);
        let order: Vec<&str> = m.transitions.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(order, ["t3", "t4", "to1", "tt1", "t0", "t1", "t2"]);
    }

    #[test]
    fn layout_takes_ours_when_both_moved_and_says_so() {
        let ours = side(|m| v(m, "Off").x += 40.0);
        let theirs = side(|m| {
            v(m, "Off").y += 60.0;
            v(m, "Off").colors.color = "#ff0000".into();
        });
        let (r, m) = run(&ours, &theirs);
        assert!(r.clean());
        let off = get(&m, "Off").unwrap();
        assert_eq!((off.x, off.y, off.colors.color.as_str()), (120.0, 20.0, "#ff0000"));
        assert_eq!(
            r.to_text("x"),
            "x: 1 change merged, 0 conflicts\n  · layout: state Off moved on both sides, kept ours\n"
        );
    }

    #[test]
    fn a_state_dragged_alone_leaves_the_other_sides_bend_points_where_that_side_put_them() {
        // Ours drags Dim, t3's source, alone: t3's bend points stay where they are on the canvas. Theirs moves one.
        let ours = side(|m| v(m, "Dim").x -= 20.0);
        let theirs = side(|m| t(m, "t3").points[0] = Point { x: 460.0, y: 170.0 });
        for (a, b) in [(&ours, &theirs), (&theirs, &ours)] {
            let (r, m) = run(a, b);
            assert!(r.clean() && r.layout.is_empty(), "{}", r.to_text("x"));
            assert_eq!(get(&m, "Dim").unwrap().x, 300.0);
            assert_eq!(tr(&m, "t3").unwrap().points, [Point { x: 460.0, y: 170.0 }]);
        }
        // Both moved the bend point: ours is kept and reported.
        let theirs2 = side(|m| t(m, "t3").points[0] = Point { x: 400.0, y: 190.0 });
        let (r, m) = run(&theirs, &theirs2);
        assert_eq!(r.layout.len(), 1);
        assert_eq!(tr(&m, "t3").unwrap().points, [Point { x: 460.0, y: 170.0 }]);
    }

    #[test]
    fn a_child_moved_inside_a_container_moved_on_the_other_side_moves_with_it() {
        // Ours drags On with its contents; theirs moves Dim inside On and adds a bend point to t3.
        let ours = side(|m| {
            for id in ["On", "Dim", "c"] {
                v(m, id).x += 100.0;
                v(m, id).y += 50.0;
            }
            for p in &mut t(m, "t3").points {
                (p.x, p.y) = (p.x + 100.0, p.y + 50.0);
            }
        });
        let theirs = side(|m| {
            v(m, "Dim").y += 30.0;
            t(m, "t3").points.push(Point { x: 480.0, y: 160.0 });
        });
        let (r, m) = run(&ours, &theirs);
        assert!(r.clean() && r.layout.is_empty());
        let on = get(&m, "On").unwrap();
        let dim = get(&m, "Dim").unwrap();
        assert_eq!((on.x, on.y), (400.0, 70.0));
        assert_eq!((dim.x, dim.y), (420.0, 160.0)); // theirs' move, inside ours' On
        assert_eq!(get(&m, "c").unwrap().x, 600.0);
        // Theirs' bend points (the one Dim left where it was, and the new one), relative to Dim (the source), where the
        // merge puts Dim: both moved with On.
        assert_eq!(tr(&m, "t3").unwrap().points, [Point { x: 550.0, y: 200.0 }, Point { x: 580.0, y: 210.0 }]);
    }

    #[test]
    fn deleted_on_ours_and_given_contents_on_theirs_stays_deleted() {
        let ours = side(|m| {
            m.vertices.retain(|v| !["On", "Dim", "c"].contains(&v.id.as_str()));
            m.transitions.retain(|t| !["t1", "t2", "t3", "t4"].contains(&t.id.as_str()));
            v(m, "note").anchors.retain(|a| a != "Dim");
        });
        let theirs = side(|m| {
            m.vertices.push(new_state("Bright", "rOn", 450, 60));
            m.transitions.push(new_transition("tb", "Off", "Dim"));
            v(m, "Dim").exit = "undim()".into();
        });
        let (r, m) = run(&ours, &theirs);
        assert!(!r.clean());
        assert!(get(&m, "On").is_none() && get(&m, "Bright").is_none() && tr(&m, "tb").is_none());
        assert_eq!(
            r.to_text("x"),
            "x: 7 changes merged, 1 conflict\n  ✕ state On: deleted on ours, state Dim changed, state Bright added, transition Off → Dim added on theirs\n      kept deleted: theirs' changes are left out\n"
        );
        let c = &r.conflicts[0];
        assert_eq!((c.kind, c.deleted_on, c.kept), (ConflictKind::Deleted, Some(Side::Ours), false));
    }

    #[test]
    fn deleted_on_theirs_and_changed_on_ours_is_kept_with_its_contents() {
        let ours = side(|m| v(m, "Dim").name = "Low".into());
        let theirs = side(|m| {
            m.vertices.retain(|v| !["On", "Dim", "c"].contains(&v.id.as_str()));
            m.transitions.retain(|t| !["t1", "t2", "t3", "t4"].contains(&t.id.as_str()));
            v(m, "Off").name = "Dark".into();
        });
        let (r, m) = run(&ours, &theirs);
        assert_eq!(r.conflicts.len(), 1);
        for id in ["On", "Dim", "c"] {
            assert!(get(&m, id).is_some(), "{id}");
        }
        for id in ["t1", "t2", "t3", "t4"] {
            assert!(tr(&m, id).is_some(), "{id}");
        }
        assert_eq!((get(&m, "Dim").unwrap().name.as_str(), get(&m, "Off").unwrap().name.as_str()), ("Low", "Dark"));
        assert!(
            r.to_text("x")
                .contains("  ✕ state On: deleted on theirs, state Dim changed on ours\n      kept, as ours has it\n"),
            "{}",
            r.to_text("x")
        );
    }

    #[test]
    fn a_deletion_without_conflict_takes_contents_transitions_and_anchors() {
        let ours = side(|m| v(m, "Off").name = "Dark".into());
        let theirs = side(|m| {
            m.vertices.retain(|v| !["On", "Dim", "c"].contains(&v.id.as_str()));
            m.transitions.retain(|t| !["t1", "t2", "t3", "t4"].contains(&t.id.as_str()));
            v(m, "note").anchors.retain(|a| a != "Dim");
        });
        let (r, m) = run(&ours, &theirs);
        assert!(r.clean());
        assert!(["On", "Dim", "c"].iter().all(|id| get(&m, id).is_none()));
        assert!(m.transitions.iter().all(|t| ["t0"].contains(&t.id.as_str())));
        assert_eq!(get(&m, "note").unwrap().anchors, ["Off"]);
    }

    #[test]
    fn a_transition_deleted_on_one_side_and_changed_on_the_other() {
        let ours = side(|m| m.transitions.retain(|t| t.id != "t1"));
        let theirs = side(|m| t(m, "t1").effect = "beep()".into());
        let (r, m) = run(&ours, &theirs);
        assert!(tr(&m, "t1").is_none());
        assert!(
            r.to_text("x").contains("  ✕ transition Off → On on press: deleted on ours, effect changed on theirs\n")
        );
        let (r, m) = run(&theirs, &ours);
        assert_eq!(tr(&m, "t1").unwrap().effect, "beep()");
        assert!(r.conflicts[0].kept);
    }

    #[test]
    fn states_moved_into_each_other_keep_ours() {
        let ours = side(|m| v(m, "A").parent = "rB".into());
        let theirs = side(|m| v(m, "B").parent = "rA".into());
        let (r, m) = run(&ours, &theirs);
        assert_eq!((get(&m, "A").unwrap().parent.as_str(), get(&m, "B").unwrap().parent.as_str()), ("rB", "r"));
        assert_eq!(r.conflicts.len(), 1);
        assert_eq!(
            r.to_text("x").lines().skip(1).collect::<Vec<_>>(),
            ["  ✕ state B: owner (a containment cycle)", "      ours    the machine", "      theirs  A"]
        );
    }

    #[test]
    fn a_region_deleted_on_ours_loses_what_theirs_put_in_it() {
        let ours = side(|m| {
            m.vertices.retain(|v| v.id != "A");
        });
        let theirs = side(|m| m.vertices.push(new_state("Inner", "rA", 40, 340)));
        let (r, m) = run(&ours, &theirs);
        assert!(get(&m, "Inner").is_none() && get(&m, "A").is_none());
        assert!(r.to_text("x").contains("✕ state A: deleted on ours, state Inner added on theirs"));
    }

    #[test]
    fn issues_neither_side_had_are_reported() {
        let ours = side(|m| m.transitions.push(new_transition("d1", "Off", "A")));
        let theirs = side(|m| m.transitions.push(new_transition("d2", "Off", "B")));
        let (r, _) = run(&ours, &theirs);
        assert!(r.clean());
        let lines: Vec<String> = r.issues.iter().map(|i| format!("{} {} {}", i.id, i.severity, i.message)).collect();
        assert!(
            lines.contains(
                &"d1 warning 'Off' has several completion transitions without triggers or guards.".to_string()
            ),
            "{lines:?}"
        );
        assert!(r.to_text("x").contains("  ! transition Off → A: warning: 'Off' has several completion transitions"));
    }

    #[test]
    fn without_a_base_both_sides_added_everything() {
        let b = base();
        let other = side(|m| v(m, "Off").name = "Dark".into());
        let r = merge(None, &b, &b).unwrap();
        assert!(r.clean());
        assert_eq!(r.text, to_xmi(&b).unwrap());
        let r = merge(None, &b, &other).unwrap();
        assert_eq!(r.conflicts.iter().map(|c| c.property).collect::<Vec<_>>(), [Some("name")]);
    }

    #[test]
    fn different_machines_are_refused() {
        let mut other = base();
        other.id = "sm2".into();
        assert_eq!(
            merge(Some(&base()), &base(), &other).unwrap_err(),
            "ours and theirs are two different state machines (ids 'sm' and 'sm2'): they can't be merged."
        );
        assert!(merge(Some(&other), &base(), &base()).unwrap_err().starts_with("The base is another state machine"));
    }

    #[test]
    fn json_document() {
        let ours = side(|m| t(m, "t1").guard = "a()".into());
        let theirs = side(|m| {
            t(m, "t1").guard = "b()".into();
            v(m, "Off").name = "Dark".into();
        });
        let r = merge(Some(&base()), &ours, &theirs).unwrap();
        let text = r.to_json();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        // Keys in this order.
        let in_order = |keys: &[&str]| {
            let at: Vec<usize> = keys.iter().map(|k| text.find(&format!("\"{k}\":")).unwrap()).collect();
            at.windows(2).all(|w| w[0] < w[1])
        };
        assert!(in_order(&["merged", "id", "element", "status", "label", "from", "conflicts"]));
        assert!(in_order(&[
            "conflicts",
            "conflict",
            "property",
            "base",
            "ours",
            "theirs",
            "deletedOn",
            "region",
            "changes",
            "kept"
        ]));
        assert!(in_order(&["kept", "layout", "issues", "counts"]));
        assert_eq!(json["conflicts"][0]["kept"], true);
        assert_eq!(json["conflicts"][0]["theirs"], "b()");
        assert_eq!(json["counts"]["conflicts"], 1);
        assert_eq!(json["merged"][0]["label"], "State Off");
    }
}
