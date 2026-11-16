# codyscript-core

The CodyScript interpreter: a small, sandboxed interpreter for programs
written as data. The **host** decides what a program can do by handing it a
set of **executors**; the **program** supplies the logic: variables,
conditions, loops and calls to those executors. Nothing else is reachable.

This crate is the whole interpreter and knows no data format: its only
dependency is `serde`. Programs, variables and results are [`Value`]s, which
implement `Serialize` and `Deserialize`, so any serde format reads and writes
them — `codyscript-json` for JSON, with a schema and conveniences, or any
other format crate directly. In Rust code, write values with [`value!`].

```rust
use codyscript_core::{value, Engine, RunOptions};

let engine = Engine::builder()
    .executor("text", |call| {
        let text = call.input.get("value").and_then(|v| v.as_str()).unwrap_or("");
        Ok(text.trim().into())
    })
    .build();

let rule = engine.compile(&value!({ "do": [
    { "set": { "name": { "op": { "text.trim": { "value": { "ref": "Name" } } } } } },
    { "if": { "x": "name == ''" }, "do": [{ "ret": { "val": { "error": "Name is required" } } }] },
    { "ret": { "ref": "name" } }
] })).unwrap();

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
`val` is read as a node. An input has exactly one of `ref`, `val`, `x`, `op`,
`do`; an instruction exactly one of `set`, `ret`, `if`, `for`.

### Inputs — produce a value

| Input | Shape | Value |
|---|---|---|
| literal | `42`, `"text"`, `true`, `null` | Itself. A bare string is text, **never** a variable. |
| `val` | `{ "val": <any> }` | Any value; needed only for objects and lists. |
| `ref` | `{ "ref": "name" }` | The variable. Undefined → `undefined_variable` error. |
| `x` | `{ "x": "amount * 0.03" }` | An expression over variables (below). |
| `op` | `{ "op": { "text.trim": { <arg>: <input>, … } }, "arg"?: <any> }` | Calls an executor (below). |
| `do` | `{ "do": [<instruction>…] }` | A block: runs in a new scope; its value is what it `ret`urns, else `null`. |

**Calls.** `op` has exactly one key, `"<executor>.<operation>"`, split at the
first dot: `{ "op": { "text.trim": { "value": … } } }` calls executor `text`
with an `ExecutorCall { op: "trim", input: { "value": <resolved> }, arg }`. A
call with no arguments is `{ "op": { "clock.now": {} } }`. `arg` is a static
argument, passed as written. Executor ids can't contain a dot; operation
names can (`"num.round.up"` is operation `round.up`).

### Instructions — do something

| Instruction | Shape | Effect |
|---|---|---|
| `set` | `{ "set": { "name": <input> } }` | Assigns one variable in the current scope. |
| `ret` | `{ "ret": <input> }` | Ends the enclosing block with that value. |
| `if` | `{ "if": <input>, "do": […], "el"?: […] }` | Runs `do` if the test is truthy, else `el`. |
| `for` | `{ "for": <input>, "do": […] }` | Runs `do` while the test is truthy. |

### Expressions

`{ "x": "…" }` covers the arithmetic and logic every host would otherwise
implement the same way, in a fraction of the bytes:

| | |
|---|---|
| Values | numbers (`2`, `0.03`), text in single quotes (`'paid'`, `'it\'s'`), `true`, `false`, `null`, variable names (`gross_sales`) |
| Arithmetic | `+ - * /`, unary `-`, parentheses — numbers only |
| Comparison | `== !=` (numbers, text, booleans, `null`; strict, so `1 == '1'` is false); `< <= > >=` (two numbers or two texts). One per level: `a < b < c` is refused — write `a < b && b < c` |
| Logic | `&&` and `\|\|` short-circuit and return an operand, as in JavaScript (`discount \|\| 0`); `!` |

Precedence, loosest first: `||`, `&&`, comparisons, `+ -`, `* /`, unary.
There are no function calls: text, dates and everything else are executors.

Expressions are parsed when the program is compiled, so a syntax error is an
`invalid_program` at that node's `/x`. Running one never re-parses it, never
evaluates anything as code, and counts a step per operator and operand.
Mixing types (`1 + 'a'`, `'a' < 1`), comparing a list or object, or dividing
by zero is a `type_error` naming the operator and the expression.

A fuller example — a 3% fee on a CSV `Amount`, capped at 300:

```jsonc
{ "do": [
  { "set": { "amount": { "op": { "num.parse": { "value": { "ref": "Amount" } } } } } },
  { "if": { "x": "amount < 0" },
    "do": [{ "ret": { "val": { "error": "Amount must be positive" } } }] },
  { "set": { "fee": { "x": "amount * 0.03" } } },
  { "if": { "x": "fee > 300" }, "do": [{ "set": { "fee": 300 } }] },
  { "ret": { "ref": "fee" } }
] }
```

### Semantics

- **Values** are `null`, booleans, numbers, strings, lists and objects with
  string keys. In a program, numbers must be finite and the keys
  `__proto__`, `prototype` and `constructor` are refused (so programs stay
  portable to the JavaScript engine).
- **Numbers** keep the form they were given (an integer stays exact). In
  expressions they are 64-bit floats, as in JavaScript; a result that is a
  whole number within the range of an `i64` is an integer (`1 + 2` is `3`,
  not `3.0`). Equality is numeric: `1 == 1.0`.
- **Truthiness is JavaScript's**: `false`, `0`, `NaN`, `""` and `null` are
  false; `[]` and `{}` are true.
- **Scope**: a nested block reads its enclosing variables, but `set` inside it
  assigns its own — nothing leaks out, and its `ret` returns only from it. A
  top-level block runs in the root scope, so its assignments are returned as
  `variables`.
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
  `Serialize` and `Deserialize`; so do `RunResult` (`{ value, variables,
  steps }`), `Limits` (`{ maxSteps, maxDepth }`), `ErrorCode` and `Issue`,
  and `Error` serializes as `{ code, message, path, tag?, issues? }`.

## Host API

```text
Engine::new() -> Engine                                   // no executors
Engine::builder()
    .executor(id, |call| -> ExecutorResult)               // sync closure
    .async_executor(id, |call| async move { … })          // async closure
    .custom_executor(id, impl Executor)                   // your own type
    .max_steps(n).max_depth(n)                            // or .limits(Limits)
    .build() -> Engine

