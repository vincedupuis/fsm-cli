//! UML 2.5.1 well-formedness rules, the text grammar and reachability.

use std::collections::{HashMap, HashSet};

use crate::expressions as expr;
use crate::model::{
    FsmModel, MachineKind, ModelIndex, PointKind, SubmachineInfo, Transition, TransitionKind, VertexType, js_space,
    js_trim, unresolved,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Issue {
    /// Id of the offending vertex or transition ('' for the machine itself).
    pub id: String,
    pub severity: Severity,
    pub message: String,
}

fn type_label(t: VertexType) -> &'static str {
    match t {
        VertexType::State => "state",
        VertexType::Final => "final state",
        VertexType::Initial => "initial pseudostate",
        VertexType::ShallowHistory => "shallow history",
        VertexType::DeepHistory => "deep history",
        VertexType::Choice => "choice",
        VertexType::Junction => "junction",
        VertexType::Fork => "fork",
        VertexType::Join => "join",
        VertexType::EntryPoint => "entry point",
        VertexType::ExitPoint => "exit point",
        VertexType::ConnectionPointRef => "connection point reference",
        VertexType::Terminate => "terminate",
        VertexType::Comment => "comment",
    }
}

struct Checker<'a> {
    model: &'a FsmModel,
    ix: ModelIndex<'a>,
    submachines: &'a HashMap<String, SubmachineInfo>,
    issues: Vec<Issue>,
}

impl Checker<'_> {
    fn add(&mut self, id: &str, severity: Severity, message: String) {
        self.issues.push(Issue { id: id.to_string(), severity, message });
    }

    fn name(&self, v: Option<usize>) -> String {
        match v {
            None => "?".to_string(),
            Some(v) => {
                let v = self.ix.v(v);
                if v.name.is_empty() { type_label(v.vtype).to_string() } else { format!("'{}'", v.name) }
            }
        }
    }

    fn machine_name(&self, href: &str) -> String {
        match self.submachines.get(href).map(|s| s.name.as_str()) {
            Some(n) if !n.is_empty() => format!("'{n}'"),
            _ => format!("'{href}'"),
        }
    }
}

