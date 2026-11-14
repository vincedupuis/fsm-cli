//! Writes state machine files: `toXmi` of fsm-editor-vscode/src/xmi.ts, byte for byte (the layout mapping is
//! described at the top of that file). A file written here and then opened and saved in any of the editors must not
//! change, so everything follows the editors' writer: the order of elements and attributes, the ids it derives (events,
//! triggers, behaviors, diagram elements), the region shapes it computes from the state's header, the label boxes,
//! JavaScript's arithmetic and its number formatting. The same applies to the editors' copies of that writer
//! (`Xmi.cs`, `Xmi.kt`).

use std::collections::{HashMap, HashSet};

use crate::geometry::{center, n1, polyline_mid};
use crate::model::{
    FsmModel, MachineKind, ModelIndex, PointKind, Region, Transition, TransitionKind, Vertex, VertexType,
};
use crate::model::{ITEM_COLOR_KEYS, RegionLayout, js_trim};
use crate::xml::{Node, write_xml};

const NS_XMI: &str = "http://www.omg.org/spec/XMI/20131001";
const NS_UML: &str = "http://www.omg.org/spec/UML/20161101";
const NS_UMLDI: &str = "http://www.omg.org/spec/UML/20161101/UMLDI";
const NS_DC: &str = "http://www.omg.org/spec/DD/20100524/DC";
const NS_DI: &str = "http://www.omg.org/spec/DD/20100524/DI";
const PROFILE_NAME: &str = "FsmStereotypes";
const PROFILE_NS: &str = "https://github.com/vincedupuis/fsm-editor/profiles/FsmStereotypes";
const UML_STATE: &str = "http://www.omg.org/spec/UML/20161101/UML.xmi#State";
const LANGUAGE: &str = "FSM";
const EXTENDER: &str = "FSM Editor";
const LABEL_W: f64 = 100.0;
const LABEL_H: f64 = 16.0;

/// JavaScript's `String(n)`: the shortest digits that read back as `n`, without a fraction when whole, in exponent
/// form from 1e21 and below 1e-6.
pub fn js_number(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n == 0.0 {
        return "0".to_string();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let sign = if n < 0.0 { "-" } else { "" };
    // Rust's `{:e}` gives the shortest round-trip digits too: `d.ddde±x`.
    let e = format!("{:e}", n.abs());
    let (mantissa, exponent) = e.split_once('e').unwrap_or((&e, "0"));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let point = exponent.parse::<i32>().unwrap_or(0) + 1;
    let body = if k <= point && point <= 21 {
        format!("{digits}{}", "0".repeat((point - k) as usize))
    } else if 0 < point && point <= 21 {
        format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
    } else if -6 < point && point <= 0 {
        format!("0.{}{digits}", "0".repeat((-point) as usize))
    } else {
        let x = point - 1;
        let x = if x >= 0 { format!("+{x}") } else { x.to_string() };
        if k == 1 { format!("{digits}e{x}") } else { format!("{}.{}e{x}", &digits[..1], &digits[1..]) }
    };
    format!("{sign}{body}")
}

/// `Math.min` of two numbers: NaN when either is.
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.min(b) }
}

/// Width of text in the diagram font as the editors estimate it (`estimateWidth`), from its length in UTF-16 units.
fn estimate_width(len: usize) -> f64 {
    len as f64 * 6.6
}

/// How many lines `wrapLines` gives for `text` at `width`: wrapped at spaces, or inside words too long for a line.
/// The widths are estimated from lengths only, so lengths (in UTF-16 units, as JavaScript counts) are enough.
pub fn wrap_line_count(text: &str, width: f64) -> usize {
    let mut lines = 0;
    for para in text.split('\n') {
        let para = para.strip_suffix('\r').unwrap_or(para);
        let mut line = 0usize;
        for word in para.split(' ') {
            let word = word.encode_utf16().count();
            let candidate = if line > 0 { line + 1 + word } else { word };
            if line == 0 || estimate_width(candidate) <= width {
                line = candidate;
            } else {
                lines += 1;
                line = word;
            }
            while line > 1 && estimate_width(line) > width {
                // The longest start that fits, at least one unit (the editors count down from line - 1; the widths
                // grow with the length, so starting just above the width gives the same place).
                let mut k = if width < 0.0 { 1 } else { (line - 1).min((width / 6.6) as usize + 2) };
                while k > 1 && estimate_width(k) > width {
                    k -= 1;
                }
                lines += 1;
                line -= k;
            }
        }
        lines += 1;
    }
    lines
}

/// The text of an internal transition, as shown in its state's activities compartment (`internalLabel`).
fn internal_label(t: &Transition, protocol: bool) -> String {
    let trig = t.triggers.join(", ");
    let mut s;
    if protocol {
        s = String::new();
        if !t.precondition.is_empty() {
            s += &format!("[{}] ", t.precondition);
        }
        s += &trig;
        if !t.postcondition.is_empty() {
            s += &format!(" / [{}]", t.postcondition);
        }
    } else {
        s = trig;
        if !t.guard.is_empty() {
            s += &format!(" [{}]", t.guard);
        }
        if !t.effect.is_empty() {
            s += &format!(" / {}", t.effect);
        }
    }
    let s = js_trim(&s);
    if s.is_empty() { "(internal)".to_string() } else { s.to_string() }
}

