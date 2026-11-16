//! The whole pipeline: read `.fsm` files (and the machines their submachine
//! states reference), validate them, build their code models and render them
//! with a template. The files are read through a callback: the file system
//! for the program, the request's files for the library (lib.rs).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::codemodel::{CodeModel, Options, to_code_model};
use crate::format::{is_old_format, read_machine};
use crate::model::{FsmModel, SubmachineInfo, VertexType, machine_connection_points};
use crate::paths::{display, resolve};
use crate::render::{Renderer, TemplateError};
use crate::validation::{Severity, validate};

pub struct FileIssue {
    /// The file, as displayed.
    pub file: String,
    /// Line of the problem in the file (1 for the machine itself).
    pub line: usize,
    /// Id of the element, derived from the names (see format.rs; '' for the machine itself or a problem of the
    /// file's text or shape).
    pub id: String,
    pub severity: Severity,
    pub message: String,
}

/// A file to write, with the machine it was generated from.
pub struct GeneratedFile {
    pub path: String,
    pub content: String,
    /// The machine's `.fsm` file, as displayed (for a file shared by several machines, the first one).
    pub source: String,
    /// Its model hash (`ReadMachine::hash` in format.rs).
    pub model_hash: String,
}

pub struct GenerateResult {
    pub files: Vec<GeneratedFile>,
    /// Validation problems and what the template reported.
    pub issues: Vec<FileIssue>,
}

pub enum GenerateError {
    /// Nothing was generated; `issues` say why (they may be empty).
    Failed {
        message: String,
        issues: Vec<FileIssue>,
    },
    Template(TemplateError),
}

impl GenerateError {
    fn new(message: String) -> GenerateError {
        GenerateError::Failed { message, issues: Vec::new() }
    }
}

struct Loaded {
    model: FsmModel,
    /// The model hash of the file.
    hash: String,
    /// Line of each element, by id.
    lines: HashMap<String, usize>,
    /// href as written in this model → path of the loaded machine.
    refs: Vec<(String, PathBuf)>,
    code: Option<Rc<CodeModel>>,
}

struct Loader<'r> {
    /// Reads a `.fsm` file (None: missing or unreadable).
    read: &'r dyn Fn(&Path) -> Option<String>,
    /// The machines read, by path (None: the file's text or shape has problems).
    loaded: HashMap<PathBuf, Option<Loaded>>,
    issues: Vec<FileIssue>,
    building: HashSet<PathBuf>,
    /// An old-format file was refused, which stops everything: its error isn't one submachine's problem.
    refused_old: bool,
}

