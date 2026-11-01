# engine

The **CodyScript interpreter**, as a library: programs are JSON, the host
supplies executors, and nothing else is reachable. Consumed by product
workers (first: `cody-script/etl`) that store tenant-written rules.
README.md is the language reference and host API; this file is the working
guide.

## Quick Reference

```bash
bun install
bun run test            # Vitest
bun run test:coverage   # thresholds: 100% lines/functions/statements
bun run type-check
bun run check           # Biome (check:fix to apply)
bun run build           # tsc → dist/
```

## Structure

```
src/
  types.ts    # The language (Value, Input, Instruction) and host types
  schema.ts   # Valibot schemas + preflight (depth, reserved keys)
  engine.ts   # createEngine → compile → run; scopes, steps, errors
  errors.ts   # CodyScriptError and its codes
  index.ts    # Public surface — keep it small
  test/       # Test builders (not shipped)
```

Runtime dependency: `valibot` only. Must run unchanged in Workers, Node and
Bun — no Node built-ins outside tests.

## Rules

- **Programs are untrusted.** Every new node type needs: a schema entry
  (`strictObject`), an executor-id walk in `checkExecutors` if it can hold
  inputs, a `step()` per evaluation, and tests for its error paths.
- **No global state.** Executors belong to an engine.
- **Variables and lookups use `Map`**, never plain objects keyed by program
  strings.
- **Errors are `CodyScriptError`** with a code, a JSON Pointer `path` and the
  node's `tag`. Adding a code is a public API change — document it in README.
- **The JSON format is a contract** with stored rules. A breaking change needs
  a version bump and an upgrade note in README.
- The README's first `json` block is executed by `src/readme.test.ts`; keep it
  runnable.

## Git Workflow

Feature branches and PRs into `main`; never push to `main`. Prefixes:
`feat/`, `fix/`, `refactor/`, `docs/`, `test/`. Releases are cut by
publishing a GitHub release after bumping `version`.

Keep this file accurate. An out-of-date CLAUDE.md is worse than none.
