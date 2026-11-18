//! The checklist the page runs, in the browser, through the WebAssembly
//! module. Each case is what the page passes to `run` — a program, its run
//! configuration (its variables, and breakpoints) and limits, as JSON text
//! — and what the outcome must contain.
//!
//! A case may also debug its program, through `debug`, with a scripted
//! debugger: it pauses at the configuration's breakpoints, and answers each
//! pause with the next of the script's actions (`continue` when they run
//! out). It must then also pause exactly where, and why, the script's
//! `pauses` say, besides having the outcome expected.
//!
//! Programs may declare (in `ext`) and call the external functions of the
//! page's demo executors, which are JavaScript (they have to be: calling
//! JavaScript is what's being tested):
//!
//! | Function | Does |
//! |---|---|
//! | `text.trim`, `text.empty` | trims `input.value`; is it empty (sync) |
//! | `slow.wait` | waits `arg.ms` on a JavaScript timer, then returns `{ id: arg.id, order }`, `order` counting calls that finished (async) |
//! | `pair.x` | returns its `input` |
//! | `fail.throw`, `fail.reject` | throws an `Error(arg)`; rejects with `arg` |

use codyscript_core::debug::Breakpoint;
use codyscript_core::{RunConfig, Value, value};
use serde::{Serialize, Serializer};

/// One check, as the page receives it.
#[derive(Serialize)]
pub struct Case {
    name: &'static str,
    program: String,
    /// The run configuration, as JSON text.
    #[serde(serialize_with = "json_text")]
    config: RunConfig,
    limits: String,
    /// Debug it, with this script, rather than just run it.
    #[serde(skip_serializing_if = "Option::is_none")]
    debug: Option<Script>,
    /// What the outcome (`{ ok, value, variables, steps }` or `{ ok, error }`)
    /// must contain: every key given here, recursively, with these values.
    /// Debugging a program doesn't change its outcome, so this holds however
    /// it's run — unless it's stopped.
    expect: Value,
}

/// A scripted debugger, for the configuration's breakpoints.
#[derive(Serialize)]
pub struct Script {
    /// How to go on from each pause, in turn.
    actions: Vec<&'static str>,
    /// Where it must pause, in order.
    pauses: Vec<Pause>,
}

/// A pause: at which step, and why (`breakpoint` or `step`).
#[derive(Serialize)]
pub struct Pause {
    path: &'static str,
    reason: &'static str,
}

/// The checklist, as JSON for the page.
pub fn json() -> String {
    codyscript_json::to_string(&cases()).expect("the cases are JSON")
}

