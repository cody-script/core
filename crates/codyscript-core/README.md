# codyscript-core

The CodyScript interpreter: a small, sandboxed interpreter for programs
written as data. The **host** decides what a program can do by handing it
**executors**, which provide named external functions; the **program**
supplies the logic: variables, conditions, loops, functions of its own, and
calls to those and to the host's. Nothing else is reachable.

This crate is the whole interpreter and knows no data format: its only
dependency is `serde`. Programs, variables and results are [`Value`]s, which
implement `Serialize` and `Deserialize`, so any serde format reads and writes
them — `codyscript-json` for JSON, with a schema and conveniences, or any
other format crate directly. In Rust code, write values with [`value!`].

```rust
use codyscript_core::{value, Engine, RunOptions};

let engine = Engine::builder()
    .executor("text", ["trim"], |call| {
        let text = call.input.get("value").and_then(|v| v.as_str()).unwrap_or("");
        Ok(text.trim().into())
    })
    .build();

let rule = engine.compile(&value!({
    "ext": { "text": ["trim"] },
    "do": [
        { "set": { "name": { "call": { "text.trim": { "value": { "ref": "Name" } } } } } },
        { "if": { "x": "name == ''" }, "do": [{ "ret": { "val": { "error": "Name is required" } } }] },
        { "ret": { "ref": "name" } }
    ]
})).unwrap();

let row = value!({ "Name": "  Ada " }).into_object().unwrap();
let result = rule.run_blocking(RunOptions::from(row)).unwrap();
assert_eq!(result.value, "Ada");
```

The examples below write programs in JSON notation, the language's usual
form; they mean the same in any format.

## The language

Programs are kept small on purpose — they are stored and sent, and a large
rule set adds up. **Every node is an object whose type is its key**:
`{ "ref": "x" }` reads a variable, `{ "set": { "x": 1 } }` assigns one. Any
node may also carry a `tag`: an opaque label (an editor's node id, a rule
name) echoed back in errors. Unknown fields are refused, so a typo is an
error rather than ignored.

**Where a node sits decides what it is.** In an input position, an object or
list is always a node — only numbers, strings, booleans and `null` stand for
themselves — so a constant object is wrapped in `val`, and nothing inside
`val` is read as a node. An input has exactly one of `val`, `ref`, `x`,
`call`, `do`; an instruction exactly one of `set`, `ret`, `if`, `for`.