impl Loader<'_> {
    /// Reads a machine and the machines its submachine states use, and validates it. A file that can't be read, or
    /// that is in the old format, stops everything; problems of a file's text or shape are its issues, and the
    /// machine is left out (Ok(false)).
    fn load(&mut self, path: &Path) -> Result<bool, GenerateError> {
        if let Some(known) = self.loaded.get(path) {
            return Ok(known.is_some());
        }
        let text = (self.read)(path)
            .ok_or_else(|| GenerateError::new(format!("{} does not exist or cannot be read.", display(path))))?;
        if is_old_format(&text) {
            self.refused_old = true;
            return Err(GenerateError::new(format!(
                "{} is in the old XMI format. Open it in FSM Editor and save it to convert it.",
                display(path)
            )));
        }
        let read = match read_machine(&text) {
            Ok(r) => r,
            Err(problems) => {
                for p in problems {
                    self.issues.push(FileIssue {
                        file: display(path),
                        line: p.line,
                        id: String::new(),
                        severity: Severity::Error,
                        message: p.message,
                    });
                }
                self.loaded.insert(path.to_path_buf(), None);
                return Ok(false);
            }
        };
        let hrefs: Vec<(String, String)> = read
            .model
            .vertices
            .iter()
            .filter(|v| v.vtype == VertexType::State)
            .filter_map(|v| v.submachine.clone().map(|s| (v.id.clone(), s)))
            .collect();
        self.loaded.insert(
            path.to_path_buf(),
            Some(Loaded { model: read.model, hash: read.hash, lines: read.lines, refs: Vec::new(), code: None }),
        );

        // Submachines, then validation with what is known about them.
        let mut info: HashMap<String, SubmachineInfo> = HashMap::new();
        for (vertex, href) in hrefs {
            if info.contains_key(&href) {
                continue;
            }
            let dir = path.parent().unwrap_or(Path::new("/"));
            let sub_path = resolve(dir, &href);
            let found = match self.load(&sub_path) {
                Ok(true) => true,
                Ok(false) => {
                    // Reported by the validator as a missing machine; say why.
                    let message = format!("{} is not a valid state machine file.", display(&sub_path));
                    self.issue(path, &vertex, message);
                    false
                }
                Err(GenerateError::Failed { message, issues }) if issues.is_empty() && !self.refused_old => {
                    self.issue(path, &vertex, message);
                    false
                }
                Err(e) => return Err(e),
            };
            if found {
                let sub = &self.loaded[&sub_path].as_ref().unwrap().model;
                let found =
                    SubmachineInfo { found: true, name: sub.name.clone(), points: machine_connection_points(sub) };
                self.loaded.get_mut(path).unwrap().as_mut().unwrap().refs.push((href.clone(), sub_path));
                info.insert(href, found);
            } else {
                info.insert(href, SubmachineInfo::default());
            }
        }
        let l = self.loaded[path].as_ref().unwrap();
        let issues = validate(&l.model, &info);
        for i in issues {
            let line = l.lines.get(&i.id).copied().unwrap_or(1);
            self.issues.push(FileIssue {
                file: display(path),
                line,
                id: i.id,
                severity: i.severity,
                message: i.message,
            });
        }
        Ok(true)
    }

    /// An error about the element `id` of the machine in `path`.
    fn issue(&mut self, path: &Path, id: &str, message: String) {
        let line = self.loaded[path].as_ref().and_then(|l| l.lines.get(id).copied()).unwrap_or(1);
        self.issues.push(FileIssue {
            file: display(path),
            line,
            id: id.to_string(),
            severity: Severity::Error,
            message,
        });
    }

    /// Stops the generation on the errors in the machines.
    fn failed(&mut self) -> GenerateError {
        let errors = self.issues.iter().filter(|i| i.severity == Severity::Error).count();
        let message = format!(
            "{errors} error{} in the state machine{}; nothing was generated.",
            plural(errors),
            plural(self.loaded.len())
        );
        GenerateError::Failed { message, issues: std::mem::take(&mut self.issues) }
    }

    /// The code model of a loaded machine, its submachines' first.
    fn code_of(&mut self, path: &Path) -> Result<Rc<CodeModel>, GenerateError> {
        let l = self.loaded[path].as_ref().unwrap();
        if let Some(code) = &l.code {
            return Ok(code.clone());
        }
        if !self.building.insert(path.to_path_buf()) {
            return Err(GenerateError::new(format!(
                "{} uses itself as a submachine (directly or through other machines).",
                display(path)
            )));
        }
        let refs = l.refs.clone();
        let mut submachines = HashMap::new();
        for (href, sub) in refs {
            submachines.insert(href, self.code_of(&sub)?);
        }
        let l = self.loaded[path].as_ref().unwrap();
        let options = Options { submachines, source_file: display(path), model_hash: l.hash.clone() };
        let code = match to_code_model(&without_generated_names(&l.model), &options) {
            Ok(mut code) => {
                code.point_ids = machine_points(&l.model);
                Rc::new(code)
            }
            Err(e) => {
                // Reported at the element it is about, as a validation error would be.
                self.issue(path, &e.id, e.message);
                return Err(self.failed());
            }
        };
        self.loaded.get_mut(path).unwrap().as_mut().unwrap().code = Some(code.clone());
        self.building.remove(path);
        Ok(code)
    }
}

/// The model as the code model sees it: generated names (ending with `*`, which the diagram doesn't show) are no
/// names, so that the code names such elements as it names unnamed ones (`Final`, `choice`, `entry`).
fn without_generated_names(model: &FsmModel) -> FsmModel {
    let mut m = model.clone();
    for v in &mut m.vertices {
        if v.name.ends_with('*') {
            v.name.clear();
        }
        for r in &mut v.regions {
            if r.name.ends_with('*') {
                r.name.clear();
            }
        }
    }
    for r in &mut m.regions {
        if r.name.ends_with('*') {
            r.name.clear();
        }
    }
    m
}