/// Checks the UML 2.5.1 well-formedness rules that apply to state machines.
/// `submachines` describes the machines referenced by submachine states, when known.
pub fn validate(model: &FsmModel, submachines: &HashMap<String, SubmachineInfo>) -> Vec<Issue> {
    let mut c = Checker { model, ix: ModelIndex::new(model), submachines, issues: Vec::new() };
    let protocol = model.kind == MachineKind::Protocol;
    use Severity::*;

    let all_regions: Vec<_> =
        model.regions.iter().chain(model.vertices.iter().flat_map(|v| v.regions.iter())).collect();
    check_names(&mut c);

    // Containment
    for (vi, v) in model.vertices.iter().enumerate() {
        match v.vtype {
            VertexType::EntryPoint | VertexType::ExitPoint => {
                // Either on the border of a state, or a connection point of the machine itself (top-level region).
                let owner = c.ix.vertex(&v.parent);
                let machine_level = model.regions.iter().any(|r| r.id == v.parent);
                if machine_level {
                    // The machine's own connection point.
                } else if owner.is_none_or(|o| c.ix.v(o).vtype != VertexType::State) {
                    let msg = format!(
                        "{}: entry and exit points belong on the border of a state or at the top level of the state machine.",
                        c.name(Some(vi))
                    );
                    c.add(&v.id, Error, msg);
                } else if let Some(sub) = &c.ix.v(owner.unwrap()).submachine {
                    let msg = format!(
                        "A submachine state cannot own entry/exit points; use a connection point reference to a point of {} instead.",
                        c.machine_name(sub)
                    );
                    c.add(&v.id, Error, msg);
                } else if c.ix.v(owner.unwrap()).regions.is_empty() {
                    let msg =
                        format!("Only composite states can have entry/exit points; add a region to {}.", c.name(owner));
                    c.add(&v.id, Error, msg);
                }
            }
            VertexType::ConnectionPointRef => {
                let owner = c.ix.vertex(&v.parent).map(|o| c.ix.v(o));
                if owner.is_none_or(|o| o.vtype != VertexType::State || o.submachine.is_none()) {
                    c.add(
                        &v.id,
                        Error,
                        "A connection point reference must be placed on the border of a submachine state.".into(),
                    );
                }
            }
            _ => {
                if c.ix.region_owner(&v.parent).is_none() {
                    let msg = format!("{} is not contained in a known region.", c.name(Some(vi)));
                    c.add(&v.id, Error, msg);
                }
            }
        }
    }

    // Regions
    for r in all_regions {
        let kids: Vec<&crate::model::Vertex> = c.ix.children_of(&r.id).into_iter().map(|k| c.ix.v(k)).collect();
        let count = |t: VertexType| kids.iter().filter(|k| k.vtype == t).map(|k| k.id.as_str()).collect::<Vec<_>>();
        let owner = c.ix.region_owner(&r.id).flatten();
        let where_ = match owner {
            Some(_) => format!("region of {}", c.name(owner)),
            None => "top-level region".to_string(),
        };
        for (t, label) in [
            (VertexType::Initial, "initial pseudostate"),
            (VertexType::ShallowHistory, "shallow history"),
            (VertexType::DeepHistory, "deep history"),
        ] {
            for id in count(t).into_iter().skip(1) {
                c.add(id, Error, format!("A {where_} can contain at most one {label}."));
            }
        }
        let histories: Vec<&str> = kids
            .iter()
            .filter(|k| matches!(k.vtype, VertexType::ShallowHistory | VertexType::DeepHistory))
            .map(|k| k.id.as_str())
            .collect();
        if !histories.is_empty() && owner.is_none() {
            for id in histories {
                c.add(id, Error, "A history pseudostate can only be inside a composite state.".into());
            }
        }
    }

    // Vertices
    for (vi, v) in model.vertices.iter().enumerate() {
        let out: Vec<_> = c.ix.outgoing(&v.id).into_iter().filter(|t| t.kind != TransitionKind::Internal).collect();
        let inc: Vec<_> = c.ix.incoming(&v.id).into_iter().filter(|t| t.kind != TransitionKind::Internal).collect();
        match v.vtype {
            VertexType::Initial => {
                if out.is_empty() || (out.len() == 1 && unresolved(&out[0].target) == Some("")) {
                    c.add(&v.id, Error, "The initial pseudostate has no target.".into());
                } else if out.len() != 1 {
                    c.add(&v.id, Error, "An initial pseudostate must have exactly one outgoing transition.".into());
                }
                if !inc.is_empty() {
                    c.add(&v.id, Error, "An initial pseudostate cannot have incoming transitions.".into());
                }
                for t in &out {
                    if !t.guard.is_empty() {
                        c.add(
                            &t.id,
                            Error,
                            "The transition leaving an initial pseudostate cannot have a guard.".into(),
                        );
                    }
                }
            }
            VertexType::Final => {
                if !out.is_empty() {
                    c.add(&v.id, Error, "A final state cannot have outgoing transitions.".into());
                }
                if !v.regions.is_empty() || !v.entry.is_empty() || !v.exit.is_empty() || !v.do_activity.is_empty() {
                    c.add(&v.id, Error, "A final state cannot have regions or entry/exit/do behaviors.".into());
                }
            }
            VertexType::Terminate => {
                if !out.is_empty() {
                    c.add(&v.id, Error, "A terminate pseudostate cannot have outgoing transitions.".into());
                }
            }
            VertexType::ShallowHistory | VertexType::DeepHistory => {
                if protocol {
                    c.add(&v.id, Error, "A protocol state machine cannot have history pseudostates.".into());
                }
                if out.len() > 1 {
                    c.add(
                        &v.id,
                        Error,
                        "A history pseudostate can have at most one outgoing (default) transition.".into(),
                    );
                }
            }
            VertexType::Fork => {
                if inc.len() != 1 {
                    c.add(&v.id, Error, "A fork must have exactly one incoming transition.".into());
                }
                if out.len() < 2 {
                    c.add(&v.id, Error, "A fork must have at least two outgoing transitions.".into());
                }
                for t in &out {
                    // A protocol transition's precondition is its guard.
                    if !t.guard.is_empty() || !t.triggers.is_empty() || (protocol && !t.precondition.is_empty()) {
                        let what = if protocol { "preconditions" } else { "guards" };
                        c.add(&t.id, Error, format!("Transitions leaving a fork cannot have {what} or triggers."));
                    }
                    if let Some(g) = c.ix.vertex(&t.target)
                        && !matches!(c.ix.v(g).vtype, VertexType::State | VertexType::Final)
                    {
                        c.add(&t.id, Error, "Transitions leaving a fork must target a state.".into());
                    }
                }
                let regions: Vec<Option<&str>> =
                    out.iter().map(|t| c.ix.vertex(&t.target).map(|g| c.ix.v(g).parent.as_str())).collect();
                if distinct(&regions) != regions.len() {
                    c.add(&v.id, Error, "The targets of a fork must be in different orthogonal regions.".into());
                }
            }
            VertexType::Join => {
                if out.len() != 1 {
                    c.add(&v.id, Error, "A join must have exactly one outgoing transition.".into());
                }
                if inc.len() < 2 {
                    c.add(&v.id, Error, "A join must have at least two incoming transitions.".into());
                }
                for t in &inc {
                    if !t.guard.is_empty() || !t.triggers.is_empty() || (protocol && !t.precondition.is_empty()) {
                        let what = if protocol { "preconditions" } else { "guards" };
                        c.add(&t.id, Error, format!("Transitions entering a join cannot have {what} or triggers."));
                    }
                    if let Some(s) = c.ix.vertex(&t.source)
                        && c.ix.v(s).vtype != VertexType::State
                    {
                        c.add(&t.id, Error, "Transitions entering a join must come from a state.".into());
                    }
                }
                let regions: Vec<Option<&str>> =
                    inc.iter().map(|t| c.ix.vertex(&t.source).map(|s| c.ix.v(s).parent.as_str())).collect();
                if distinct(&regions) != regions.len() {
                    c.add(&v.id, Error, "The sources of a join must be in different orthogonal regions.".into());
                }
            }
            VertexType::Choice | VertexType::Junction => {
                let kind = v.vtype.as_str();
                if inc.is_empty() {
                    c.add(&v.id, Error, format!("A {kind} must have at least one incoming transition."));
                }
                if out.is_empty() {
                    c.add(&v.id, Error, format!("A {kind} must have at least one outgoing transition."));
                }
                let what = if protocol { "precondition" } else { "guard" };
                let unguarded = out.iter().any(|t| js_trim(c.ix.selector(t)).is_empty());
                if out.len() > 1 {
                    // An unguarded branch is always true: with others, which one is taken would be arbitrary.
                    if unguarded {
                        c.add(
                            &v.id,
                            Error,
                            format!(
                                "Every branch of a {kind} with several outgoing transitions needs a {what}: use [else] for the default branch."
                            ),
                        );
                    }
                    let elses: Vec<_> = out.iter().filter(|t| js_trim(c.ix.selector(t)) == "else").collect();
                    for t in elses.iter().skip(1) {
                        c.add(&t.id, Error, format!("A {kind} can have at most one [else] branch."));
                    }
                }
                // A choice is reached after its source was left: with no branch to take, the machine would be stuck.
                if v.vtype == VertexType::Choice
                    && !out.is_empty()
                    && !unguarded
                    && !out.iter().any(|t| js_trim(c.ix.selector(t)) == "else")
                {
                    c.add(&v.id, Error, format!("A choice needs an [else] branch, taken when no other {what} holds."));
                }
            }
            VertexType::EntryPoint => {
                let owner = c.ix.vertex(&v.parent);
                // Every transition leaving an entry point is taken, as a fork's are.
                for t in &out {
                    if !t.guard.is_empty() || (protocol && !t.precondition.is_empty()) {
                        let what = if protocol { "preconditions" } else { "guards" };
                        c.add(&t.id, Error, format!("Transitions leaving an entry point cannot have {what}."));
                    }
                }
                // At most one transition into each region of its state (of the machine, for the machine's own).
                let mut regions = HashSet::new();
                for t in &out {
                    let Some(target) = c.ix.vertex(&t.target) else { continue };
                    if let Some(r) = region_within(&c, target, owner)
                        && !regions.insert(r)
                    {
                        let msg = format!(
                            "Entry point {} can have at most one transition into each region.",
                            c.name(Some(vi))
                        );
                        c.add(&t.id, Error, msg);
                    }
                }
                if owner.is_none() && out.is_empty() {
                    let msg =
                        format!("Entry point {} of the state machine has no outgoing transition.", c.name(Some(vi)));
                    c.add(&v.id, Error, msg);
                }
                if let Some(o) = owner {
                    for t in &out {
                        if let Some(target) = c.ix.vertex(&t.target)
                            && !c.ix.is_inside(target, &c.ix.v(o).id)
                        {
                            let msg = format!(
                                "Transitions leaving entry point {} must target a vertex inside {}.",
                                c.name(Some(vi)),
                                c.name(Some(o))
                            );
                            c.add(&t.id, Error, msg);
                        }
                    }
                }
            }
            VertexType::ExitPoint => {
                let owner = c.ix.vertex(&v.parent);
                // Reached from several orthogonal regions, an exit point acts as a join (UML 14.2.3.7).
                let regions: HashSet<String> = inc
                    .iter()
                    .filter_map(|t| c.ix.vertex(&t.source))
                    .filter_map(|s| region_within(&c, s, owner))
                    .collect();
                if regions.len() > 1 {
                    for t in &inc {
                        if !t.guard.is_empty() || !t.triggers.is_empty() || (protocol && !t.precondition.is_empty()) {
                            let what = if protocol { "preconditions" } else { "guards" };
                            let msg = format!(
                                "Exit point {} is reached from several orthogonal regions, so it acts as a join: the transitions entering it cannot have {what} or triggers.",
                                c.name(Some(vi))
                            );
                            c.add(&t.id, Error, msg);
                        }
                        if let Some(s) = c.ix.vertex(&t.source)
                            && c.ix.v(s).vtype != VertexType::State
                        {
                            let msg = format!(
                                "Exit point {} is reached from several orthogonal regions, so it acts as a join: the transitions entering it must come from a state.",
                                c.name(Some(vi))
                            );
                            c.add(&t.id, Error, msg);
                        }
                    }
                }
                if owner.is_none() && !out.is_empty() {
                    for t in &out {
                        let msg = format!(
                            "Exit point {} of the state machine cannot have outgoing transitions; the referencing submachine state continues from its connection point reference.",
                            c.name(Some(vi))
                        );
                        c.add(&t.id, Error, msg);
                    }
                }
                if let Some(o) = owner
                    && !inc.is_empty()
                    && out.is_empty()
                {
                    let msg = format!(
                        "Exit point {} has no outgoing transition: leaving {} through it would enter no state.",
                        c.name(Some(vi)),
                        c.name(Some(o))
                    );
                    c.add(&v.id, Error, msg);
                }
                if let Some(o) = owner {
                    for t in &out {
                        if let Some(target) = c.ix.vertex(&t.target) {
                            let oid = &c.ix.v(o).id;
                            if c.ix.v(target).id == *oid || c.ix.is_inside(target, oid) {
                                let msg = format!(
                                    "Transitions leaving exit point {} must target a vertex outside {}.",
                                    c.name(Some(vi)),
                                    c.name(Some(o))
                                );
                                c.add(&t.id, Error, msg);
                            }
                        }
                    }
                }
            }
            VertexType::ConnectionPointRef => {
                let owner = c.ix.vertex(&v.parent);
                let kind = v.point_kind;
                let owner_sub = owner.and_then(|o| c.ix.v(o).submachine.clone());
                let info = owner_sub.as_ref().and_then(|s| c.submachines.get(s));
                let target = info.filter(|i| i.found).and_then(|i| i.points.iter().find(|p| p.id == v.point_ref));
                let label = match target {
                    Some(t) if !t.name.is_empty() => format!("'{}'", t.name),
                    _ => "connection point reference".to_string(),
                };
                if v.point_ref.is_empty() {
                    c.add(
                        &v.id,
                        Error,
                        "The connection point reference does not refer to an entry or exit point of the submachine."
                            .into(),
                    );
                } else if let (Some(sub), Some(true)) = (&owner_sub, info.map(|i| i.found)) {
                    match target {
                        None => {
                            let msg =
                                format!("{} has no entry or exit point named '{}'.", c.machine_name(sub), v.point_ref);
                            c.add(&v.id, Error, msg);
                        }
                        Some(t) if t.kind != kind => {
                            let msg = format!(
                                "{label} is an {} point of {}, but it is referenced as an {} point.",
                                t.kind.as_str(),
                                c.machine_name(sub),
                                kind.as_str()
                            );
                            c.add(&v.id, Error, msg);
                        }
                        _ => {}
                    }
                }
                if kind == PointKind::Entry {
                    if !out.is_empty() {
                        c.add(&v.id, Error, format!("Entry reference {label} cannot have outgoing transitions; execution continues at the entry point inside the submachine."));
                    }
                    if inc.is_empty() {
                        c.add(&v.id, Error, format!("Entry reference {label} has no incoming transition."));
                    }
                    for t in &inc {
                        if let (Some(src), Some(o)) = (c.ix.vertex(&t.source), owner) {
                            let oid = &c.ix.v(o).id;
                            if c.ix.v(src).id == *oid || c.ix.is_inside(src, oid) {
                                let msg = format!(
                                    "Transitions into entry reference {label} must come from outside {}.",
                                    c.name(Some(o))
                                );
                                c.add(&t.id, Error, msg);
                            }
                        }
                    }
                } else {
                    if !inc.is_empty() {
                        c.add(&v.id, Error, format!("Exit reference {label} cannot have incoming transitions; it is reached when the submachine leaves through its exit point."));
                    }
                    if out.is_empty() {
                        c.add(&v.id, Error, format!("Exit reference {label} has no outgoing transition."));
                    }
                }
                if let Some(o) = owner {
                    let oid = &c.ix.v(o).id;
                    let dup = model.vertices.iter().position(|x| {
                        !std::ptr::eq(x, v)
                            && x.vtype == VertexType::ConnectionPointRef
                            && x.parent == *oid
                            && !x.point_ref.is_empty()
                            && x.point_ref == v.point_ref
                    });
                    if dup.is_some_and(|d| d < vi) {
                        let msg = format!("{} already references {label}.", c.name(Some(o)));
                        c.add(&v.id, Error, msg);
                    }
                }
            }
            VertexType::State => {
                if let Some(sub) = &v.submachine {
                    if c.submachines.get(sub).is_some_and(|i| !i.found) {
                        c.add(&v.id, Error, format!("The referenced state machine '{sub}' was not found."));
                    }
                    if !v.regions.is_empty() {
                        let msg = format!("Submachine state {} cannot also own regions.", c.name(Some(vi)));
                        c.add(&v.id, Error, msg);
                    }
                }
                if protocol && (!v.entry.is_empty() || !v.exit.is_empty() || !v.do_activity.is_empty()) {
                    let msg = format!(
                        "States of a protocol state machine cannot have entry/exit/do behaviors ({}).",
                        c.name(Some(vi))
                    );
                    c.add(&v.id, Error, msg);
                }
            }
            VertexType::Comment => {}
        }
    }

    // Transitions
    for t in &model.transitions {
        let s = c.ix.vertex(&t.source);
        let g = c.ix.vertex(&t.target);
        if s.is_none() {
            c.add(&t.id, Error, format!("Transition source '{}' does not exist.", t.source));
        }
        if g.is_none() {
            match unresolved(&t.target) {
                // An initial without target, reported on the initial pseudostate.
                Some("") if s.is_some_and(|s| c.ix.v(s).vtype == VertexType::Initial) => {}
                Some("") => {
                    c.add(&t.id, Error, "The default transition of a history pseudostate has no target.".into())
                }
                Some(r) => c.add(&t.id, Error, format!("Transition target '{r}' does not exist.")),
                None => c.add(&t.id, Error, format!("Transition target '{}' does not exist.", t.target)),
            }
        }
        let (Some(s), Some(g)) = (s, g) else { continue };
        let (sv, gv) = (c.ix.v(s), c.ix.v(g));
        if sv.vtype == VertexType::Comment || gv.vtype == VertexType::Comment {
            c.add(&t.id, Error, "Comments cannot be connected by transitions.".into());
            continue;
        }
        if (sv.vtype.is_pseudostate() || sv.vtype == VertexType::ConnectionPointRef) && !t.triggers.is_empty() {
            let msg = format!("Transitions leaving a pseudostate cannot have triggers (from {}).", c.name(Some(s)));
            c.add(&t.id, Error, msg);
        }
        if t.kind == TransitionKind::Internal && (s != g || sv.vtype != VertexType::State) {
            c.add(&t.id, Error, "An internal transition must start and end on the same state.".into());
        }
        // UML's state_is_external: a transition leaving an entry point stays inside its state (or machine).
        if sv.vtype == VertexType::EntryPoint && t.kind == TransitionKind::External {
            c.add(&t.id, Error, "Transitions leaving an entry point must be local.".into());
        }
        if t.kind == TransitionKind::Local {
            let composite = sv.vtype == VertexType::State && !sv.regions.is_empty();
            let container = if sv.vtype == VertexType::EntryPoint { &sv.parent } else { &sv.id };
            // An entry point's targets are checked with the entry point (inside its state; anywhere in the machine).
            if !(composite || sv.vtype == VertexType::EntryPoint)
                || !(sv.vtype == VertexType::EntryPoint || c.ix.is_inside(g, container))
            {
                c.add(
                    &t.id,
                    Error,
                    "A local transition must go from a composite state to a vertex it contains.".into(),
                );
            }
        }
        if protocol {
            if !t.action.is_empty() {
                c.add(&t.id, Error, "Protocol transitions cannot have actions.".into());
            }
            if !t.guard.is_empty() {
                c.add(&t.id, Error, "Protocol transitions cannot have guards: use a precondition.".into());
            }
        } else if !t.precondition.is_empty() || !t.postcondition.is_empty() {
            c.add(&t.id, Error, "Preconditions and postconditions only apply to protocol state machines.".into());
        }
    }

    if protocol && js_trim(&model.context).is_empty() {
        c.add(
            "",
            Error,
            "A protocol state machine needs a context: the name of the classifier whose calls it describes.".into(),
        );
    }
    check_unguarded_conflicts(&mut c);
    check_orthogonal_regions(&mut c);
    check_pseudostate_cycles(&mut c);
    check_text(&mut c);
    check_reachability(&mut c);
    check_default_entries(&mut c);
    c.issues
}