engine.compile(&program) -> Result<CompiledProgram, Error>
engine.run(&program, options).await -> Result<RunResult, Error>   // compile + run
engine.run_blocking(&program, options) -> Result<RunResult, Error>
compiled.run(options).await / compiled.run_blocking(options)
compiled.program() -> &Value

RunOptions::new().with_variables(map).with_signal(signal)  // or RunOptions::from(map)
RunResult { value: Value, variables: Map, steps: u64 }
```

- **Executors** receive an `ExecutorCall { op, arg, input, signal }` and return
  `Result<Value, BoxError>` (`Value::Null` for nothing), directly or from a
  `Send` future. `BoxError` converts from any error and from strings, so `?`
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
  expression (see the safety model for what raising it costs).
- **Cancellation** — pass a `Signal` in `RunOptions`; `signal.abort()` (or
  `abort_with(reason)`) from anywhere stops the run at its next step with
  `aborted`, and executors receive the signal to stop their own work.
- **Errors** — every failure is a `codyscript_core::Error` with a `code()`
  (`InvalidProgram`, `UnknownExecutor`, `UndefinedVariable`, `TypeError`,
  `ExecutorFailed`, `StepLimit`, `Aborted` — `code().as_str()` gives
  `"invalid_program"` and so on), a JSON Pointer `path()` into the program,
  the node's `tag()`, a `cause()` (an executor's error, an abort reason) and,
  for `InvalidProgram` / `UnknownExecutor`, every `issues()` found. An
  executor that returns a boxed `codyscript_core::Error` fails the run with
  exactly that error.
- **Isolation** — `compile` copies the program, so later changes to the
  source value can't reach a compiled program.

An async executor, a run with variables, and an error:

```rust
use codyscript_core::{value, Engine, ErrorCode, RunOptions, Value};

