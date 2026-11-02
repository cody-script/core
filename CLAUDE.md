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
bun run size            # gzipped bundle bytes; CI budget is 3072
```

## Structure

```
src/
  types.ts    # The language (Value, Input, Instruction) and host types
  validate.ts # One walk: shape, JSON values, depth, reserved keys, executor ids
  engine.ts   # createEngine → compile → run; scopes, steps, errors
  errors.ts   # CodyScriptError and its codes
  index.ts    # Public surface — keep it small
  test/       # Test builders and shared fixtures (not shipped)
schema.json   # JSON Schema for hand-writing programs (shipped)
```

**Zero runtime dependencies** — keep it that way; the validator is
hand-written for exactly this reason (Valibot was ~60% of the bundle). The
family's "use Valibot" rule is for apps, not this library. Must run unchanged
in Workers, Node and Bun — no Node built-ins outside tests.

## Rules

- **Programs are untrusted.** Every new node type needs: its fields in
  `INPUTS`/`INSTRUCTIONS` and a case in `validate.ts`, a case in the engine
  with a `step()` per evaluation, a definition in `schema.json`, and cases in
  `src/test/programs.ts` (run against both the validator and the schema).
- **Programs are compact on purpose.** A node's type is its key (`{ "ref": "x" }`,
  no `"$"`), keys stay short and literals stay bare; payload size matters to
  consumers. Position decides meaning: an object in an input position is
  always a node (constants go in `val`), and a node has exactly one type key.
  Free-form names (variables, calls) live in their own one-key objects so
  they can never collide with `tag` or a type key.
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
`feat/`, `fix/`, `refactor/`, `docs/`, `test/`.

**Releasing is bumping `version` in `package.json` in a PR.** Merging it to
`main` runs `.github/workflows/release.yml`: CI, publish to GitHub Packages,
then tag `v<version>` and create the GitHub release. Bump in any PR that
changes what ships (`src/`, `schema.json`, `package.json`); leave it alone
for docs- or test-only PRs. CI enforces this: a PR touching shipped files
(`src/` outside tests, `schema.json`) fails unless it bumps to an unreleased
version. Never create tags or releases by hand — the workflow treats an
existing tag as "already released".

**Size budget.** CI fails if the minified bundle passes 3 KB gzipped
(`SIZE_BUDGET_BYTES` in `ci.yml`). Raise it only deliberately, saying why in
the PR.

Keep this file accurate. An out-of-date CLAUDE.md is worse than none.
