# @cody-script/core — CodyScript

`@cody-script/core` is the CodyScript interpreter: a small, sandboxed interpreter for programs written as JSON. The **host**
decides what a program can do by handing it a set of **executors**; the
**program** — stored config, written by a person or an editor — supplies the
logic: variables, conditions, loops and calls to those executors. Nothing else
is reachable.

It is meant for rules that live in data rather than code: a tenant's CSV
mapping, a validation rule, a pipeline step. It has no runtime dependencies
and runs unchanged in Workers, Node (18+) and Bun.

It's MIT licensed and published on npm:

```bash
npm install @cody-script/core   # or: bun add / pnpm add / yarn add
```

Each release is also published to GitHub Packages, for projects that already
map the `@cody-script` scope there.

## Quick start

A rule that cleans a CSV row's `Name` column and refuses a blank one:

```json
{
  "do": [
    { "set": { "name": { "op": { "text.trim": { "value": { "ref": "Name" } } } } } },
    {
      "if": { "op": { "text.empty": { "value": { "ref": "name" } } } },
      "do": [{ "ret": { "val": { "error": "Name is required" } } }],
      "tag": "name-required"
    },
    { "ret": { "ref": "name" } }
  ]
}
```

```ts
import { createEngine, type Executor } from "@cody-script/core";

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
rule set adds up. **Every node is a JSON object whose type is its key**:
`{ "ref": "x" }` reads a variable, `{ "set": { "x": 1 } }` assigns one. Any
node may also carry a `tag`: an opaque label (an editor's node id, a rule
name) echoed back in errors. Unknown fields are refused, so a typo is an
error rather than ignored.

**Where a node sits decides what it is.** In an input position, an object or
array is always a node — only numbers, strings, booleans and `null` stand for
themselves — so a constant object is wrapped in `val`, and nothing inside
`val` is read as a node. An input has exactly one of `ref`, `val`, `x`, `op`, `do`;
an instruction exactly one of `set`, `ret`, `if`, `for`.

### Inputs — produce a value

| Input | Shape | Value |
|---|---|---|
| literal | `42`, `"text"`, `true`, `null` | Itself. A bare string is text, **never** a variable. |
| `val` | `{ "val": <json> }` | Any value; needed only for objects and arrays. |
| `ref` | `{ "ref": "name" }` | The variable. Undefined → `undefined_variable` error. |
| `x` | `{ "x": "amount * 0.03" }` | An expression over variables (below). |
| `op` | `{ "op": { "text.trim": { <arg>: <input>, … } }, "arg"?: <json> }` | Calls an executor (below). |
| `do` | `{ "do": [<instruction>…] }` | A block: runs in a new scope; its value is what it `ret`urns, else `null`. |

**Calls.** `op` has exactly one key, `"<executor>.<operation>"`, split at the
first dot: `{ "op": { "text.trim": { "value": … } } }` calls executor `text`
with `{ op: "trim", input: { value: <resolved> }, arg }`. A call with no
arguments is `{ "op": { "clock.now": {} } }`. `arg` is a static argument,
passed as written. Executor ids can't contain a dot; operation names can
(`"num.round.up"` is operation `round.up`).

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
| Logic | `&&` and `||` short-circuit and return an operand, as in JavaScript (`discount || 0`); `!` |

Precedence, loosest first: `||`, `&&`, comparisons, `+ -`, `* /`, unary.
There are no function calls: text, dates and everything else are executors.

Expressions are parsed when the program is compiled, so a syntax error is an
`invalid_program` at that node's `/x`. Running one never re-parses it, never
evaluates anything as JavaScript, and counts a step per operator and operand.
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

### Writing programs by hand

[`schema.json`](schema.json) (shipped as `@cody-script/core/schema.json`) is
a JSON Schema for the language. Point your editor at it for completion and
inline errors — in VS Code, `.vscode/settings.json`:

```jsonc
{ "json.schemas": [{ "fileMatch": ["*.cody.json"], "url": "./node_modules/@cody-script/core/schema.json" }] }
```

It checks shape only; `compile` also checks executor ids, nesting depth and
that numbers are finite. A test keeps the two in agreement.

### Storing programs

Store programs minified, and compress them if size matters: gzip
(`CompressionStream("gzip")` in Workers) shrinks the fee example above from
272 bytes to 163.

### Semantics

- **Values are JSON**: `null`, booleans, finite numbers, strings, arrays,
  objects. Keys `__proto__`, `prototype` and `constructor` are refused.
- **Truthiness is JavaScript's**: `false`, `0`, `""` and `null` are false;
  `[]` and `{}` are true.
- **Scope**: a nested block reads its enclosing variables, but `set` inside it
  assigns its own — nothing leaks out, and its `ret` returns only from it. A
  top-level block runs in the root scope, so its assignments are returned as
  `variables`.
- **Order**: a call's arguments are resolved one at a time, in key order.
- **Expressions** do arithmetic, comparison and logic; text, dates and
  everything else are executors the host provides.

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
  `type_error`, `executor_failed`, `step_limit`, `aborted`), a JSON Pointer `path` into the
  program, the node's `tag`, and for `invalid_program` / `unknown_executor`
  every `issue` found.
- **Isolation** — `compile` copies the program after validating it, so
  later changes to the source object can't reach a validated program.

## Safety model

A program can only call the executors it was given, and only the operations
those executors accept: **an executor is the whole attack surface**, so treat
its inputs as untrusted. The engine refuses malformed or over-deep programs
before running them, bounds every run by steps, keeps variables and executor
lookups off `Object.prototype`, and never evaluates strings as code:
expressions are parsed by a small hand-written parser into a tree the engine
walks itself.

## Upgrading

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

Host-side: executors now always receive `input` (`{}` for a call with no
arguments); the `Method` type is now `Block`; error paths follow the new
shape (`…/set/x`, `…/op/text.trim/value`); a variable can no longer be named
`__proto__`, `prototype` or `constructor`.

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

This package succeeds `@cody-pope/codyscript`; apply the tables above as well.

| 0.0.x | 0.3 |
|---|---|
| `Executors.register("id", fn)` (global) | `createEngine({ executors: { id: fn } })` |
| executor returns `{ value }` | executor returns the value |
| `Inputs.resolve({ frame, input })` | `engine.run(program, { variables })` |
| unknown variable → `undefined` | `undefined_variable` error |
| a nested `m()` ended its caller | a nested block returns to its caller |
| `op.map` resolved in parallel | resolved in order |

## Development

```bash
bun install
bun run test            # vitest
bun run test:coverage
bun run type-check
bun run check           # biome lint + format
bun run build           # dist/
bun run size            # gzipped bundle size (CI budget: 4 KB)
```

### Releasing

Releases are automatic. To ship one, bump `version` in `package.json` in your
PR (semver: a format or API break is a minor bump while we're on 0.x, a
major one after 1.0). When it merges, `.github/workflows/release.yml` runs
CI, publishes that version to npm (npmjs.org) and to GitHub Packages, and
creates the `v<version>` tag and GitHub release, with notes listing the
merged PRs. A merge that leaves the version alone releases nothing.
Publishing to npm needs the repository secret `NPM_TOKEN`: an npm automation
token that can publish `@cody-script/core`.

CI checks this on every PR: one that changes shipped files (`src/` outside
tests, or `schema.json`) fails until it bumps to a version not yet released,
so a change can't merge and then never ship.

If a release fails partway, re-run the workflow from the Actions tab; it
skips whatever already succeeded.
