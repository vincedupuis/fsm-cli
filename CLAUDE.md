# fsm-cli

`fsm`, a command-line tool in Rust for UML state machines (`*.fsm` files, JSON). It has one command, `fsm write`, a code generator with Jinja templates rendered by MiniJinja, and is also a library without a file system (JSON in and out). It owns the file format: its description (README.md, "The .fsm file format"), the JSON Schemas (`schemas/`), the reader and the validator, and the test cases (`tests/cases`). User docs: `README.md` and `docs/CODEGEN.md` (the bundled templates, the code model, the semantics of the generated code).

## Commands

```sh
cargo build                 # target/debug/fsm; `-t ts` finds templates/ through ../../templates
cargo build --release       # target/release/fsm
cargo build --lib --target wasm32-unknown-unknown   # the library builds without a file system or a process
cargo test                  # unit tests (the JSON reader, grammar, front matter, case filters, globs, paths, the
                            # library's requests, arguments), tests/cases.rs (the test cases and the schemas) and
                            # tests/cli.rs (the program as a process)
FSM_UPDATE_CASES=1 cargo test --test cases   # writes tests/cases/*/expected.json: check the change first
cargo clippy --all-targets
cargo fmt                   # rustfmt.toml: 120 columns
cargo run -- write "tests/cases/examples/*.fsm" -t sml -o <dir>
```

## Architecture

The library (`src/`) holds the pipeline; the program (`src/cli/`) holds the commands and everything that touches the file system.

| Module | |
| --- | --- |
| `cli/main.rs` | `fsm <command>`: dispatch, `-h`, `-v`, usage errors (exit 2). One module per command. |
| `cli/write.rs` | `fsm write`: arguments, glob expansion, template lookup (a bare name: `templates/<name>.jinja` next to the executable), writing the files, `--check`. |
| `lib.rs` | The pipeline without a file system: a JSON request (the `.fsm` texts, the template's text) → a JSON response (files, issues, exit code): `run`, `run_json`. Documented in `README.md`, "Library". |
| `generate.rs` | Loading the machines and their submachines through a reader callback (the refusal of old XMI files), validating, rendering; `check`, the problems without generating (the test cases). Generated names (`*`) are no names for the code model. |
| `json.rs` | A strict JSON reader keeping the line of every value and field, the minimal escapes, the canonical form the model hash is computed from. |
| `format.rs` | Reading `.fsm` files: the shape (every problem with its line), the flat model with ids derived from the names, the lines of the elements, the model hash. |
| `model.rs` | `FsmModel`; `ModelIndex` designates vertices by index in `model.vertices`. A reference that names nothing is a target starting with `UNRESOLVED`. |
| `expressions.rs` | The text grammar. |
| `validation.rs` | The validation rules and their messages. |
| `codemodel.rs` | `to_code_model()`. Serialized with serde in camelCase. |
| `tables.rs` | The nested transition tables (`tables` in the code model). |
| `render.rs`, `languages.rs` | Front matter, the MiniJinja environment and its filters and functions, the refusal of `.hbs` templates. |
| `paths.rs` | Resolving and displaying paths (with a current-folder override for the library), globs. |

## Rules

- **Independent:** fsm-cli knows no editor or other tool that uses it. No mention of them, their files or their behavior in the code, messages, templates, schemas, docs, tests or workflows, and nothing read from outside this repo. Tools using fsm-cli follow it (its format description, schemas, test cases and release assets), never the reverse: changing them for those tools' sake is their concern, and their docs say so, not these.
- **The format:** fsm-cli owns reading `.fsm` files (`json.rs`, `format.rs`: the problems, their messages and lines, the derived ids) and validation (`validation.rs`). Change the README's format description, the schemas and the test cases with the reader: `tests/cases.rs` checks that the schemas accept every file the reader takes and refuse every file it refuses. `expected.json` is written by `fsm` (`FSM_UPDATE_CASES=1`), never by hand.
- **The code model** is documented in `docs/CODEGEN.md` ("Code model reference"); keep it in step with `codemodel.rs` and `tables.rs`, and the sections on the bundled templates in step with `templates/`.
- **The library gives what the program gives:** it must give the program's files and messages, byte for byte (`tests/cli.rs` checks one case). What `lib.rs::run` calls must not touch the file system or the process: it reads through `generate`'s callback and gets the current folder from `paths::set_cwd` (`paths::expand_glob` is the program's only).
- **Ids** are derived from the names as README.md, "Ids", describes: problems, the code model and generated code's traces use them.
- **Old files** (XMI) are refused with their own message. fsm-cli has no XML code.
- **Output:** a change that alters generated files, messages or exit codes is a change of behavior: say so. Otherwise compare `fsm write` on many models before and after.
- Templates are Jinja only (`.jinja`): `.hbs` is refused with a pointer to the README's "Coming from the Handlebars templates".
- Templates are data: no filter or function may run code or read files from the template's repository.
- Template-facing names (filters, functions, variables, the `file` block) are documented in `README.md`; update it when they change.
- Out of scope: state machine inheritance/redefinition and the transition-oriented notation.

## Releasing

From the umbrella repo, on a clean, pushed main: `./release.sh cli <version>`. It sets the version in `Cargo.toml` and `Cargo.lock` (`-v` prints `CARGO_PKG_VERSION`), commits "Release <version>", tags `v<version>` and pushes. The tag starts `.github/workflows/release.yml`: `cargo test`, then attached to the GitHub Release:

- a release build per platform (`linux-x64`, `linux-arm64`, `darwin-x64`, `darwin-arm64`, `windows-x64`), each archived as `fsm-<version>-<platform>.tar.gz` (`.zip` on Windows) holding `fsm` (`fsm.exe`) and `templates/` next to it;
- `fsm-templates-<version>.tar.gz` (`templates/`), `fsm-schemas-<version>.tar.gz` (`schemas/`) and `fsm-cases-<version>.tar.gz` (`cases/`, from `tests/cases`).

A tag can't be reused: if the workflow fails, fix, push and release the next patch version. Check the run with `gh run list -R vincedupuis/fsm-cli`.