/// The ids of a machine's own points by their names in the file (the first of a name used twice), which connection
/// point references name: not rebuilt from the names, as an id already taken (a point named `initial`, beside the
/// machine's initial pseudostate) gets `~2`.
fn machine_points(model: &FsmModel) -> HashMap<String, String> {
    let top: HashSet<&str> = model.regions.iter().map(|r| r.id.as_str()).collect();
    let mut out = HashMap::new();
    for v in &model.vertices {
        if matches!(v.vtype, VertexType::EntryPoint | VertexType::ExitPoint) && top.contains(v.parent.as_str()) {
            out.entry(v.name.clone()).or_insert_with(|| v.id.clone());
        }
    }
    out
}

fn plural(n: usize) -> &'static str {
    if n > 1 { "s" } else { "" }
}

/// Generates the files of `sources` (absolute paths) with `renderer`.
pub fn generate(
    sources: &[PathBuf],
    renderer: &Renderer,
    read: &dyn Fn(&Path) -> Option<String>,
) -> Result<GenerateResult, GenerateError> {
    let mut loader =
        Loader { read, loaded: HashMap::new(), issues: Vec::new(), building: HashSet::new(), refused_old: false };
    for s in sources {
        loader.load(s)?;
    }
    if loader.issues.iter().any(|i| i.severity == Severity::Error) {
        return Err(loader.failed());
    }

    let mut files: Vec<GeneratedFile> = Vec::new();
    let mut owner: HashMap<String, PathBuf> = HashMap::new();
    let mut done = HashSet::new();
    for path in sources {
        if !done.insert(path) {
            continue;
        }
        let code = loader.code_of(path)?;
        let (rendered, reported) = renderer.render(&code).map_err(GenerateError::Template)?;
        // What the template reports about this machine, as issues of its file.
        for r in reported {
            let line = loader.loaded[path].as_ref().and_then(|l| l.lines.get(&r.id).copied()).unwrap_or(1);
            loader.issues.push(FileIssue {
                file: display(path),
                line,
                id: r.id,
                severity: r.severity,
                message: r.message,
            });
        }
        for f in rendered {
            if let Some(other) = owner.get(&f.path) {
                // A file shared by several machines (a common header) is written once, when they agree on its content.
                if files.iter().find(|x| x.path == f.path).is_some_and(|x| x.content == f.content) {
                    continue;
                }
                return Err(GenerateError::new(format!(
                    "{} and {} both generate '{}': give the state machines different names.",
                    display(path),
                    display(other),
                    f.path
                )));
            }
            owner.insert(f.path.clone(), path.clone());
            files.push(GeneratedFile {
                path: f.path,
                content: f.content,
                source: display(path),
                model_hash: loader.loaded[path].as_ref().unwrap().hash.clone(),
            });
        }
    }
    let errors = loader.issues.iter().filter(|i| i.severity == Severity::Error).count();
    if errors > 0 {
        let message = format!("The template reported {errors} error{}; nothing was generated.", plural(errors));
        return Err(GenerateError::Failed { message, issues: loader.issues });
    }
    Ok(GenerateResult { files, issues: loader.issues })
}

/// Reads and validates `sources` (absolute paths) and the machines they use, without generating anything: the
/// problems `fsm write` would report, or why it would stop before validating (a missing file, an old one).
pub fn check(sources: &[PathBuf], read: &dyn Fn(&Path) -> Option<String>) -> Result<Vec<FileIssue>, String> {
    let mut loader =
        Loader { read, loaded: HashMap::new(), issues: Vec::new(), building: HashSet::new(), refused_old: false };
    for s in sources {
        match loader.load(s) {
            Ok(_) => {}
            Err(GenerateError::Failed { message, .. }) => return Err(message),
            Err(GenerateError::Template(_)) => unreachable!("loading renders no template"),
        }
    }
    Ok(loader.issues)
}
