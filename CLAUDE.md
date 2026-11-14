# fsm-cli

`fsm`, the command-line tool of FSM Editor, in Rust, for its state machines (`*.fsm`: XMI 2.5.1). It is FSM Editor's code generator, with Jinja templates rendered by MiniJinja (`fsm write`), compares two versions of a machine (`fsm diff`) and merges two versions made from a common one (`fsm merge`, what a Git merge driver calls), which takes a `.fsm` writer: a copy of the editors'. The Visual Studio and JetBrains editions run the executable; the VS Code edition runs the library built as WebAssembly, on the desktop and on the web. User docs: `README.md`.

## Commands

```sh
cargo build                 # target/debug/fsm; `-t ts` finds templates/ through ../../templates
cargo build --release       # target/release/fsm
cargo rustc --lib --crate-type cdylib --profile wasm --target wasm32-unknown-unknown
                            # target/wasm32-unknown-unknown/wasm/fsm_cli.wasm
cargo test                  # unit tests (grammar, front matter, case filters, globs, paths, the library's
                            # requests, arguments, reading the layout, diff, the writer, merge) and tests/cli.rs (the program
                            # as a process)
cargo clippy --all-targets
cargo fmt                   # rustfmt.toml: 120 columns
cargo run -- write "../fsm-editor-vscode/examples/*.fsm" -t sml -o <dir>
cargo run -- diff old.fsm new.fsm [--json]
cargo run -- merge base.fsm ours.fsm theirs.fsm -o merged.fsm [--name <path>] [--json]
```

The WebAssembly module's crate type is given on the command line, not in `Cargo.toml`: a library built as rlib and cdylib at once gets no LTO (1.38 MB instead of 1.24 MB). The `wasm` profile is the release one at `opt-level = "s"`.

## Architecture

The library (`src/`) holds the pipeline; the program (`src/cli/`) holds the commands and everything that touches the file system.

