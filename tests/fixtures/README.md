# Parity fixtures

Reference results produced by FSM Editor for VS Code (TypeScript), the same as
the Visual Studio and JetBrains editions' fixtures, used by `src/parity.rs` to
check that fsm-cli reads, writes and validates state machines as the editors do:

- `expected.json`: validation issues of `examples/*.fsm` (validated with their
  submachines resolved, as the editors do) and of the cases below (validated
  without submachines), and the results of the grammar checks (not compared
  here);
- `<case>.fsm`: edge-case models, described in the editions' fixtures README
  (`fsm-editor-vs/test/FsmEditor.Core.Tests/fixtures/README.md`);
- `<case>.roundtrip.xmi`: `toXmi(fromXmi(<case>.fsm))`, which the writer must
  give byte for byte, and which saves to itself when the case's ids are unique;
- `examples/`: the VS Code edition's examples, which `expected.json` describes.

Don't edit them: regenerate them from the umbrella repo with
`node parity/fixtures.mjs` (`--cli` points it at another checkout), which
rewrites them here and in both editions from the VS Code edition's sources, then
run `cargo test`. The case list is read from `expected.json`.
