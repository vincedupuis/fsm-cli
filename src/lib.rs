//! The pipeline of `fsm write` (read `.fsm` files, validate them, build their code models, render them with a Jinja
//! template), used by the program (src/cli), and the same pipeline without a file system, for WebAssembly: the
//! `.fsm` files and the template come in a JSON request, the generated files and the messages go back in a JSON
//! response. It does what `fsm write` does but the glob expansion, the template lookup, writing the files and
//! `--check`, which the caller does with the files it gets.
//!
//! WebAssembly interface (no wasm-bindgen, no imports): `fsm_alloc(len)` returns a buffer for the request,
//! `fsm_generate(ptr, len)` frees it and returns a buffer holding the response's length (little-endian u32) followed
//! by the response, `fsm_free(ptr, len + 4)` frees that buffer. See README.md, "WebAssembly".

mod codemodel;
pub mod diff;
mod expressions;
pub mod generate;
pub mod geometry;
mod languages;
pub mod model;
pub mod paths;
pub mod render;
mod sha256;
mod tables;
mod validation;
pub mod writer;
pub mod xmi;
mod xml;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use generate::{FileIssue, GenerateError, generate, line_of};
use paths::absolute;
use render::{Renderer, TemplateError, parse_template, refuse_handlebars};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    /// The folder relative paths are resolved against and paths are displayed relative to (`/` by default).
    #[serde(default)]
    pub cwd: Option<String>,
    /// Every `.fsm` file the generation may read, path → text: the files to generate and the machines their
    /// submachine states use. A file that is not here doesn't exist.
    pub files: HashMap<String, String>,
    /// The files to generate, in order.
    pub generate: Vec<String>,
    /// The template's text.
    pub template: String,
    /// Its name in messages (`ts.jinja`).
    pub template_name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutFile {
    /// Relative to the output folder, `/`-separated.
    pub path: String,
    pub content: String,
    /// The `.fsm` file it was generated from, as displayed.
    pub source: String,
    /// The model hash of that file, for `--check`.
    pub model_hash: String,
}

#[derive(Serialize)]
pub struct OutIssue {
    /// As displayed: relative to `cwd` when inside it.
    pub file: String,
    /// Line of the element in the file (1 when unknown).
    pub line: usize,
    /// xmi:id of the element (empty for the machine itself).
    pub id: String,
    /// `error` or `warning`.
    pub severity: &'static str,
    pub message: String,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    /// `fsm write`'s exit code: 0 generated, 1 errors in the machines or the template, 2 bad request.
    pub exit_code: u8,
    /// The files to write (none unless `exit_code` is 0).
    pub files: Vec<OutFile>,
    /// Validation problems and what the template reported, in `fsm write`'s order.
    pub issues: Vec<OutIssue>,
    /// Why nothing was generated: `fsm write`'s last line without its `fsm: ` prefix.
    pub error: Option<String>,
}

/// Sets the current folder for one request, and restores the process's when dropped.
struct CwdGuard;

impl CwdGuard {
    fn set(dir: &str) -> CwdGuard {
        paths::set_cwd(Some(PathBuf::from(dir)));
        CwdGuard
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        paths::set_cwd(None);
    }
}

