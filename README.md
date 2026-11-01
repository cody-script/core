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
      "to": { "$": "op", "op": "text.trim", "map": { "value": { "$": "ref", "ref": "Name" } } }
    },
    {
      "$": "if",
      "if": { "$": "op", "op": "text.empty", "map": { "value": { "$": "ref", "ref": "name" } } },
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

Programs are kept small on purpose — they are stored and sent, and a large
rule set adds up — so keys are short. Every node is a JSON object whose `$`
says what it is. Any node may carry a `tag`: an opaque label (an editor's
node id, a rule name) echoed back in errors. Unknown fields are refused, so a
typo is an error rather than ignored.

### Inputs — produce a value

| Input | Shape | Value |
|---|---|---|
| literal | `42`, `"text"`, `true`, `null` | Itself. A bare string is text, **never** a variable. |
| `val` | `{ "$": "val", "val": <json> }` | Any value; needed only for objects and arrays. |
| `ref` | `{ "$": "ref", "ref": "name" }` | The variable. Undefined → `undefined_variable` error. |
| `op` | `{ "$": "op", "op": "text.trim", "map"?: {…}, "arg"?: <json> }` | Calls an executor (below). |
| `m()` | `{ "$": "m()", "do": [<instruction>…] }` | Runs the block in a new scope; its value is what it `ret`urns, else `null`. |

**Calls.** `op` is `"<executor>.<operation>"`, split at the first dot:
`"text.trim"` calls executor `text` with `{ op: "trim", input, arg }`, where
`input` is `map` with each argument resolved and `arg` is passed as written.
Executor ids can't contain a dot; operation names can (`"num.round.up"` is
operation `round.up`).

### Instructions — do something

| `$` | Shape | Effect |
|---|---|---|
| `set` | `{ "$": "set", "set": "name", "to": <input> }` | Assigns a variable in the current scope. |
| `ret` | `{ "$": "ret", "ret": <input> }` | Ends the enclosing `m()` with that value. |
| `if` | `{ "$": "if", "if": <input>, "do": […], "el"?: […] }` | Runs `do` if the test is truthy, else `el`. |
| `for` | `{ "$": "for", "if": <input>, "do": […] }` | Runs `do` while the test is truthy. |

A fuller example — a 3% fee on a CSV `Amount`, capped at 300:

```jsonc
{ "$": "m()", "do": [
  { "$": "set", "set": "amount", "to": { "$": "op", "op": "num.parse", "map": { "value": { "$": "ref", "ref": "Amount" } } } },
  { "$": "if", "if": { "$": "op", "op": "cmp.lt", "map": { "a": { "$": "ref", "ref": "amount" }, "b": 0 } },
    "do": [{ "$": "ret", "ret": { "$": "val", "val": { "error": "Amount must be positive" } } }] },
  { "$": "set", "set": "fee", "to": { "$": "op", "op": "math.mul", "map": { "a": { "$": "ref", "ref": "amount" }, "b": 0.03 } } },
  { "$": "if", "if": { "$": "op", "op": "cmp.gt", "map": { "a": { "$": "ref", "ref": "fee" }, "b": 300 } },
    "do": [{ "$": "set", "set": "fee", "to": 300 }] },
  { "$": "ret", "ret": { "$": "ref", "ref": "fee" } }
] }
```

### Writing programs by hand

[`schema.json`](schema.json) (shipped as `@cody-script/engine/schema.json`) is
a JSON Schema for the language. Point your editor at it for completion and
inline errors — in VS Code, `.vscode/settings.json`:

```jsonc
{ "json.schemas": [{ "fileMatch": ["*.cody.json"], "url": "./node_modules/@cody-script/engine/schema.json" }] }
```

It checks shape only; `compile` also checks executor ids, nesting depth and
that numbers are finite. A test keeps the two in agreement.

### Storing programs

Store programs minified, and compress them if size matters: gzip
(`CompressionStream("gzip")` in Workers) shrinks the fee example above from
557 bytes to 217, far more than any key-shortening could.

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

## Upgrading

### 0.1 → 0.2 (format change)

No rules were stored in the 0.1 format, so 0.2 reads only the new one.

| 0.1 | 0.2 |
|---|---|
| `{ "$": "op", "op": { "id": "trim", "map": M }, "exe": { "id": "text", "arg": A } }` | `{ "$": "op", "op": "text.trim", "map": M, "arg": A }` |
| `{ "$": "val", "val": 5 }` | `5` (the old form still works) |
| `{ "$": "m()", "id": "main", … }` | no `id`; use `tag` |
| `InputSchema` / `InstructionSchema` / `ValueSchema` | `schema.json` |
| unknown-executor error path `…/exe/id` | `…/op` |

### From `@cody-pope/codyscript` 0.0.x

This package succeeds `@cody-pope/codyscript`; apply the table above as well.

| 0.0.x | 0.2 |
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

### Releasing

Releases are automatic. To ship one, bump `version` in `package.json` in your
PR (semver: a format or API break is a minor bump while we're on 0.x, a
major one after 1.0). When it merges, `.github/workflows/release.yml` runs
CI, publishes that version to GitHub Packages, and creates the `v<version>`
tag and GitHub release, with notes listing the merged PRs. A merge that
leaves the version alone releases nothing.

If a release fails partway, re-run the workflow from the Actions tab; it
skips whatever already succeeded.