/// The height of a state's header, as the diagram editor draws it (`headerHeight`): its name, its stereotype, its
/// internal activities wrapped to the state's width.
fn header_height(v: &Vertex, m: &FsmModel) -> f64 {
    let mut h = 24.0 + if v.stereotype.is_empty() { 0.0 } else { 12.0 };
    let mut lines = Vec::new();
    if !v.entry.is_empty() {
        lines.push(format!("entry / {}", v.entry));
    }
    if !v.exit.is_empty() {
        lines.push(format!("exit / {}", v.exit));
    }
    if !v.do_activity.is_empty() {
        lines.push(format!("do / {}", v.do_activity));
    }
    for d in &v.deferrable {
        lines.push(format!("{d} / defer"));
    }
    let protocol = m.kind == MachineKind::Protocol;
    for t in &m.transitions {
        if t.kind == TransitionKind::Internal && t.source == v.id && t.target == v.id {
            lines.push(internal_label(t, protocol));
        }
    }
    let n: usize = lines.iter().map(|l| wrap_line_count(l, v.w - 16.0)).sum();
    if n > 0 {
        h += n as f64 * 14.0 + 6.0;
    }
    h
}

/// The extents of a state's regions along its region layout, out of `total` (`regionSizes`): their own sizes, the
/// last one taking the rest, or equal parts when some size is missing or they leave nothing for the last one.
pub fn region_sizes(regions: &[Region], total: f64) -> Vec<f64> {
    let own = &regions[..regions.len().saturating_sub(1)];
    let sum = own.iter().fold(0.0, |a, r| a + r.size.unwrap_or(0.0));
    if own.iter().all(|r| r.size.is_some_and(|s| s > 0.0)) && sum < total {
        let mut out: Vec<f64> = own.iter().map(|r| r.size.unwrap_or(0.0)).collect();
        out.push(total - sum);
        return out;
    }
    regions.iter().map(|_| total / regions.len() as f64).collect()
}

/// The boxes of a state's regions (`regionBounds`): under its header, side by side or stacked.
pub fn region_bounds(v: &Vertex, m: &FsmModel) -> Vec<(f64, f64, f64, f64)> {
    let top = v.y + js_min(header_height(v, m), v.h - 20.0);
    let bh = v.y + v.h - top;
    let horizontal = v.region_layout == Some(RegionLayout::Horizontal);
    let sizes = region_sizes(&v.regions, if horizontal { v.w } else { bh });
    let mut at = if horizontal { v.x } else { top };
    sizes
        .iter()
        .take(v.regions.len())
        .map(|&s| {
            let b = if horizontal { (at, top, s, bh) } else { (v.x, at, v.w, s) };
            at += s;
            b
        })
        .collect()
}

/// The model as `toXmi` sees it after `JSON.parse(JSON.stringify(model))` and `normalizeModel`: numbers that JSON
/// can't hold become null, which arithmetic takes for 0 and a region size for none; groups the diagram can't show are
/// dissolved; a state's self-transition keeps 1 or 3 bend points or none.
fn prepared(model: &FsmModel) -> FsmModel {
    let mut m = model.clone();
    let finite = |n: &mut f64| {
        if !n.is_finite() {
            *n = 0.0;
        }
    };
    for v in &mut m.vertices {
        for n in [&mut v.x, &mut v.y, &mut v.w, &mut v.h] {
            finite(n);
        }
        for r in &mut v.regions {
            r.size = r.size.filter(|s| s.is_finite());
        }
    }
    for t in &mut m.transitions {
        for p in &mut t.points {
            finite(&mut p.x);
            finite(&mut p.y);
        }
        if let Some(p) = &mut t.label_offset {
            finite(&mut p.x);
            finite(&mut p.y);
        }
    }
    if m.id.is_empty() {
        m.id = "sm".to_string();
    }
    if m.regions.is_empty() {
        m.regions.push(Region::new("r_root", ""));
    }
    crate::xmi::normalize(&mut m);
    m
}

fn s(v: &str) -> String {
    v.to_string()
}

fn kind_str(k: TransitionKind) -> &'static str {
    match k {
        TransitionKind::External => "external",
        TransitionKind::Internal => "internal",
        TransitionKind::Local => "local",
    }
}

/// Writes the machine as the editors save it.
pub fn to_xmi(model: &FsmModel) -> Result<String, String> {
    let m = prepared(model);
    Writer::new(&m).document()
}

