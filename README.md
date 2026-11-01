# engine — CodyScript

`@cody-script/engine` is the CodyScript interpreter: a small, sandboxed interpreter for programs written as JSON. The **host**
decides what a program can do by handing it a set of **executors**; the
**program** — stored config, written by a person or an editor — supplies the
logic: variables, conditions, loops and calls to those executors. Nothing else
is reachable.

It is meant for rules that live in data rather than code: a tenant's CSV
mapping, a validation rule, a pipeline step. It has no runtime dependencies
and runs unchanged in Workers, Node (18+) and Bun.

It is published privately to GitHub Packages. A consumer needs the scope
mapped to that registry and a token with `read:packages`:

```ini
# .npmrc (or bunfig.toml: [install.scopes] "@cody-script" = { url = "https://npm.pkg.github.com", token = "$GITHUB_TOKEN" })
@cody-script:registry=https://npm.pkg.github.com
//npm.pkg.github.com/:_authToken=${GITHUB_TOKEN}
```

```bash
bun add @cody-script/engine
```

## Quick start

A rule that cleans a CSV row's `Name` column and refuses a blank one:

```json
{
  "$": "m()",
  "do": [
    {
      "$": "set",
      "set": "name",
      "to": {
        "$": "op",
        "op": { "id": "trim", "map": { "value": { "$": "ref", "ref": "Name" } } },
        "exe": { "id": "text" }
      }
    },
    {
      "$": "if",
      "if": {
        "$": "op",
        "op": { "id": "empty", "map": { "value": { "$": "ref", "ref": "name" } } },
        "exe": { "id": "text" }
      },
      "do": [{ "$": "ret", "ret": { "$": "val", "val": { "error": "Name is required" } } }],
      "tag": "name-required"
    },
    { "$": "ret", "ret": { "$": "ref", "ref": "name" } }
  ]
}
```

```ts
import { createEngine, type Executor } from "@cody-script/engine";

const text: Executor = ({ op, input }) => {
  const value = String(input?.value ?? "");
  switch (op) {
    case "trim": return value.trim();
    case "empty": return value.length === 0;
    default: throw new Error(`text has no "${op}"`);
  }
};

const engine = createEngine({ executors: { text } });

const rule = engine.compile(ruleJson);          // validate once…
for (const row of rows) {
  const { value } = await rule.run({ variables: row }); // …run per row
}
```

`compile` checks the program's shape, depth and every executor id up front, so
a broken rule fails when it's saved — not halfway through a file.

## The language

Every node is a JSON object whose `$` says what it is. Any node may carry a
`tag`: an opaque label (an editor's node id, a rule name) echoed back in
errors. Unknown fields are refused, so a typo is an error rather than ignored.

### Inputs — produce a value

| `$` | Shape | Value |
|---|---|---|
| `val` | `{ "$": "val", "val": <json> }` | The literal. |
| `ref` | `{ "$": "ref", "ref": "name" }` | The variable. Undefined → `undefined_variable` error. |
| `op` | `{ "$": "op", "op": { "id", "map"? }, "exe": { "id", "arg"? } }` | Calls executor `exe.id` with `{ op: op.id, arg: exe.arg, input: <map resolved> }`. |
| `m()` | `{ "$": "m()", "do": [<instruction>…], "id"? }` | Runs the block in a new scope; its value is what it `ret`urns, else `null`. |

### Instructions — do something

| `$` | Shape | Effect |
|---|---|---|
| `set` | `{ "$": "set", "set": "name", "to": <input> }` | Assigns a variable in the current scope. |
| `ret` | `{ "$": "ret", "ret": <input> }` | Ends the enclosing `m()` with that value. |
| `if` | `{ "$": "if", "if": <input>, "do": […], "el"?: […] }` | Runs `do` if the test is truthy, else `el`. |
| `for` | `{ "$": "for", "if": <input>, "do": […] }` | Runs `do` while the test is truthy. |

### Semantics

- **Values are JSON**: `null`, booleans, finite numbers, strings, arrays,
  objects. Keys `__proto__`, `prototype` and `constructor` are refused.
- **Truthiness is JavaScript's**: `false`, `0`, `""` and `null` are false;
  `[]` and `{}` are true.
- **Scope**: a nested `m()` reads its enclosing variables, but `set` inside it
  assigns its own — nothing leaks out, and its `ret` returns only from it. A
  top-level `m()` runs in the root scope, so its assignments are returned as
  `variables`.
- **Order**: an `op`'s `map` is resolved one argument at a time, in key order.
- There is no expression syntax: arithmetic, comparison, string handling and
  so on are executors the host provides.

## Host API

```ts
createEngine({ executors?, limits? }): Engine
engine.compile(program: unknown): CompiledProgram   // throws CodyScriptError
engine.run(program: unknown, options?): Promise<RunResult>  // compile + run
compiled.run({ variables?, signal? }): Promise<RunResult>
// RunResult = { value, variables, steps }
```

- **Executors** — `({ op, arg?, input?, signal? }) => Value | undefined |
  Promise<…>`. `undefined` becomes `null`. Each engine owns its executors;
  there is no global registry.
- **Limits** — `maxSteps` (default 100 000) bounds the work a run may do, so
  an endless `for` fails with `step_limit` instead of hanging a worker;
  `maxDepth` (default 128) bounds JSON nesting.
- **Cancellation** — pass an `AbortSignal` as `signal`; the run stops at the
  next step with `aborted`, and executors receive it to stop their own work.
- **Errors** — every failure is a `CodyScriptError` with a `code`
  (`invalid_program`, `unknown_executor`, `undefined_variable`,
  `executor_failed`, `step_limit`, `aborted`), a JSON Pointer `path` into the
  program, the node's `tag`, and for `invalid_program` / `unknown_executor`
  every `issue` found.
- **Isolation** — `compile` copies the program after validating it, so
  later changes to the source object can't reach a validated program.

## Safety model

A program can only call the executors it was given, and only the operations
those executors accept: **an executor is the whole attack surface**, so treat
its inputs as untrusted. The engine refuses malformed or over-deep programs
before running them, bounds every run by steps, keeps variables and executor
lookups off `Object.prototype`, and never evaluates strings as code.

## Upgrading from `@cody-pope/codyscript` 0.0.x

This package succeeds `@cody-pope/codyscript`. The JSON format is unchanged except that `tag` and a method's `id` are now
optional and `null` is a value. The host API is new:

| 0.0.x | 0.1 |
|---|---|
| `Executors.register("id", fn)` (global) | `createEngine({ executors: { id: fn } })` |
| executor returns `{ value }` | executor returns the value |
| `Inputs.resolve({ frame, input })` | `engine.run(program, { variables })` |
| unknown variable → `undefined` | `undefined_variable` error |
| a nested `m()` ended its caller | a nested `m()` returns to its caller |
| `op.map` resolved in parallel | resolved in order |

## Development

```bash
bun install
bun run test            # vitest
bun run test:coverage
bun run type-check
bun run check           # biome lint + format
bun run build           # dist/
```

Publishing happens from a GitHub release (`.github/workflows/publish.yml`):
it runs the same checks CI runs on every push, then publishes the release's
`package.json` version to GitHub Packages.