/// Pseudostates that transitions go through in a cycle: a compound transition taking the cycle would never reach a
/// state, and the code generator can't express it. One error per group of pseudostates linked by cycles, on its first
/// vertex, naming the shortest cycle through it.
fn check_pseudostate_cycles(c: &mut Checker) {
    let model = c.model;
    let ix = &c.ix;
    // The pseudostates a compound transition goes on from (the machine's own entry and exit points end it).
    let passes = |v: usize| {
        let v = ix.v(v);
        match v.vtype {
            VertexType::Junction
            | VertexType::Choice
            | VertexType::ShallowHistory
            | VertexType::DeepHistory
            | VertexType::Fork => true,
            VertexType::EntryPoint | VertexType::ExitPoint => ix.has_vertex(&v.parent),
            _ => false,
        }
    };
    let next = |v: usize| -> Vec<usize> {
        ix.outgoing(&ix.v(v).id)
            .iter()
            .filter(|t| t.kind != TransitionKind::Internal)
            .filter_map(|t| ix.vertex(&t.target))
            .filter(|&w| passes(w))
            .collect()
    };
    let previous = |v: usize| -> Vec<usize> {
        ix.incoming(&ix.v(v).id)
            .iter()
            .filter(|t| t.kind != TransitionKind::Internal)
            .filter_map(|t| ix.vertex(&t.source))
            .filter(|&w| passes(w))
            .collect()
    };
    // `start` and every vertex reached from it through `step`, breadth first, with the vertex each was reached from.
    let walk = |start: usize, step: &dyn Fn(usize) -> Vec<usize>| {
        let mut from: HashMap<usize, Option<usize>> = HashMap::from([(start, None)]);
        let mut queue = vec![start];
        let mut i = 0;
        while i < queue.len() {
            for w in step(queue[i]) {
                if let std::collections::hash_map::Entry::Vacant(e) = from.entry(w) {
                    e.insert(Some(queue[i]));
                    queue.push(w);
                }
            }
            i += 1;
        }
        (queue, from)
    };
    let mut done: HashSet<usize> = HashSet::new();
    let mut found: Vec<(usize, Vec<usize>)> = Vec::new();
    for (v, vertex) in model.vertices.iter().enumerate() {
        // A vertex whose id is used again later is not the one transitions refer to.
        if !passes(v) || ix.vertex(&vertex.id) != Some(v) || done.contains(&v) {
            continue;
        }
        let (order, from) = walk(v, &next);
        // The shortest cycle through v ends with the first vertex reached that has a transition back to v.
        let Some(last) = order.iter().copied().find(|&u| next(u).contains(&v)) else { continue };
        let (_, backward) = walk(v, &previous);
        done.extend(order.iter().copied().filter(|u| backward.contains_key(u)));
        let mut cycle = vec![v];
        let mut u = Some(last);
        while let Some(x) = u.filter(|&x| x != v) {
            cycle.insert(1, x);
            u = from[&x];
        }
        cycle.push(v);
        found.push((v, cycle));
    }
    for (v, cycle) in found {
        let path: Vec<String> = cycle.iter().map(|&x| c.name(Some(x))).collect();
        let types: Vec<VertexType> = cycle.iter().map(|&x| model.vertices[x].vtype).collect();
        let msg = format!("{} form a cycle without a state: {}.", cycle_kinds(&types), path.join(" -> "));
        c.add(&model.vertices[v].id, Severity::Error, msg);
    }
}