/// The order in which reading the written file gives the vertices and the transitions (indexes into `model`'s lists):
/// the document's, region by region (comments, then vertices, each state followed by its connection points, its
/// connection point references and the contents of its regions, then the region's transitions), the machine's own
/// connection points last. What the document doesn't hold (in no region) keeps its place after that. The diagram's
/// shapes follow the model's order, so a model in another order is written differently once read back.
pub fn document_order(model: &FsmModel) -> (Vec<usize>, Vec<usize>) {
    let w = Writer::new(model);
    let m = model;
    let mut vertices = Vec::new();
    let mut transitions = Vec::new();
    let mut seen = vec![false; m.vertices.len()];
    fn region(w: &Writer, id: &str, seen: &mut Vec<bool>, vertices: &mut Vec<usize>, transitions: &mut Vec<usize>) {
        let m = w.m;
        let kids: Vec<usize> = (0..m.vertices.len())
            .filter(|&i| m.vertices[i].parent == id && !w.is_machine_point(&m.vertices[i]))
            .collect();
        for &i in kids.iter().filter(|&&i| m.vertices[i].vtype == VertexType::Comment) {
            if !seen[i] {
                seen[i] = true;
                vertices.push(i);
            }
        }
        for &i in kids.iter().filter(|&&i| m.vertices[i].vtype != VertexType::Comment) {
            if seen[i] {
                continue;
            }
            seen[i] = true;
            vertices.push(i);
            let v = &m.vertices[i];
            if v.vtype == VertexType::Final || v.vtype.is_pseudostate() {
                continue;
            }
            for kinds in [[VertexType::EntryPoint, VertexType::ExitPoint], [VertexType::ConnectionPointRef; 2]] {
                for (j, cp) in m.vertices.iter().enumerate() {
                    if cp.parent == v.id && kinds.contains(&cp.vtype) && !seen[j] {
                        seen[j] = true;
                        vertices.push(j);
                    }
                }
            }
            for r in &v.regions {
                region(w, &r.id, seen, vertices, transitions);
            }
        }
        transitions.extend(w.transitions_by_region.get(id).cloned().unwrap_or_default());
    }
    for r in &m.regions {
        region(&w, &r.id, &mut seen, &mut vertices, &mut transitions);
    }
    for (i, v) in m.vertices.iter().enumerate() {
        if w.is_machine_point(v) && !seen[i] {
            seen[i] = true;
            vertices.push(i);
        }
    }
    vertices.extend((0..m.vertices.len()).filter(|&i| !seen[i]));
    let mut placed = vec![false; m.transitions.len()];
    transitions.retain(|&t| !std::mem::replace(&mut placed[t], true));
    transitions.extend((0..m.transitions.len()).filter(|&t| !placed[t]));
    (vertices, transitions)
}

struct Writer<'a> {
    m: &'a FsmModel,
    index: ModelIndex<'a>,
    top_regions: HashSet<&'a str>,
    protocol: bool,
    /// Trigger text → event id, and the events' elements, in the order the triggers are written.
    events: HashMap<String, String>,
    event_ids: HashSet<String>,
    event_elements: Vec<Node>,
    /// Region id → its transitions (indexes), in file order.
    transitions_by_region: HashMap<String, Vec<usize>>,
    /// States being written, to refuse a state inside one of its own regions.
    writing: Vec<usize>,
}

