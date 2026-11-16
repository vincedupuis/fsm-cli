# fsm-cli

`fsm`, the command-line tool of [FSM Editor](https://github.com/vincedupuis/fsm-editor-vscode), for its UML 2.5.1 state machines (`.fsm` files, JSON: see [The .fsm file format](#the-fsm-file-format)). `fsm write` generates code from them with [Jinja](https://jinja.palletsprojects.com/) templates, rendered by [MiniJinja](https://github.com/mitsuhiko/minijinja). One template can write any number of files, in any language.

It is FSM Editor's code generator: the editions for Visual Studio and JetBrains IDEs run the `fsm` executable, and the one for VS Code runs the same code built as WebAssembly, on the desktop and on the web. It validates the machines with the editors' rules and messages, and builds the code model, the language-neutral description of a machine that templates print. It is also the reference for the file format: its description below, its JSON Schemas (`schemas/`) and its test cases (`tests/cases`, the files with the problems `fsm` reports for them), which the editors check themselves against.

- [Building](#building)
- [Usage](#usage)
- [The .fsm file format](#the-fsm-file-format)
- [Bundled templates](#bundled-templates)
- [Writing a template](#writing-a-template)
- [WebAssembly](#webassembly)
- [Coming from the Handlebars templates](#coming-from-the-handlebars-templates)

## Building

```sh
cargo build --release      # target/release/fsm
cargo test
```

The program is a single executable without dependencies. Templates are files, not built into it: ship it with the `templates/` folder next to it, where `-t <name>` finds them.

Prebuilt executables for Linux and macOS (x64, arm64) and Windows (x64), each with its `templates/` folder, are on the [Releases](https://github.com/vincedupuis/fsm-cli/releases) page (`fsm-<version>-<platform>.tar.gz`, `.zip` on Windows), with the WebAssembly module (`fsm-<version>.wasm`), the templates alone (`fsm-templates-<version>.tar.gz`), the JSON Schemas (`fsm-schemas-<version>.tar.gz`) and the test cases of the format (`fsm-cases-<version>.tar.gz`).

## Usage

```sh
fsm <command> [options]
fsm --help                 # the commands
fsm <command> --help       # the options of a command
fsm --version
```

A missing or unknown command is bad usage (exit code `2`).

### fsm write

```sh
fsm write <files...> --template <template> [--out <folder>] [--check]
```

| Option | |
| --- | --- |
| `-t, --template <file>` | Template file (`.jinja`). A bare name such as `ts` means `templates/ts.jinja` next to the program (or, for `cargo run`, in this folder). |
| `-o, --out <folder>` | Output folder (default: current folder). |
| `--check` | Write nothing: report the files of the output folder that are missing or out of date (see below). |
| `-h, --help` | |

```sh
fsm write "models/**/*.fsm" -t sml -o src/generated
```

- The machines are read and validated first. Nothing is generated when a file isn't valid JSON, has the wrong shape or has validation errors: every problem is reported with its file and line. Warnings are reported too.
- Files in the old format (XMI, FSM Editor 0.6 and earlier) are refused: `fsm: Order.fsm is in the old XMI format. Open it in FSM Editor and save it to convert it.` The same when a machine uses a submachine file still in the old format; the message names that file.
- The layout files (`.fsm.layout`) are never read: the drawing changes nothing in the generated code.
- Machines used by submachine states are read and validated too, but only the given files are generated. Generate the submachines' files as well, e.g. with `"models/*.fsm"`.
- Quote glob patterns (`*`, `?`, `[...]`, `**`): the program expands them itself, the same in bash, zsh, PowerShell and cmd.
- Problems, including the errors found while building the code model (such as pseudostates forming a cycle) and the warnings and errors a template reports, are printed to stderr as `file:line: severity: message`, which editors' problem matchers and CI logs understand.
- Exit codes: `0` success, `1` errors in the machines or the template (with `--check`, also files missing or out of date), `2` bad usage.
- Generated files are always overwritten.
- Handlebars templates (`.hbs`) are refused: see [Coming from the Handlebars templates](#coming-from-the-handlebars-templates).
- `--check` writes nothing: it renders the template in memory and checks the output folder instead, for instance as a CI step that fails when someone changed a model without regenerating its code. The model hash is computed from the model's content (the JSON value, without `$schema`), not from the file's text: reformatting a file changes nothing. Every file the template generates for a machine and writes the machine's `machine.modelHash` in is reported as up to date (stdout) when the file in the folder holds that hash, and as an error (stderr, `file:line: error: …`) when it is missing or holds another hash. Files without the hash, such as the `FsmTimers.h` that the `sml` template shares between machines, aren't checked, and only the hash is compared: a file generated by another version of the template counts as up to date. A machine's files carry its own model hash only, while a change to a submachine can also change the files of the machines that use it (their events include the submachine's): check the submachines' files too, e.g. with `"models/*.fsm"`, and regenerate them all.

## The .fsm file format

A state machine is two files side by side: `<Name>.fsm`, the model, and `<Name>.fsm.layout`, its drawing. Both are JSON (UTF-8 without a byte order mark, LF line ends, CRLF accepted when reading, 2-space indentation, a newline at the end). The model file is meant to be read and edited by hand; it holds no ids, as elements are identified by their names, and nothing about the drawing. Nobody is expected to read the layout file, which is built for small diffs and rare merge conflicts. `fsm` reads the model files only.

The JSON Schemas of the two files are [`schemas/fsm.schema.json`](schemas/fsm.schema.json) and [`schemas/fsm-layout.schema.json`](schemas/fsm-layout.schema.json) (draft-07): an editor that knows them checks and completes the files as you type. A file may name its schema in a top-level `$schema` field, which readers ignore and FSM Editor doesn't write back. The schemas can't see one problem: a field given twice in one object, which `fsm` reports too.

### The model file

Fields are listed in the order FSM Editor writes them. `format`, `name` and `type` are always written; the other fields only when they have content (an empty text or list is never written, with one exception below).

**The machine** (the top-level object):

| Field | |
| --- | --- |
| `format` | The number `1`. Required, first. |
| `name` | The machine's name. Required. |
| `type` | `"behavioral"` or `"protocol"`. Required. |
| `documentation` | Text. |
| `points` | The machine's own entry and exit points, which the states using it as a submachine connect to (list of points). |
| the region fields, or `regions` | Its content (below). |

**Region fields**, used by the machine, by a state with one region, and by each item of `regions`:

| Field | |
| --- | --- |
| `initial` | The initial pseudostate and its transition: `{ "target": <reference>, "action": <text> }`, `action` optional. |
| `history` | A shallow history pseudostate, with its default transition: `{ "target": <reference>, "action": <text> }`, both optional. |
| `deepHistory` | A deep history pseudostate, the same way. |
| `elements` | The region's elements (list). A composite state whose only region is empty writes `"elements": []`, so that it stays composite. |

**`regions`**: a list of two regions or more (orthogonal regions), each `{ "name": <text>, …region fields }`, `name` required. A state or machine with one region writes its region fields directly, and that region has no name. `regions` and the region fields can't be used together.

**Elements** (items of `elements`): `name` and `type` (required), then by type:

| `type` | Fields |
| --- | --- |
| `"state"` | `stereotype`, `submachine`, `entry`, `do`, `exit`, `defer`, `invariant`, `transitions`, `points`, then the region fields or `regions`. |
| `"choice"`, `"junction"`, `"fork"`, `"join"` | `transitions` |
| `"final"`, `"terminate"` | none |
| `"comment"` | `text`, `anchors` |

**Points** (items of `points`): `name` and `type` (`"entryPoint"` or `"exitPoint"`), both required, and `transitions`. On a state: its entry and exit points. On a submachine state: the points of the submachine it uses (connection point references), by the names they have in the other file. On the machine: its own connection points.

**Transitions** (items of `transitions`, inside their source, in priority order): `type` (`"external"`, `"internal"` or `"local"`, required), `trigger`, `guard`, `action`, `precondition`, `postcondition`, `target`. `target` is required, except for an internal transition, which has none.

**Texts:**

- `trigger`: the triggers as the diagram shows them, separated by `, ` (`"play, resume"`, `"after(5m)"`): event names, or `after(<number><ms|s|m|h>)`. One object per arrow. `defer`, the deferred events, is written the same way.
- `entry`, `exit`, `do`, `action`: argument-less calls separated by `; ` (`"start(); log()"`).
- `guard`, `invariant`, `precondition`, `postcondition`: calls combined with `!`, `&&`, `||` and parentheses; `else` on the branches of a choice or junction.
- `submachine`: the path of the other `.fsm` file, relative to this one (one machine per file).
- `stereotype`: a name. `documentation` and a comment's `text`: free text.

A comment's **`anchors`** are what it is attached to: a list of references (texts), or, for a transition, `{ "source": <reference>, "trigger": <text>, "target": <reference> }` (`trigger` and `target` left out when the transition has none). A comment without `anchors` is valid, and so is an anchor that matches nothing: the comment is drawn, without that anchor's dashed line.

### Names and references

- Every element, region of `regions` and point has a name, never empty.
- Names are unique in the file among the machine, every region and every element of every type. Point names are unique on their owner, and a point can't be named `history` or `deepHistory`.
- A name never contains spaces (ids and trace lines are space-separated words) or `.`, and contains `*` only as its last character. **A name ending with `*` is a generated name**, which the diagram doesn't show: FSM Editor gives such names to the elements it creates, except states (`choice1*`, `final1*`, `comment1*`, `region1*`, `entry1*`, …), and generated code names them as it names unnamed elements.
- A **reference** (`target`, a comment's anchors) is the name of an element; `<owner>.<point>`, a point of a state or of the machine (the owner being the machine's name); or `<owner>.history` / `<owner>.deepHistory`, the owner being the state, the region when the state has several, or the machine's name at the top level. There are no paths: `A.B.C` is never a reference.
- Transitions, `initial`, `history` and `deepHistory` have no name. Where a transition must be found again (in the layout file, in a comment's anchor), it is found under its source by its `trigger` and `target`, then by order among identical ones.

### Formatting

- An object is written on one line (`{ "type": "external", "target": "Off" }`) when it holds no list and no object, else one field per line; the top-level object always one field per line. A list is written one item per line.
- Texts use JSON's minimal escapes (`\"`, `\\`, `\n`, `\t`, `\u00XX` for other control characters); other characters are written as they are.
- Writing is deterministic: reading a file FSM Editor wrote and writing it again gives the same bytes.

<details>
<summary>An example: <code>MediaPlayer.fsm</code> (FSM Editor's example)</summary>

```json
{
  "format": 1,
  "name": "MediaPlayer",
  "type": "behavioral",
  "documentation": "Example UML state machine: composite and orthogonal states, deep history, choice, exit point and an internal transition.",
  "initial": { "target": "Off" },
  "elements": [
    {
      "name": "comment1*",
      "type": "comment",
      "text": "Deep history restores the\nlast active playback substates\nafter a fault is reset.",
      "anchors": [
        "On.deepHistory"
      ]
    },
    {
      "name": "Off",
      "type": "state",
      "entry": "powerDown()",
      "transitions": [
        { "type": "external", "trigger": "powerOn", "target": "On" },
        { "type": "external", "trigger": "unplug", "target": "final1*" }
      ]
    },
    { "name": "final1*", "type": "final" },
    {
      "name": "On",
      "type": "state",
      "entry": "ampOn()",
      "exit": "ampOff()",
      "transitions": [
        { "type": "external", "trigger": "powerOff", "target": "Off" }
      ],
      "points": [
        {
          "name": "fault",
          "type": "exitPoint",
          "transitions": [
            { "type": "external", "target": "Fault" }
          ]
        }
      ],
      "initial": { "target": "Stopped" },
      "deepHistory": { "target": "Stopped" },
      "elements": [
        {
          "name": "Stopped",
          "type": "state",
          "entry": "rewind()",
          "transitions": [
            { "type": "external", "trigger": "play", "target": "Playing" },
            { "type": "external", "trigger": "eject", "target": "choice1*" }
          ]
        },
        {
          "name": "Paused",
          "type": "state",
          "do": "blinkLed()",
          "transitions": [
            { "type": "external", "trigger": "play", "target": "Playing" },
            { "type": "external", "trigger": "stop", "target": "Stopped" },
            { "type": "internal", "trigger": "volumeChanged", "action": "updateVolume()" },
            { "type": "external", "trigger": "after(5m)", "action": "saveBookmark()", "target": "Stopped" }
          ]
        },
        {
          "name": "choice1*",
          "type": "choice",
          "transitions": [
            { "type": "external", "guard": "else", "action": "openTray()", "target": "Stopped" },
            { "type": "external", "guard": "isTrayJammed()", "target": "On.fault" }
          ]
        },
        {
          "name": "Playing",
          "type": "state",
          "transitions": [
            { "type": "external", "trigger": "pause", "target": "Paused" }
          ],
          "regions": [
            {
              "name": "audio",
              "initial": { "target": "Stereo" },
              "elements": [
                {
                  "name": "Stereo",
                  "type": "state",
                  "transitions": [
                    { "type": "external", "trigger": "toggleAudio", "target": "Mono" }
                  ]
                },
                {
                  "name": "Mono",
                  "type": "state",
                  "transitions": [
                    { "type": "external", "trigger": "toggleAudio", "target": "Stereo" }
                  ]
                }
              ]
            },
            {
              "name": "video",
              "initial": { "target": "HD" },
              "elements": [
                {
                  "name": "HD",
                  "type": "state",
                  "transitions": [
                    { "type": "external", "trigger": "lowBandwidth", "target": "SD" }
                  ]
                },
                {
                  "name": "SD",
                  "type": "state",
                  "transitions": [
                    { "type": "external", "trigger": "highBandwidth", "target": "HD" }
                  ]
                }
              ]
            }
          ]
        }
      ]
    },
    {
      "name": "Fault",
      "type": "state",
      "entry": "showError()",
      "transitions": [
        { "type": "external", "trigger": "reset", "target": "On.deepHistory" }
      ]
    }
  ]
}
```

A submachine state, from `Order.fsm`, whose `Payment.fsm` declares the points `retry`, `failed` and `cancelled` in its top-level `points` (inside `Payment.fsm` they are referenced as `Payment.failed`):

```json
{
  "name": "Payment",
  "type": "state",
  "submachine": "Payment.fsm",
  "transitions": [
    { "type": "external", "target": "Shipped" }
  ],
  "points": [
    { "name": "retry", "type": "entryPoint" },
    {
      "name": "failed",
      "type": "exitPoint",
      "transitions": [
        { "type": "external", "target": "PaymentFailed" }
      ]
    },
    {
      "name": "cancelled",
      "type": "exitPoint",
      "transitions": [
        { "type": "external", "target": "Cart" }
      ]
    }
  ]
}
```

</details>

A protocol machine has `"type": "protocol"`, and its transitions carry `precondition` and `postcondition`.

### The layout file

`<Name>.fsm.layout` holds the drawing: positions and sizes, the arrangement and sizes of regions, bend points and label positions of transitions, colors, groups. It mirrors the model file, with the same nesting, names and order:

| Object | Fields |
| --- | --- |
| top level | `format` (`1`, required, first), `points`, then the region fields or `regions` |
| region fields | `initial`, `history`, `deepHistory` (each `x`, `y`, `group`, and for its arrow `bends`, `label`, `color`, `itemColors`), `elements` |
| item of `regions` | `name`, `size` (its extent along its state's region layout), then the region fields |
| element | `name`, `x`, `y`, `width`, `height` (states, comments, forks and joins: the others have a fixed size), `color`, `itemColors`, `group`, `regionLayout` (`"horizontal"` or `"vertical"`), `transitions`, `points`, then the region fields or `regions` |
| point | `name`, `x`, `y`, `color`, `itemColors`, `group` (the machine's points), `transitions` |
| transition, under its source | `trigger` and `target` (as in the model file: they identify it), `bends` (list of `{ "x", "y" }`), `label` (`{ "x", "y" }`, from the middle of the line), `color`, `itemColors`. A transition has an entry only when it has something to store. |

`itemColors` gives items their own colors (`#rrggbb`), keyed by the model file's field names: `background` (a composite state's regions), `entry`, `exit`, `do`, `defer`, `invariant`, `trigger`, `guard`, `action`, `precondition`, `postcondition`.

What keeps diffs small:

- Every object is written one field per line, except an object holding only `x` and `y` (a bend point, a `label`, a bare position), which is one line: two people changing different elements don't touch adjacent lines.
- `x` and `y` are relative to the top-left corner of the owning state (the diagram's origin at the top level); a point's position is relative to its owner. Bend points are relative to the top-left corner of the innermost state containing both ends of the transition (the state itself for a transition between a state and what it contains, or a self-transition; the origin at the top level). Moving a state changes its own `x` and `y` only; moving one end of a transition changes nothing in the transition's entry.
- Numbers are rounded to 0.1. Saving rewrites nothing that didn't change: reading the two files and writing them again gives the same bytes.

### When files are broken or disagree

FSM Editor never drops or guesses anything in the model: it either refuses to draw the diagram, or draws it and keeps everything.

1. **Not valid JSON**: no diagram, `fsm` generates nothing; the error is reported with its line.
2. **Valid JSON, wrong shape**: a missing `format`, `name` or `type`, an unknown value or field (`"gaurd"`), a value of the wrong kind, a `format` greater than 1 ("needs a newer version of FSM Editor"). No diagram, nothing generated; each problem is reported with its line. Reading is strict: nothing is defaulted.
3. **The right shape, a wrong meaning**: a reference that names nothing, a name used twice, a point the submachine doesn't have, a bad name, an `initial` without `target`. These are validation errors: FSM Editor draws the diagram (a transition whose target doesn't exist ends in a marker) and saves everything back as it was; `fsm` generates nothing.
4. **An unreadable layout file**: FSM Editor places the elements automatically, with a warning, and doesn't overwrite the file until you confirm when saving.
5. **The two files disagree** (a hand edit, a merge, a lost layout file): an element without a layout entry is placed automatically (default size, the first free spot beside the elements already in its region, its state growing to fit; the same on every open), and a layout entry that matches nothing is ignored and disappears at the next save. Opening never changes a file, and none of this is a validation error. Renaming an element by hand in the model file loses its position and the bend points of the transitions to it: names are its identity.

### Ids

The model has no ids, but `fsm` and FSM Editor derive them from the names, the same way: they name elements in problems (`id` in the [WebAssembly](#webassembly) response) and in the code model, and so in generated code's traces.

| Element | Id |
| --- | --- |
| an element, a region of `regions` | its name |
| a point | `<state>.<point>`, or `<machine>.<point>` for the machine's own |
| an initial, history or deep history pseudostate | `<owner>.initial`, `<owner>.history`, `<owner>.deepHistory` (the owner as in references) |
| the only region of a state, of the machine | `<state>.`, `<machine>.` |
| a transition | `<source>#<n>`, n counting the source's transitions from 1: `Paused#3`, `On.fault#1`, `MediaPlayer.initial#1` |

An id already taken (a name used twice, which is an error) gets `~2`, `~3`, … appended; an element without a name gets `~`.

### What changed from the old format

Until FSM Editor 0.6, a `.fsm` file was XMI 2.5.1 with a UML DI diagram, the model and the drawing in one XML file. FSM Editor still reads such files, and converts one when it is saved: it writes the JSON model file and creates the layout file, converting the machines linked to it by submachine references at the same time. `fsm` refuses them. What changed: names replace ids (unnamed elements get generated names, names used twice get a number, spaces in names become `_`); `effect` is now `action`; a machine has no `context` any more; a submachine is a file path (no `#id`), and its points are referenced by name; the layout lives in its own file, with relative coordinates; the model hash is computed from the model's content, not the file's text, so reformatting a file doesn't change it.

## Bundled templates

| Template | Writes, for a machine named `Order` |
| --- | --- |
| `ts` | TypeScript without dependencies: `OrderActions.ts` (events, states, the `OrderActions` interface to implement) and `Order.ts` (the `Order` class). |
| `sml` | C++17 for [Boost.SML](https://github.com/boost-ext/sml): `OrderEvents.h`, `OrderActions.h`, `Order.h`, `Order.cpp`, and `FsmTimers.h` shared by the machines with time events. |

Both implement UML run-to-completion semantics: an event queue, completion events, deferred events, timers through an injectable clock, do activities as start/stop hooks plus `activityDone()`, constraint checks reported to `onConstraintViolation`, one class per submachine, `onTerminate` for terminate pseudostates, and an `onTrace` hook reporting events, states entered and left, and transitions taken. FSM Editor's [CODEGEN.md](https://github.com/vincedupuis/fsm-editor-vscode/blob/main/docs/CODEGEN.md) describes how to use the generated code and what the Boost.SML template works around.

## Writing a template

A template is a Jinja file, optionally starting with front matter between `---` lines:

```jinja
---
lang: cpp                   # preset for the guard and id filters: c, cpp, ts, java, python
guard:
  prefix: "actions_."       # overrides the preset: prefix, suffix, not, and, or, true
keywords: [signals, slots]  # more reserved words for the id filter
options:                    # free values, available as {{ options.namespace }}
  namespace: fsm
---
{% filter file(machine.name ~ "Actions.h") %}
#pragma once
namespace {{ options.namespace }} {
struct {{ machine.name }}Actions {
{% for a in actions %}
    virtual void {{ a | id }}() = 0;
{% endfor %}
{% for c in conditions %}
    virtual bool {{ c | id }}() = 0;
{% endfor %}
};
}
{% endfilter %}
```

The front matter is a small YAML subset: nested `key: value` maps, `- item` lists, inline `[a, b]` lists, quoted strings, numbers, booleans and `#` comments.

The template is rendered once per generated machine. Only the content of `file` blocks is written; text outside them is ignored. Machines may write the same file (a shared header) when they give it the same content. File names are relative to the output folder and may contain folders (`"include/" ~ machine.name ~ ".h"`), but not `..`.

Templates are data: they can't run code, and every filter is built in, so generating code from a repository never runs code from it.

### Variables

The [code model](https://github.com/vincedupuis/fsm-editor-vscode/blob/main/docs/CODEGEN.md#code-model-reference) of the machine, one variable per field: `machine`, `states`, `regions`, `topRegions`, `events`, `timers`, `signals`, `actions`, `conditions`, `activities`, `transitions`, `submachines`, `entryPoints`, `exitPoints`, `start`, `invariants`, `features` and `tables`. Besides them, `options` (the front matter's `options`), `config` (the whole front matter) and `lang` (the language settings: `name`, `keywords`, `escape`, `guard`).

### Filters and functions

Besides Jinja's own (`join`, `map`, `selectattr`, `length`, `first`, `indent`, `lower`, `tojson`, ...):

| | Example | Result |
| --- | --- | --- |
| `file` | `{% filter file("a/B.h") %}…{% endfilter %}` | Writes the block's output to a file. |
| `guard` | `{{ t.guard \| guard }}`, `{{ c \| guard(prefix="self.") }}` | Condition tree in the template's language; `true` (the preset's) when there is none. |
| `id` | `{{ "delete" \| id }}` | Escapes reserved words: `delete_`. |
| `pascal`, `camel`, `snake`, `upper_snake` | `{{ "blinkLed" \| upper_snake }}` | `BLINK_LED`, also escaping reserved words. |
| `quote` | `{{ label \| quote }}` | A double-quoted string literal (C, C++, Java, TypeScript, Python). |
| `warn()` | `{% do warn("No forks: ", name, id=id) %}` | Reports a warning about the element with the id `id` (optional, see [Ids](#ids)), at its line in the `.fsm` file. Arguments are joined. |
| `error()` | `{% do error("Cannot generate ", name, id=id) %}` | Reports an error: every error is printed once the template has been rendered, and nothing is written. |

### Whitespace

Templates are rendered with `trim_blocks` and `lstrip_blocks`, the trailing newline kept, and no HTML escaping:

- A block tag (`{% for %}`, `{% if %}`, `{% endfor %}`, ...) alone on its line leaves no line, and so does a comment.
- A block tag at the end of a line also removes that line's newline: write `{{ ", " if not loop.first }}` rather than ending a line with `{% endif %}`, or use `+%}`.
- `{%-`, `-%}`, `{{-` and `-}}` remove the white space (newlines included) before or after the tag: `= { {{- list | join(", ") -}} };` prints `= {1, 2};`.
- Booleans print as `True` and `False`: write `{{ s.isFinal | lower }}` for `true`. Fields that may be null (`parent`, `submachine`, `guard`, ...) print as `None`: test them before printing.

Multi-line pieces, such as the steps of a transition, are macros called with `indent`, which indents every line but the first (the call's own indentation). An excerpt, printing three kinds of steps:

```jinja
{% macro steps(list) %}
{% for s in list %}
{% if s.kind == "call" %}
actions.{{ s.action | id }}();
{% elif s.kind == "enter" %}
enter({{ s.stateIndex }}); // {{ s.state }}
{% elif s.kind == "choice" %}
{% for b in s.branches %}
{{ "if" if loop.first else "} else if" }} ({{ b.guard | guard }}) {
  {{ steps(b.steps) | indent(2) }}
{% endfor %}
}
{% endif %}
{% endfor %}
{% endmacro %}

void fire(int t) {
    switch (t) {
{% for t in transitions %}
    case {{ t.index }}:
{% if t.steps %}
        {{ steps(t.steps) | indent(8) }}
{% endif %}
        break;
{% endfor %}
    }
}
```

Test the list before such a call when it may be empty, or the call's indentation is left as a line of spaces. `templates/ts.jinja` is a complete example to copy: it handles every step kind and has the small runtime the steps rely on.

## WebAssembly

The library (`src/lib.rs`) is the pipeline of `fsm write` without a file system, built as a WebAssembly module without imports and without wasm-bindgen:

```sh
rustup target add wasm32-unknown-unknown
cargo rustc --lib --crate-type cdylib --profile wasm --target wasm32-unknown-unknown
# target/wasm32-unknown-unknown/wasm/fsm_cli.wasm
```

It exports its `memory` and three functions. The request and the response are UTF-8 JSON:

- `fsm_alloc(len) → ptr`: a buffer of `len` bytes, to copy the request into;
- `fsm_generate(ptr, len) → out`: runs the request and frees its buffer. At `out`, the response's length in bytes (a little-endian u32), then the response;
- `fsm_free(out, length + 4)`: frees the response's buffer.

```js
const x = new WebAssembly.Instance(new WebAssembly.Module(bytes), {}).exports;
function generate(request) {
  const req = new TextEncoder().encode(JSON.stringify(request));
  const p = x.fsm_alloc(req.length) >>> 0;
  new Uint8Array(x.memory.buffer, p, req.length).set(req);
  const out = x.fsm_generate(p, req.length) >>> 0;
  const len = new DataView(x.memory.buffer).getUint32(out, true);
  const response = JSON.parse(new TextDecoder().decode(new Uint8Array(x.memory.buffer, out + 4, len)));
  x.fsm_free(out, len + 4);
  return response;
}
```

The request holds what `fsm write` would read: every `.fsm` file the generation may need (the files to generate and the machines their submachine states use, found through the states' `submachine` paths, relative to the folder of the file holding them), and the template's text. Paths are `/`-separated; relative ones are resolved against `cwd`, and messages show paths relative to it.

```jsonc
{
  "cwd": "/work",                          // optional, "/" by default
  "files": { "/work/Order.fsm": "{\n  \"format\": 1, …", "/work/sub/Payment.fsm": "{ …" },  // path → text
  "generate": ["Order.fsm"],               // the files to generate, in order
  "template": "---\nlang: ts\n---\n…",     // the template's text
  "templateName": "ts.jinja"               // its name in messages
}
```

The response holds what `fsm write` would write and print:

```jsonc
{
  "exitCode": 0,                           // fsm write's: 0, 1 errors in the machines or the template, 2 bad request
  "files": [                               // the files to write (none unless exitCode is 0)
    { "path": "Order.ts", "content": "…", "source": "Order.fsm", "modelHash": "3f1c…" }
  ],
  "issues": [                              // its stderr lines: file:line: severity: message
    { "file": "Order.fsm", "line": 12, "id": "Cart#1", "severity": "warning", "message": "…" }
  ],
  "error": null                            // or its last line, without "fsm: "
}
```

A file missing from `files` is reported as `fsm write` reports a file that can't be read. `modelHash` is what `--check` looks for in a generated file. An issue's `id`, which was the element's xmi:id in the old format, is now the element's [id](#ids), derived from the names (`Off`, `On.fault`, `Cart#1`): the one the code model and generated code's traces use. It is empty for the machine itself and for a problem of the file's text or shape, which `line` places.

## Coming from the Handlebars templates

FSM Editor's templates used to be Handlebars (`.hbs`). They no longer are: `fsm` reads Jinja templates only, and refuses a `.hbs` one. To convert a template, start from the bundled `ts.jinja` or `sml.jinja`, which were converted from `ts.hbs` and `sml.hbs` and generate the same files, and translate what you changed with this table:

| Handlebars | Jinja |
| --- | --- |
| `{{#file (concat machine.name ".h")}}…{{/file}}` | `{% filter file(machine.name ~ ".h") %}…{% endfilter %}` |
| `{{guard guard prefix="self."}}`, `{{id name}}`, `{{pascal name}}` | `{{ guard \| guard(prefix="self.") }}`, `{{ name \| id }}`, `{{ name \| pascal }}` |
| `{{upper name}}` | `{{ name \| upper_snake }}` (Jinja's `upper` only changes the case) |
| `{{join states ", " "index"}}` | `{{ states \| map(attribute="index") \| join(", ") }}` |
| `{{quote label}}` | `{{ label \| quote }}` |
| `(includes list x)`, `(eq a b)`, `(and a b)`, `(not a)` | `x in list`, `a == b`, `a and b`, `not a` |
| `{{#switch kind}}{{#case "a" "b"}}…{{/case}}{{#default}}…{{/default}}{{/switch}}` | `{% if kind in ["a", "b"] %}…{% else %}…{% endif %}` |
| `{{warn "x" id=id}}`, `{{error "x" id=id}}` | `{% do warn("x", id=id) %}`, `{% do error("x", id=id) %}` |
| `{{#each list}}{{#unless @first}}, {{/unless}}…{{/each}}`, `@index`, `@last` | `{% for x in list %}{{ ", " if not loop.first }}…{% endfor %}`, `loop.index0`, `loop.last` |
| `{{../name}}`, `@root.machine` | name the loop variables (`{% for s in states %}`); top-level variables are visible everywhere |
| `{{#*inline "steps"}}…{{/inline}}` and `{{> steps}}` | `{% macro steps(list) %}…{% endmacro %}` and `{{ steps(list) \| indent(n) }}` |
| `{{lookup map key}}` | `map[key]` |
| `{{list.length}}` | `list \| length`, or `list` alone in a test |