/// The kinds of pseudostates in `cycle`: `Junctions`, `Junctions and choices`, ... (as in the code generator's error).
fn cycle_kinds(cycle: &[VertexType]) -> String {
    use VertexType::*;
    let kinds: [(&str, &[VertexType]); 6] = [
        ("junctions", &[Junction]),
        ("choices", &[Choice]),
        ("history pseudostates", &[ShallowHistory, DeepHistory]),
        ("entry points", &[EntryPoint]),
        ("exit points", &[ExitPoint]),
        ("forks", &[Fork]),
    ];
    let found: Vec<&str> =
        kinds.iter().filter(|(_, types)| cycle.iter().any(|t| types.contains(t))).map(|(kind, _)| *kind).collect();
    let text = match found.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => found.first().copied().unwrap_or_default().to_string(),
    };
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |f| f.to_uppercase().collect::<String>() + chars.as_str())
}

fn distinct(items: &[Option<&str>]) -> usize {
    items.iter().collect::<HashSet<_>>().len()
}

/// Names are the identity of elements in the file: the rules of a name (no spaces, as ids and trace lines are
/// space-separated words), names unique in the machine (the machine's,
/// the regions' when their owner has several, the elements'), point names unique on their owner.
fn check_names(c: &mut Checker) {
    use Severity::Error;
    let model = c.model;
    // The rules of one name; false when it breaks one (it is then not compared with the others).
    let rules = |c: &mut Checker, id: &str, name: &str, label: &str| -> bool {
        if name.is_empty() {
            c.add(id, Error, format!("Every {label} needs a name."));
            return false;
        }
        if name.contains('.') {
            c.add(id, Error, format!("Name '{name}' cannot contain '.'."));
            return false;
        }
        if name.chars().any(js_space) {
            c.add(id, Error, format!("Name '{name}' cannot contain spaces."));
            return false;
        }
        let last = name.chars().last().map_or(0, char::len_utf8);
        if name[..name.len() - last].contains('*') {
            c.add(id, Error, format!("Name '{name}' can contain '*' only as its last character."));
            return false;
        }
        true
    };
    let mut names: HashSet<String> = HashSet::new();
    let mut unique = |c: &mut Checker, id: &str, name: &str| {
        if !names.insert(name.to_string()) {
            c.add(id, Error, format!("Name '{name}' is used more than once in the state machine."));
        }
    };
    if rules(c, "", &model.name, "state machine") {
        unique(c, "", &model.name);
    }
    let region_names =
        |c: &mut Checker, regions: &[crate::model::Region], unique: &mut dyn FnMut(&mut Checker, &str, &str)| {
            if regions.len() < 2 {
                return;
            }
            for r in regions {
                if rules(c, &r.id, &r.name, "region") {
                    unique(c, &r.id, &r.name);
                }
            }
        };
    region_names(c, &model.regions, &mut unique);
    let mut points: HashSet<(&str, &str)> = HashSet::new();
    for v in &model.vertices {
        match v.vtype {
            VertexType::Initial | VertexType::ShallowHistory | VertexType::DeepHistory => {}
            VertexType::EntryPoint | VertexType::ExitPoint | VertexType::ConnectionPointRef => {
                let reference = v.vtype == VertexType::ConnectionPointRef;
                let name = if reference { v.point_ref.as_str() } else { v.name.as_str() };
                // A reference without a point is reported with the references.
                if (reference && name.is_empty()) || !rules(c, &v.id, name, type_label(v.vtype)) {
                    continue;
                }
                // `<owner>.initial`, `<owner>.history` and `<owner>.deepHistory` are references to pseudostates.
                if name == "initial" || name == "history" || name == "deepHistory" {
                    c.add(&v.id, Error, format!("A point cannot be named '{name}'."));
                } else if !points.insert((v.parent.as_str(), name)) {
                    let owner = match c.ix.vertex(&v.parent) {
                        Some(o) => c.name(Some(o)),
                        None => format!("'{}'", model.name),
                    };
                    c.add(&v.id, Error, format!("{owner} has two points named '{name}'."));
                }
            }
            _ => {
                if rules(c, &v.id, &v.name, type_label(v.vtype)) {
                    unique(c, &v.id, &v.name);
                }
                region_names(c, &v.regions, &mut unique);
            }
        }
    }
}