impl<'a> Writer<'a> {
    fn new(m: &'a FsmModel) -> Writer<'a> {
        let index = ModelIndex::new(m);
        let mut w = Writer {
            m,
            index,
            top_regions: m.regions.iter().map(|r| r.id.as_str()).collect(),
            protocol: m.kind == MachineKind::Protocol,
            events: HashMap::new(),
            event_ids: HashSet::new(),
            event_elements: Vec::new(),
            transitions_by_region: HashMap::new(),
            writing: Vec::new(),
        };
        // A transition is owned by the innermost region that contains both of its ends.
        for (i, t) in m.transitions.iter().enumerate() {
            let sc = w.index.vertex(&t.source).map(|s| w.region_chain(s)).unwrap_or_default();
            let gc: HashSet<String> =
                w.index.vertex(&t.target).map(|g| w.region_chain(g)).unwrap_or_default().into_iter().collect();
            let container = sc.into_iter().find(|r| gc.contains(r)).unwrap_or_else(|| m.regions[0].id.clone());
            w.transitions_by_region.entry(container).or_default().push(i);
        }
        w
    }

    fn is_machine_point(&self, v: &Vertex) -> bool {
        matches!(v.vtype, VertexType::EntryPoint | VertexType::ExitPoint)
            && self.top_regions.contains(v.parent.as_str())
    }

    /// Regions enclosing a vertex, innermost first.
    fn region_chain(&self, v: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut cur = Some(v);
        while let Some(c) = cur {
            let vx = self.index.v(c);
            if !seen.insert(vx.id.as_str()) {
                break;
            }
            if self.index.is_border(c) {
                cur = self.index.vertex(&vx.parent);
                continue;
            }
            out.push(vx.parent.clone());
            cur = self.index.region_owner(&vx.parent).flatten();
        }
        out
    }

    /// One signal event (and signal) per event name, one time event per time trigger: the id of `trigger`'s.
    fn event_for(&mut self, trigger: &str) -> String {
        if let Some(id) = self.events.get(trigger) {
            return id.clone();
        }
        let time = time_expression(trigger);
        let mut safe: String = trigger
            .chars()
            .flat_map(|c| {
                let keep = c.is_ascii_alphanumeric() || c == '_';
                std::iter::repeat_n(if keep { c } else { '_' }, if keep { 1 } else { c.len_utf16() })
            })
            .collect();
        // after(1s) and an event named after_1s_ would share an id: the later one gets another _.
        while self.event_ids.contains(&format!("ev_{safe}")) {
            safe.push('_');
        }
        let id = format!("ev_{safe}");
        match time {
            Some(expr) => self.event_elements.push(
                Node::new(
                    "packagedElement",
                    vec![
                        ("xmi:type", s("uml:TimeEvent")),
                        ("xmi:id", id.clone()),
                        ("name", s(trigger)),
                        ("isRelative", s("true")),
                    ],
                )
                .with([Node::new(
                    "when",
                    vec![("xmi:type", s("uml:TimeExpression")), ("xmi:id", format!("{id}_when"))],
                )
                .with([Node::new(
                    "expr",
                    vec![("xmi:type", s("uml:LiteralString")), ("xmi:id", format!("{id}_expr")), ("value", s(expr))],
                )])]),
            ),
            None => {
                self.event_elements.push(Node::new(
                    "packagedElement",
                    vec![
                        ("xmi:type", s("uml:SignalEvent")),
                        ("xmi:id", id.clone()),
                        ("name", s(trigger)),
                        ("signal", format!("sig_{safe}")),
                    ],
                ));
                self.event_elements.push(Node::new(
                    "packagedElement",
                    vec![("xmi:type", s("uml:Signal")), ("xmi:id", format!("sig_{safe}")), ("name", s(trigger))],
                ));
            }
        }
        self.events.insert(trigger.to_string(), id.clone());
        self.event_ids.insert(id.clone());
        id
    }

    fn behavior(tag: &str, id: String, body: &str) -> Option<Node> {
        (!body.is_empty()).then(|| {
            Node::new(tag, vec![("xmi:type", s("uml:OpaqueBehavior")), ("xmi:id", id)])
                .with([Node::text("language", LANGUAGE), Node::text("body", body)])
        })
    }

    fn constraint(tag: &str, id: String, body: &str) -> Option<Node> {
        (!body.is_empty()).then(|| {
            Node::new(tag, vec![("xmi:type", s("uml:Constraint")), ("xmi:id", id.clone())]).with([Node::new(
                "specification",
                vec![("xmi:type", s("uml:OpaqueExpression")), ("xmi:id", format!("{id}_spec"))],
            )
            .with([Node::text("language", LANGUAGE), Node::text("body", body)])])
        })
    }

    fn triggers(&mut self, tag: &'static str, owner: &str, list: &[String]) -> Vec<Node> {
        list.iter()
            .enumerate()
            .map(|(i, t)| {
                let event = self.event_for(t);
                Node::new(
                    tag,
                    vec![
                        ("xmi:type", s("uml:Trigger")),
                        ("xmi:id", format!("{owner}_{tag}{}", i + 1)),
                        ("name", t.clone()),
                        ("event", event),
                    ],
                )
            })
            .collect()
    }

    fn transition_node(&mut self, t: usize) -> Node {
        let t = &self.m.transitions[t];
        let mut node = Node::new(
            "transition",
            vec![
                ("xmi:type", s(if self.protocol { "uml:ProtocolTransition" } else { "uml:Transition" })),
                ("xmi:id", t.id.clone()),
                ("kind", s(kind_str(t.kind))),
                ("source", t.source.clone()),
                ("target", t.target.clone()),
            ],
        );
        let triggers = self.triggers("trigger", &t.id, &t.triggers);
        node = node.with(triggers);
        if self.protocol {
            node = node.with(Self::constraint("preCondition", format!("{}_pre", t.id), &t.precondition));
            node = node.with(Self::constraint("postCondition", format!("{}_post", t.id), &t.postcondition));
        } else {
            node = node.with(Self::constraint("guard", format!("{}_guard", t.id), &t.guard));
        }
        node.with(Self::behavior("effect", format!("{}_effect", t.id), &t.effect))
    }

    fn comment_node(c: &Vertex) -> Node {
        Node::new(
            "ownedComment",
            vec![("xmi:type", s("uml:Comment")), ("xmi:id", c.id.clone()), ("annotatedElement", c.anchors.join(" "))],
        )
        .with([Node::text("body", &c.text)])
    }

    fn point_node(tag: &str, v: &Vertex) -> Node {
        Node::new(
            tag,
            vec![
                ("xmi:type", s("uml:Pseudostate")),
                ("xmi:id", v.id.clone()),
                ("name", v.name.clone()),
                ("kind", s(v.vtype.as_str())),
            ],
        )
    }

    fn vertex_node(&mut self, i: usize) -> Result<Node, String> {
        let m = self.m;
        let v = &m.vertices[i];
        if v.vtype == VertexType::Final {
            return Ok(Node::new(
                "subvertex",
                vec![("xmi:type", s("uml:FinalState")), ("xmi:id", v.id.clone()), ("name", v.name.clone())],
            ));
        }
        if v.vtype.is_pseudostate() {
            return Ok(Self::point_node("subvertex", v));
        }
        let mut node = Node::new(
            "subvertex",
            vec![("xmi:type", s("uml:State")), ("xmi:id", v.id.clone()), ("name", v.name.clone())],
        );
        node = node.with(Self::behavior("entry", format!("{}_entry", v.id), &v.entry));
        node = node.with(Self::behavior("exit", format!("{}_exit", v.id), &v.exit));
        node = node.with(Self::behavior("doActivity", format!("{}_do", v.id), &v.do_activity));
        node = node.with(Self::constraint("stateInvariant", format!("{}_inv", v.id), &v.invariant));
        let deferred = self.triggers("deferrableTrigger", &v.id, &v.deferrable);
        node = node.with(deferred);
        for cp in m.vertices.iter().filter(|cp| cp.parent == v.id) {
            match cp.vtype {
                VertexType::EntryPoint | VertexType::ExitPoint => node.push(Self::point_node("connectionPoint", cp)),
                VertexType::ConnectionPointRef => {
                    let file = v.submachine.as_deref().unwrap_or("").split('#').next().unwrap_or("");
                    let end = if cp.point_kind == PointKind::Exit { "exit" } else { "entry" };
                    node.push(
                        Node::new(
                            "connection",
                            vec![("xmi:type", s("uml:ConnectionPointReference")), ("xmi:id", cp.id.clone())],
                        )
                        .with([Node::new(end, vec![("href", format!("{file}#{}", cp.point_ref))])]),
                    );
                }
                _ => {}
            }
        }
        // States on the way down: a region id used again (a file edited by hand) can make a state its own descendant.
        if self.writing.contains(&i) {
            return Err(format!("State '{}' is inside one of its own regions.", v.id));
        }
        self.writing.push(i);
        for r in &v.regions {
            let region = self.region_node(r)?;
            node.push(region);
        }
        self.writing.pop();
        if let Some(sub) = v.submachine.as_deref().filter(|s| !s.is_empty()) {
            node.push(Node::new("submachine", vec![("href", s(sub))]));
        }
        Ok(node)
    }

    fn region_node(&mut self, r: &Region) -> Result<Node, String> {
        let m = self.m;
        let kids: Vec<usize> = (0..m.vertices.len())
            .filter(|&i| m.vertices[i].parent == r.id && !self.is_machine_point(&m.vertices[i]))
            .collect();
        let mut node = Node::new(
            "region",
            vec![("xmi:type", s("uml:Region")), ("xmi:id", r.id.clone()), ("name", r.name.clone())],
        );
        for &i in &kids {
            if m.vertices[i].vtype == VertexType::Comment {
                node.push(Self::comment_node(&m.vertices[i]));
            }
        }
        for &i in &kids {
            if m.vertices[i].vtype != VertexType::Comment {
                let v = self.vertex_node(i)?;
                node.push(v);
            }
        }
        for t in self.transitions_by_region.get(&r.id).cloned().unwrap_or_default() {
            let t = self.transition_node(t);
            node.push(t);
        }
        Ok(node)
    }

    fn bounds(x: f64, y: f64, w: f64, h: f64) -> Node {
        Node::new(
            "bounds",
            vec![
                ("xmi:type", s("dc:Bounds")),
                ("x", js_number(n1(x))),
                ("y", js_number(n1(y))),
                ("width", js_number(n1(w))),
                ("height", js_number(n1(h))),
            ],
        )
    }

    fn diagram(&self) -> Node {
        let m = self.m;
        let mut diagram = Node::new(
            "umldi:UMLStateMachineDiagram",
            vec![
                ("xmi:id", format!("{}_diagram", m.id)),
                ("name", m.name.clone()),
                ("modelElement", m.id.clone()),
                ("isFrame", s("false")),
            ],
        );
        let shape = |id: &str, (x, y, w, h): (f64, f64, f64, f64)| {
            Node::new(
                "ownedElement",
                vec![("xmi:type", s("umldi:UMLShape")), ("xmi:id", format!("di_{id}")), ("modelElement", s(id))],
            )
            .with([Self::bounds(x, y, w, h)])
        };
        for v in &m.vertices {
            diagram.push(shape(&v.id, (v.x, v.y, v.w, v.h)));
            if v.vtype == VertexType::State {
                for (r, b) in v.regions.iter().zip(region_bounds(v, m)) {
                    diagram.push(shape(&r.id, b));
                }
            }
        }
        for t in &m.transitions {
            let (Some(si), Some(gi)) = (self.index.vertex(&t.source), self.index.vertex(&t.target)) else { continue };
            let (sv, gv) = (self.index.v(si), self.index.v(gi));
            if t.kind == TransitionKind::Internal && si == gi && sv.vtype == VertexType::State {
                continue; // drawn in the state's compartment
            }
            let mut pts = vec![center(sv.x, sv.y, sv.w, sv.h)];
            pts.extend(t.points.iter().copied());
            pts.push(center(gv.x, gv.y, gv.w, gv.h));
            let mut edge = Node::new(
                "ownedElement",
                vec![
                    ("xmi:type", s("umldi:UMLEdge")),
                    ("xmi:id", format!("di_{}", t.id)),
                    ("modelElement", t.id.clone()),
                    ("source", format!("di_{}", sv.id)),
                    ("target", format!("di_{}", gv.id)),
                ],
            );
            for p in &pts {
                edge.push(Node::new(
                    "waypoint",
                    vec![("xmi:type", s("dc:Point")), ("x", js_number(n1(p.x))), ("y", js_number(n1(p.y)))],
                ));
            }
            if let Some(off) = t.label_offset {
                let mid = polyline_mid(&pts);
                let (cx, cy) = (mid.x + off.x, mid.y + off.y);
                edge.push(
                    Node::new(
                        "ownedElement",
                        vec![
                            ("xmi:type", s("umldi:UMLLabel")),
                            ("xmi:id", format!("di_{}_label", t.id)),
                            ("modelElement", t.id.clone()),
                        ],
                    )
                    .with([Self::bounds(cx - LABEL_W / 2.0, cy - LABEL_H / 2.0, LABEL_W, LABEL_H)]),
                );
            }
            diagram.push(edge);
        }
        for c in m.vertices.iter().filter(|c| c.vtype == VertexType::Comment) {
            for (i, a) in c.anchors.iter().enumerate() {
                if !self.index.has_vertex(a) && !m.transitions.iter().any(|t| &t.id == a) {
                    continue;
                }
                diagram.push(Node::new(
                    "ownedElement",
                    vec![
                        ("xmi:type", s("umldi:UMLEdge")),
                        ("xmi:id", format!("di_{}_anchor{}", c.id, i + 1)),
                        ("modelElement", c.id.clone()),
                        ("source", format!("di_{}", c.id)),
                        ("target", format!("di_{a}")),
                    ],
                ));
            }
        }
        // Colors and groups.
        let mut styles = Vec::new();
        let colored =
            m.vertices.iter().map(|v| (&v.id, &v.colors)).chain(m.transitions.iter().map(|t| (&t.id, &t.colors)));
        for (id, c) in colored {
            let mut attrs = Vec::new();
            if is_color(&c.color) {
                attrs.push(("color", c.color.clone()));
            }
            for k in ITEM_COLOR_KEYS {
                if let Some((_, value)) = c.items.iter().find(|(key, value)| *key == k && is_color(value)) {
                    attrs.push((k, value.clone()));
                }
            }
            if !attrs.is_empty() {
                let mut all = vec![("element", id.clone())];
                all.extend(attrs);
                styles.push(Node::new("style", all));
            }
        }
        let mut groups: Vec<(&str, Vec<&str>)> = Vec::new();
        for v in m.vertices.iter().filter(|v| !v.group.is_empty()) {
            match groups.iter_mut().find(|(g, _)| *g == v.group) {
                Some((_, members)) => members.push(&v.id),
                None => groups.push((&v.group, vec![&v.id])),
            }
        }
        for (id, members) in groups {
            styles.push(Node::new("group", vec![("id", s(id)), ("members", members.join(" "))]));
        }
        if !styles.is_empty() {
            diagram.push(Node::new("xmi:Extension", vec![("extender", s(EXTENDER))]).with(styles));
        }
        diagram
    }

    fn document(mut self) -> Result<String, String> {
        let m = self.m;
        let sm = &m.id;
        let mut machine = Node::new(
            "packagedElement",
            vec![
                ("xmi:type", s(if self.protocol { "uml:ProtocolStateMachine" } else { "uml:StateMachine" })),
                ("xmi:id", sm.clone()),
                ("name", m.name.clone()),
            ],
        );
        if !m.documentation.is_empty() {
            machine.push(
                Node::new(
                    "ownedComment",
                    vec![
                        ("xmi:type", s("uml:Comment")),
                        ("xmi:id", format!("{sm}_doc")),
                        ("annotatedElement", sm.clone()),
                    ],
                )
                .with([Node::text("body", &m.documentation)]),
            );
        }
        for v in m.vertices.iter().filter(|v| self.is_machine_point(v)) {
            machine.push(Self::point_node("connectionPoint", v));
        }
        for r in &m.regions {
            let region = self.region_node(r)?;
            machine.push(region);
        }

        let mut packaged = Vec::new();
        if m.context.is_empty() {
            packaged.push(machine);
        } else {
            // The context classifier owns the state machine as its classifier behavior.
            machine.name = "ownedBehavior".to_string();
            packaged.push(
                Node::new(
                    "packagedElement",
                    vec![
                        ("xmi:type", s("uml:Class")),
                        ("xmi:id", format!("{sm}_context")),
                        ("name", m.context.clone()),
                        ("classifierBehavior", sm.clone()),
                    ],
                )
                .with([machine]),
            );
        }
        // Events are collected while building the machine, so they come after it.
        packaged.append(&mut self.event_elements);

        // Stereotypes: an embedded profile with one stereotype per name, extending UML::State.
        let stereotyped: Vec<&Vertex> =
            m.vertices.iter().filter(|v| v.vtype == VertexType::State && !v.stereotype.is_empty()).collect();
        let mut names: Vec<&str> = Vec::new();
        for v in &stereotyped {
            if !names.contains(&v.stereotype.as_str()) {
                names.push(&v.stereotype);
            }
        }
        let mut model_node = Node::new("uml:Model", vec![("xmi:id", format!("{sm}_model")), ("name", m.name.clone())]);
        let mut applications = Vec::new();
        if !names.is_empty() {
            let mut profile = Node::new(
                "packagedElement",
                vec![
                    ("xmi:type", s("uml:Profile")),
                    ("xmi:id", s("fsm_profile")),
                    ("name", s(PROFILE_NAME)),
                    ("URI", s(PROFILE_NS)),
                ],
            )
            .with([Node::new(
                "metaclassReference",
                vec![("xmi:type", s("uml:ElementImport")), ("xmi:id", s("fsm_profile_state"))],
            )
            .with([Node::new("importedElement", vec![("xmi:type", s("uml:Class")), ("href", s(UML_STATE))])])]);
            for st in &names {
                profile.push(
                    Node::new(
                        "packagedElement",
                        vec![("xmi:type", s("uml:Stereotype")), ("xmi:id", format!("st_{st}")), ("name", s(st))],
                    )
                    .with([Node::new(
                        "ownedAttribute",
                        vec![
                            ("xmi:type", s("uml:Property")),
                            ("xmi:id", format!("st_{st}_base")),
                            ("name", s("base_State")),
                            ("association", format!("ext_{st}")),
                        ],
                    )
                    .with([Node::new("type", vec![("href", s(UML_STATE))])])]),
                );
                profile.push(
                    Node::new(
                        "packagedElement",
                        vec![
                            ("xmi:type", s("uml:Extension")),
                            ("xmi:id", format!("ext_{st}")),
                            ("name", format!("E_{st}_State")),
                            ("memberEnd", format!("st_{st}_base ext_{st}_end")),
                        ],
                    )
                    .with([Node::new(
                        "ownedEnd",
                        vec![
                            ("xmi:type", s("uml:ExtensionEnd")),
                            ("xmi:id", format!("ext_{st}_end")),
                            ("name", format!("extension_{st}")),
                            ("type", format!("st_{st}")),
                            ("association", format!("ext_{st}")),
                            ("aggregation", s("composite")),
                        ],
                    )]),
                );
            }
            packaged.push(profile);
            model_node.push(Node::new(
                "profileApplication",
                vec![
                    ("xmi:type", s("uml:ProfileApplication")),
                    ("xmi:id", s("fsm_profile_app")),
                    ("appliedProfile", s("fsm_profile")),
                ],
            ));
            for v in &stereotyped {
                let mut a = Node::new("", vec![("xmi:id", format!("{}_st", v.id)), ("base_State", v.id.clone())]);
                a.name = format!("{PROFILE_NAME}:{}", v.stereotype);
                applications.push(a);
            }
        }
        model_node = model_node.with(packaged);

        let mut attrs = vec![
            ("xmi:version", s("20131001")),
            ("xmlns:xmi", s(NS_XMI)),
            ("xmlns:uml", s(NS_UML)),
            ("xmlns:umldi", s(NS_UMLDI)),
            ("xmlns:dc", s(NS_DC)),
            ("xmlns:di", s(NS_DI)),
        ];
        if !names.is_empty() {
            attrs.push(("xmlns:FsmStereotypes", s(PROFILE_NS)));
        }
        let diagram = self.diagram();
        let doc = Node::new("xmi:XMI", attrs).with([model_node, diagram]).with(applications);
        Ok(write_xml(&doc))
    }
}

/// `/^after\((.+)\)$/`: the time expression of a time trigger (`after(2s)` → `2s`).
fn time_expression(trigger: &str) -> Option<&str> {
    let inner = trigger.strip_prefix("after(")?.strip_suffix(')')?;
    (!inner.is_empty() && !inner.contains(['\n', '\r', '\u{2028}', '\u{2029}'])).then_some(inner)
}

/// The editors' color value: `#rrggbb`, lowercase.
fn is_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xmi::from_xmi;

