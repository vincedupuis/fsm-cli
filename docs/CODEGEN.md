# Code generation

`fsm write` reads the machines, validates them, builds the code model and renders a template on it:

```
.fsm (JSON) ──read──▶ model ──validate──▶ code model ──render──▶ files
```

The code model works out the UML semantics once for every language: the state tables, the exits and entries of every transition in order, the choices, the static junctions, the joins, the history and the timers. Guards become condition trees, not text in some language. The template language (front matter, variables, filters and functions, `file` blocks) is documented in the [README](../README.md#writing-a-template); this page describes the bundled templates, the code model they receive, and the semantics of the generated code.

- [The TypeScript template](#the-typescript-template)
- [The Boost.SML template](#the-boostsml-template)
- [Tracing](#tracing)
- [Code model reference](#code-model-reference)
- [Semantics of the generated code](#semantics-of-the-generated-code)

## The TypeScript template

The `ts` template (`ts.jinja`) writes two files per machine, for example for `Order`:

| File | Contents |
| --- | --- |
| `OrderActions.ts` | Event, state, activity and entry/exit point types, the `OrderActions` interface, options |
| `Order.ts` | The `Order` class |

Generated files are overwritten every time the code is generated, so don't edit them. Put your code in your own class implementing `OrderActions`, in a file of your own:

```ts
import { Order } from './Order';
import { MyOrderActions } from './MyOrderActions'; // your class: implements OrderActions

const order = new Order(new MyOrderActions(), { onFinished: () => console.log('done') });
order.start();
order.dispatch('checkout');
order.isIn('Payment');   // true
order.activeStates;      // ['Payment']
```

- **Actions:** every function called by an entry, exit or transition action behavior is a `void` method of the actions interface. Every function called by a guard, invariant or pre/postcondition is a `boolean` method.
- **Do activities** `do / blinkLed()` become `startBlinkLed()` and `stopBlinkLed()`. Call `machine.activityDone('blinkLed')` when an activity ends by itself. A state with do activities completes when all of them have ended. An activity listed twice in one `do` is started, stopped and awaited once.
- **Time events** `after(5m)` use `setTimeout`. Pass `{ clock }` to control time in tests.
- **Submachines:** each machine gets its own class. `OrderActions.submachines.Payment` gives the actions of the `Payment` submachine state. The parent routes events to the running submachine and follows its exit points. `Order.ts` imports `Payment.ts`, so generate `Payment.fsm` too, for example with `fsm write "models/*.fsm" …`.
- **Constraints:** invariants, preconditions and postconditions call `onConstraintViolation(kind, element)`.
- `terminate` calls `onTerminate()` and stops the machine.
- **Tracing:** pass `onTrace` in the options to know what the machine does, for example to write a log (see [Tracing](#tracing)).
- **Other methods:** `startAt(point)` enters through one of the machine's entry points (a machine without an initial pseudostate has no `start()`: it is entered this way only). `stop()` leaves every state. `resume()` re-enters what was active when it was stopped.
- Identifiers that are TypeScript keywords get a `_` suffix (`delete()` → `delete_()`).

The generated code has no dependencies, is strict-mode clean, and runs in browsers and Node.js.

## The Boost.SML template

The `sml` template (`sml.jinja`) writes C++17 for [Boost.SML](https://github.com/boost-ext/sml) (tested with 1.2.0), four files per machine, for example for `Order`, and `FsmTimers.h`, shared by all machines:

| File | Contents |
| --- | --- |
| `OrderEvents.h` | What the machine reacts to: the `OrderEvents` interface, one method per event (and `activityDone()`), and the `OrderEvent` enum. |
| `OrderActions.h` | What the machine needs: the `OrderActions` interface to implement, with its behaviors, conditions, do activities, `onConstraintViolation()`, `onTerminate()` and submachine actions. |
| `FsmTimers.h` | The `FsmTimers` interface that runs the timers of time events, the same for every machine. Written only by machines with time events (their own or their submachines'). |
| `Order.h` | The `Order` class, which implements `OrderEvents`, and the `OrderState`, `OrderEntryPoint` and `OrderExitPoint` enums. |
| `Order.cpp` | Its implementation: the Boost.SML transition tables and the runtime around them. Only this file includes `<boost/sml.hpp>`. |

Compile the `.cpp` files with your code, with `boost/sml.hpp` on the include path and C++ exceptions enabled (a terminate pseudostate stops the machine by unwinding). Put your code in your own class implementing `OrderActions`:

```cpp
#include "Order.h"
#include "MyOrderActions.h" // your class: public OrderActions

MyOrderActions actions;
Order order(actions);
order.startMachine();
order.checkout();                      // an event
order.isIn(OrderState::Payment);       // true
order.activeStates();                  // {OrderState::Payment}
```

- **Events** are the methods of `OrderEvents`. They are queued and processed one at a time, each to completion, also when sent from within an action.
- **Behaviors** are `void` methods of the actions interface and **conditions** (guards, invariants, pre/postconditions) `bool` methods, as in the TypeScript template.
- **Do activities** become `startBlinkLed()` and `stopBlinkLed()`; call `activityDone(MediaPlayerActivity::blinkLed)` when one ends by itself.
- **Timers** go through `FsmTimers`, which you implement once and pass to the constructor of every machine with time events, `Order(actions, timers)`. A machine passes it on to its submachines. `startTimer(delay, fire)` must call `fire()` once after `delay` unless `cancelTimer()` comes first, on the thread that sends the events. Delays are rounded up to whole milliseconds. A late `fire()` does nothing.
- **Submachines:** each machine gets its own class, run by its submachine states. `OrderActions::submachinePayment()` gives the actions of the `Payment` submachine state. Generate `Payment.fsm` too.
- **Other methods:** `startMachineAt(point)` enters through an entry point (a machine without an initial pseudostate has no `startMachine()`: it is entered this way only), `stopMachine()` leaves every state, `resumeMachine()` re-enters what was active when it was stopped. `setListener()` is told when the machine finishes or leaves through an exit point, and through `Listener::onTrace()` what the machine does (see [Tracing](#tracing)). They are named so that they don't clash with events such as `start` or `stop`.
- Identifiers that are C++ keywords get a `_` suffix.

Boost.SML runs the transition tables: one per composite state, with a `*` initial state per region, and the machine's own. The runtime around them gives the machine UML's semantics where Boost.SML's differ: the event queue, completion events (processed before other events), deferred events (dispatched again after the next state change, after the completion events), timers, constraints and submachines. Boost.SML has no counterpart for some UML features; the template works around them, keeping the order of exits, transition actions and entries, and prints a warning for each:

| UML | Generated code |
| --- | --- |
| Transition leaving a composite state from one of its substates, exit point, join | The substate takes the event, then the table of the composite state being left takes an internal event that continues the transition. A join's sources check, when they complete, that the others have completed. |
| Transition entering a nested state, entry point, fork, history | Every region starts in an initial pseudostate whose anonymous transitions enter the region by default, follow the route set by the transition, or restore the region's last active state (shallow or deep). |
| Local transition | A transition of each substate of the composite state, which leaves the active substate but not the composite state. |
| Orthogonal regions reacting to the same event | Boost.SML fires the transitions region after region, so a later region's guards see the earlier regions' actions (UML selects them all first). This is the one difference in behavior, hence the warning. |

What it cannot express stops the generation with an error, and nothing is written: a local transition of a state with several regions, and names that would not compile (an event named like a method of the generated class, a function used both as a behavior and as a condition).

## Tracing

Both templates report what the machine does through a hook, always generated and doing nothing until you give it something to do: `onTrace` in the TypeScript options, `Listener::onTrace()` in C++ (set with `setListener()`). It is called with four strings:

| Argument | |
| --- | --- |
| `machine` | The machine's name. For a submachine, followed by the submachine states leading to it: `Order.Payment`. |
| `kind` | `event` (an event or time event is processed), `enter` (a state is entered, before its entry behavior), `exit` (a state was left, after its exit behavior), `transition` (a transition is taken, before its action). |
| `id` | The id of the state or transition in its `.fsm` file, derived from the names (see [Ids](../README.md#ids)): a state's name (`Off`), a transition's source and position among its transitions (`Off#1`, `On.fault#1`, `MediaPlayer.initial#1`); for an event, its name. |
| `name` | The name of the state, the transition (`Cart -> Payment (checkout)`) or the event. |

A compound transition reports each of its transitions: through junctions and choices, the branches taken. For example, to write them as log lines:

```ts
const order = new Order(new MyOrderActions(), {
  onTrace: (machine, kind, id, name) => console.log(`${new Date().toISOString()} ${machine} ${kind} ${id} ${name}`),
});
```

```cpp
struct TraceToLog : Order::Listener {
    void onTrace(const char* machine, const char* kind, const char* id, const char* name) override {
        std::clog << timestamp() << ' ' << machine << ' ' << kind << ' ' << id << ' ' << name << '\n';
    }
};
TraceToLog tracer;
order.setListener(&tracer);
```

A template prints the `transition` steps (see [Steps](#steps)) and the table action of the same name to report transitions.

## Code model reference

Top level:

| Field | |
| --- | --- |
| `machine` | `name` (identifier), `displayName`, `kind` (`behavioral`/`protocol`), `protocol`, `context` (the name of the classifier the machine belongs to, or empty), `documentation`, `sourceFile`, `modelHash` (the hash of the model in the `.fsm` file, computed from its content: see `--check` in [Usage](../README.md#usage)) |
| `states` | All states and final states, depth first ([below](#states)). |
| `regions`, `topRegions` | `index`, `name`, `displayName`, `top`, `owner`/`ownerIndex` (-1 at the top), `states`, `defaultSteps` (the initial transition; empty when the region has no initial pseudostate) |
| `events` | Named events: `name`, `index`, `signalIndex`, `own` (false when only a submachine handles it) |
| `timers` | One per `after(...)` and source state: `name` (`Paused_after_5m`), `index`, `signalIndex`, `ms`, `trigger`, `state`, `stateIndex` |
| `signals` | Events then timers, numbered from 0: `name`, `index`, `kind` (`event`/`timer`) |
| `actions`, `conditions`, `activities` | Names of the functions called by behaviors, by conditions, and by do activities. |
| `transitions` | Everything that can fire: `index`, `id` (of the UML transition, or of the join), `name`, `label`, `kind` (`external`/`internal`/`local`/`join`/`exitPoint`), `source`/`sourceIndex`, `steps`, `via` (the pseudostates it goes through, in order: `kind`, `name`, `id`; those of its own way through junctions, and those of every branch of its choices) |
| `submachines` | Distinct machines used by submachine states: `name`, `href` (the path of its file, as the submachine state writes it), `events`, `entryPoints` and `exitPoints` (names), `defaultEntry` (it can be started by default), `timers` (it uses time events, itself or in its submachines), `states` |
| `entryPoints`, `exitPoints` | The machine's own connection points: `name`, `id`, and `steps` for entry points. |
| `start` | Steps of the default entry. |
| `invariants` | The states that have an invariant. |
| `features` | Booleans: `defaultEntry` (every top-level region has an initial pseudostate; without it, the machine is entered through its entry points only, and the bundled templates write no `start()`/`startMachine()`), `timers`, `submachineTimers`, `anyTimers` (one of the two), `activities`, `history`, `deferral`, `choices`, `joins`, `terminate`, `submachines`, `entryPoints`, `exitPoints`, `invariants`, `preconditions`, `postconditions`, `constraints`, so a template can leave out unused runtime parts. |
| `tables` | The machine as nested transition tables ([below](#transition-tables)). |

References to other elements are `{ name, index }` objects, or a `…Index` number.

### States

| Field | |
| --- | --- |
| `index`, `id`, `name`, `displayName` | `id` is the state's id in the file: its name. `name` is unique (an identifier made from the name). Generated names (ending with `*`) count as no name: final states without a name of their own are named `Final`. |
| `kind`, `isFinal` | `simple`, `composite`, `submachine`, `final` |
| `region`/`regionIndex`, `parent`/`parentIndex`, `depth`, `regions` | Position in the hierarchy. |
| `entry`, `exit` | Steps of the state's own entry: the entry behavior, then starting do activities and timers. Steps of its own exit: stopping the submachine, timers and activities, then the exit behavior. |
| `activities`, `timers`, `deferred`, `invariant`, `stereotype`, `submachine` | `submachine` is `{ machine, href }` or null. |
| `completes` | When the completion event occurs: `immediate`, `activities`, `regions`, `submachine`, `never` |
| `handlers` | Triggered transitions grouped by signal: `signal`, `signalIndex`, `kind`, `candidates`, `exhaustive` (the last candidate is unconditional). A transition triggered by `all` is a candidate for every event the state's other transitions don't name and the state doesn't defer. |
| `completion` | Candidates for the completion event. |
| `exits` | Submachine states: `{ point, candidates }` for each exit point reference. |

A **candidate** has `transition`/`transitionIndex`, `guard` (condition or null), `precondition` (protocol machines), `requires` (joins: states that must all be active and completed) and `label`. Candidates are in priority order: take the first enabled one.

A **condition** is `{ op: "call", name }`, `{ op: "in", state, stateIndex }` (a state test: true while that state is active), `{ op: "not", arg }`, or `{ op: "and" | "or", args }`.

### Steps

| `kind` | Fields | Meaning |
| --- | --- | --- |
| `call` | `action` | Call a behavior function. |
| `startActivity`, `stopActivity` | `activity` | Start or abort a do activity. |
| `startTimer`, `cancelTimer` | `timer`, `timerIndex`, `ms` | Arm or cancel a time event. |
| `exit` | `state`, `stateIndex` | If the state is active: exit its active substates (innermost first), run its `exit` steps, and record it as its region's history. |
| `exitRegion` | `region`, `regionIndex` | Exit the region's active state, if any. |
| `enter` | `state`, `stateIndex` | Make the state active and run its `entry` steps, but not its regions: they get their own steps. |
| `enterRegion` | `region`, `regionIndex` | Default entry of the region (run its `defaultSteps`). |
| `restoreHistory` | `region`, `regionIndex`, `deep`, `defaultSteps` | Re-enter the region's last active state (and, for deep history, theirs, recursively), or run `defaultSteps` when there is none. |
| `choice` | `label`, `branches` (`guard`, `isElse`, `steps`, `via`: the pseudostates the branch goes through after the choice), `hasElse`, `region`/`regionIndex` | Evaluate the guards now and take the first enabled branch. `region` is where the choice is evaluated. |
| `startSubmachine` | `state`, `stateIndex`, `point` | Start the submachine by default, or through its entry point `point`. |
| `stopSubmachine` | `state`, `stateIndex` | Exit all states of the submachine. |
| `exitMachine` | `point` | Leave through the machine's exit point: exit every state, then tell the parent machine. |
| `terminate` | | Stop the machine without exit behaviors. |
| `check` | `constraint` (`postcondition`), `element`, `condition` | Report a violation when the condition is false. |
| `transition` | `id`, `label` | A transition is taken (one of the transitions of a compound transition, before its action): report it for [tracing](#tracing), or print nothing. |

`exit`, `exitRegion`, `enterRegion` and `restoreHistory` depend on the configuration at run time. They need a small runtime: an active state per region, a history per region, and the tables in the code model. See `ts.jinja`.

### Transition tables

`tables` is the same machine for frameworks whose transitions stay inside one table, such as Boost.SML: a table per composite state with the transitions between its substates, and one for the machine. Such frameworks can't leave a composite state from a substate or enter a nested state directly, so the tables do it with two mechanisms that keep UML's order of exits, transition actions and entries:

- **Continuations** (leaving from inside): the substate takes the event without changing state and posts a continuation, a table event that the table of the composite state being left takes right away, once per state left when actions come between the exits.
- **Routes** (entering inside): every region starts in an `init` pseudostate whose anonymous rows enter the region by default (route 0), restore its last active state (route 1, deep resume, or the route of a history pseudostate), or follow the route a transition set before entering the composite state.

| Field | |
| --- | --- |
| `list` | Tables, innermost first, the machine's last: `state` (null for the machine's), `name`, `top`, `regions` (`index`, `name`, `init`, `off`: the pseudostate the machine's regions wait in until started), `states`, `pseudostates`, `rows` |
| `events` | Table events, numbered from 0: `name`, `index`, `kind` (`start`, `stop`, `timer`, `completion`, `continuation`, `exitPoint`), `ref` (the timer or state), `point` |
| `timerEvents`, `completionEvents` | The table event of each timer, and of each state's completion (null when no transition waits for it). |
| `postconditions` | `index`, `element`, `condition`: checked after their transition. |
| `routes` | Number of routes; 0 and 1 are reserved. |
| `entryPoints` | The machine's entry points: `name`, and the `route` actions to start through it. |
| `notes`, `problems` | What the tables work around, and what they can't express: `kind`, `id`, `element`, `state`, `detail`. A template turns them into `warn()` and `error()`. |

A **row** has a `source` and a `target` (null for an internal transition), each `{ kind: "state" \| "pseudo", name, index, composite, role }`, `initial` (the region's initial state), `trigger` (`{ kind, name, index }` with kind `event`, `table`, `entry` or `exit`, or null for an anonymous row taken as soon as the source is entered), `guard` (terms that must all hold, in order), `actions` and a `comment`. Rows are in priority order for a source and trigger: offers to a submachine, then the transitions, then deferral.

Guard terms: `condition`, `precondition` (`condition`, `label`), `requires` (joins), `offer` (the submachine of `state` uses the event), `routed` (`region`, `route`), `restores` (`region`, `route`, `state`), `defaults` (`region`).

Actions: `stateEntry` and `stateExit` (with the `state`, whose `entry`/`exit` steps to run), `call`, `transition` (`id`, `label`: for tracing, as the step), `postcondition`, `terminate`, `exitMachine`, `continue` (post the continuation `event`), `route` (`region`, `route`, `history`: 0, 1 shallow, 2 deep), `submachineAt` (`state`, `point`, `pointIndex`), `entering` and `restore` (the region forgets its route), `defer`, `noBranch` (`label`).

## Semantics of the generated code

The code model and the bundled templates implement UML run-to-completion semantics:

- **Events** are queued and processed one at a time. Each run-to-completion step first selects the enabled transitions, then fires them.
- **Priority:** the innermost states get the event first. A state's own transitions apply only when none of its substates used the event. Orthogonal regions each fire their own transition in the same step. When two selected transitions conflict (the first one fired has left the source of the other), only the first fires.
- **Completion events** (states entered, do activities ended, all regions final, submachine finished) are processed before the next event. A completion event whose transitions are all disabled is lost.
- **State tests** `in(S)` in a guard, an invariant or a pre/postcondition hold while state S is active when the condition is evaluated.
- **Any-receive events:** a transition triggered by `all` takes every named event that the other transitions of its state don't name, and that the state doesn't defer. Time events and completion events aren't received messages: `all` doesn't take them.
- **Deferred events** are kept while a deferring state is active and there is no transition for them. They are dispatched again after the next state change. A deferring state has priority over transitions of the states around it.
- **Order of actions:** exits from the innermost state outwards, then the transition actions, then entries from the outside in. Other orthogonal regions get their default entry.
- **Choices** are evaluated when reached, after the exits and actions before them. A choice always has an `[else]` branch (validation requires it); the generated code still throws if no branch is enabled.
- **Junctions** are static: each way through a chain of junctions becomes its own transition whose guard combines all the guards (`else` is the negation of its siblings). A transition fires only when a whole path is enabled.
- **Forks** enter all their targets, and regions without a target get their default entry. **Joins** fire when all their source states are active and completed (join incoming transitions have no triggers).
- **History:** shallow history restores the last active substate with default entry below it. Deep history restores all levels, including submachines. A region last left in a final state restores by default.
- **Entry points** of a composite state: the state is entered, then all the entry point's transitions, which have no guards. **Exit points:** the state is exited, then the exit point's transitions are taken like a junction's. An exit point reached from several orthogonal regions acts as a join: the state is left once all the sources have completed, then the actions of the transitions entering the point run in order.
- **Submachines** run as separate objects. The parent offers them each event before its own transitions (they have priority like substates), starts them by default or through an entry point, stops them when the submachine state is exited, and follows their exit points and completion.
- **Protocol state machines:** preconditions select transitions. An event that finds no transition, because a precondition is false or because no transition of the current states takes it, is a protocol violation, reported with the transition's label or the event's name (not when a parent machine offers the event to it as a submachine: the parent may still take it). On the branches of choices and junctions, the precondition selects the branch, as a guard does (a branch's `guard` in the code model). Postconditions are checked after the transition (a branch's, after the rest of its way), and invariants after every step.
