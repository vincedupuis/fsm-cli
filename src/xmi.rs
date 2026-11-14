//! Reads state machine files: XMI 2.5.1 holding a UML 2.5.1 model plus a UML
//! DI diagram (the layout). The same reading rules as `fromXmi` in
//! fsm-editor-vscode/src/xmi.ts, layout included (its mapping is described at
//! the top of that file), and the model hash of a file.

use std::collections::HashMap;

use crate::geometry::polyline_mid;
use crate::model::{
    Colors, FsmModel, ITEM_COLOR_KEYS, MachineKind, ModelIndex, Point, PointKind, Region, RegionLayout, Transition,
    TransitionKind, Vertex, VertexType, js_space, js_trim,
};
use crate::sha256::sha256;
use crate::xml::{XmlElement, parse_xml};

const NS_XMI: &str = "http://www.omg.org/spec/XMI/20131001";
const NS_UML: &str = "http://www.omg.org/spec/UML/20161101";
const NS_UMLDI: &str = "http://www.omg.org/spec/UML/20161101/UMLDI";
const NS_DC: &str = "http://www.omg.org/spec/DD/20100524/DC";
const NS_DI: &str = "http://www.omg.org/spec/DD/20100524/DI";
const PROFILE_NS: &str = "https://github.com/vincedupuis/fsm-editor/profiles/FsmStereotypes";
const EXTENDER: &str = "FSM Editor";

fn type_of(el: &XmlElement) -> String {
    match el.attr("xmi:type") {
        Some(t) => t.to_string(),
        None if el.ns == NS_UML => format!("uml:{}", el.local),
        None => String::new(),
    }
}

fn id_of(el: &XmlElement) -> &str {
    el.attr("xmi:id").unwrap_or("")
}

fn kids<'a>(el: &'a XmlElement, name: &'a str) -> impl Iterator<Item = &'a XmlElement> + 'a {
    el.children.iter().filter(move |c| c.name == name)
}

fn kid<'a>(el: &'a XmlElement, name: &str) -> Option<&'a XmlElement> {
    el.children.iter().find(|c| c.name == name)
}

/// A reference property, written as an attribute (`a="id1 id2"`) or as child elements (`<a xmi:idref|href>`).
fn refs(el: &XmlElement, name: &str) -> Vec<String> {
    let mut out: Vec<String> = el.attr(name).unwrap_or("").split_whitespace().map(str::to_string).collect();
    for c in kids(el, name) {
        if let Some(r) = c.attr("xmi:idref").or_else(|| c.attr("href"))
            && !r.is_empty()
        {
            out.push(r.to_string());
        }
    }
    out
}

fn local_id(r: &str) -> &str {
    match r.find('#') {
        Some(i) => &r[i + 1..],
        None => r,
    }
}

/// Text of an opaque behavior or constraint (its first body), trimmed.
fn body_of(el: Option<&XmlElement>) -> String {
    js_trim(&raw_body_of(el)).to_string()
}

/// Text of an opaque behavior, constraint or comment (its first body), as written.
fn raw_body_of(el: Option<&XmlElement>) -> String {
    let Some(el) = el else { return String::new() };
    let spec = kid(el, "specification").unwrap_or(el);
    if let Some(b) = kid(spec, "body") {
        return b.text.clone();
    }
    spec.attr("body").or_else(|| spec.attr("value")).unwrap_or("").to_string()
}

/// JavaScript's `Number(s)` (`+s`): NaN when `s` is absent or isn't a number.
fn js_number(s: Option<&str>) -> f64 {
    let Some(s) = s else { return f64::NAN };
    let t = js_trim(s);
    if t.is_empty() {
        return 0.0;
    }
    let b = t.as_bytes();
    if b.len() > 2 && b[0] == b'0' {
        let radix = match b[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 0,
        };
        if radix != 0 {
            let mut n = 0.0;
            for c in t[2..].chars() {
                match c.to_digit(radix) {
                    Some(d) => n = n * f64::from(radix) + f64::from(d),
                    None => return f64::NAN,
                }
            }
            return n;
        }
    }
    let (sign, rest) = match t.strip_prefix('-') {
        Some(r) => (-1.0, r),
        None => (1.0, t.strip_prefix('+').unwrap_or(t)),
    };
    if rest == "Infinity" {
        return sign * f64::INFINITY;
    }
    // Digits, an optional fraction and an optional exponent, with at least one digit before the exponent.
    let mantissa = rest.split(['e', 'E']).next().unwrap_or("");
    let exponent = &rest[mantissa.len()..];
    let digits = |s: &str| s.bytes().all(|c| c.is_ascii_digit());
    let mantissa_ok = match mantissa.split_once('.') {
        Some((i, f)) => digits(i) && digits(f) && !(i.is_empty() && f.is_empty()),
        None => !mantissa.is_empty() && digits(mantissa),
    };
    let exponent_ok = exponent.is_empty() || {
        let e = &exponent[1..];
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        !e.is_empty() && digits(e)
    };
    if !mantissa_ok || !exponent_ok {
        return f64::NAN;
    }
    rest.parse::<f64>().map_or(f64::NAN, |n| sign * n)
}