/// Behaviors and conditions are argument-less function calls; triggers are event names or after(...).
fn check_text(c: &mut Checker) {
    let model = c.model;
    let protocol = model.kind == MachineKind::Protocol;
    let report = |c: &mut Checker, id: &str, what: &str, r: Result<(), String>| {
        if let Err(e) = r {
            c.add(id, Severity::Error, format!("{what}: {e}"));
        }
    };
    report(c, "", "Context of the state machine", expr::check_name(&model.context));
    for (vi, v) in model.vertices.iter().enumerate() {
        if v.vtype != VertexType::State {
            continue;
        }
        let n = c.name(Some(vi));
        report(c, &v.id, &format!("entry of {n}"), expr::check_actions(&v.entry));
        report(c, &v.id, &format!("exit of {n}"), expr::check_actions(&v.exit));
        report(c, &v.id, &format!("do of {n}"), expr::check_actions(&v.do_activity));
        report(c, &v.id, &format!("Invariant of {n}"), expr::check_condition(&v.invariant));
        state_tests(c, &v.id, &format!("Invariant of {n}"), &v.invariant);
        if !v.stereotype.is_empty() {
            report(c, &v.id, &format!("Stereotype of {n}"), expr::check_name(&v.stereotype));
        }
        for d in &v.deferrable {
            report(c, &v.id, &format!("Deferrable event of {n}"), expr::check_event(d));
        }
    }
    for t in &model.transitions {
        let s = c.ix.vertex(&t.source).map(|s| c.ix.v(s).vtype);
        for trig in &t.triggers {
            report(c, &t.id, "Trigger", expr::check_trigger(trig));
        }
        let allow_else = matches!(s, Some(VertexType::Choice | VertexType::Junction));
        report(c, &t.id, "Guard", expr::check_guard(&t.guard, allow_else));
        report(c, &t.id, "Action", expr::check_actions(&t.action));
        // A protocol transition's precondition is its guard: `else` on a choice's or junction's branch.
        let pre = if protocol {
            expr::check_guard(&t.precondition, allow_else)
        } else {
            expr::check_condition(&t.precondition)
        };
        report(c, &t.id, "Precondition", pre);
        report(c, &t.id, "Postcondition", expr::check_condition(&t.postcondition));
        state_tests(c, &t.id, "Guard", &t.guard);
        state_tests(c, &t.id, "Precondition", &t.precondition);
        state_tests(c, &t.id, "Postcondition", &t.postcondition);
        let times = t.triggers.iter().any(|x| expr::time_trigger_ms(x).is_some());
        if times && s.is_some_and(|s| s != VertexType::State) {
            c.add(
                &t.id,
                Severity::Error,
                "Time triggers (after) can only be used on transitions leaving a state.".into(),
            );
        }
    }
}

/// A state test `in(S)` names a state of this machine.
fn state_tests(c: &mut Checker, id: &str, what: &str, text: &str) {
    for name in expr::state_tests(text) {
        let known =
            c.model.vertices.iter().any(|v| v.name == name && matches!(v.vtype, VertexType::State | VertexType::Final));
        if !known {
            c.add(id, Severity::Error, format!("{what}: '{name}' is not a state of the state machine."));
        }
    }
}

/// The region of `owner` (None: the machine's top-level regions) holding vertex `v`, itself or through the states
/// around it.
fn region_within(c: &Checker, v: usize, owner: Option<usize>) -> Option<String> {
    std::iter::once(v).chain(c.ix.ancestors(v)).find_map(|w| {
        let parent = &c.ix.v(w).parent;
        (c.ix.region_owner(parent) == Some(owner)).then(|| parent.clone())
    })
}