    #[test]
    fn numbers_as_javascript_writes_them() {
        let cases: [(f64, &str); 16] = [
            (0.0, "0"),
            (-0.0, "0"),
            (12.0, "12"),
            (-12.5, "-12.5"),
            (0.1 + 0.2, "0.30000000000000004"),
            (123456789012345680000.0, "123456789012345680000"),
            (1e21, "1e+21"),
            (1.5e300, "1.5e+300"),
            (0.000001, "0.000001"),
            (0.0000012, "0.0000012"),
            (1e-7, "1e-7"),
            (-2.5e-8, "-2.5e-8"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
            (5e-324, "5e-324"),
        ];
        for (n, want) in cases {
            assert_eq!(js_number(n), want, "{n:e}");
        }
    }

    #[test]
    fn wraps_as_the_editors_do() {
        // 10 units are 66 px.
        assert_eq!(wrap_line_count("entry / a()", 200.0), 1);
        assert_eq!(wrap_line_count("aaaa bbbb cccc", 66.0), 2); // "aaaa bbbb" fits (9 units), "cccc" goes down
        assert_eq!(wrap_line_count("abcdefghijklmnopqrstu", 66.0), 3); // cut after 10, 10, then 1 left
        assert_eq!(wrap_line_count("a\nb\r\nc", 66.0), 3);
        assert_eq!(wrap_line_count("abc", -16.0), 3);
        assert_eq!(wrap_line_count("", 10.0), 1);
        // UTF-16 units: an emoji counts twice.
        assert_eq!(wrap_line_count("😀😀😀😀😀", 66.0), 1);
        assert_eq!(wrap_line_count("😀😀😀😀😀😀", 66.0), 2);
    }

    #[test]
    fn regions_share_what_their_sizes_leave() {
        let r = |size: Option<f64>| Region { id: String::new(), name: String::new(), size };
        assert_eq!(region_sizes(&[r(Some(30.0)), r(Some(50.0))], 100.0), [30.0, 70.0]);
        assert_eq!(region_sizes(&[r(None), r(Some(50.0))], 100.0), [50.0, 50.0]);
        assert_eq!(region_sizes(&[r(Some(120.0)), r(None)], 100.0), [50.0, 50.0]);
        assert_eq!(region_sizes(&[r(Some(20.0)), r(Some(20.0)), r(None)], 90.0), [20.0, 20.0, 50.0]);
    }

    #[test]
    fn writes_what_it_reads() {
        let text = r##"<?xml version="1.0" encoding="UTF-8"?>
<xmi:XMI xmi:version="20131001" xmlns:xmi="http://www.omg.org/spec/XMI/20131001" xmlns:uml="http://www.omg.org/spec/UML/20161101" xmlns:umldi="http://www.omg.org/spec/UML/20161101/UMLDI" xmlns:dc="http://www.omg.org/spec/DD/20100524/DC" xmlns:di="http://www.omg.org/spec/DD/20100524/DI">
  <uml:Model xmi:id="sm_model" name="Lamp">
    <packagedElement xmi:type="uml:StateMachine" xmi:id="sm" name="Lamp">
      <region xmi:type="uml:Region" xmi:id="r">
        <ownedComment xmi:type="uml:Comment" xmi:id="n" annotatedElement="Off">
          <body>Two
lines &amp; &quot;quotes&quot;</body>
        </ownedComment>
        <subvertex xmi:type="uml:Pseudostate" xmi:id="i" kind="initial"/>
        <subvertex xmi:type="uml:State" xmi:id="Off" name="Off">
          <entry xmi:type="uml:OpaqueBehavior" xmi:id="Off_entry">
            <language>FSM</language>
            <body>dim()</body>
          </entry>
          <deferrableTrigger xmi:type="uml:Trigger" xmi:id="Off_deferrableTrigger1" name="press" event="ev_press"/>
        </subvertex>
        <transition xmi:type="uml:Transition" xmi:id="t0" kind="external" source="i" target="Off"/>
        <transition xmi:type="uml:Transition" xmi:id="t1" kind="external" source="Off" target="Off">
          <trigger xmi:type="uml:Trigger" xmi:id="t1_trigger1" name="after(2s)" event="ev_after_2s_"/>
        </transition>
      </region>
    </packagedElement>
    <packagedElement xmi:type="uml:SignalEvent" xmi:id="ev_press" name="press" signal="sig_press"/>
    <packagedElement xmi:type="uml:Signal" xmi:id="sig_press" name="press"/>
    <packagedElement xmi:type="uml:TimeEvent" xmi:id="ev_after_2s_" name="after(2s)" isRelative="true">
      <when xmi:type="uml:TimeExpression" xmi:id="ev_after_2s__when">
        <expr xmi:type="uml:LiteralString" xmi:id="ev_after_2s__expr" value="2s"/>
      </when>
    </packagedElement>
  </uml:Model>
  <umldi:UMLStateMachineDiagram xmi:id="sm_diagram" name="Lamp" modelElement="sm" isFrame="false">
    <ownedElement xmi:type="umldi:UMLShape" xmi:id="di_n" modelElement="n">
      <bounds xmi:type="dc:Bounds" x="300" y="20" width="160" height="64"/>
    </ownedElement>
    <ownedElement xmi:type="umldi:UMLShape" xmi:id="di_i" modelElement="i">
      <bounds xmi:type="dc:Bounds" x="20" y="40" width="20" height="20"/>
    </ownedElement>
    <ownedElement xmi:type="umldi:UMLShape" xmi:id="di_Off" modelElement="Off">
      <bounds xmi:type="dc:Bounds" x="80.5" y="20" width="140" height="60"/>
    </ownedElement>
    <ownedElement xmi:type="umldi:UMLEdge" xmi:id="di_t0" modelElement="t0" source="di_i" target="di_Off">
      <waypoint xmi:type="dc:Point" x="30" y="50"/>
      <waypoint xmi:type="dc:Point" x="150.5" y="50"/>
    </ownedElement>
    <ownedElement xmi:type="umldi:UMLEdge" xmi:id="di_t1" modelElement="t1" source="di_Off" target="di_Off">
      <waypoint xmi:type="dc:Point" x="150.5" y="50"/>
      <waypoint xmi:type="dc:Point" x="230.5" y="0"/>
      <waypoint xmi:type="dc:Point" x="150.5" y="50"/>
      <ownedElement xmi:type="umldi:UMLLabel" xmi:id="di_t1_label" modelElement="t1">
        <bounds xmi:type="dc:Bounds" x="190.5" y="9" width="100" height="16"/>
      </ownedElement>
    </ownedElement>
    <ownedElement xmi:type="umldi:UMLEdge" xmi:id="di_n_anchor1" modelElement="n" source="di_n" target="di_Off"/>
    <xmi:Extension extender="FSM Editor">
      <style element="Off" color="#336699" entry="#ff0000"/>
    </xmi:Extension>
  </umldi:UMLStateMachineDiagram>
</xmi:XMI>
"##;
        assert_eq!(to_xmi(&from_xmi(text).unwrap()).unwrap(), text);
    }

    #[test]
    fn refuses_a_state_inside_its_own_region() {
        let mut m = from_xmi(
            r#"<xmi:XMI xmlns:xmi="http://www.omg.org/spec/XMI/20131001" xmlns:uml="http://www.omg.org/spec/UML/20161101">
  <uml:StateMachine xmi:id="sm"><region xmi:id="r"><subvertex xmi:type="uml:State" xmi:id="A"><region xmi:id="ra"/></subvertex></region></uml:StateMachine>
</xmi:XMI>"#,
        )
        .unwrap();
        // C, in the machine's region, has a region with A's id: A is inside its own region.
        let mut c = Vertex::new("C", VertexType::State, "C", "r");
        c.regions.push(Region::new("ra", ""));
        m.vertices.insert(0, c);
        m.vertices[1].parent = "ra".to_string();
        assert_eq!(to_xmi(&m), Err("State 'A' is inside one of its own regions.".to_string()));
    }
}