/// `+s || 0`: a coordinate of the diagram, 0 when absent or not a number.
fn coordinate(s: Option<&str>) -> f64 {
    let n = js_number(s);
    if n.is_nan() || n == 0.0 { 0.0 } else { n }
}

/// JavaScript's `s.split(/\s+/)`.
fn js_split_space(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_space = false;
    for (i, c) in s.char_indices() {
        if js_space(c) {
            if !in_space {
                out.push(&s[start..i]);
                in_space = true;
            }
            start = i + c.len_utf8();
        } else {
            in_space = false;
        }
    }
    out.push(&s[start..]);
    out
}

/// The editors' color value: `#rrggbb`, lowercase.
fn is_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn walk<'a>(el: &'a XmlElement, out: &mut Vec<&'a XmlElement>) {
    out.push(el);
    for c in &el.children {
        walk(c, out);
    }
}

fn canonical() -> HashMap<&'static str, &'static str> {
    [(NS_XMI, "xmi"), (NS_UML, "uml"), (NS_UMLDI, "umldi"), (NS_DC, "dc"), (NS_DI, "di")].into_iter().collect()
}

/// Hash of the state machine model in a file (`text`): the first 16 hex digits of the SHA-256 of the
/// UTF-8 canonical form of its document element, leaving out the layout. The canonical form writes
/// each element as `<name a="v" …>text children</name>`, with
/// - names under the canonical prefixes (as parse_xml gives them), attributes sorted by name, without
///   the namespace declarations;
/// - the element's own text, left out when it is only XML white space (space, tab, CR, LF);
/// - `&`, `<`, `>` and `"` written as `&amp;`, `&lt;`, `&gt;` and `&quot;` in values and text;
/// - no UML DI elements (the diagram) and no FSM Editor xmi:Extension (colors, groups, the hash itself).
pub fn model_hash(text: &str) -> Result<String, String> {
    fn esc(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
    }
    fn write(el: &XmlElement, out: &mut String) {
        if el.ns == NS_UMLDI || (el.name == "xmi:Extension" && el.attr("extender") == Some(EXTENDER)) {
            return;
        }
        out.push('<');
        out.push_str(&el.name);
        let mut attrs: Vec<&(String, String)> = el.attrs().iter().collect();
        attrs.sort_by(|a, b| a.0.cmp(&b.0));
        for (k, v) in attrs {
            out.push_str(&format!(" {k}=\"{}\"", esc(v)));
        }
        out.push('>');
        if el.text.chars().any(|c| !matches!(c, ' ' | '\t' | '\r' | '\n')) {
            out.push_str(&esc(&el.text));
        }
        for c in &el.children {
            write(c, out);
        }
        out.push_str(&format!("</{}>", el.name));
    }
    let root = parse_xml(text, &canonical()).map_err(|e| e.0)?;
    let mut out = String::new();
    write(&root, &mut out);
    Ok(sha256(&out)[..16].to_string())
}