| Module | |
| --- | --- |
| `cli/main.rs` | `fsm <command>`: dispatch, `-h`, `-v`, usage errors (exit 2). One module per command. |
| `cli/write.rs` | `fsm write`: arguments, glob expansion, template lookup (a bare name: `templates/<name>.jinja` next to the executable), writing the files, `--check`. |
| `cli/diff.rs` | `fsm diff`: arguments, reading the two files (a missing or empty one is a side without a machine), printing text or JSON. |
| `cli/merge.rs` | `fsm merge`: arguments, reading the three files (all of them before writing: the output may be one; a missing or empty base is no machine), writing the result in one go, the report on stderr or JSON on stdout, exit 1 on conflicts and on validation errors the merge caused. |
| `lib.rs` | The pipeline without a file system: a JSON request (the `.fsm` texts, the template's text) → a JSON response (files, issues, exit code), and the WebAssembly interface (`fsm_alloc`, `fsm_generate`, `fsm_free`). Documented in `README.md`, "WebAssembly". |
| `generate.rs` | Loading the machines and their submachines through a reader callback, validating, rendering. |
| `xml.rs`, `xmi.rs` | Reading `.fsm` files, layout included (`fromXmi`'s rules, JavaScript's number parsing, default places, `normalizeModel`'s groups and self-loops), and the model hash; `xml.rs` also writes XML (`writeXml`). |
| `writer.rs` | Writing `.fsm` files: `toXmi` of fsm-editor-vscode's `src/xmi.ts`, byte for byte (the derived events, ids, region shapes from `headerHeight`/`wrapLines`/`regionBounds` with the region sizes as the file holds them, label boxes, JavaScript's `String(number)`), from bounds rounded first, each element once, the diagram in document order; `document_order`, the order reading a written file gives. Used by `merge` only. |
| `model.rs` | `FsmModel`, with the layout (bounds, region layout and sizes, bend points, label offsets, colors, groups) and comment texts and anchors, which only `diff`, `merge` and the writer use; `ModelIndex` designates vertices by index in `model.vertices`. |
| `geometry.rs` | Diagram geometry computed as the editors' JavaScript does: `n1`, `Math.round`, V8's `Math.hypot`, `polyline_mid`, `center`. |
| `diff.rs` | `fsm diff`: pairing elements by xmi:id (`pair`), their model and drawing properties as named values (`*_properties`, `*_layout`), the statuses, labels and sentences, the text and JSON output. `merge` reuses the properties, the labels (`Namer`) and the value display. |
| `merge.rs` | `fsm merge`: elements matched by id (k-th occurrence) across base, ours and theirs; deletions grouped under their topmost element and checked against the other side; properties merged three-way (lists as sets), the layout on exact values with conflicts judged at 0.1 px, positions relative to the owner and bend points to the source, then made absolute again (bend points that one side only moved on the canvas taken as they are, as `diff` counts them changed only when they moved both relative to the source and on the canvas); containment cycles, the cascade of what lost its container or ends; the result written, read back and validated; the report (text, JSON). |
| `expressions.rs` | The text grammar. |
| `validation.rs` | |
| `codemodel.rs` | `to_code_model()`. Serialized with serde in camelCase. |
| `tables.rs` | The nested transition tables (`tables` in the code model). |
| `render.rs`, `languages.rs` | Front matter, the MiniJinja environment and its filters and functions, the refusal of `.hbs` templates. |
| `paths.rs` | Resolving and displaying paths (with a current-folder override for the library), `decodeURI`, globs. |

## Rules

- **The editors:** reading `.fsm` files, the text grammar and validation (rules and messages) follow the editions of FSM Editor, whose reference is fsm-editor-vscode (`src/xmi.ts`, `media/expressions.js`, `src/validation.ts`): a change there needs the same change here, and the reverse.
- **The code model** is documented in fsm-editor-vscode's `docs/CODEGEN.md` ("Code model reference"); keep it in step with `codemodel.rs` and `tables.rs`.
- **Native and WebAssembly alike:** the library must give the program's files and messages, byte for byte (`tests/cli.rs` checks one case). What `lib.rs::run` calls must not touch the file system or the process: it reads through `generate`'s callback and gets the current folder from `paths::set_cwd` (`paths::expand_glob` is the program's only).
- **Reading the layout** must give the model `fromXmi` gives, layout included: compare the two on many files (models dumped as JSON from both) after changing `xmi.rs` or `geometry.rs`.
- **`fsm diff`'s labels, sentences and JSON** are documented in `README.md` ("fsm diff") and other tools show them: keep them stable, and update the README when they change. The same goes for `fsm merge`'s report and JSON ("fsm merge"), which reuse them.
- **The writer** (`writer.rs`, `xml.rs`'s writing) must write what `toXmi` of fsm-editor-vscode's `src/xmi.ts` writes, byte for byte, and so what `Xmi.cs` and `Xmi.kt` write: a file `fsm merge` writes must not change when an editor saves it, and one save settles any file: `to_xmi(from_xmi(s)) == s` for any `s` it wrote (`one_save_settles`). It is a fourth copy of the `.fsm` writer: a change to the XMI output in the editions must reach it too. After changing it, compare `write(read(x))` with `toXmi(fromXmi(x))` (bundled with the vscode edition's esbuild) on many files, and check the editions' round-trip fixtures (`<case>.roundtrip.xmi`).
- **`fsm merge`** must keep `merge(x, x, x) = write(x)`, `merge(base, ours, base) = write(ours)` and `merge(base, base, theirs) = write(theirs)`, and its output must read back in `fromXmi`: check them on many files after changing `merge.rs`. It is the executable's only, not in the WebAssembly interface.
- **Output:** a change that alters generated files, messages or exit codes is a change of behavior: say so. Otherwise compare `fsm write` on many models before and after.
- Templates are Jinja only (`.jinja`): `.hbs` is refused with a pointer to the README's "Coming from the Handlebars templates".
- Templates are data: no filter or function may run code or read files from the template's repository.
- Template-facing names (filters, functions, variables, the `file` block) are documented in `README.md`; update it when they change.
- Out of scope, as in the editions: state machine inheritance/redefinition and the transition-oriented notation.

## Releasing

From the umbrella repo, on a clean, pushed main: `./release.sh cli <version>`. It sets the version in `Cargo.toml` and `Cargo.lock` (`-v` prints `CARGO_PKG_VERSION`), commits "Release <version>", tags `v<version>` and pushes. The tag starts `.github/workflows/release.yml`: `cargo test`, then attached to the GitHub Release:

- a release build per platform (`linux-x64`, `linux-arm64`, `darwin-x64`, `darwin-arm64`, `windows-x64`), each archived as `fsm-<version>-<platform>.tar.gz` (`.zip` on Windows) holding `fsm` (`fsm.exe`) and `templates/` next to it;
- `fsm-<version>.wasm` (the WebAssembly module) and `fsm-templates-<version>.tar.gz` (`templates/`), which the VS Code edition's release downloads.

The editions pin the release they bundle with `FSM_CLI_VERSION` in their `release.yml`. A tag can't be reused: if the workflow fails, fix, push and release the next patch version. Check the run with `gh run list -R vincedupuis/fsm-cli`.
