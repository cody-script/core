# codyscript-text

Text functions for [CodyScript](../../README.md) programs: an
[executor](../codyscript-core/README.md#host-api) of basic string
functions. It's also the example to follow when writing an executor crate
of your own (see [Writing an executor crate](#writing-an-executor-crate)).

```rust
use codyscript_core::{Engine, RunOptions, Variables};
use codyscript_json::EngineJsonExt;

// The host: an engine with the text functions, under the id `text`.
let engine = Engine::builder()
    .executor("text", codyscript_text::FUNCTIONS, codyscript_text::text)
    .build();

// The program: it declares the functions it uses in `ext`, and calls them.
let greet = engine.compile_json(r#"{
    "ext": { "text": ["trim", "upper", "is_empty"] },
    "do": [
        { "set": { "name": { "call": { "text.trim": { "value": { "ref": "Name" } } } } } },
        { "if": { "call": { "text.is_empty": { "value": { "ref": "name" } } } },
          "do": [{ "ret": "nobody" }] },
        { "ret": { "call": { "text.upper": { "value": { "ref": "name" } } } } }
    ]
}"#).unwrap();

let row: Variables = codyscript_json::from_str(r#"{ "Name": "  Ada  " }"#).unwrap();
assert_eq!(greet.run_blocking(RunOptions::from(row)).unwrap().value, "ADA");
```

## The functions

Every argument is named, and every one is required. A missing argument, an
unexpected one (a typo, say) or one of the wrong type fails the run with
`function_failed`, naming the argument: `Function "text.upper" failed:
"value" must be a string, got number`.

| Function | Arguments | Returns |
|---|---|---|
| `trim`, `trim_start`, `trim_end` | `value` | `value` without leading and/or trailing white space |
| `upper`, `lower` | `value` | `value` in upper or lower case (Unicode, so `ß` uppercases to `SS`) |
| `char_count` | `value` | how many characters `value` has: Unicode scalar values, not bytes (`"é"` is 1) |
| `is_empty` | `value` | whether `value` is `""` |
| `contains` | `value`, `part` | whether `part` is in `value` |
| `starts_with`, `ends_with` | `value`, `prefix` / `suffix` | whether `value` starts or ends with it |
| `replace` | `value`, `from`, `to` | `value` with every `from` replaced by `to` |
| `split` | `value`, `separator` | the list of parts of `value` between `separator`s (`separator` mustn't be empty) |
| `join` | `values` (a list of strings), `separator` | the strings of `values`, with `separator` between them |

They behave as Rust's `str` methods of the same names do. A program
declares just the ones it uses: `"ext": { "text": ["trim", "split"] }`.

## Writing an executor crate

An executor is host code that a program can call, and nothing else: a
program's `ext` lists everything it can reach outside the sandbox. To
package one as a crate:

1. **Depend on `codyscript-core` only.** An executor works on `Value`s; it
   needs no data format, so it works for programs stored as JSON, CBOR or
   anything else.
2. **Export the executor.** For functions that answer at once, a plain
   function `fn(ExecutorCall) -> ExecutorResult`, as [`text`] is, which a
   host registers with `.executor(id, functions, f)`. For functions that
   wait (I/O), an async one for `.async_executor(…)`. For an executor with
   state of its own (a connection pool, a cache), a type that implements
   [`Executor`](codyscript_core::Executor), for `.custom_executor(…)`: it
   returns `ExecutorFuture::ready(result)` (no allocation) or
   `ExecutorFuture::new(future)`. In each, `call.op()` says which of your
   functions was called, `call.input` holds its arguments by name, and
   `call.arg()` a static `arg` from the program, if any. Return
   `Ok(value)`, or `Err(…)` to fail the run with `function_failed` and your
   message.
3. **Export the names of your functions** — a constant, like
   [`FUNCTIONS`] — for the host to register them with. `compile` checks
   every function a program declares against them, so a program that uses
   one you don't provide is refused before it runs (`unknown_function`).
4. **Let the host choose the id.** `.executor("text", …)` makes your
   functions `text.trim` and so on; another host might register them as
   `str`. Programs follow the host's choice.
5. **Check arguments strictly.** Programs are untrusted: refuse missing,
   unexpected and mistyped arguments with a message that names them, rather
   than defaulting quietly. Never panic: a panic isn't caught, and reaches
   whoever runs the program.

Executors needn't be crates: a host can register a closure directly. A
crate is for functions several hosts share.

```rust
use codyscript_core::{value, ExecutorCall};

// An executor can be tested on its own, without a program or an engine.
let input = value!({ "value": "a,b", "separator": "," }).into_object().unwrap();
let parts = codyscript_text::text(ExecutorCall::new("split", input)).unwrap();
assert_eq!(parts, value!(["a", "b"]));
```