/// Parses a state machine file.
pub fn from_xmi(text: &str) -> Result<FsmModel, String> {
    let root = parse_xml(text, &canonical()).map_err(|e| e.0)?;
    if root.name != "xmi:XMI" && root.name != "uml:Model" {
        return Err(format!("Not an XMI document: the root element is <{}>, expected <xmi:XMI>.", root.name));
    }

    let mut elements_by_id: HashMap<&str, &XmlElement> = HashMap::new();
    let mut machine: Option<(&XmlElement, Option<&XmlElement>)> = None;
    fn visit<'a>(
        el: &'a XmlElement,
        parent: Option<&'a XmlElement>,
        by_id: &mut HashMap<&'a str, &'a XmlElement>,
        machine: &mut Option<(&'a XmlElement, Option<&'a XmlElement>)>,
    ) {
        let id = id_of(el);
        if !id.is_empty() {
            by_id.insert(id, el);
        }
        let t = type_of(el);
        if machine.is_none() && (t == "uml:StateMachine" || t == "uml:ProtocolStateMachine") {
            let context = parent.filter(|p| type_of(p) == "uml:Class");
            *machine = Some((el, context));
        }
        for c in &el.children {
            visit(c, Some(el), by_id, machine);
        }
    }
    visit(&root, None, &mut elements_by_id, &mut machine);
    let Some((sm, context)) = machine else {
        return Err("The document does not contain a UML state machine.".to_string());
    };

    let event_text = |trig: &XmlElement| -> String {
        let event_ref = trig
            .attr("event")
            .map(str::to_string)
            .or_else(|| refs(trig, "event").into_iter().next())
            .unwrap_or_default();
        let ev = elements_by_id.get(local_id(&event_ref)).copied();
        let trig_name = trig.attr("name").unwrap_or("");
        if let Some(ev) = ev {
            let t = type_of(ev);
            if t == "uml:TimeEvent" {
                let expr = kid(kid(ev, "when").unwrap_or(ev), "expr");
                let value = match expr {
                    Some(e) => e.attr("value").map(str::to_string).unwrap_or_else(|| body_of(Some(e))),
                    None => String::new(),
                };
                if !value.is_empty() {
                    return format!("after({value})");
                }
            }
            if t == "uml:SignalEvent" {
                let sig = elements_by_id.get(local_id(ev.attr("signal").unwrap_or(""))).copied();
                for name in [sig.and_then(|s| s.attr("name")), ev.attr("name"), Some(trig_name)] {
                    if let Some(n) = name.filter(|n| !n.is_empty()) {
                        return n.to_string();
                    }
                }
                return String::new();
            }
            if let Some(n) = ev.attr("name").filter(|n| !n.is_empty()) {
                return n.to_string();
            }
        }
        trig_name.to_string()
    };

    let id = id_of(sm);
    let mut model = FsmModel {
        id: if id.is_empty() { "sm".to_string() } else { id.to_string() },
        name: sm.attr("name").unwrap_or("").to_string(),
        kind: if type_of(sm) == "uml:ProtocolStateMachine" { MachineKind::Protocol } else { MachineKind::Behavioral },
        context: context.and_then(|c| c.attr("name")).unwrap_or("").to_string(),
        documentation: String::new(),
        regions: Vec::new(),
        vertices: Vec::new(),
        transitions: Vec::new(),
    };
    for c in kids(sm, "ownedComment") {
        if refs(c, "annotatedElement").contains(&model.id) {
            model.documentation = body_of(Some(c));
        }
    }

    struct Reader<'a, F: Fn(&XmlElement) -> String> {
        event_text: F,
        model: &'a mut FsmModel,
    }

    impl<F: Fn(&XmlElement) -> String> Reader<'_, F> {
        fn transition(&mut self, el: &XmlElement) {
            let end = |name: &str| {
                local_id(
                    &el.attr(name)
                        .map(str::to_string)
                        .or_else(|| refs(el, name).into_iter().next())
                        .unwrap_or_default(),
                )
                .to_string()
            };
            let kind = match el.attr("kind") {
                Some("internal") => TransitionKind::Internal,
                Some("local") => TransitionKind::Local,
                _ => TransitionKind::External,
            };
            let t = Transition {
                id: id_of(el).to_string(),
                source: end("source"),
                target: end("target"),
                kind,
                triggers: kids(el, "trigger").map(|t| (self.event_text)(t)).filter(|t| !t.is_empty()).collect(),
                guard: body_of(kid(el, "guard")),
                effect: body_of(kid(el, "effect")),
                precondition: body_of(kid(el, "preCondition")),
                postcondition: body_of(kid(el, "postCondition")),
                points: Vec::new(),
                label_offset: None,
                colors: Colors::default(),
            };
            self.model.transitions.push(t);
        }

        fn point(&mut self, el: &XmlElement, parent: &str) {
            let kind =
                if el.attr("kind") == Some("exitPoint") { VertexType::ExitPoint } else { VertexType::EntryPoint };
            self.model.vertices.push(Vertex::new(id_of(el), kind, el.attr("name").unwrap_or(""), parent));
        }

        fn vertex(&mut self, el: &XmlElement, parent: &str) {
            let t = type_of(el);
            let name = el.attr("name").unwrap_or("");
            if t == "uml:FinalState" {
                self.model.vertices.push(Vertex::new(id_of(el), VertexType::Final, name, parent));
                return;
            }
            if t == "uml:Pseudostate" {
                let kind = VertexType::pseudostate(el.attr("kind").unwrap_or("initial")).unwrap_or(VertexType::Initial);
                self.model.vertices.push(Vertex::new(id_of(el), kind, name, parent));
                return;
            }
            let mut v = Vertex::new(id_of(el), VertexType::State, name, parent);
            v.entry = body_of(kid(el, "entry"));
            v.exit = body_of(kid(el, "exit"));
            v.do_activity = body_of(kid(el, "doActivity"));
            v.invariant = body_of(kid(el, "stateInvariant"));
            v.deferrable =
                kids(el, "deferrableTrigger").map(|t| (self.event_text)(t)).filter(|t| !t.is_empty()).collect();
            v.submachine = refs(el, "submachine").into_iter().next();
            let state = self.model.vertices.len();
            let vid = v.id.clone();
            self.model.vertices.push(v);
            for cp in kids(el, "connectionPoint") {
                self.point(cp, &vid);
            }
            for c in kids(el, "connection") {
                let entry_ref = refs(c, "entry").into_iter().next();
                let exit_ref = refs(c, "exit").into_iter().next();
                let mut r = Vertex::new(id_of(c), VertexType::ConnectionPointRef, "", &vid);
                r.point_kind =
                    if exit_ref.is_some() && entry_ref.is_none() { PointKind::Exit } else { PointKind::Entry };
                r.point_ref = local_id(&entry_ref.or(exit_ref).unwrap_or_default()).to_string();
                self.model.vertices.push(r);
            }
            for r in kids(el, "region") {
                let region = Region::new(id_of(r), r.attr("name").unwrap_or(""));
                self.model.vertices[state].regions.push(region);
                self.region(r);
            }
        }

        fn region(&mut self, el: &XmlElement) {
            let id = id_of(el);
            for c in &el.children {
                match c.name.as_str() {
                    "subvertex" => self.vertex(c, id),
                    "transition" => self.transition(c),
                    "ownedComment" => {
                        let mut v = Vertex::new(id_of(c), VertexType::Comment, c.attr("name").unwrap_or(""), id);
                        v.text = raw_body_of(Some(c));
                        v.anchors = refs(c, "annotatedElement");
                        self.model.vertices.push(v);
                    }
                    _ => {}
                }
            }
        }
    }

    let mut reader = Reader { event_text, model: &mut model };
    for r in kids(sm, "region") {
        reader.model.regions.push(Region::new(id_of(r), r.attr("name").unwrap_or("")));
        reader.region(r);
    }
    if reader.model.regions.is_empty() {
        let id = format!("{}_region", reader.model.id);
        reader.model.regions.push(Region::new(&id, ""));
    }
    let first_region = reader.model.regions[0].id.clone();
    for cp in kids(sm, "connectionPoint") {
        reader.point(cp, &first_region);
    }

    // Stereotype applications: elements of the embedded profile's namespace.
    for c in &root.children {
        if c.ns != PROFILE_NS {
            continue;
        }
        let base = local_id(c.attr("base_State").unwrap_or(""));
        // The last vertex with that id, as a lookup by id finds it.
        if let Some(v) = model.vertices.iter_mut().rev().find(|v| v.id == base) {
            v.stereotype = c.local.clone();
        }
    }
    read_layout(&root, &mut model);
    normalize(&mut model);
    Ok(model)
}

