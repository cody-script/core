//! The checklist the page runs, in the browser, through the WebAssembly
//! module. Each case is what the page passes to `run` — a program, its
//! variables and limits, as JSON text — and what the outcome must contain.
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

use codyscript_core::{Value, value};
use serde::Serialize;

/// One check, as the page receives it.
#[derive(Serialize)]
pub struct Case {
    name: &'static str,
    program: String,
    variables: String,
    limits: String,
    /// What the outcome (`{ ok, value, variables, steps }` or `{ ok, error }`)
    /// must contain: every key given here, recursively, with these values.
    expect: Value,
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
        case("A CodyScript function calls a JavaScript one, seeing only its arguments", &value!({
            "ext": { "text": ["trim"] },
            "fn": { "text.clean": { "call": { "text.trim": { "value": { "ref": "value" } } } } },
            "do": [
                { "set": { "name": { "call": { "text.clean": { "value": { "ref": "Name" } } } } } },
                { "ret": { "ref": "name" } }
            ]
        }))
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
        variables: "{}".into(),
        limits: "{}".into(),
        expect: value!({ "ok": true }),
    }
}

impl Case {
    fn variables(mut self, variables: Value) -> Self {
        self.variables = codyscript_json::to_string(&variables).expect("JSON");
        self
    }

    fn limits(mut self, limits: Value) -> Self {
        self.limits = codyscript_json::to_string(&limits).expect("JSON");
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
