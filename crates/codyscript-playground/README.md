# codyscript-playground

A page for smoke-testing [`codyscript-wasm`](../codyscript-wasm/README.md) —
CodyScript compiled to WebAssembly — in a real browser.

```bash
rustup target add wasm32-unknown-unknown          # once
cargo run -p codyscript-playground                # then open http://127.0.0.1:8080
```

It compiles `codyscript-wasm` to WebAssembly, generates the JavaScript glue
(with `wasm-bindgen-cli-support`, so no CLI to install), and serves the page
and the module from memory on 127.0.0.1. In the browser:

1. The title should read `codyscript-wasm 0.7.0, 31 cases`.
2. Click **Run all**: every case in the checklist should get a green ✓,
   ending in "✓ all 31 passed". (<http://127.0.0.1:8080/#all> runs them on
   load.) The "Debugging" cases run under a scripted debugger and check
   where it paused.
3. Pick any case to see its program, run configuration (its variables and
   breakpoints), limits and expected outcome; edit them and **Run** to try
   your own.
4. **Debug** it: the program's steps are listed under it — click one to set
   or clear its breakpoint, which is written into the run configuration:
   by the step's tag if it has one (so editing the program doesn't move
   it), else by its path. A breakpoint that names no step is flagged under
   the list. **Debug** runs to the first breakpoint (with none, it just
   runs), then shows where it paused, the call stack and the variables
   (click a frame for its own), with **Step over**, **Step into**,
   **Continue** and **Stop**. Breakpoints can change while it's paused. To
   step from the start, put one on the first step.

`--port <port>` changes the port; `--debug` serves a debug build (faster to
compile, slower to run, and panics show Rust's message in the console).

It's all Rust but the page:

| File | |
|---|---|
| `src/main.rs` | What it's for, arguments, startup. |
| `src/wasm.rs` | Compiles `codyscript-wasm` to wasm32 and generates its glue. |
| `src/server.rs` | Serves everything, with `axum`. |
| `src/cases.rs` | The checklist: programs and the outcomes to expect. Add cases here. |
| `src/index.html` | The page: demo executors, running, comparing, showing; the debugger's buttons and panels. |

It's its own crate, not a binary in `codyscript-wasm`, so that `axum`,
`tokio` and `wasm-bindgen-cli-support` are never dependencies of the
WebAssembly crate. It doesn't depend on `codyscript-wasm` either: it builds
it, by running cargo, into `target/playground`.