/// The containers of `v`, innermost first: regions and states, by id (a point on a state's border is in the state,
/// in none of its regions).
fn containers(c: &Checker, v: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // The machine's own entry and exit points are on its border, in none of its regions.
    let vx = c.ix.v(v);
    if matches!(vx.vtype, VertexType::EntryPoint | VertexType::ExitPoint) && !c.ix.has_vertex(&vx.parent) {
        return out;
    }
    let mut cur = v;
    loop {
        let parent = &c.ix.v(cur).parent;
        if out.contains(parent) {
            break;
        }
        if let Some(owner) = c.ix.region_owner(parent) {
            out.push(parent.clone());
            let Some(o) = owner else { break };
            if out.contains(&c.ix.v(o).id) {
                break;
            }
            out.push(c.ix.v(o).id.clone());
            cur = o;
        } else if let Some(o) = c.ix.vertex(parent) {
            out.push(parent.clone());
            cur = o;
        } else {
            break;
        }
    }
    out
}

/// UML 14.2.3.9.6: no transition from one region to another of the same state (nor between top-level regions).
fn check_orthogonal_regions(c: &mut Checker) {
    let model = c.model;
    let mut found = Vec::new();
    for t in &model.transitions {
        let (Some(s), Some(g)) = (c.ix.vertex(&t.source), c.ix.vertex(&t.target)) else { continue };
        if t.kind == TransitionKind::Internal || s == g {
            continue;
        }
        let (cs, cg) = (containers(c, s), containers(c, g));
        let common = cs.iter().position(|x| cg.contains(x));
        let crossing = match common {
            // Both in top-level regions, but not the same one.
            None => {
                cs.last().is_some_and(|x| model.regions.iter().any(|r| r.id == *x))
                    && cg.last().is_some_and(|x| model.regions.iter().any(|r| r.id == *x))
            }
            // The innermost common container is a state, and each side is in one of its regions (not on its border).
            Some(i) => {
                let a = &cs[i];
                let j = cg.iter().position(|x| x == a).unwrap();
                c.ix.has_vertex(a)
                    && i > 0
                    && j > 0
                    && c.ix.region_owner(&cs[i - 1]).is_some()
                    && c.ix.region_owner(&cg[j - 1]).is_some()
            }
        };
        if crossing {
            let owner = common.map(|i| cs[i].clone());
            found.push((t.id.clone(), owner));
        }
    }
    for (id, owner) in found {
        let of = match owner.and_then(|o| c.ix.vertex(&o)) {
            Some(o) => c.name(Some(o)),
            None => "the state machine".to_string(),
        };
        c.add(&id, Severity::Error, format!("A transition cannot go from one orthogonal region of {of} to another."));
    }
}

/// Transitions that could fire together without a guard to choose between them: two leaving the same state on the
/// same trigger (or several completion transitions), and two in orthogonal regions that leave the same state on the
/// same event.
fn check_unguarded_conflicts(c: &mut Checker) {
    let model = c.model;
    let protocol = model.kind == MachineKind::Protocol;
    let what = if protocol { "preconditions" } else { "guards" };
    // A transition leaving a state is selected by its guard, or in a protocol machine by its precondition.
    let unguarded = |t: &Transition| js_trim(if protocol { &t.precondition } else { &t.guard }).is_empty();
    let mut found: Vec<(String, String)> = Vec::new();
    // Same state, same trigger. A completion transition has no trigger (key ""); time events by their delay.
    for (si, sv) in model.vertices.iter().enumerate() {
        if sv.vtype != VertexType::State {
            continue;
        }
        let mut seen: HashMap<String, usize> = HashMap::new();
        for t in c.ix.outgoing(&sv.id) {
            if !unguarded(t) {
                continue;
            }
            let keys: Vec<(String, String)> = if t.triggers.is_empty() {
                vec![(String::new(), String::new())]
            } else {
                t.triggers
                    .iter()
                    .map(|x| {
                        let x = js_trim(x);
                        let key = expr::time_trigger_ms(x).map_or_else(|| x.to_string(), |ms| format!("after {ms}"));
                        (key, x.to_string())
                    })
                    .collect()
            };
            for (key, text) in keys {
                let n = seen.entry(key.clone()).or_insert(0);
                *n += 1;
                if *n == 2 {
                    let msg = if key.is_empty() {
                        format!("{} has several completion transitions without {what}.", c.name(Some(si)))
                    } else {
                        format!("{} has several transitions on '{text}' without {what}.", c.name(Some(si)))
                    };
                    found.push((t.id.clone(), msg));
                }
            }
        }
    }
    // Orthogonal regions: two transitions on the same event, without guards, that both leave the state owning the
    // regions (only one can fire).
    let candidates: Vec<(&Transition, usize, usize)> = model
        .transitions
        .iter()
        .filter(|t| t.kind != TransitionKind::Internal && unguarded(t))
        .filter_map(|t| {
            let s = c.ix.vertex(&t.source)?;
            (c.ix.v(s).vtype == VertexType::State).then_some((t, s, c.ix.vertex(&t.target)?))
        })
        .collect();
    let events = |t: &Transition| -> Vec<String> {
        t.triggers
            .iter()
            .map(|x| js_trim(x).to_string())
            .filter(|x| expr::time_trigger_ms(x).is_none() && x != expr::ANY_RECEIVE)
            .collect()
    };
    let mut reported: HashSet<&str> = HashSet::new();
    for (i, &(t2, s2, g2)) in candidates.iter().enumerate() {
        for &(t1, s1, g1) in &candidates[..i] {
            if s1 == s2 || reported.contains(t2.id.as_str()) {
                continue;
            }
            let shared = events(t1).into_iter().find(|e| events(t2).contains(e));
            let Some(event) = shared else { continue };
            // The innermost state around both, if they are in different regions of it.
            let ancestors2 = c.ix.ancestors(s2);
            let Some(a) = c.ix.ancestors(s1).into_iter().find(|a| ancestors2.contains(a)) else { continue };
            let (r1, r2) = (region_within(c, s1, Some(a)), region_within(c, s2, Some(a)));
            if r1.is_none() || r1 == r2 {
                continue;
            }
            let aid = &c.ix.v(a).id;
            let leaves = |g: usize| g == a || !c.ix.is_inside(g, aid);
            if leaves(g1) && leaves(g2) {
                let msg = format!(
                    "{} and {} are in orthogonal regions of {} and both leave it on '{event}' without {what}.",
                    c.name(Some(s1)),
                    c.name(Some(s2)),
                    c.name(Some(a))
                );
                reported.insert(t2.id.as_str());
                found.push((t2.id.clone(), msg));
            }
        }
    }
    for (id, msg) in found {
        c.add(&id, Severity::Error, msg);
    }
}

