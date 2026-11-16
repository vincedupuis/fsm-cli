# fsm-cli

`fsm`, the command-line tool of FSM Editor, in Rust, for its state machines (`*.fsm`: JSON, the model only; the layout is in `*.fsm.layout`, which `fsm` never reads). It has one command, `fsm write`: FSM Editor's code generator, with Jinja templates rendered by MiniJinja. It is the reference for the file format: its description (README.md, "The .fsm file format"), the JSON Schemas (`schemas/`), the reader and the validator, and the test cases (`tests/cases`). The Visual Studio and JetBrains editions run the executable; the VS Code edition runs the library built as WebAssembly, on the desktop and on the web. User docs: `README.md`.

## Commands

```sh
cargo build                 # target/debug/fsm; `-t ts` finds templates/ through ../../templates
cargo build --release       # target/release/fsm
cargo rustc --lib --crate-type cdylib --profile wasm --target wasm32-unknown-unknown
                            # target/wasm32-unknown-unknown/wasm/fsm_cli.wasm
cargo test                  # unit tests (the JSON reader, grammar, front matter, case filters, globs, paths, the
                            # library's requests, arguments), tests/cases.rs (the test cases and the schemas) and
                            # tests/cli.rs (the program as a process)
FSM_UPDATE_CASES=1 cargo test --test cases   # writes tests/cases/*/expected.json: check the change first
cargo clippy --all-targets
cargo fmt                   # rustfmt.toml: 120 columns
cargo run -- write "../fsm-editor-vscode/examples/*.fsm" -t sml -o <dir>
```

The WebAssembly module's crate type is given on the command line, not in `Cargo.toml`: a library built as rlib and cdylib at once gets no LTO (1.38 MB instead of 1.24 MB). The `wasm` profile is the release one at `opt-level = "s"`.

## Architecture

The library (`src/`) holds the pipeline; the program (`src/cli/`) holds the commands and everything that touches the file system.

| Module | |
| --- | --- |
| `cli/main.rs` | `fsm <command>`: dispatch, `-h`, `-v`, usage errors (exit 2). One module per command. |
| `cli/write.rs` | `fsm write`: arguments, glob expansion, template lookup (a bare name: `templates/<name>.jinja` next to the executable), writing the files, `--check`. |
| `lib.rs` | The pipeline without a file system: a JSON request (the `.fsm` texts, the template's text) → a JSON response (files, issues, exit code), and the WebAssembly interface (`fsm_alloc`, `fsm_generate`, `fsm_free`). Documented in `README.md`, "WebAssembly". |
| `generate.rs` | Loading the machines and their submachines through a reader callback (the refusal of old XMI files), validating, rendering; `check`, the problems without generating (the test cases). Generated names (`*`) are no names for the code model. |
| `json.rs` | A strict JSON reader keeping the line of every value and field (its errors are the editors'), the minimal escapes, the canonical form the model hash is computed from. |
| `format.rs` | Reading `.fsm` files: the shape (every problem with its line), the flat model with ids derived from the names, the lines of the elements, the model hash. |
| `model.rs` | `FsmModel`; `ModelIndex` designates vertices by index in `model.vertices`. A reference that names nothing is a target starting with `UNRESOLVED`. |
| `expressions.rs` | The text grammar. |
| `validation.rs` | The editors' validation rules and messages (`src/validation.ts`). |
| `codemodel.rs` | `to_code_model()`. Serialized with serde in camelCase. |
| `tables.rs` | The nested transition tables (`tables` in the code model). |
| `render.rs`, `languages.rs` | Front matter, the MiniJinja environment and its filters and functions, the refusal of `.hbs` templates. |
| `paths.rs` | Resolving and displaying paths (with a current-folder override for the library), globs. |

## Rules

- **The format and the editors:** fsm-cli is the reference for reading `.fsm` files (`json.rs`, `format.rs`: the problems, their messages and lines, the derived ids) and for validation (`validation.rs`). The editors follow it: fsm-editor-vscode's `src/json.ts`, `src/format.ts` and `src/validation.ts` give the same problems on `tests/cases`, which `npm run build:fsm` copies into the extension's `fsm/cases`, with the schemas. A change here needs the same change there, and the text grammar (`media/expressions.js`) is shared the same way. Change the README's format description, the schemas and the test cases with the reader: `tests/cases.rs` checks that the schemas accept every file the reader takes and refuse every file it refuses. `expected.json` is written by `fsm` (`FSM_UPDATE_CASES=1`), never by hand.
- **The code model** is documented in fsm-editor-vscode's `docs/CODEGEN.md` ("Code model reference"); keep it in step with `codemodel.rs` and `tables.rs`.
- **Native and WebAssembly alike:** the library must give the program's files and messages, byte for byte (`tests/cli.rs` checks one case). What `lib.rs::run` calls must not touch the file system or the process: it reads through `generate`'s callback and gets the current folder from `paths::set_cwd` (`paths::expand_glob` is the program's only).
- **Ids** are derived from the names the same way here and in the editors (README.md, "Ids"): problems, the code model and generated code's traces use them, and FSM Editor's Trace Player finds the elements by them.
- **Old files** (XMI) are refused with their own message; FSM Editor converts them. fsm-cli has no XML code any more.
- **Output:** a change that alters generated files, messages or exit codes is a change of behavior: say so. Otherwise compare `fsm write` on many models before and after.
- Templates are Jinja only (`.jinja`): `.hbs` is refused with a pointer to the README's "Coming from the Handlebars templates".
- Templates are data: no filter or function may run code or read files from the template's repository.
- Template-facing names (filters, functions, variables, the `file` block) are documented in `README.md`; update it when they change.
- Out of scope, as in the editions: state machine inheritance/redefinition and the transition-oriented notation.

## Releasing

From the umbrella repo, on a clean, pushed main: `./release.sh cli <version>`. It sets the version in `Cargo.toml` and `Cargo.lock` (`-v` prints `CARGO_PKG_VERSION`), commits "Release <version>", tags `v<version>` and pushes. The tag starts `.github/workflows/release.yml`: `cargo test`, then attached to the GitHub Release:

- a release build per platform (`linux-x64`, `linux-arm64`, `darwin-x64`, `darwin-arm64`, `windows-x64`), each archived as `fsm-<version>-<platform>.tar.gz` (`.zip` on Windows) holding `fsm` (`fsm.exe`) and `templates/` next to it;
- `fsm-<version>.wasm` (the WebAssembly module), `fsm-templates-<version>.tar.gz` (`templates/`), `fsm-schemas-<version>.tar.gz` (`schemas/`) and `fsm-cases-<version>.tar.gz` (`cases/`, from `tests/cases`), which the VS Code edition's workflows download (the VSIX bundles all but the cases, which its tests use).

The editions pin the release they bundle with `FSM_CLI_VERSION` in their `release.yml`. A tag can't be reused: if the workflow fails, fix, push and release the next patch version. Check the run with `gh run list -R vincedupuis/fsm-cli`.