let engine = Engine::builder()
    .async_executor("db", |call| async move {
        // e.g. look something up; here, echo the argument back
        Ok(call.input.get("id").cloned().unwrap_or(Value::Null))
    })
    .max_steps(1_000)
    .build();

let program = value!({ "do": [
    { "set": { "row": { "op": { "db.get": { "id": { "x": "n + 1" } } } } } },
    { "ret": { "x": "row * 10" } }
] });
let options = RunOptions::from(value!({ "n": 4 }).into_object().unwrap());
let result = engine.run_blocking(&program, options).unwrap();
assert_eq!(result.value, 50);
assert_eq!(Value::Object(result.variables), value!({ "n": 4, "row": 5 }));

let error = engine.run_blocking(&value!({ "x": "n * 2" }), RunOptions::new()).unwrap_err();
assert_eq!(error.code(), ErrorCode::UndefinedVariable);
assert_eq!(error.path(), "/x");
assert_eq!(error.to_string(), r#"Variable "n" is not defined in "n * 2" at /x"#);
```

## Safety model

A program can only call the executors it was given, and only the operations
those executors accept: **an executor is the whole attack surface**, so treat
its inputs as untrusted. The engine refuses malformed or over-deep programs
before running them, bounds every run by steps, and never evaluates strings
as code: expressions are parsed by a small hand-written parser into a tree
the engine walks itself.

No program can overflow the stack. Running one doesn't recurse at all: the
engine is a loop over explicit stacks, however deep the program; nor does
evaluating or dropping an expression, however long. Only compiling recurses,
once per nesting level, so `max_depth` — which the host sets and no program
can exceed — bounds it. At the default of 128, compiling and running the
deepest allowed program needs under 512 KiB of stack in a native debug build
and under 192 KiB in release: within any thread's, and WebAssembly's 1 MiB.
Raising `max_depth` raises that proportionally: on WebAssembly's 1 MiB stack
(measured: about 590 levels fit in a debug build, 2,000 in release), keep it
under 500 for debug builds and 1,500 for release.

An executor that panics isn't caught: the panic propagates to whoever polls
the run, as panics do.

## Differences from the TypeScript package

Programs, error codes, messages and paths are the same; the differences are
in the host API and in what JavaScript can and Rust can't express:

- Executors return `Result<Value, BoxError>` rather than returning or
  throwing; there's no `undefined` (return `Value::Null`). Register them on
  `Engine::builder()` rather than passing an object or `Map`.
- `Signal` replaces `AbortSignal`: one cloneable handle that is both
  controller and signal. Its reason is any error (or string).
- `compile` takes a `Value`, so the checks for values no data format can
  express (functions, dates, `undefined`) can't arise; non-finite numbers
  still can, and are refused as in JavaScript.
- Object keys keep their document order. (JavaScript objects list
  integer-like keys such as `"1"` first; here they aren't reordered, so with
  several problems in such an object the issues can be listed in a
  different order.)
- A very long expression chain (`1 + 1 + …` with tens of thousands of
  terms) is evaluated without recursion, so it runs, or stops at
  `step_limit`. JavaScript throws a `RangeError` (stack overflow), which
  isn't a `CodyScriptError`.
- `Unexpected "<character>"` names the whole character. For characters
  outside the Basic Multilingual Plane (emoji), JavaScript names half of a
  UTF-16 surrogate pair.

## Upgrading

The program format is shared with the TypeScript package, so its history
applies.

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