fn cases() -> Vec<Case> {
    let readme = value!({ "ext": { "text": ["trim", "empty"] }, "do": [
        { "set": { "name": { "call": { "text.trim": { "value": { "ref": "Name" } } } } } },
        { "if": { "call": { "text.empty": { "value": { "ref": "name" } } } },
          "do": [{ "ret": { "val": { "error": "Name is required" } } }], "tag": "name-required" },
        { "ret": { "ref": "name" } }
    ] });
    let sum = value!({ "do": [
        { "set": { "i": 0 } }, { "set": { "sum": 0 } },
        { "for": { "x": "i < n" }, "do": [{ "set": { "i": { "x": "i + 1" } } }, { "set": { "sum": { "x": "sum + i" } } }] },
        { "ret": { "ref": "sum" } }
    ] });
    let clean_name = value!({
        "ext": { "text": ["trim"] },
        "fn": { "text.clean": { "call": { "text.trim": { "value": { "ref": "value" } } } } },
        "do": [
            { "set": { "name": { "call": { "text.clean": { "value": { "ref": "Name" } } } } } },
            { "ret": { "ref": "name" } }
        ]
    });
    let recursive_sum = value!({ "call": { "sum": { "n": { "ref": "n" } } }, "fn": {
        "sum": { "do": [
            { "if": { "x": "n == 0" }, "do": [{ "ret": 0 }] },
            { "set": { "rest": { "call": { "sum": { "n": { "x": "n - 1" } } } } } },
            { "ret": { "x": "n + rest" } }
        ] }
    } });

    vec![
        // Programs, functions and expressions.
        case("The README rule cleans a name", &readme)
            .variables(value!({ "Name": "  Ada " }))
            .expect(value!({ "ok": true, "value": "Ada", "steps": 8 })),
        case("The README rule refuses a blank name", &readme)
            .variables(value!({ "Name": "   " }))
            .returns(value!({ "error": "Name is required" })),
        case("Expressions: a capped fee", &value!({ "do": [
            { "set": { "fee": { "x": "amount * 0.03" } } },
            { "if": { "x": "fee > 300" }, "do": [{ "set": { "fee": 300 } }] },
            { "ret": { "ref": "fee" } }
        ] }))
            .variables(value!({ "amount": 20000 }))
            .expect(value!({ "ok": true, "value": 300, "variables": { "fee": 300 } })),
        case("Expressions: logic returns an operand", &value!({ "x": "missing_ok || (a + 2) * 3 / 4 > 1 && s" }))
            .variables(value!({ "missing_ok": null, "a": 2, "s": "yes" }))
            .returns(value!("yes")),
        case("A loop: the sum of 1..n", &sum)
            .variables(value!({ "n": 1000 }))
            .returns(value!(500500)),
        case("A CodyScript function calls a JavaScript one, seeing only its arguments", &clean_name)
            .variables(value!({ "Name": "  Ada " }))
            .expect(value!({ "ok": true, "value": "Ada", "variables": { "Name": "  Ada ", "name": "Ada" } })),
        case("Async functions resolve one at a time, in order", &value!({
            "ext": { "pair": ["x"], "slow": ["wait"] },
            "call": { "pair.x": {
                "first": { "call": { "slow.wait": {} }, "arg": { "id": "a", "ms": 30 } },
                "second": { "call": { "slow.wait": {} }, "arg": { "id": "b", "ms": 0 } }
            } }
        }))
            .returns(value!({ "first": { "id": "a", "order": 1 }, "second": { "id": "b", "order": 2 } })),
        // Debugging: where a scripted debugger pauses.
        case("Debugging: with no breakpoints, it never pauses", &sum)
            .variables(value!({ "n": 3 }))
            .debug(&[], &[])
            .returns(value!(6)),
        case("Debugging: a breakpoint pauses before its step, on every pass", &sum)
            .variables(value!({ "n": 3 }))
            .breakpoints([Breakpoint::path("/do/2/do/1")])
            .debug(&[], &[("/do/2/do/1", "breakpoint"), ("/do/2/do/1", "breakpoint"), ("/do/2/do/1", "breakpoint")])
            .returns(value!(6)),
        case("Debugging: a breakpoint names a step by its tag", &readme)
            .variables(value!({ "Name": "Ada" }))
            .breakpoints([Breakpoint::tag("name-required")])
            .debug(&[], &[("/do/1", "breakpoint")])
            .returns(value!("Ada")),
        case("Debugging: step into a function, step over what's left of it", &clean_name)
            .variables(value!({ "Name": "  Ada " }))
            .breakpoints([Breakpoint::path("/do/0")])
            .debug(&["stepInto", "stepInto", "stepInto", "stepOver"], &[
                ("/do/0", "breakpoint"), ("/do/0/set/name", "step"),
                ("/do/0/set/name/call/text.clean/value", "step"), ("/fn/text.clean", "step"), ("/do/1", "step")
            ])
            .returns(value!("Ada")),
        case("Debugging: step over a whole loop, on to the step after it", &sum)
            .variables(value!({ "n": 100 }))
            .breakpoints([Breakpoint::path("/do/2")])
            .debug(&["stepOver"], &[("/do/2", "breakpoint"), ("/do/3", "step")])
            .returns(value!(5050)),
        case("Debugging: stop ends the run", &readme)
            .variables(value!({ "Name": "Ada" }))
            .breakpoints([Breakpoint::tag("name-required")])
            .debug(&["stop"], &[("/do/1", "breakpoint")])
            .fails("aborted", "Run stopped by the debugger at /do/1 (tag name-required)"),
        case("Debugging: a breakpoint that names no step is an error", &readme)
            .breakpoints([Breakpoint::tag("name-required"), Breakpoint::path("/do/9")])
            .debug(&[], &[])
            .fails("invalid_config", r#"Invalid run configuration: No step at "/do/9" at /breakpoints/1"#),
        // Every kind of error.
        case("A missing variable", &readme).expect(value!({ "ok": false, "error": {
            "code": "undefined_variable", "path": "/do/0/set/name/call/text.trim/value"
        } })),
        case("Division by zero", &value!({ "x": "1 / z" }))
            .variables(value!({ "z": 0 }))
            .fails("type_error", r#""/" gave Infinity (division by zero?) in "1 / z" at /x"#),
        case("A throwing function", &value!({ "ext": { "fail": ["throw"] },
            "do": [{ "set": { "x": { "call": { "fail.throw": {} }, "arg": "boom", "tag": "n1" } } }] }))
            .fails("function_failed", r#"Function "fail.throw" failed: boom at /do/0/set/x (tag n1)"#),
        case("A rejecting function", &value!({ "ext": { "fail": ["reject"] }, "call": { "fail.reject": {} }, "arg": "nope" }))
            .fails("function_failed", r#"Function "fail.reject" failed: nope"#),
        case("An endless loop stops at maxSteps", &value!({ "do": [{ "for": true, "do": [], "tag": "spin" }] }))
            .limits(value!({ "maxSteps": 50 }))
            .fails("step_limit", "Program exceeded 50 steps at /do/0/for"),
        case("Endless recursion stops at maxDepth", &value!({
            "call": { "spin": {} },
            "fn": { "spin": { "call": { "spin": {} }, "tag": "again" } }
        }))
            .fails("depth_limit", "Calls nest deeper than 128 levels at /fn/spin (tag again)"),
        case("A function the page doesn't provide is refused before running", &value!({ "ext": { "text": ["trim", "upper"] }, "x": "1" }))
            .fails("unknown_function", r#"Unknown function "text.upper" at /ext/text/1"#),
        case("A function the program doesn't declare is refused", &value!({ "call": { "text.trim": {} } }))
            .fails("invalid_program", r#"Invalid program: Function "text.trim" isn't declared in "ext" at /call/text.trim"#),
        case("An invalid program lists every issue", &value!({ "do": [{ "set": { "": 1 } }, { "ret": [1] }] }))
            .expect(value!({ "ok": false, "error": { "code": "invalid_program", "issues": [
                { "path": "/do/0/set", "message": "Expected a non-empty string" },
                { "path": "/do/1/ret", "message": "Expected an input object" }
            ] } })),
        text_case("Text that isn't JSON", r#"{ "x": "1 + 2" "#.into())
            .expect(value!({ "ok": false, "error": { "code": "invalid_program" } })),
        // Limits of the WebAssembly build: nothing overflows its stack.
        case("The deepest program the default limit allows", &nested_blocks(42)).returns(value!(1)),
        case("One level deeper is refused", &nested_blocks(43))
            .expect(value!({ "ok": false, "error": { "code": "invalid_program" } })),
        text_case("100 000-deep JSON is refused, not a crash", format!("{}1{}", "[".repeat(100_000), "]".repeat(100_000)))
            .expect(value!({ "ok": false, "error": { "code": "invalid_program" } })),
        case("A 100 000-term expression stops at maxSteps, not a crash", &value!({ "x": vec!["1"; 100_000].join("+") }))
            .expect(value!({ "ok": false, "error": { "code": "step_limit" } })),
        case("A 100 000-iteration loop finishes", &sum)
            .variables(value!({ "n": 100_000 }))
            .limits(value!({ "maxSteps": 10_000_000 }))
            .returns(value!(5_000_050_000_u64)),
        case("10 000-deep recursion finishes: calls don't use the stack", &recursive_sum)
            .variables(value!({ "n": 9_999 }))
            .limits(value!({ "maxDepth": 10_000, "maxSteps": 10_000_000 }))
            .returns(value!(49_995_000)),
        case("The module still works after all that", &readme)
            .variables(value!({ "Name": " again " }))
            .returns(value!("again")),
    ]
}

/// Blocks nested `levels` deep, three nesting levels each:
/// `{ "do": [{ "ret": { "do": [ … ] } }] }`.
fn nested_blocks(levels: usize) -> Value {
    let mut program = value!(1);
    for _ in 0..levels {
        program = value!({ "do": [{ "ret": program }] });
    }
    program
}

/// A case running `program` (shown indented) with no variables or limits.
fn case(name: &'static str, program: &Value) -> Case {
    text_case(
        name,
        codyscript_json::to_string_pretty(program).expect("JSON"),
    )
}

/// A case whose program is given as text (which may not even be JSON).
fn text_case(name: &'static str, program: String) -> Case {
    Case {
        name,
        program,
        config: RunConfig::default(),
        limits: "{}".into(),
        debug: None,
        expect: value!({ "ok": true }),
    }
}

impl Case {
    fn variables(mut self, variables: Value) -> Self {
        self.config.variables = variables.into_object().expect("an object");
        self
    }

    fn breakpoints(mut self, breakpoints: impl IntoIterator<Item = Breakpoint>) -> Self {
        self.config.breakpoints = breakpoints.into_iter().collect();
        self
    }

    fn limits(mut self, limits: Value) -> Self {
        self.limits = codyscript_json::to_string(&limits).expect("JSON");
        self
    }

    /// Debugs the program with a script: `actions`, and the `pauses` it
    /// must make, as (path, reason).
    fn debug(mut self, actions: &[&'static str], pauses: &[(&'static str, &'static str)]) -> Self {
        let pauses = pauses.iter().map(|&(path, reason)| Pause { path, reason });
        self.debug = Some(Script {
            actions: actions.to_vec(),
            pauses: pauses.collect(),
        });
        self
    }

    fn expect(mut self, outcome: Value) -> Self {
        self.expect = outcome;
        self
    }

    /// Expects the run to return `value`.
    fn returns(self, value: Value) -> Self {
        self.expect(value!({ "ok": true, "value": value }))
    }

    /// Expects the run to fail with `code` and exactly `message`.
    fn fails(self, code: &str, message: &str) -> Self {
        self.expect(value!({ "ok": false, "error": { "code": code, "message": message } }))
    }
}

/// A run configuration as the page edits it: indented JSON text.
fn json_text<S: Serializer>(config: &RunConfig, serializer: S) -> Result<S::Ok, S::Error> {
    let text = codyscript_json::to_string_pretty(config).map_err(serde::ser::Error::custom)?;
    serializer.serialize_str(&text)
}
