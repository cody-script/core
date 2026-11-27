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
`val` is read as a node; to build one from computed values, use `obj` or
`list`, inside which every value is an input. An input has exactly one of
`val`, `ref`, `x`, `call`, `do`, `obj`, `list`, `is`, `problems`, `run`; an
instruction exactly one of `set`, `ret`, `if`, `for`, `expect`.

**A program is an input** — usually a block — and what it produces is the
run's value. Its root node may also carry four fields, and only it: `fn`,
the functions the program defines in CodyScript, and `ext`, the external
functions it uses from the host ([Functions](#functions)); `types`, what
its data looks like ([Types](#types)); and `tests`, programs that test it
([Tests](#tests)).

### Inputs — produce a value

| Input | Shape | Value |
|---|---|---|
| literal | `42`, `"text"`, `true`, `null` | Itself. A bare string is text, **never** a variable. |
| `val` | `{ "val": <any> }` | Any value; needed only for objects and lists. |
| `ref` | `{ "ref": "name" }` | The variable. Undefined → `undefined_variable` error. |
| `x` | `{ "x": "amount * 0.03" }` | An expression over variables (below). |
| `call` | `{ "call": { "text.trim": { <arg>: <input>, … } }, "arg"?: <any> }` | Calls a [function](#functions); its value is what the function returns. |
| `do` | `{ "do": [<instruction>…] }` | A block: runs in a new scope; its value is what it `ret`urns, else `null`. |
| `obj` | `{ "obj": { "name": <input>, … } }` | An object of the inputs' values, keys in the order written. |
| `list` | `{ "list": [<input>…] }` | A list of the inputs' values. |
| `is` | `{ "is": <input>, "type": <type> }` | Whether the value is of the [type](#types): `true` or `false`. |
| `problems` | `{ "problems": <input>, "type": <type> }` | What's wrong with the value as one: a list of `{ "path", "message" }`, empty if it is one. |
| `run` | `{ "run": { "name": <input>, … } }` | In a [test](#tests) only: runs the program with these variables; its value is what the program returns. |

**Building data.** `val` is a constant: `{ "val": { "status": 200 } }` is
that object, and nothing inside it is read. `obj` and `list` build from
inputs — literals, variables, expressions, calls, other `obj`s: their
parts are resolved one at a time, in the order written, like a call's
arguments.

```json
{ "obj": {
    "status": 200,
    "name": { "x": "request.body.name" },
    "tags": { "list": ["new", { "ref": "kind" }] },
    "meta": { "val": { "version": 1 } }
} }
```

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
| `expect` | `{ "expect": <input>, "eq"?: <input> }` | Fails the run with `expectation_failed` unless the value is `true` — or, with `eq`, equal to `eq`'s ([Tests](#tests)). |

### Expressions

`{ "x": "…" }` covers the arithmetic and logic every host would otherwise
implement the same way, in a fraction of the bytes:

| | |
|---|---|
| Values | numbers (`2`, `0.03`), text in single quotes (`'paid'`, `'it\'s'`; escapes `\n`, `\t`, `\\`, `\'`), `true`, `false`, `null`, variable names (`gross_sales`) |
| Fields and items | `body.user.name`; `items[0]`, a list's item by a whole number from 0; `headers['content-type']` or `prices[product]`, an object's field by text. Missing is an error; `?.` and `?[` read what may be missing: `body?.nickname`, `items?[9]` are `null` if it is, or if the value read from is `null` |
| Arithmetic | `+ - * /`, unary `-`, parentheses — numbers only |
| Comparison | `== !=` (numbers, text, booleans, `null`; strict, so `1 == '1'` is false; anything, a list or object included, against `null`); `< <= > >=` (two numbers or two texts). One per level: `a < b < c` is refused — write `a < b && b < c` |
| Logic | `&&`, `\|\|` and `!`, on booleans only; `&&` and `\|\|` short-circuit |
| Default | `??`: the left side unless it's `null`, else the right (`discount ?? 0`), like Rust's `Option::unwrap_or`; the right side is evaluated only when needed |

Precedence, loosest first: `??`, `||`, `&&`, comparisons, `+ -`, `* /`,
unary, then reads (`-a.b` is `-(a.b)`). After `.`, any word is a field's
name, keywords included (`response.type`, `flags.null`).
Expressions don't call functions: for text, dates and everything else, use
`call`.

Expressions are parsed when the program is compiled, so a syntax error is an
`invalid_program` at that node's `/x`. Running one never re-parses it, never
evaluates anything as code, and counts a step per operator and operand.
Mixing types (`1 + 'a'`, `'a' < 1`, `!0`, `1 && true`), comparing a list
or object with anything but `null`, or dividing by zero is a `type_error` naming the operator and the
expression. So is reading what isn't there — `Object has no field "nmae"`,
`Index 9 is out of range for a list of 3` — or reading from the wrong kind
of value (`".name" needs an object, got number`, `items[1.5]`, `'abc'[0]`:
text has no items). `?.` and `?[` forgive only absence: `n?.a` of a
number `n` is still an error. Each read is a step, like an operator.

A request handler — the host passes the request in as a variable:

```json
{ "do": [
    { "if": { "x": "request.body?.email == null" },
      "do": [{ "ret": { "obj": { "status": 400, "error": "email is required" } } }] },
    { "ret": { "obj": {
        "status": 200,
        "total": { "x": "request.body.items[0].price * (request.body?.quantity ?? 1)" }
    } } }
] }
```

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

### Types

A program can say what its data looks like. The root's `types` declares
types by name, each written in the shape of the data it describes; `is`
and `problems` check a value against one — or against a type written in
place — and a host can check values against them before running anything
([Host API](#host-api)). A handler that answers bad input with what's wrong
with it:

```json
{
  "types": {
    "Todo": { "obj": {
      "title": "string",
      "done": "boolean",
      "tags": { "list": "string" },
      "due": "string?",
      "owner": "Person?"
    } },
    "Person": { "obj": { "name": "string", "manager": "Person?" } }
  },
  "do": [
    { "if": { "is": { "ref": "body" }, "type": "Todo" },
      "do": [{ "ret": { "obj": { "status": 201, "todo": { "ref": "body" } } } }] },
    { "ret": { "obj": {
        "status": 400,
        "problems": { "problems": { "ref": "body" }, "type": "Todo" }
    } } }
  ]
}
```

With `body` `{ "title": 7, "tags": ["a", 2], "urgent": true }`, it returns
status 400 and these problems, each with a JSON Pointer into the value:

```text
/urgent   Unknown field "urgent"
          Missing field "done"
/title    Expected a string, got number
/tags/1   Expected a string, got number
```

| Type | Is |
|---|---|
| `"any"` | Anything, `null` included. |
| `"null"` | `null`. |
| `"boolean"`, `"number"`, `"string"` | That kind of value. |
| `"integer"` | A whole number: `2`, or `2.0`. |
| `"list"`, `"object"` | Any list; any object. |
| `"Todo"` | A type the program declares in `types`. |
| `"T?"` | A `T`, or `null`; as an object's field, one that may also be left out. |
| `{ "obj": { "field": <type>, … } }` | An object with these fields, and no others. |
| `{ "list": <type> }` | A list whose every item is one. |
| `{ "map": <type> }` | An object whose every field's value is one, whatever the field's name. |

- **Objects are strict**: a field the type doesn't list is a problem
  (`Unknown field "urgent"`), as is one it lists that's missing, unless
  it's optional — so a misspelt field is caught rather than ignored, as in
  programs themselves. For objects with any fields, use `object` or `map`.
- **Types name each other, and themselves**, in any order: a `Person` has
  a `manager` who is a `Person` (or `null`). Only through an `obj`, `list`
  or `map`, though: `"A": "B", "B": "A?"` describes nothing, and is
  refused. A declared type's name is any non-empty text that doesn't end in
  `?` and isn't a built-in type's; a name a type uses that's neither is an
  `invalid_program` (`Unknown type "Persn"`), at its place.
- **Checking** gives each problem as `{ "path", "message" }`, in the order
  found: in an object, unknown fields, then missing ones, then what's in
  each field it has, in the order its type lists them; a list's items in
  order. A check stops after 100. Types describe values; they don't
  convert them: `"5"` isn't a `number`.
- **A check is steps**: one for each value it looks at, so checking a large
  value is bounded by `max_steps` like anything else. It walks the value
  with a stack of its own, so no value is too deep to check.

### Tests

A program can carry its own tests, in the same document: the root's
`tests`, by name, each a program of its own — any input, usually a block.
A test calls the program's functions, and the host's, runs the program
itself with `run`, and says what it expects with `expect`. **A test passes
if it runs to the end**; an `expect` that doesn't hold fails it, as any
other error would. Whatever runs the program can run its tests: an editor,
CI, a host before it puts a new version to work.

```json
{
  "fn": {
    "fee": { "do": [
      { "set": { "fee": { "x": "amount * rate" } } },
      { "if": { "x": "fee > 300" }, "do": [{ "ret": 300 }] },
      { "ret": { "ref": "fee" } }
    ] }
  },
  "tests": {
    "a fee is the rate's share": { "do": [
      { "expect": { "call": { "fee": { "amount": 1000, "rate": 0.03 } } }, "eq": 30 }
    ] },
    "fees stop at 300": { "do": [
      { "expect": { "call": { "fee": { "amount": 20000, "rate": 0.03 } } }, "eq": 300 }
    ] },
    "the program charges 3%": { "do": [
      { "set": { "charged": { "run": { "Amount": 1000 } } } },
      { "expect": { "x": "charged == 30" } }
    ] }
  },
  "do": [
    { "ret": { "call": { "fee": { "amount": { "ref": "Amount" }, "rate": 0.03 } } } }
  ]
}
```

- **`expect`** — `{ "expect": <input> }` needs `true` (a value that isn't
  a boolean is a `type_error`); with `eq`, the two values must be equal:
  numbers by value (`2` and `2.0`), lists item by item, objects field by
  field in any order. Otherwise the run fails with `expectation_failed` at
  the `expect`, saying what it expected and what it got — `Expected 300,
  got 301` — with values written as JSON, cut short past 120 characters.
  It's an instruction like any other, so a program or a function may use
  one too, to fail loudly when something that must hold doesn't.
- **`run`** runs the program, from a test, with only the variables it
  names — resolved in order, like a call's arguments — in a scope of its
  own, as a function call does: the program sees none of the test's
  variables, nor the test the program's. Its value is what the program
  returns. A test may run the program as often as it likes; only a test may
  `run` (anywhere else it's an `invalid_program`), so a program never runs
  itself.
- **A test runs as the program does**: with the host's executors — so it
  can call any function the program declares, and nothing else — and the
  variables, limits and debugger it's given. Its top-level assignments come
  back as `variables`; its paths start with `/tests/<name>`, in errors and
  breakpoints (`/` in a name is `~1`, as in any JSON Pointer).
- **The host runs them** ([Host API](#host-api)), each on its own: tests
  share nothing, so they may run in any order, or at once. To give each its
  own executors — a fresh database, say — compile the program with an
  engine built for that test.
- **Tests don't change the program**: running it never runs them, and it
  behaves exactly as it would without them. A test's name is any non-empty
  text.

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
  doesn't `ret`, an executor with nothing to return, an optional read of
  what isn't there — `Option::None` is what serde writes as `null`). Like
  `None`, it can't be used as anything else: arithmetic, ordering, logic and
  reads on it are `type_error`s. Test for it with `== null` (any value can
  be: a list or object is never `null`), replace it with
  `??`, or pass it on. A variable that doesn't exist is an error, not
  `null`, and so is a field or item that doesn't, unless it's read with `?.`
  or `?[` — as Rust's `map[k]` panics where `map.get(k)` gives `None`.
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
a hash index once it outgrows a short scan; equality ignores order. Its keys
are `Arc<str>`, so cloning a map, or reusing a name as a key, shares them
rather than copying: a program's argument and variable names become keys at
every call and every `set` without allocating. `insert` takes any
`impl Into<Arc<str>>` (`&str`, `String`, `Arc<str>`); iterating yields
`(&str, &Value)`, and an owned map yields `(Arc<str>, Value)`.
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

compiled.types() -> impl Iterator<Item = &str>             // the types it declares
compiled.problems(&value, "Todo") -> Option<Vec<Issue>>    // None: no such type
compiled.tests() -> impl Iterator<Item = Test>             // its tests, in order
compiled.test("fees stop at 300") -> Option<Test>
test.name() -> &str
test.run(options).await / test.run_blocking(options)       // Ok: it passed

RunOptions::new().with_variables(map).with_signal(signal)  // or RunOptions::from(map)
    .with_debugger(breakpoints, debugger)                   // to debug the run
RunResult { value: Value, variables: Map, steps: u64 }
```

- **Executors** provide external functions, and are registered with the
  ones they provide: `.executor("text", ["trim", "empty"], f)` provides
  `text.trim` and `text.empty`, and `compile` checks every function a
  program declares in `ext` against that list. A call of one reaches the
  executor as an `ExecutorCall`: `call.op()` is which of its functions
  (`"trim"`), `call.arg()` the program's `arg`, `call.input` the resolved
  arguments, `call.signal()` the run's signal, and `call.tag()` the `call`
  node's tag — which call it is, for a host tracing its functions' work. The function's name and
  `arg` are shared with the compiled program, not copied for each call. It
  returns `Result<Value, BoxError>` (`Value::Null` for nothing), directly
  or from a `Send` future. A custom `Executor` returns an `ExecutorFuture`:
  `ExecutorFuture::ready(result)` (or `result.into()`) answers at once with
  no allocation, `ExecutorFuture::new(future)` boxes a future. Registering
  an id again replaces its executor and its list. `BoxError` converts from
  any error and from strings, so `?` and `Err("…".into())` work. Each engine owns its executors; there is no
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
  `FunctionFailed`, `StepLimit`, `DepthLimit`, `Aborted`, `InvalidConfig`,
  `ExpectationFailed` —
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
- **Types and tests** — `compiled.problems(&value, name)` checks a value
  against one of the program's [types](#types), as `problems` does in the
  program — say, a request's body, before running a handler with it. It
  counts no steps: the value is the host's. `compiled.tests()` lists the
  program's [tests](#tests), and `test.run(options)` runs one, as
  `compiled.run(options)` runs the program: `Ok` means it passed, and a
  failed `expect` is an `ExpectationFailed` error.

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

A program's tests and types, from the host:

```rust
use codyscript_core::{value, Engine, ErrorCode, RunOptions};

let program = Engine::new().compile(&value!({
    "types": { "Order": { "obj": { "amount": "number", "note": "string?" } } },
    "tests": {
        "charges 3%": { "do": [{ "expect": { "run": { "amount": 1000 } }, "eq": 30 }] },
        "charges 4%": { "do": [{ "expect": { "run": { "amount": 1000 } }, "eq": 40, "tag": "four" }] }
    },
    "x": "amount * 0.03"
})).unwrap();

// Run every test: one passes if it runs to the end.
let failed: Vec<_> = program
    .tests()
    .filter_map(|test| Some((test.name(), test.run_blocking(RunOptions::new()).err()?)))
    .collect();
assert_eq!(failed.len(), 1);
let (name, error) = &failed[0];
assert_eq!(*name, "charges 4%");
assert_eq!(error.code(), ErrorCode::ExpectationFailed);
assert_eq!(error.to_string(), "Expected 40, got 30 at /tests/charges 4%/do/0 (tag four)");

// Check a value against a type, as a host would a request's body.
let problems = program.problems(&value!({ "amount": "12" }), "Order").unwrap();
assert_eq!(problems[0].path, "/amount");
assert_eq!(problems[0].message, "Expected a number, got string");
```

## Debugging

A host can run a program under a **debugger** — host code, usually with a
person behind it — which pauses it at breakpoints, shows its variables, and
steps on. It's all in the [`debug`] module.

- **A step is a node**: an instruction (`set`, `ret`, `if`, `for`,
  `expect`), or an input (a literal, `val`, `ref`, `x`, `call`, a `do`
  block, `obj`, `list`, `is`, `problems` or `run`) — the nodes
  a run counts as steps. (Operands inside an expression count as steps
  too, but run as part of their `x`: a run never pauses between them.) A
  step is named by its path, as in errors (`/do/1`, `/fn/fee/do/0/set/fee`),
  and `compiled.steps()` lists every one in the order written — the
  body's, each function's, then each test's — with its kind, what it names
  (a variable, a function, a type), its tag, the function or test it's in,
  and how deeply it's nested.
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
  `run_blocking`, or `engine.run`, or a test's `run`) as usual. It pauses before each step a
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
serializes as `{ reason, path, kind, name?, tag?, function?, test?, depth,
steps, frames, breakpoints }` — its step's fields, and the rest — ready to
send one. In a test, the program it `run`s is a frame of its own, as a
function's would be: its `call` is the `run`'s path.

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
its host's executors provide — its tests too: **the executors are the
whole attack surface**, so treat their inputs as untrusted. A program's `ext` lists
everything it can reach outside the sandbox, so a host can read it before
running anything. The engine refuses malformed or over-deep programs
before running them, bounds every run by steps, and never evaluates strings
as code: expressions are parsed by a small hand-written parser into a tree
the engine walks itself.

No program can overflow the stack. Running one doesn't recurse at all: the
engine is a loop over explicit stacks, however deep the program, and a
function call is an entry on those stacks, however deep the recursion; nor
does evaluating or dropping an expression, however long, or checking a
value against a type, however deep. Recursion can't
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

### 0.11 → 0.12 (types and tests)

Programs can now say what their data looks like, check values against it,
and carry their own tests, in the same document. Every 0.11 program means
what it did, with the same results and steps; only programs 0.11 refused
change — one message, and `types` and `tests` at the root, which 0.11
refused as unknown fields. What's new:

- **Types**: the root's `types`, and the `is` and `problems` inputs. See
  [Types](#types). A host checks values against them with
  `CompiledProgram::problems`, and lists them with `types()`.
- **Tests**: the root's `tests`, the `run` input (only in tests), and the
  `expect` instruction (anywhere). See [Tests](#tests). A host lists them
  with `CompiledProgram::tests()` or finds one with `test(name)`, and runs
  one with `Test::run` (or `run_blocking`).
- **A new error code**, `ErrorCode::ExpectationFailed`
  (`"expectation_failed"`): an `expect` that doesn't hold. A `match` on
  `ErrorCode` without a wildcard arm needs it.
- **Messages**: an input with no type key (or several) now lists `"is"`,
  `"problems"` and `"run"` among the types expected, and an instruction
  with none lists `"expect"`.
- **Debugging**: `debug::NodeKind` has four new variants, `Is`, `Problems`,
  `Run` and `Expect` (`"is"`, `"problems"`, `"run"`, `"expect"`): a `match`
  on it without a wildcard arm needs them. `Step` has a new field, `test`,
  the test a step is part of (serialized when it has one, in a `Pause`
  too), which a struct literal or a pattern without `..` needs; `steps()`
  lists each test's steps after the functions'. A `Frame`'s `call` may be
  a test's `run`.
- **The schema** (`codyscript-json`) defines `types`, `tests`, `is`,
  `problems`, `run`, `expect` and types themselves.
- **JavaScript** (`codyscript-wasm`): `tests`, `test`, `debugTest` and
  `problems`.

### 0.10 → 0.11 (reading and building data)

Programs can now read into objects and lists, build them from computed
values, and test any value for `null`. Every 0.10 program means what it
did, with the same results and steps; only programs 0.10 refused change —
two messages, and `==`/`!=` between a list or object and `null`, which now
work. What's new:

- **Fields and items in expressions**: `body.user.name`, `items[0]`,
  `headers['content-type']`; `?.` and `?[` give `null` for what may be
  missing (`body?.nickname ?? 'anon'`). Reading a missing field or item, or
  from the wrong kind of value, is a `type_error` — no new error code. See
  [Expressions](#expressions).
- **`obj` and `list` inputs** build an object or a list whose values are
  inputs, resolved in order: `{ "obj": { "total": { "x": "a + b" } } }`.
  Each is a step, and so is each of its parts.
- **Testing anything for `null`**: `row == null` and `items != null`
  work whatever the other side holds — a list or object is never `null`,
  as any `Option` compares with `None`. 0.10 refused them (`"==" can't
  compare object and null`); comparing a list or object with anything
  else is still a `type_error`.
- **Messages**: an input with no type key (or several) now lists `"obj"`
  and `"list"` among the types expected; `1.` in an expression is
  `Expected a field name after "."` rather than `Unexpected "."`.
- **Executors see which call is calling**: `ExecutorCall::tag()` is the
  `call` node's `tag`, shared with the program like `op` (and `tag` in
  JavaScript's call object), so a host can trace its functions' work back
  to the node — an editor's block — that asked for it.
- **Rust**: `debug::NodeKind` has two new variants, `Obj` and `List`
  (`"obj"`, `"list"`): a `match` on it without a wildcard arm needs them.
- **The schema** (`codyscript-json`) defines `obj` and `list`.

### 0.9 → 0.10 (faster compiling and calls; API changes)

The program format, behaviour, messages and error codes are unchanged.
Compiling is 32–37% faster and allocates less than half as much; a
compiled program takes 20–50% less memory. Runs that call executors are
20–35% faster, CodyScript function calls about 12%, and loops of
expressions 9–15%; reading a scope of more than 8 variables is about 5%
slower. What changed in the API:

- **`CompiledProgram::program()` is removed**: a compiled program no
  longer keeps a copy of the program it was compiled from. Keep the
  program yourself if you need it after compiling.
- **`Engine::compile_owned(program)` is removed**: it existed only to skip
  that copy. Use `engine.compile(&program)`, which no longer copies.
- **`Map`'s keys are `Arc<str>`**, shared rather than copied: an executor
  call's arguments and a run's new variables take their names from the
  program without allocating, and cloning an object shares its keys.
  `map.insert(key, value)` takes any `impl Into<Arc<str>>`, so write
  `map.insert("a", v)` (not `"a".into()`, which no longer infers).
  `keys()` and `iter()` yield `&str` rather than `&String`; an owned map
  yields `(Arc<str>, Value)`; `FromIterator` and `Extend` take any key
  that converts into an `Arc<str>`.
- **`ExecutorCall`'s `op`, `arg` and `signal` are methods**, no longer
  fields: `call.op()` (a `&str`: `match call.op() { "trim" => … }`),
  `call.arg()` (`Option<&Value>`), `call.signal()`. The function's name
  and `arg` are shared with the compiled program instead of copied for
  each call. `call.input` is still a field. To test an executor, make a
  call with `ExecutorCall::new(op, input)`, `.with_arg(…)` and
  `.with_signal(…)`.
- **A custom `Executor` returns an `ExecutorFuture`** rather than a
  `BoxFuture`: `ExecutorFuture::ready(result)` (or `result.into()`) for a
  result it has at once, which needs no allocation, or
  `ExecutorFuture::new(future)` for one to await. Executors made with
  `.executor(…)` and `.async_executor(…)` need no change.
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
