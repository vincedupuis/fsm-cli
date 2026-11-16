# Test cases of the .fsm format

New-format machines with the problems `fsm` reports for them (`expected.json` in each folder: by file, each problem's line, element id, severity and message, in `fsm write`'s order). Each folder is read as `fsm write` would read it, submachines resolved within it.

- `examples/`: FSM Editor's examples, with their layout files.
- `rules/`: every validation rule that a file can break.
- `submachines/`: submachine states, their points, missing and broken machine files.
- `shape/`: text that isn't JSON, and every kind of shape error.
- `layout/`: layout files the layout schema accepts (`valid/`) and refuses (`invalid/`).

`expected.json` is written by `fsm`: `FSM_UPDATE_CASES=1 cargo test --test cases`. FSM Editor checks its reader and validator against these files (`npm run build:fsm` copies them into its `fsm/cases`).
