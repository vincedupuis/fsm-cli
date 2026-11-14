# fsm-cli

`fsm`, the command-line tool of FSM Editor, in Rust, for its state machines (`*.fsm`: XMI 2.5.1). It is FSM Editor's code generator, with Jinja templates rendered by MiniJinja: `fsm write`. The Visual Studio and JetBrains editions run the executable; the VS Code edition runs the library built as WebAssembly, on the desktop and on the web. User docs: `README.md`.

## Commands

```sh
cargo build                 # target/debug/fsm; `-t ts` finds templates/ through ../../templates
cargo build --release       # target/release/fsm
cargo rustc --lib --crate-type cdylib --profile wasm --target wasm32-unknown-unknown
                            # target/wasm32-unknown-unknown/wasm/fsm_cli.wasm
cargo test                  # unit tests (grammar, front matter, case filters, globs, paths, the library's
                            # requests, arguments) and tests/cli.rs (the program as a process)
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
| `generate.rs` | Loading the machines and their submachines through a reader callback, validating, rendering. |
| `xml.rs`, `xmi.rs` | Reading `.fsm` files (without the layout) and the model hash. |
| `model.rs` | `FsmModel`; `ModelIndex` designates vertices by index in `model.vertices`. |
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