/// Generates the request's files: what `fsm write` does after finding the files and the template.
pub fn run(req: Request) -> Response {
    let _cwd = CwdGuard::set(req.cwd.as_deref().unwrap_or("/"));
    let texts: HashMap<PathBuf, String> = req.files.into_iter().map(|(p, t)| (absolute(&p), t)).collect();
    let read = |p: &Path| texts.get(p).cloned();
    let issues = |list: Vec<FileIssue>| -> Vec<OutIssue> {
        list.into_iter()
            .map(|i| OutIssue {
                line: texts.get(&absolute(&i.file)).map_or(1, |text| line_of(text, &i.id)),
                file: i.file,
                id: i.id,
                severity: i.severity.as_str(),
                message: i.message,
            })
            .collect()
    };
    let fail = |code: u8, message: String| Response { exit_code: code, error: Some(message), ..Response::default() };
    let template_failure = |e: TemplateError| match e.line {
        Some(line) => fail(1, format!("{}:{line}: {}", req.template_name, e.message)),
        None => fail(1, format!("{}: {}", req.template_name, e.message)),
    };

    if let Some(message) = refuse_handlebars(&req.template_name) {
        return fail(2, message);
    }
    if req.generate.is_empty() {
        return fail(2, "No .fsm files given.".to_string());
    }
    let mut sources: Vec<PathBuf> = Vec::new();
    for f in &req.generate {
        let p = absolute(f);
        if !sources.contains(&p) {
            sources.push(p);
        }
    }
    let template = match parse_template(&req.template) {
        Ok(t) => t,
        Err(e) => return template_failure(e),
    };
    let renderer = match Renderer::new(&template, &req.template_name) {
        Ok(r) => r,
        Err(e) => return template_failure(e),
    };
    match generate(&sources, &renderer, &read) {
        Ok(result) => Response {
            exit_code: 0,
            files: result
                .files
                .into_iter()
                .map(|f| OutFile { path: f.path, content: f.content, source: f.source, model_hash: f.model_hash })
                .collect(),
            issues: issues(result.issues),
            error: None,
        },
        Err(GenerateError::Failed { message, issues: list }) => {
            Response { exit_code: 1, issues: issues(list), error: Some(message), files: Vec::new() }
        }
        Err(GenerateError::Template(e)) => template_failure(e),
    }
}

/// JSON request → JSON response.
pub fn run_json(request: &[u8]) -> Vec<u8> {
    let response = match serde_json::from_slice::<Request>(request) {
        Ok(req) => run(req),
        Err(e) => Response { exit_code: 2, error: Some(format!("Bad request: {e}")), ..Response::default() },
    };
    serde_json::to_vec(&response).unwrap()
}

/// A buffer of `len` bytes for the request.
#[unsafe(no_mangle)]
pub extern "C" fn fsm_alloc(len: usize) -> *mut u8 {
    Box::into_raw(vec![0u8; len].into_boxed_slice()) as *mut u8
}

/// Frees a buffer.
///
/// # Safety
/// `ptr` and `len` come from `fsm_alloc`, or from `fsm_generate` (`len` then includes the 4 bytes of the length);
/// each buffer is freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fsm_free(ptr: *mut u8, len: usize) {
    drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)) });
}

/// Runs the JSON request in `ptr` and frees it; returns the response's length (little-endian u32) followed by the
/// JSON response.
///
/// # Safety
/// `ptr` and `len` come from `fsm_alloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fsm_generate(ptr: *mut u8, len: usize) -> *mut u8 {
    let request = unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)) };
    let json = run_json(&request);
    drop(request);
    let mut out = Vec::with_capacity(4 + json.len());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&json);
    Box::into_raw(out.into_boxed_slice()) as *mut u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const MACHINE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xmi:XMI xmi:version="20131001" xmlns:xmi="http://www.omg.org/spec/XMI/20131001" xmlns:uml="http://www.omg.org/spec/UML/20161101">
  <uml:Model xmi:id="m" name="Main">
    <packagedElement xmi:type="uml:StateMachine" xmi:id="sm" name="Main">
      <region xmi:type="uml:Region" xmi:id="r">
        <subvertex xmi:type="uml:Pseudostate" xmi:id="i" kind="initial"/>
        <subvertex xmi:type="uml:State" xmi:id="A" name="A" submachine="sub/Sub.fsm#sub"/>
        <transition xmi:type="uml:Transition" xmi:id="t0" source="i" target="A"/>
      </region>
    </packagedElement>
  </uml:Model>
</xmi:XMI>
"#;

    const SUB: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xmi:XMI xmi:version="20131001" xmlns:xmi="http://www.omg.org/spec/XMI/20131001" xmlns:uml="http://www.omg.org/spec/UML/20161101">
  <uml:Model xmi:id="m" name="Sub">
    <packagedElement xmi:type="uml:StateMachine" xmi:id="sub" name="Sub">
      <region xmi:type="uml:Region" xmi:id="r">
        <subvertex xmi:type="uml:Pseudostate" xmi:id="i" kind="initial"/>
        <subvertex xmi:type="uml:State" xmi:id="S" name="S"/>
        <subvertex xmi:type="uml:State" xmi:id="X" name=""/>
        <transition xmi:type="uml:Transition" xmi:id="t0" source="i" target="S"/>
      </region>
    </packagedElement>
  </uml:Model>