**A program is an input** — usually a block — and what it produces is the
run's value. Its root node may also carry two fields, and only it: `fn`,
the functions the program defines in CodyScript, and `ext`, the external
functions it uses from the host ([Functions](#functions)).

### Inputs — produce a value

| Input | Shape | Value |
|---|---|---|
| literal | `42`, `"text"`, `true`, `null` | Itself. A bare string is text, **never** a variable. |
| `val` | `{ "val": <any> }` | Any value; needed only for objects and lists. |
| `ref` | `{ "ref": "name" }` | The variable. Undefined → `undefined_variable` error. |
| `x` | `{ "x": "amount * 0.03" }` | An expression over variables (below). |
| `call` | `{ "call": { "text.trim": { <arg>: <input>, … } }, "arg"?: <any> }` | Calls a [function](#functions); its value is what the function returns. |
| `do` | `{ "do": [<instruction>…] }` | A block: runs in a new scope; its value is what it `ret`urns, else `null`. |

**Calls.** `call` has exactly one key, the function's name, whose value
names its arguments: `{ "call": { "text.trim": { "value": … } } }`, or
`{ "call": { "clock.now": {} } }` with none. The arguments are resolved one
at a time, in the order written, before the function runs. A call looks the
same whichever kind of function it calls; `arg`, a static argument passed
as written, is only for external ones.

### Instructions — do something

| Instruction | Shape | Effect |
|---|---|---|
| `set` | `{ "set": { "name": <input> } }` | Assigns one variable in the current scope. |
| `ret` | `{ "ret": <input> }` | Ends the enclosing block with that value. |
| `if` | `{ "if": <input>, "do": […], "el"?: […] }` | Runs `do` if the test is `true`, else `el`. The test must be a boolean. |
| `for` | `{ "for": <input>, "do": […] }` | Runs `do` while the test is `true`. The test must be a boolean. |

### Expressions

`{ "x": "…" }` covers the arithmetic and logic every host would otherwise
implement the same way, in a fraction of the bytes:

| | |
|---|---|
| Values | numbers (`2`, `0.03`), text in single quotes (`'paid'`, `'it\'s'`; escapes `\n`, `\t`, `\\`, `\'`), `true`, `false`, `null`, variable names (`gross_sales`) |
| Arithmetic | `+ - * /`, unary `-`, parentheses — numbers only |
| Comparison | `== !=` (numbers, text, booleans, `null`; strict, so `1 == '1'` is false); `< <= > >=` (two numbers or two texts). One per level: `a < b < c` is refused — write `a < b && b < c` |
| Logic | `&&`, `\|\|` and `!`, on booleans only; `&&` and `\|\|` short-circuit |
| Default | `??`: the left side unless it's `null`, else the right (`discount ?? 0`), like Rust's `Option::unwrap_or`; the right side is evaluated only when needed |

Precedence, loosest first: `??`, `||`, `&&`, comparisons, `+ -`, `* /`,
unary.
Expressions don't call functions: for text, dates and everything else, use
`call`.

Expressions are parsed when the program is compiled, so a syntax error is an
`invalid_program` at that node's `/x`. Running one never re-parses it, never
evaluates anything as code, and counts a step per operator and operand.
Mixing types (`1 + 'a'`, `'a' < 1`, `!0`, `1 && true`), comparing a list
or object, or dividing by zero is a `type_error` naming the operator and the
expression.

A fuller example — a 3% fee on a CSV `Amount`, capped at 300:

```jsonc
{ "ext": { "num": ["parse"] },
  "do": [
    { "set": { "amount": { "call": { "num.parse": { "value": { "ref": "Amount" } } } } } },
    { "if": { "x": "amount < 0" },
      "do": [{ "ret": { "val": { "error": "Amount must be positive" } } }] },
    { "set": { "fee": { "x": "amount * 0.03" } } },
    { "if": { "x": "fee > 300" }, "do": [{ "set": { "fee": 300 } }] },
    { "ret": { "ref": "fee" } }
] }
```

### Functions

Everything a program calls is a function, of one of two kinds, side by
side at the program's root:

- **External functions** are the host's, provided by its executors. A
  program declares the ones it uses in `ext`, by executor:
  `"ext": { "text": ["trim", "empty"] }` declares `text.trim` and
  `text.empty`. Like Rust's `extern`, the list is the program's whole reach
  outside the sandbox, and it's checked against the host when the program
  is compiled: a function the host doesn't provide is an
  `unknown_function`.
- **CodyScript functions** are the program's own, defined in `fn` by name.
  Each is a CodyScript program itself — any input, usually a block.

```json
{
  "ext": { "text": ["trim", "empty"] },
  "fn": {
    "fee": {
      "do": [
        { "set": { "fee": { "x": "amount * rate" } } },
        { "if": { "x": "fee > 300" }, "do": [{ "ret": 300 }] },
        { "ret": { "ref": "fee" } }
      ]
    }
  },
  "do": [
    { "set": { "name": { "call": { "text.trim": { "value": { "ref": "Name" } } } } } },
    { "if": { "call": { "text.empty": { "value": { "ref": "name" } } } },
      "do": [{ "ret": { "val": { "error": "Name is required" } } }] },
    { "ret": { "call": { "fee": { "amount": { "ref": "Amount" }, "rate": 0.03 } } } }
  ]
}
```

- **The program runs as it always has**: its input is where the run starts,
  with the host's variables, in the root scope, so its top-level assignments
  come back as `variables`. Defining and declaring functions changes nothing
  until they're called.
- **A CodyScript function sees only its arguments.** They are its
  variables, in a fresh scope: it can't read its caller's variables or the
  host's — pass what it needs — and its own assignments end with it. Like a
  program, its value is what it `ret`urns (`null` if it doesn't); a `ret`
  ends the function, and the caller carries on. Its top-level block isn't a
  step of its own; the `call` is one.
- **CodyScript functions may call each other and themselves**, in any order
  of definition. Calls may be in progress at most `max_depth` deep (default
  128): one more fails the run with `depth_limit`, so endless recursion stops
  even when steps are to spare. For long repetition, use `for`.
- **Names.** An external function's name is its executor's id, a dot, and
  its name within the executor (`text.trim`; `num.round.up` is executor
  `num`'s `round.up`), so executor ids can't contain a dot. A CodyScript
  function may have any non-empty name, dots included, so functions can be
  grouped by prefix (`text.size`, `pricing.fee`) — even beside an executor of
  the same name, as long as no name is both defined in `fn` and declared in
  `ext`.
- **Checked when compiled**: every `call` must name a function the program
  defines or declares; a name can't be in both; only external calls take
  `arg`; names are non-empty and an `ext` list names each function once —
  any of these is an `invalid_program`, at the node. Errors inside a
  function point into it (`/fn/fee/do/1/if`).

### Semantics

- **Values** are `null`, booleans, numbers, strings, lists and objects with
  string keys, each key appearing once (reading an object with a repeated
  key is an error, in any format). In a program, numbers must be finite —
  including a number literal in an expression, which can't be too large for
  an `f64`.
- **Numbers** keep the form they were given (an integer stays exact). In
  expressions they are 64-bit floats (`f64`); a result that is a whole
  number within the range of an `i64` is an integer (`1 + 2` is `3`, not
  `3.0`). Equality is numeric: `1 == 1.0`.
- **Text** compares by Unicode code point, as Rust's `str` does (`'Z' <
  'a'`).
- **Booleans are strict**, as in Rust: `if` and `for` tests, `!`, `&&` and
  `||` take only `true` or `false`. There's no truthiness — `if amount` with
  `amount` a number is a `type_error`; write `if amount > 0`.
- **`null` is CodyScript's `None`**: the value for "nothing" (a block that
  doesn't `ret`, an executor with nothing to return, a missing field in
  data — `Option::None` is what serde writes as `null`). Like `None`, it
  can't be used as anything else: arithmetic, ordering and logic on it are
  `type_error`s. Test for it with `== null`, replace it with `??`, or pass
  it on. A variable that doesn't exist is an error, not `null`.
- **Scope**: a nested block reads its enclosing variables, but `set` inside it
  assigns its own — nothing leaks out, and its `ret` returns only from it. A
  top-level block runs in the root scope, so its assignments are returned as
  `variables`. A function's scope starts with its arguments and reads nothing
  outside it.
- **Order**: objects keep their keys in the order they were written (a data
  format reads them in document order). A call's arguments are resolved one
  at a time, in that order; `variables` come back in the order they were
  first set (initial variables first).

## The data model

[`Value`] is an enum of `Null`, `Bool`, `Number`, `String`, `Array` and
`Object`. [`Map`] (an object) keeps insertion order and looks keys up through
a hash index once it outgrows a short scan; equality ignores order.
[`Number`] holds an integer exactly or any `f64`, including `NaN` and the
infinities that formats like YAML and CBOR can express — `compile` refuses
them in programs.

- Build values with [`value!`] (`value!({ "a": [1, null, x] })`, splicing in
  Rust expressions), `From` (`Value::from(1)`, `"text".into()`), or
  `collect()`.
- Read them with `as_str`, `as_f64`, `as_object`, `get`, and indexing
  (`v["a"][0]`, `null` when missing).
- Convert with any serde format: `Value`, `Map` and `Number` implement
  `Serialize` and `Deserialize` (an object with a repeated key is refused);
  so do `RunResult` (`{ value, variables,
  steps }`), `Limits` (`{ max_steps, max_depth }`), `RunConfig` (`{
  variables, breakpoints }`, see [Debugging](#debugging)), `ErrorCode` and
  `Issue`, and `Error` serializes as `{ code, message, path, tag?, issues?
  }`.

## Host API

```text
Engine::new() -> Engine                                   // no executors
Engine::builder()
    .executor(id, ["fn", …], |call| -> ExecutorResult)    // sync closure
    .async_executor(id, ["fn", …], |call| async move { … }) // async closure
    .custom_executor(id, ["fn", …], impl Executor)        // your own type
    .max_steps(n).max_depth(n)                            // or .limits(Limits)
    .build() -> Engine

engine.has_function("text.trim") -> bool

engine.compile(&program) -> Result<CompiledProgram, Error>
engine.run(&program, options).await -> Result<RunResult, Error>   // compile + run once
engine.run_blocking(&program, options) -> Result<RunResult, Error>
compiled.run(options).await / compiled.run_blocking(options)
compiled.steps() -> Vec<debug::Step>                       // what breakpoints name
compiled.check(&run_config) -> Result<(), Error>           // its breakpoints name steps?

RunOptions::new().with_variables(map).with_signal(signal)  // or RunOptions::from(map)
    .with_debugger(breakpoints, debugger)                   // to debug the run
RunResult { value: Value, variables: Map, steps: u64 }
```

- **Executors** provide external functions, and are registered with the
  ones they provide: `.executor("text", ["trim", "empty"], f)` provides
  `text.trim` and `text.empty`, and `compile` checks every function a
  program declares in `ext` against that list. A call of one reaches the
  executor as an `ExecutorCall { op, arg, input, signal }` — `op` is which
  of its functions (`"trim"`) — and it returns
  `Result<Value, BoxError>` (`Value::Null` for nothing), directly or from a
  `Send` future. Registering an id again replaces its executor and its list. `BoxError` converts from any error and from strings, so `?`
  and `Err("…".into())` work. Each engine owns its executors; there is no
  global registry. `Engine` is cheap to clone, and `Engine` and
  `CompiledProgram` are `Send + Sync`, so one compiled program can serve
  many runs at once.
- **Running** — `run` is `async` and needs no particular runtime; its
  future is `Send`, so it can be spawned. `run_blocking` drives it on the
  current thread, for sync hosts and sync executors (an executor's future
  that needs a runtime's reactor, like a Tokio timer, must be run with `run`
  from within that runtime).
- **Limits** — `max_steps` (default 100 000) bounds the work a run may do, so
  an endless `for` fails with `step_limit` instead of hanging a worker;
  `max_depth` (default 128) bounds nesting, in the program and in an
  expression (see the safety model for what raising it costs), and how
  deeply function calls nest while running (`depth_limit`).
- **Cancellation** — pass a `Signal` in `RunOptions`; `signal.abort()` (or
  `abort_with(reason)`) from anywhere stops the run at its next step with
  `aborted`, and executors receive the signal to stop their own work.
- **Errors** — every failure is a `codyscript_core::Error` with a `code()`
  (`InvalidProgram`, `UnknownFunction`, `UndefinedVariable`, `TypeError`,
  `FunctionFailed`, `StepLimit`, `DepthLimit`, `Aborted`, `InvalidConfig` —
  `code().as_str()` gives `"invalid_program"` and so on), a JSON Pointer
  `path()` into the program (for `InvalidConfig`, into the run
  configuration), the node's `tag()`, a `cause()` (an executor's error, an
  abort reason) and, for `InvalidProgram`, `UnknownFunction` and
  `InvalidConfig`, every `issues()` found. An
  executor that returns a boxed `codyscript_core::Error` fails the run with
  exactly that error.
- **Isolation** — a compiled program keeps nothing of the value it was
  compiled from: later changes to that value can't reach it, and the value
  can be dropped as soon as `compile` returns.
- **Compile once** — compiling is most of the cost of a short run (parsing
  and compiling a small rule takes several times as long as running it), so
  compile a program once and keep the `CompiledProgram` for every run.

An async executor, a run with variables, and an error:

```rust
use codyscript_core::{value, Engine, ErrorCode, RunOptions, Value};

let engine = Engine::builder()
    .async_executor("db", ["get"], |call| async move {
        // e.g. look something up; here, echo the argument back
        Ok(call.input.get("id").cloned().unwrap_or(Value::Null))
    })
    .max_steps(1_000)
    .build();

let program = value!({
    "ext": { "db": ["get"] },
    "do": [
        { "set": { "row": { "call": { "db.get": { "id": { "x": "n + 1" } } } } } },
        { "ret": { "x": "row * 10" } }
    ]
});
let options = RunOptions::from(value!({ "n": 4 }).into_object().unwrap());
let result = engine.run_blocking(&program, options).unwrap();
assert_eq!(result.value, 50);
assert_eq!(Value::Object(result.variables), value!({ "n": 4, "row": 5 }));

let error = engine.run_blocking(&value!({ "x": "n * 2" }), RunOptions::new()).unwrap_err();
assert_eq!(error.code(), ErrorCode::UndefinedVariable);
assert_eq!(error.path(), "/x");
assert_eq!(error.to_string(), r#"Variable "n" is not defined in "n * 2" at /x"#);
```

## Debugging

A host can run a program under a **debugger** — host code, usually with a
person behind it — which pauses it at breakpoints, shows its variables, and
steps on. It's all in the [`debug`] module.

- **A step is a node**: an instruction (`set`, `ret`, `if`, `for`), or an
  input (a literal, `val`, `ref`, `x`, `call`, or a `do` block) — the nodes
  a run counts as steps. (Operands inside an expression count as steps
  too, but run as part of their `x`: a run never pauses between them.) A
  step is named by its path, as in errors (`/do/1`, `/fn/fee/do/0/set/fee`),
  and `compiled.steps()` lists every one in the order written, with its kind,
  what it names (a variable, a function), its tag, the function it's in, and
  how deeply it's nested.
- **Breakpoints** name the steps to pause before: `Breakpoint::tag("cap")`
  names every step tagged `cap`; `Breakpoint::path("/do/1")` the step at
  that path. Prefer tags where steps have them — an editor's node ids are
  ideal: a path changes when the program around its step is edited
  (insert an instruction before `/do/1`, and it's `/do/2`), but a tag
  moves with its node. `step.breakpoint()` makes the best one for a step.
  **A breakpoint that names no step is an error**: `invalid_config`, listing
  every one that doesn't, by its place in the list (`/breakpoints/0`).
- **A run configuration** (`RunConfig`) is how a program is to be run,
  kept apart from the program, which behaves the same whatever it says:
  its `variables`, and its `breakpoints`. As data — in JSON, what an IDE
  keeps beside a program, with a [schema] in `codyscript-json`:

  ```json
  { "variables": { "Amount": 20000 },
    "breakpoints": [{ "tag": "cap" }, { "path": "/do/1" }] }
  ```

  Either field may be left out; any other is refused. It holds no limits:
  those are the host's, for where it runs. `compiled.check(&config)` checks
  its breakpoints against the program, without running it.
- **A debugged run** is any run whose options have a debugger:
  `RunOptions::new().with_debugger(breakpoints, debugger)`, then `run` (or
  `run_blocking`, or `engine.run`) as usual. It pauses before each step a
  breakpoint names, and then wherever the debugger's actions take it —
  nowhere else: not at the start, not at the end, and with no breakpoints,
  never. (Its variables and breakpoints usually come from a `RunConfig`:
  `RunOptions::from(config.variables).with_debugger(config.breakpoints,
  debugger)`.) It checks the breakpoints first. The debugger may borrow
  (a closure that records its pauses in a local), for as long as the
  options live. At each pause the
  debugger gets a `Pause`: the step (`step()`, as `steps()` lists it) and
  why it paused (`Breakpoint` or `Step`), the steps taken, the variables as
  that step sees them (`scopes()`, `variable(name)`), the call stack
  (`frames()`: each CodyScript function running, the `call` that's running
  it, and its scopes), and the breakpoints, which it may change — checked
  again as the run goes on. It answers with how to go on:

| Action | Pauses next… |
|---|---|
| `StepInto` | at the very next step: this one's first part, or — for a call of a CodyScript function — the first step inside it, after the arguments |
| `StepOver` | at the next step once this one is done, with all its parts and every function it calls; that can be in the caller, when it was a function's last. If it was the program's last, the run ends |
| `Continue` | at the next breakpoint |
| `Stop` | nowhere: the run fails with `aborted` (`Run stopped by the debugger`) at this step, which isn't taken |

To step through a program from its start, put a breakpoint on its first
step (`compiled.steps()[0].breakpoint()`). A breakpoint pauses the run wherever it's
reached, even in the middle of a step over. A debugger answers with a
future, so the run can wait as long as a person takes to click; it may keep
the `Pause` while it waits. `debug::from_fn` makes a debugger from a closure
that answers at once; for a user interface, implement `Debugger` and resolve
the future from it (see `codyscript-wasm` for one in JavaScript). A `Pause`
serializes as `{ reason, path, kind, name?, tag?, function?, depth, steps,
frames, breakpoints }` — its step's fields, and the rest — ready to send
one.

Apart from its pauses, a debugged run is a run: it has the same result,
steps, limits and errors, and can't change what the program does — a pause
shows the variables but doesn't let anyone change them. A run that isn't
debugged pays nothing for this: its loop is built without the checks. A
debugged one asks its debugging session before every step: between
pauses, it runs about 1.3–2× slower (see `codyscript-bench`).

```rust
use codyscript_core::debug::{self, Action, Breakpoint};
use codyscript_core::{value, Engine, RunOptions};

let program = Engine::new().compile(&value!({
    "fn": { "fee": { "do": [
        { "set": { "fee": { "x": "amount * 0.03" } } },
        { "if": { "x": "fee > 300" }, "do": [{ "ret": 300 }], "tag": "cap" },
        { "ret": { "ref": "fee" } }
    ] } },
    "do": [
        { "set": { "fee": { "call": { "fee": { "amount": { "ref": "Amount" } } } } } },
        { "ret": { "x": "Amount + fee" } }
    ]
})).unwrap();

let mut seen = Vec::new();
let debugger = debug::from_fn(|pause| {
    // A pause lasts until the debugger answers: keep copies.
    let function = pause.frames().last().and_then(|frame| frame.function);
    let fee = pause.variable("fee").cloned();
    seen.push((pause.step().path.to_owned(), function.map(str::to_owned), fee));
    Action::StepOver
});
// A breakpoint inside the function, before its `if`: by its tag, which
// edits to the program won't move.
let breakpoints = [Breakpoint::tag("cap")];
let options = RunOptions::from(value!({ "Amount": 20000 }).into_object().unwrap())
    .with_debugger(breakpoints, debugger);
let result = program.run_blocking(options).unwrap();
assert_eq!(result.value, 20300);
assert_eq!(seen, [
    // In `fee`, which has worked out 600…
    ("/fn/fee/do/1".to_owned(), Some("fee".to_owned()), Some(value!(600))),
    // …and after stepping over the `if` that returns 300 from it, back in
    // the program's body. Stepping over that, the last step, ends the run.
    ("/do/1".to_owned(), None, Some(value!(300))),
]);
```

## Safety model

A program can only call the external functions it declares, and only those
its host's executors provide: **the executors are the whole attack
surface**, so treat their inputs as untrusted. A program's `ext` lists
everything it can reach outside the sandbox, so a host can read it before
running anything. The engine refuses malformed or over-deep programs
before running them, bounds every run by steps, and never evaluates strings
as code: expressions are parsed by a small hand-written parser into a tree
the engine walks itself.

No program can overflow the stack. Running one doesn't recurse at all: the
engine is a loop over explicit stacks, however deep the program, and a
function call is an entry on those stacks, however deep the recursion; nor
does evaluating or dropping an expression, however long. Recursion can't
grow memory without bound either: calls nest at most `max_depth` deep. Only compiling recurses,
once per nesting level, so `max_depth` — which the host sets and no program
can exceed — bounds it. At the default of 128, compiling and running the
deepest allowed program needs under 512 KiB of stack in a native debug build
and under 192 KiB in release: within any thread's, and WebAssembly's 1 MiB.
Raising `max_depth` raises that proportionally: on WebAssembly's 1 MiB stack
(measured: about 590 levels fit in a debug build, 2,000 in release), keep it
under 500 for debug builds and 1,500 for release.

An executor that panics isn't caught: the panic propagates to whoever polls
the run, as panics do. The same goes for a debugger, which is host code too.
Debugging changes none of this: a debugged run has the same limits, and a
`Pause` is a read-only view of the variables.

## Upgrading

CodyScript began as a port of the TypeScript package `@cody-script/core`;
since 0.9 it no longer follows it (see below), and earlier notes mention it
for history only.

### 0.9 → 0.10 (faster compiling; two methods removed)

The program format, behaviour, messages and error codes are unchanged.
Compiling is 30–35% faster and allocates less than half as much, and a
compiled program takes 20–50% less memory, because it no longer keeps a
copy of the program it was compiled from. That copy is gone from
the API:

- **`CompiledProgram::program()` is removed.** Keep the program yourself if
  you need it after compiling.
- **`Engine::compile_owned(program)` is removed**: it existed only to skip
  the copy. Use `engine.compile(&program)`, which no longer copies.
- `CompiledProgram`'s `Debug` shows its compiled tree.

### 0.8 → 0.9 (its own semantics, not JavaScript's)

CodyScript no longer matches the TypeScript package's behaviour and
messages: where those followed JavaScript, it now has rules of its own,
closer to Rust's. The program format, the Rust API and the error codes are
unchanged; field names in data are now snake_case. What changed:

| | 0.8 | 0.9 |
|---|---|---|
| Keys `__proto__`, `prototype`, `constructor` | refused in programs | allowed, like any key |
| An object with a repeated key | kept its first place and last value | an error, in every format (`repeated key "a"`); in JSON programs, an `invalid_program` |
| Escapes in `'text'` | `\x` meant `x` (`'a\nb'` was `"anb"`) | `\n` line break, `\t` tab, `\\`, `\'`; any other is an `invalid_program` |
| Text order (`< <= > >=`) | UTF-16 code units | Unicode code points |
| Whitespace in expressions | JavaScript's `\s` | Unicode white space (`char::is_whitespace`) |
| A number literal too large for an `f64` | `Infinity` | an `invalid_program`: `Number "…" is too large` |
| Dividing by zero | `"/" gave Infinity (division by zero?)` | `"/" divided by zero` (still a `type_error`) |
| Overflow (`big * 10`) | `"*" gave Infinity (division by zero?)` | `"*" gave Infinity` |
| An unexpected token | `Unexpected 2`, `Unexpected "a"` (as `JSON.stringify` writes it) | as written: `Unexpected "2"`, `Unexpected "'a'"` |
| Unterminated text | `Unexpected "'"` | `Text has no closing "'"` |
| `if` / `for` tests, `!`, `&&`, `\|\|` | any value, by truthiness (`0`, `""`, `null`, `NaN` false) | booleans only; anything else is a `type_error` (`"if" needs a boolean, got number`) |
| `a && b`, `a \|\| b` | returned an operand (`discount \|\| 0`) | return a boolean; for a default, `discount ?? 0` |
| `??` | — | new: the left side unless it's `null`, else the right |
| Field names in data | `maxSteps`, `maxDepth` (`Limits`); `"stepOver"`, `"stepInto"` (`Action`) | snake_case, as Rust and the error codes already are: `max_steps`, `max_depth`; `"step_over"`, `"step_into"` |

Stored rules most likely to be affected are those that rely on
truthiness: an `if` or `for` whose test isn't a boolean, `!` on a
non-boolean, or `x || default` (now `x ?? default`). Each now fails with a
`type_error` naming the test, rather than silently choosing. Otherwise, a
rule is affected only if it uses a `\n` or `\t` in text (whose meaning
changes), another escape such as `\q` (now refused), text comparisons
between characters from U+E000 to U+FFFF and ones above U+FFFF, or an
overflowing literal. Stored limits and debugger actions need their new
names (`max_steps`, `"step_over"`), in JavaScript too.

### 0.7 → 0.8 (one way to run; API only)

The program format, behaviour, messages and error codes are unchanged:
every 0.7 program runs as before. Only the Rust API changed — **a
debugger is now a run option**, so every run, debugged or not, goes
through `run`:

| 0.7 | 0.8 |
|---|---|
| `compiled.debug(options, breakpoints, debugger).await` | `compiled.run(options.with_debugger(breakpoints, debugger)).await` |
| `compiled.debug_blocking(options, breakpoints, debugger)` | `compiled.run_blocking(options.with_debugger(breakpoints, debugger))` |

`engine.run` and `engine.run_blocking` take a debugger the same way. A run
without one still pays nothing for debugging: which loop runs is decided
once, as the run starts.

**`RunOptions` has a lifetime, `RunOptions<'d>`**: its debugger's, which may
borrow. Elided (`RunOptions<'_>`) or inferred almost everywhere; a function
that makes options for its caller is generic over it (`fn options<'d>(…) ->
RunOptions<'d>`), leaving the caller free to add any debugger. It
is no longer `Clone` (a debugger can't be cloned): build the options again
for each run instead, as their variables are moved into the run anyway.
Its `variables` and `signal` are still public fields, but it now has
private ones too, so it can't be built with a struct literal; use
`RunOptions::new()` or `RunOptions::from(variables)` and the `with_…`
methods.

### 0.6 → 0.7 (adds a debugger; one new error code)

The program format, behaviour and messages are unchanged: every 0.6
program runs as before, with the same results, steps and errors, and a run
that isn't debugged never checks for a pause. What's new is
[debugging](#debugging):

- **`compiled.debug(options, breakpoints, debugger)`** (and
  `debug_blocking`) runs a program under a debugger, which pauses it at
  breakpoints and steps on (the [`debug`] module). `compiled.steps()` lists
  what breakpoints can name, and `compiled.check(&config)` checks a run
  configuration's breakpoints without running the program.
- **`RunConfig`** holds a program's `variables` and `breakpoints`, kept
  beside it. `codyscript-json` has its schema (`RUN_CONFIG_SCHEMA`).
- **In JavaScript**, `debug`, `steps`, `check` and `runConfigSchema` do the
  same (see `codyscript-wasm`).

The one breaking change: **`ErrorCode` has a new variant, `InvalidConfig`**
(`"invalid_config"`), for a breakpoint that names no step. A `match` on
`ErrorCode` without a wildcard arm needs one for it. Only debugging and
`check` return it; `run` never does. `Aborted` now also covers a run its
debugger stopped (`Run stopped by the debugger`).

### 0.6.0 → 0.6.1 (additive)

Faster and leaner, with the same format, behaviour, messages and error
codes: every 0.6.0 program runs as before. Compiling allocates about half
as much and runs allocate far less. Two additions:

- **`Engine::compile_owned(program)`** compiles a program it takes, rather
  than copying it; `compile_json` uses it.
- **In JavaScript, `compile(program, executors, limits)`** returns a
  `Program` to run many times with `program.run(variables)`. It throws a
  `CodyScriptError` for an invalid program (see `codyscript-wasm`).

Compile a program once and keep the `CompiledProgram` (or `Program`) for
every run: for a small rule, compiling costs several times what a run does.

### 0.5 → 0.6 (format change; needs Rust 1.99)

No rules were stored in the 0.5 format, so 0.6 reads only the new one.
Everything a program calls is now a [function](#functions): the host's
external functions, which a program declares in `ext`, and functions of its
own, which it defines in `fn`. Both are called with `call`, which replaces
`op`:

| 0.5 | 0.6 |
|---|---|
| `{ "op": { "text.trim": M } }` | `{ "call": { "text.trim": M } }`, and `"text": ["trim"]` in the root's `ext` |
| `{ "op": { "text.trim": M }, "arg": A }` | `{ "call": { "text.trim": M }, "arg": A }`, declared the same way |
| `{ "do": [ … ] }` calling `text.trim` and `text.empty` | `{ "ext": { "text": ["trim", "empty"] }, "do": [ … ] }` |
| error path `…/op/text.trim/value` | `…/call/text.trim/value` |
| `unknown_executor`, at the call (`…/op/nope.x`) | `unknown_function`, at the declaration (`/ext/nope/0`); an undeclared call is an `invalid_program` at `…/call/nope.x` |
| `executor_failed`: `Executor "text" failed on "trim": …` | `function_failed`: `Function "text.trim" failed: …` |

- **Calls check more.** A call must name a function the program declares
  or defines, and each declared function must be one the host provides —
  not just any operation of a known executor.
- **Hosts list their functions** when registering an executor:
  `.executor("text", ["trim", "empty"], f)` (and the same for
  `async_executor` and `custom_executor`). `EngineBuilder::executors` is
  gone (register an `Arc<dyn Executor>` with `custom_executor`), and
  `Engine::has_executor("text")` is `Engine::has_function("text.trim")`.
  In JavaScript, an executor is an object of its functions (see
  `codyscript-wasm`).
- **Error codes**: `ErrorCode::UnknownExecutor` is `UnknownFunction`
  (`"unknown_function"`), `ExecutorFailed` is `FunctionFailed`
  (`"function_failed"`), and `DepthLimit` (`"depth_limit"`) is new.
- **Messages**: an input-shape error lists `"val", "ref", "x", "call", "do"`.
- **Rust**: the crates use edition 2024 and need Rust 1.99 or newer (0.5
  needed 1.77).

### 0.3 → 0.4 (additive)

Adds the `x` expression node and the `type_error` error code. Every 0.3
program is still valid and behaves the same.

### 0.2 → 0.3 (format change)

No rules were stored in the 0.2 format, so 0.3 reads only the new one. The
node type moves from `"$"` into the key:

| 0.2 | 0.3 |
|---|---|
| `{ "$": "val", "val": V }` | `{ "val": V }` |
| `{ "$": "ref", "ref": "x" }` | `{ "ref": "x" }` |
| `{ "$": "op", "op": "text.trim", "map": M, "arg": A }` | `{ "op": { "text.trim": M }, "arg": A }` (`M` is `{}` when empty) |
| `{ "$": "m()", "do": […] }` | `{ "do": […] }` |
| `{ "$": "set", "set": "x", "to": X }` | `{ "set": { "x": X } }` |
| `{ "$": "ret", "ret": X }` | `{ "ret": X }` |
| `{ "$": "if", "if": X, "do": …, "el": … }` | `{ "if": X, "do": …, "el": … }` |
| `{ "$": "for", "if": X, "do": … }` | `{ "for": X, "do": … }` |

Error paths follow the new shape (`…/set/x`, `…/op/text.trim/value`); a
variable can no longer be named `__proto__`, `prototype` or `constructor`.

### 0.1 → 0.2 (format change)

No rules were stored in the 0.1 format, so 0.2 reads only the new one.

| 0.1 | 0.2 |
|---|---|
| `{ "$": "op", "op": { "id": "trim", "map": M }, "exe": { "id": "text", "arg": A } }` | `{ "$": "op", "op": "text.trim", "map": M, "arg": A }` |
| `{ "$": "val", "val": 5 }` | `5` (the old form still works) |
| `{ "$": "m()", "id": "main", … }` | no `id`; use `tag` |
| unknown-executor error path `…/exe/id` | `…/op` |

[`Value`]: https://docs.rs/codyscript-core/latest/codyscript_core/enum.Value.html
[`Map`]: https://docs.rs/codyscript-core/latest/codyscript_core/struct.Map.html
[`Number`]: https://docs.rs/codyscript-core/latest/codyscript_core/struct.Number.html
[`value!`]: https://docs.rs/codyscript-core/latest/codyscript_core/macro.value.html
[`debug`]: https://docs.rs/codyscript-core/latest/codyscript_core/debug/index.html
[schema]: ../codyscript-json/run-config.schema.json