/// Every region entered by default needs an initial pseudostate (UML 14.2.3.2, 14.2.3.4.4–6): the machine's top-level
/// regions; the regions of a composite state targeted on its border; the regions of a state entered that no target
/// of the entering transition is in (another region entered explicitly, a fork or an entry point that doesn't reach
/// them); a history's region with nothing to restore and no default transition; the regions of the states a history
/// restores.
fn check_default_entries(c: &mut Checker) {
    let model = c.model;
    let lacks = |r: &crate::model::Region| {
        c.ix.initial_of(&r.id).is_none()
            && c.ix
                .children_of(&r.id)
                .into_iter()
                .any(|k| matches!(model.vertices[k].vtype, VertexType::State | VertexType::Final))
    };
    let mut flagged: Vec<(Option<usize>, String)> = Vec::new();
    let mut flag = |owner: Option<usize>, r: &crate::model::Region| {
        if lacks(r) && !flagged.iter().any(|(_, id)| *id == r.id) {
            flagged.push((owner, r.id.clone()));
        }
    };
    // A machine with entry points can be entered through them only (its generated code has no default start).
    let machine_entries =
        model.vertices.iter().any(|v| v.vtype == VertexType::EntryPoint && !c.ix.has_vertex(&v.parent));
    if !machine_entries {
        for r in &model.regions {
            flag(None, r);
        }
    }
    // The targets of each entering step, from its source: a fork's are taken together, and so are an entry point's.
    let mut groups: Vec<(usize, Vec<usize>, bool)> = Vec::new();
    for t in &model.transitions {
        let (Some(s), Some(g)) = (c.ix.vertex(&t.source), c.ix.vertex(&t.target)) else { continue };
        if t.kind == TransitionKind::Internal || matches!(c.ix.v(s).vtype, VertexType::Fork | VertexType::EntryPoint) {
            continue;
        }
        let local_self = t.kind == TransitionKind::Local && s == g;
        if c.ix.v(g).vtype == VertexType::Fork {
            let targets = c.ix.outgoing(&c.ix.v(g).id).iter().filter_map(|o| c.ix.vertex(&o.target)).collect();
            groups.push((s, targets, false));
        } else {
            groups.push((s, vec![g], local_self));
        }
    }
    for (vi, v) in model.vertices.iter().enumerate() {
        if v.vtype == VertexType::EntryPoint {
            let targets = c.ix.outgoing(&v.id).iter().filter_map(|o| c.ix.vertex(&o.target)).collect();
            groups.push((vi, targets, false));
        }
    }
    for (s, targets, local_self) in groups {
        let sid = c.ix.v(s).id.clone();
        // The states entered, with the regions a target is in.
        let mut entered: Vec<(usize, HashSet<String>)> = Vec::new();
        for &x in &targets {
            let point_owner = c.ix.is_border(x).then(|| c.ix.owner_state(x)).flatten();
            for a in c.ix.ancestors(x) {
                if a == s || c.ix.is_inside(s, &c.ix.v(a).id) || c.ix.v(a).id == sid {
                    break;
                }
                let i = match entered.iter().position(|(e, _)| *e == a) {
                    Some(i) => i,
                    None => {
                        entered.push((a, HashSet::new()));
                        entered.len() - 1
                    }
                };
                if Some(a) == point_owner {
                    // Entered through one of its points: the point's own transitions say which regions.
                    entered[i].1.extend(c.ix.v(a).regions.iter().map(|r| r.id.clone()));
                } else if let Some(r) = region_within(c, x, Some(a)) {
                    entered[i].1.insert(r);
                }
            }
            // A composite state targeted on its border is entered by default.
            let xv = c.ix.v(x);
            if xv.vtype == VertexType::State && !local_self {
                for r in &xv.regions {
                    flag(Some(x), r);
                }
            }
        }
        // An entry point's own state is entered too, when something enters through the point; the machine's own
        // entry point enters the machine: its top-level regions that the point doesn't reach start by default.
        let sv = c.ix.v(s);
        if sv.vtype == VertexType::EntryPoint {
            match c.ix.vertex(&sv.parent) {
                Some(a) if !c.ix.incoming(&sv.id).is_empty() => {
                    let explicit = targets.iter().filter_map(|&x| region_within(c, x, Some(a))).collect();
                    entered.push((a, explicit));
                }
                Some(_) => {}
                None => {
                    let explicit: HashSet<String> = targets.iter().filter_map(|&x| region_within(c, x, None)).collect();
                    for r in &model.regions {
                        if !explicit.contains(&r.id) {
                            flag(None, r);
                        }
                    }
                }
            }
        }
        for (a, explicit) in entered {
            for r in &c.ix.v(a).regions {
                if !explicit.contains(&r.id) {
                    flag(Some(a), r);
                }
            }
        }
    }
    for v in &model.vertices {
        if !matches!(v.vtype, VertexType::ShallowHistory | VertexType::DeepHistory) || c.ix.incoming(&v.id).is_empty() {
            continue;
        }
        let Some(owner) = c.ix.region_owner(&v.parent) else { continue };
        let Some(region) = all_regions(model).into_iter().find(|r| r.id == v.parent) else { continue };
        if c.ix.outgoing(&v.id).is_empty() {
            flag(owner, region);
        }
        // A state the history restores is entered as by default where it has no history of its own.
        for k in c.ix.children_of(&v.parent) {
            if model.vertices[k].vtype == VertexType::State {
                for r in &model.vertices[k].regions {
                    flag(Some(k), r);
                }
            }
        }
    }
    // A submachine state entered on its border (or restored by a shallow history) starts its submachine by default.
    let mut started: Vec<usize> = Vec::new();
    for t in &model.transitions {
        if let Some(g) = c.ix.vertex(&t.target)
            && t.kind != TransitionKind::Internal
            && !started.contains(&g)
        {
            started.push(g);
        }
    }
    for v in &model.vertices {
        if v.vtype == VertexType::ShallowHistory && !c.ix.incoming(&v.id).is_empty() {
            for k in c.ix.children_of(&v.parent) {
                if !started.contains(&k) {
                    started.push(k);
                }
            }
        }
    }
    let mut no_default: Vec<(usize, String)> = Vec::new();
    for g in started {
        let Some(sub) = &c.ix.v(g).submachine else { continue };
        if let Some(info) = c.submachines.get(sub)
            && info.found
            && !info.default_entry
        {
            no_default.push((g, c.machine_name(sub)));
        }
    }
    for (owner, rid) in flagged {
        match owner {
            None => c.add("", Severity::Error, "The top-level region has no initial pseudostate.".into()),
            Some(o) => {
                let r = all_regions(model).into_iter().find(|r| r.id == rid).unwrap();
                let region = if r.name.is_empty() { "region".to_string() } else { format!("region '{}'", r.name) };
                let msg =
                    format!("{} is entered by default but its {region} has no initial pseudostate.", c.name(Some(o)));
                c.add(&c.ix.v(o).id.clone(), Severity::Error, msg);
            }
        }
    }
    for (g, sub) in no_default {
        let msg = format!(
            "{} is entered by default, but its submachine {sub} has no initial pseudostate: enter it through an entry point.",
            c.name(Some(g))
        );
        c.add(&c.ix.v(g).id.clone(), Severity::Error, msg);
    }
}

fn all_regions(model: &FsmModel) -> Vec<&crate::model::Region> {
    model.regions.iter().chain(model.vertices.iter().flat_map(|v| v.regions.iter())).collect()
}