</xmi:XMI>
"#;

    const TEMPLATE: &str =
        "{% filter file(machine.name ~ \".txt\") %}{{ machine.name }}: {{ submachines | length }}\n{% endfilter %}";

    fn call(request: Value) -> Value {
        serde_json::from_slice(&run_json(&serde_json::to_vec(&request).unwrap())).unwrap()
    }

    fn request(files: Value, template_name: &str) -> Value {
        json!({ "cwd": "/w", "files": files, "generate": ["Main.fsm"], "template": TEMPLATE, "templateName": template_name })
    }

    #[test]
    fn reads_files_with_crlf_line_ends() {
        let lf = call(request(json!({ "/w/Main.fsm": MACHINE, "sub/Sub.fsm": SUB }), "t.jinja"));
        let crlf = call(request(
            json!({ "/w/Main.fsm": MACHINE.replace('\n', "\r\n"), "sub/Sub.fsm": SUB.replace('\n', "\r\n") }),
            "t.jinja",
        ));
        assert_eq!(crlf["exitCode"], 0, "{crlf}");
        assert_eq!(crlf["files"], lf["files"]);
    }

    #[test]
    fn generates_from_the_request_files() {
        let r = call(request(json!({ "/w/Main.fsm": MACHINE, "sub/Sub.fsm": SUB }), "t.jinja"));
        assert_eq!(r["exitCode"], 0);
        assert_eq!(r["error"], Value::Null);
        assert_eq!(r["files"][0]["path"], "Main.txt");
        assert_eq!(r["files"][0]["content"], "Main: 1\n");
        assert_eq!(r["files"][0]["source"], "Main.fsm");
        assert_eq!(r["files"][0]["modelHash"].as_str().unwrap().len(), 16);
        // The submachine's unnamed state, at its line in its file, displayed relative to cwd.
        assert_eq!(r["issues"][0]["file"], "sub/Sub.fsm");
        assert_eq!(r["issues"][0]["line"], 8);
        assert_eq!(r["issues"][0]["severity"], "warning");
    }

    #[test]
    fn missing_files_and_bad_requests() {
        let r = call(request(json!({ "/w/Main.fsm": MACHINE }), "t.jinja"));
        assert_eq!(r["exitCode"], 1);
        assert_eq!(r["issues"][0]["message"], "sub/Sub.fsm does not exist or cannot be read.");
        assert_eq!(r["issues"][0]["line"], 7);
        assert_eq!(r["error"], "2 errors in the state machine; nothing was generated.");

        let r = call(request(json!({}), "ts.hbs"));
        assert_eq!(r["exitCode"], 2);
        assert!(r["error"].as_str().unwrap().starts_with("ts.hbs is a Handlebars template"));

        let r = call(json!({ "files": {} }));
        assert_eq!(r["exitCode"], 2);
        assert!(r["error"].as_str().unwrap().starts_with("Bad request: missing field"));
        // The process's current folder is back.
        assert_eq!(paths::cwd(), std::env::current_dir().unwrap());
    }

    #[test]
    fn c_interface() {
        let req = serde_json::to_vec(&request(json!({ "/w/Main.fsm": MACHINE, "/w/sub/Sub.fsm": SUB }), "t")).unwrap();
        let response = unsafe {
            let p = fsm_alloc(req.len());
            std::ptr::copy_nonoverlapping(req.as_ptr(), p, req.len());
            let r = fsm_generate(p, req.len());
            let len = u32::from_le_bytes(std::slice::from_raw_parts(r, 4).try_into().unwrap()) as usize;
            let json: Value = serde_json::from_slice(std::slice::from_raw_parts(r.add(4), len)).unwrap();
            fsm_free(r, len + 4);
            json
        };
        assert_eq!(response["files"][0]["content"], "Main: 1\n");
    }
}