#[derive(Clone, Copy)]
struct Bounds {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// The layout, from the UML DI diagram and the FSM Editor extension: shapes, edges, colors and groups. Elements are
/// found by id, the last one with an id as `fromXmi`'s maps do; vertices without a shape get a default place.
fn read_layout(root: &XmlElement, model: &mut FsmModel) {
    let vertex_at: HashMap<String, usize> = model.vertices.iter().enumerate().map(|(i, v)| (v.id.clone(), i)).collect();
    let transition_at: HashMap<String, usize> =
        model.transitions.iter().enumerate().map(|(i, t)| (t.id.clone(), i)).collect();
    let mut region_shapes: HashMap<String, Bounds> = HashMap::new();
    for d in root.children.iter().filter(|d| d.ns == NS_UMLDI) {
        for ext in kids(d, "xmi:Extension").filter(|e| e.attr("extender") == Some(EXTENDER)) {
            for st in kids(ext, "style") {
                let element = st.attr("element").unwrap_or("");
                let colors = if let Some(&i) = vertex_at.get(element) {
                    &mut model.vertices[i].colors
                } else if let Some(&i) = transition_at.get(element) {
                    &mut model.transitions[i].colors
                } else {
                    continue;
                };
                let color = st.attr("color").unwrap_or("").to_lowercase();
                if is_color(&color) {
                    colors.color = color;
                }
                let items: Vec<(&'static str, String)> = ITEM_COLOR_KEYS
                    .iter()
                    .map(|&k| (k, st.attr(k).unwrap_or("").to_lowercase()))
                    .filter(|(_, c)| is_color(c))
                    .collect();
                if !items.is_empty() {
                    colors.items = items;
                }
            }
            // A vertex listed in several groups stays in the first.
            for g in kids(ext, "group") {
                let id = g.attr("id").unwrap_or("");
                if id.is_empty() {
                    continue;
                }
                for member in js_split_space(g.attr("members").unwrap_or("")) {
                    if let Some(&i) = vertex_at.get(member)
                        && model.vertices[i].group.is_empty()
                    {
                        model.vertices[i].group = id.to_string();
                    }
                }
            }
        }
        let mut all = Vec::new();
        walk(d, &mut all);
        for el in all {
            let target = match el.attr("modelElement") {
                Some(t) => t.to_string(),
                None => refs(el, "modelElement").into_iter().next().unwrap_or_default(),
            };
            if target.is_empty() {
                continue;
            }
            let t = type_of(el);
            let b = kid(el, "bounds");
            if let (true, Some(b)) = (t == "umldi:UMLShape", b) {
                let bounds = Bounds {
                    x: coordinate(b.attr("x")),
                    y: coordinate(b.attr("y")),
                    w: coordinate(b.attr("width")),
                    h: coordinate(b.attr("height")),
                };
                match vertex_at.get(&target) {
                    Some(&i) => {
                        let v = &mut model.vertices[i];
                        (v.x, v.y, v.w, v.h) = (bounds.x, bounds.y, bounds.w, bounds.h);
                    }
                    None => {
                        region_shapes.insert(target, bounds);
                    }
                }
            } else if let (true, Some(&i)) = (t == "umldi:UMLEdge", transition_at.get(&target)) {
                let tr = &mut model.transitions[i];
                let pts: Vec<Point> = kids(el, "waypoint")
                    .map(|w| Point { x: coordinate(w.attr("x")), y: coordinate(w.attr("y")) })
                    .collect();
                if pts.len() > 2 {
                    tr.points = pts[1..pts.len() - 1].to_vec();
                }
                let label = el.children.iter().find(|c| type_of(c) == "umldi:UMLLabel");
                if let (Some(lb), true) = (label.and_then(|l| kid(l, "bounds")), pts.len() >= 2) {
                    let mid = polyline_mid(&pts);
                    let n = |k: &str| js_number(lb.attr(k));
                    // Not rounded: the middle is off the 0.1 px grid, and an offset rounded to 0.1 px could move the
                    // label by 0.1 px on each save. Written again from the same waypoints, it gives the same label box.
                    tr.label_offset =
                        Some(Point { x: n("x") + n("width") / 2.0 - mid.x, y: n("y") + n("height") / 2.0 - mid.y });
                }
            }
        }
    }
    for v in &mut model.vertices {
        if v.regions.len() >= 2
            && let (Some(a), Some(b)) = (region_shapes.get(&v.regions[0].id), region_shapes.get(&v.regions[1].id))
        {
            let horizontal = (b.x - a.x).abs() > (b.y - a.y).abs();
            v.region_layout = Some(if horizontal { RegionLayout::Horizontal } else { RegionLayout::Vertical });
        }
        // Each region's own size, along the layout.
        let horizontal = v.region_layout == Some(RegionLayout::Horizontal);
        for r in &mut v.regions {
            if let Some(b) = region_shapes.get(&r.id) {
                r.size = Some(if horizontal { b.w } else { b.h });
            }
        }
    }
    // Vertices without a shape get a default place and size.
    let mut slot = 0;
    for v in &mut model.vertices {
        if v.w == 0.0 || v.w.is_nan() || v.h == 0.0 || v.h.is_nan() {
            (v.w, v.h) = default_size(v.vtype);
        }
        if v.x.is_nan() || v.y.is_nan() {
            v.x = f64::from(40 + (slot % 5) * 180);
            v.y = f64::from(40 + (slot / 5) * 120);
            slot += 1;
        }
    }
}

/// The size of a vertex without one (no shape, or a width or height of 0).
pub(crate) fn default_size(t: VertexType) -> (f64, f64) {
    match t {
        VertexType::State => (140.0, 60.0),
        VertexType::Final | VertexType::ShallowHistory | VertexType::DeepHistory => (26.0, 26.0),
        VertexType::Initial | VertexType::Terminate => (20.0, 20.0),
        VertexType::Choice => (28.0, 28.0),
        VertexType::Junction => (14.0, 14.0),
        VertexType::Fork | VertexType::Join => (90.0, 8.0),
        VertexType::EntryPoint | VertexType::ExitPoint | VertexType::ConnectionPointRef => (16.0, 16.0),
        VertexType::Comment => (160.0, 64.0),
    }
}

/// `normalizeModel`'s layout rules: groups the diagram can't show are dissolved, and a state's self-transition
/// keeps its bend points only when it has 1 or 3 of them.
pub(crate) fn normalize(model: &mut FsmModel) {
    // A group is the vertices sharing a group id: those on a state's border leave it, and so do those outside the
    // region of its first member (in file order). A group left with fewer than two members is dissolved.
    let border: Vec<bool> = {
        let index = ModelIndex::new(model);
        (0..model.vertices.len()).map(|i| index.is_border(i)).collect()
    };
    let mut parents: HashMap<String, String> = HashMap::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (i, v) in model.vertices.iter_mut().enumerate() {
        if v.group.is_empty() || border[i] {
            v.group.clear();
            continue;
        }
        match parents.get(&v.group) {
            None => {
                parents.insert(v.group.clone(), v.parent.clone());
            }
            Some(p) if *p != v.parent => {
                v.group.clear();
                continue;
            }
            Some(_) => {}
        }
        *counts.entry(v.group.clone()).or_default() += 1;
    }
    for v in &mut model.vertices {
        if !v.group.is_empty() && counts.get(&v.group).copied().unwrap_or(0) < 2 {
            v.group.clear();
        }
    }
    let types: HashMap<&str, VertexType> = model.vertices.iter().map(|v| (v.id.as_str(), v.vtype)).collect();
    for t in &mut model.transitions {
        let n = t.points.len();
        if t.source == t.target
            && t.kind != TransitionKind::Internal
            && types.get(t.source.as_str()) == Some(&VertexType::State)
            && (n == 2 || n > 3)
        {
            t.points.clear();
            t.label_offset = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_as_javascript_reads_them() {
        let n = |s: &str| js_number(Some(s));
        assert_eq!(n(" 12.5 "), 12.5);
        assert_eq!(n(""), 0.0);
        assert_eq!(n("1e2"), 100.0);
        assert_eq!(n("+.5e1"), 5.0);
        assert_eq!(n("5."), 5.0);
        assert_eq!(n("0x1F"), 31.0);
        assert_eq!(n("-Infinity"), f64::NEG_INFINITY);
        for bad in ["abc", "12px", "1e", ".", "+", "-0x10", "1_0", "inf", "NaN", "0x"] {
            assert!(n(bad).is_nan(), "{bad}");
        }
        assert!(js_number(None).is_nan());
        assert_eq!(coordinate(Some("abc")), 0.0);
        assert_eq!(coordinate(Some("-0")).to_bits(), 0.0f64.to_bits());
        assert_eq!(js_split_space(" a  b "), ["", "a", "b", ""]);
        assert_eq!(js_split_space(""), [""]);
    }

    const MACHINE: &str = r##"<xmi:XMI xmlns:xmi="http://www.omg.org/spec/XMI/20131001" xmlns:uml="http://www.omg.org/spec/UML/20161101" xmlns:umldi="http://www.omg.org/spec/UML/20161101/UMLDI">
  <uml:StateMachine xmi:id="sm" name="M">
    <region xmi:id="r">
      <ownedComment xmi:id="n" annotatedElement="A B"><body> note </body></ownedComment>
      <subvertex xmi:type="uml:State" xmi:id="A" name="A">
        <region xmi:id="ra"/><region xmi:id="rb"/>
        <connectionPoint xmi:type="uml:Pseudostate" xmi:id="ep" kind="entryPoint"/>
      </subvertex>
      <subvertex xmi:type="uml:State" xmi:id="B" name="B"/>
      <subvertex xmi:type="uml:Pseudostate" xmi:id="i" kind="initial"/>
      <transition xmi:id="loop" source="A" target="A"/>
      <transition xmi:id="t" source="A" target="B"/>
    </region>
  </uml:StateMachine>
  <umldi:UMLStateMachineDiagram xmi:id="d" modelElement="sm">
    <ownedElement xmi:type="umldi:UMLShape" modelElement="A"><bounds x="10" y="20" width="300" height="0"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLShape" modelElement="ra"><bounds x="10" y="40" width="150" height="80"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLShape" modelElement="rb"><bounds x="160" y="40" width="150" height="80"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLEdge" modelElement="loop"><waypoint x="1" y="1"/><waypoint x="2" y="2"/><waypoint x="3" y="3"/><waypoint x="4" y="4"/></ownedElement>
    <ownedElement xmi:type="umldi:UMLEdge" modelElement="t"><waypoint x="0" y="0"/><waypoint x="100" y="0"/><ownedElement xmi:type="umldi:UMLLabel"><bounds x="0" y="10" width="100" height="16"/></ownedElement></ownedElement>
    <xmi:Extension extender="FSM Editor">
      <style element="B" color="#AABBCC" guard="#112233" entry="red"/>
      <style element="t" effect="#000000"/>
      <group id="g1" members="A ep"/>
      <group id="g2" members=" B i A "/>
    </xmi:Extension>
  </umldi:UMLStateMachineDiagram>
</xmi:XMI>"##;

    #[test]
    fn reads_the_layout() {
        let m = from_xmi(MACHINE).unwrap();
        let v = |id: &str| m.vertices.iter().find(|v| v.id == id).unwrap();
        // A zero height takes the default size; vertices without a shape take default places, in file order.
        assert_eq!((v("A").x, v("A").y, v("A").w, v("A").h), (10.0, 20.0, 140.0, 60.0));
        assert_eq!((v("n").x, v("n").y, v("n").w, v("n").h), (40.0, 40.0, 160.0, 64.0));
        assert_eq!((v("ep").x, v("ep").y), (220.0, 40.0));
        assert_eq!((v("n").text.as_str(), v("n").anchors.clone()), (" note ", vec!["A".to_string(), "B".to_string()]));
        assert_eq!(v("A").region_layout, Some(RegionLayout::Horizontal));
        assert_eq!(v("A").regions.iter().map(|r| r.size).collect::<Vec<_>>(), [Some(150.0), Some(150.0)]);
        assert_eq!(v("B").colors.color, "#aabbcc");
        assert_eq!(v("B").colors.items, [("guard", "#112233".to_string())]);
        // The entry point on A's border leaves g1, left with one member; A stays in its first group.
        assert_eq!(
            (v("A").group.as_str(), v("ep").group.as_str(), v("B").group.as_str(), v("i").group.as_str()),
            ("", "", "g2", "g2")
        );
        // A self-transition of a state with 2 bend points goes back to the default loop.
        assert!(m.transitions[0].points.is_empty());
        assert_eq!(m.transitions[1].label_offset, Some(Point { x: 0.0, y: 18.0 }));
        assert_eq!(m.transitions[1].colors.items, [("effect", "#000000".to_string())]);
    }
}