fn check_reachability(c: &mut Checker) {
    let model = c.model;
    // Every top-level region is entered when the machine starts.
    let root_initials: Vec<Option<usize>> = model.regions.iter().map(|r| c.ix.initial_of(&r.id)).collect();
    // The machine can also be entered through its own entry points (from a referencing submachine state).
    let machine_entries: Vec<usize> = (0..model.vertices.len())
        .filter(|&i| {
            let v = &model.vertices[i];
            v.vtype == VertexType::EntryPoint && model.regions.iter().any(|r| r.id == v.parent)
        })
        .collect();
    if root_initials.iter().all(Option::is_none) && machine_entries.is_empty() {
        return;
    }
    let mut reached: HashSet<String> = HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    let visit = |v: Option<usize>, reached: &mut HashSet<String>, queue: &mut std::collections::VecDeque<usize>| {
        if let Some(v) = v
            && reached.insert(model.vertices[v].id.clone())
        {
            queue.push_back(v);
        }
    };
    for &initial in &root_initials {
        visit(initial, &mut reached, &mut queue);
    }
    for e in machine_entries {
        visit(Some(e), &mut reached, &mut queue);
    }
    while let Some(vi) = queue.pop_front() {
        // Entering a vertex enters its enclosing states and their other orthogonal regions.
        for a in c.ix.ancestors(vi) {
            visit(Some(a), &mut reached, &mut queue);
        }
        let v = &model.vertices[vi];
        if v.vtype == VertexType::State {
            for r in &v.regions {
                visit(c.ix.initial_of(&r.id), &mut reached, &mut queue);
            }
            for cp in c.ix.connection_points(vi) {
                let p = &model.vertices[cp];
                if p.vtype == VertexType::ExitPoint
                    || (p.vtype == VertexType::ConnectionPointRef && p.point_kind == PointKind::Exit)
                {
                    visit(Some(cp), &mut reached, &mut queue);
                }
            }
        }
        for t in c.ix.outgoing(&v.id) {
            visit(c.ix.vertex(&t.target), &mut reached, &mut queue);
        }
    }
    for (vi, v) in model.vertices.iter().enumerate() {
        if v.vtype == VertexType::State && !reached.contains(&v.id) {
            let msg = format!("State {} can never be entered.", c.name(Some(vi)));
            c.add(&v.id, Severity::Error, msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Region, Transition, Vertex};

    /// A machine with states A and B, A entered first, and `vertices` (id, type, name, parent).
    fn machine(vertices: &[(&str, VertexType, &str, &str)]) -> FsmModel {
        let mut m = FsmModel {
            name: "M".into(),
            kind: MachineKind::Behavioral,
            context: String::new(),
            documentation: String::new(),
            regions: vec![Region::new("r", "")],
            vertices: vec![
                Vertex::new("A", VertexType::State, "A", "r"),
                Vertex::new("B", VertexType::State, "B", "r"),
                Vertex::new("i", VertexType::Initial, "", "r"),
            ],
            transitions: Vec::new(),
        };
        m.vertices.extend(vertices.iter().map(|&(id, t, name, parent)| Vertex::new(id, t, name, parent)));
        transition(&mut m, "i", "A", "");
        m
    }

    fn transition(m: &mut FsmModel, source: &str, target: &str, guard: &str) {
        let mut t = Transition::new(&format!("t{}", m.transitions.len()), source, target, TransitionKind::External);
        t.triggers = if source == "A" { vec!["e".into()] } else { Vec::new() };
        t.guard = guard.into();
        m.transitions.push(t);
    }

    fn cycles(m: &FsmModel) -> Vec<(String, &'static str, String)> {
        validate(m, &HashMap::new())
            .into_iter()
            .filter(|i| i.message.contains("form a cycle"))
            .map(|i| (i.id, i.severity.as_str(), i.message))
            .collect()
    }

    #[test]
    fn choices_leading_to_each_other() {
        let mut m = machine(&[("c1", VertexType::Choice, "c1", "r"), ("c2", VertexType::Choice, "c2", "r")]);
        transition(&mut m, "A", "c1", "");
        transition(&mut m, "c1", "c2", "g()");
        transition(&mut m, "c1", "B", "else");
        transition(&mut m, "c2", "c1", "h()");
        transition(&mut m, "c2", "B", "else");
        let msg = "Choices form a cycle without a state: 'c1' -> 'c2' -> 'c1'.";
        assert_eq!(cycles(&m), vec![("c1".into(), "error", msg.into())]);
    }

    #[test]
    fn choice_targeting_itself() {
        let mut m = machine(&[("c", VertexType::Choice, "c", "r")]);
        transition(&mut m, "A", "c", "");
        transition(&mut m, "c", "c", "g()");
        transition(&mut m, "c", "B", "else");
        let msg = "Choices form a cycle without a state: 'c' -> 'c'.";
        assert_eq!(cycles(&m), vec![("c".into(), "error", msg.into())]);
    }

    #[test]
    fn unnamed_junction_and_choice() {
        let mut m = machine(&[("c", VertexType::Choice, "", "r"), ("j", VertexType::Junction, "", "r")]);
        transition(&mut m, "A", "j", "");
        transition(&mut m, "j", "c", "");
        transition(&mut m, "c", "j", "g()");
        transition(&mut m, "c", "B", "else");
        let msg = "Junctions and choices form a cycle without a state: choice -> junction -> choice.";
        assert_eq!(cycles(&m), vec![("c".into(), "error", msg.into())]);
    }

    #[test]
    fn one_issue_for_cycles_sharing_pseudostates() {
        let mut m = machine(&[
            ("j1", VertexType::Junction, "j1", "r"),
            ("j2", VertexType::Junction, "j2", "r"),
            ("j3", VertexType::Junction, "j3", "r"),
            ("j4", VertexType::Junction, "j4", "r"),
        ]);
        transition(&mut m, "A", "j1", "");
        transition(&mut m, "j1", "j2", "a()");
        transition(&mut m, "j1", "j3", "b()");
        transition(&mut m, "j1", "B", "else");
        transition(&mut m, "j2", "j4", "");
        transition(&mut m, "j4", "j1", "");
        transition(&mut m, "j3", "j1", "");
        let msg = "Junctions form a cycle without a state: 'j1' -> 'j3' -> 'j1'.";
        assert_eq!(cycles(&m), vec![("j1".into(), "error", msg.into())]);
    }

    #[test]
    fn loops_through_states_and_machine_entry_points() {
        let mut m = machine(&[
            ("c", VertexType::Choice, "c", "r"),
            ("ep", VertexType::EntryPoint, "ep", "r"),
            ("j", VertexType::Junction, "j", "r"),
        ]);
        transition(&mut m, "A", "c", "");
        transition(&mut m, "c", "A", "g()");
        transition(&mut m, "c", "B", "else");
        transition(&mut m, "ep", "j", "");
        transition(&mut m, "j", "ep", "g()");
        transition(&mut m, "j", "B", "else");
        assert!(cycles(&m).is_empty());
    }
}
