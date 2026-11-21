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
    // An API handler: the request in, a response (status, headers, body) out.
    let greet = value!({ "ext": { "text": ["trim", "empty"] }, "do": [
        { "set": { "name": { "call": { "text.trim": { "value": { "x": "request.query?.name ?? ''" } } } } } },
        { "if": { "call": { "text.empty": { "value": { "ref": "name" } } } }, "do": [
            { "ret": { "obj": {
                "status": 400,
                "body": { "obj": { "error": "name is required", "field": "query.name" } }
            } } }
        ], "tag": "no-name" },
        { "ret": { "obj": {
            "status": 200,
            "headers": { "val": { "content-type": "application/json" } },
            "body": { "obj": {
                "name": { "ref": "name" },
                "language": { "x": "request.headers?['accept-language'] ?? 'en'" }
            } }
        } } }
    ] });
    // A loop over a list's items: `?[i]` is null past the end (and `?.` of
    // null is null; objects themselves don't compare).
    let order_total = value!({ "do": [
        { "set": { "i": 0 } }, { "set": { "subtotal": 0 } },
        { "for": { "x": "order.lines?[i]?.sku != null" }, "do": [
            { "set": { "subtotal": { "x": "subtotal + order.lines[i].price * order.lines[i].qty" } } },
            { "set": { "i": { "x": "i + 1" } } }
        ] },
        { "set": { "shipping": 5 } },
        { "if": { "x": "subtotal >= 50" }, "do": [{ "set": { "shipping": 0 } }] },
        { "ret": { "obj": {
            "customer": { "x": "order.customer.name" },
            "skus": { "list": [{ "x": "order.lines[0].sku" }, { "x": "order.lines?[1]?.sku" }] },
            "lines": { "ref": "i" },
            "subtotal": { "ref": "subtotal" },
            "shipping": { "ref": "shipping" },
            "total": { "x": "subtotal + shipping" }
        } } }
    ] });
    let order =
        |lines: Value| value!({ "order": { "customer": { "name": "Ada" }, "lines": lines } });
    let profile = value!({ "do": [{ "ret": { "obj": {
        "name": { "x": "user.name" },
        "tags": { "list": [{ "ref": "first" }, { "ref": "second" }] }
    } } }] });

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
        case("Expressions: a default for null, and logic on booleans", &value!({ "x": "(maybe ?? 0) + (a + 2) * 3 / 4 > 1 && ok" }))
            .variables(value!({ "maybe": null, "a": 2, "ok": true }))
            .returns(value!(true)),
        case("Data: read fields and items, build an object and a list", &value!({ "obj": {
            "name": { "x": "request.body.user.name" },
            "nickname": { "x": "request.body.user?.nickname ?? 'none'" },
            "items": { "list": [{ "x": "request.body.items[1]" }, { "x": "request.body.items?[9]" }] }
        } }))
            .variables(value!({ "request": { "body": { "user": { "name": "Ada" }, "items": [1, 2] } } }))
            .returns(value!({ "name": "Ada", "nickname": "none", "items": [2, null] })),
        case("Data: an API handler answers with a status, headers and a body", &greet)
            .variables(value!({ "request": {
                "method": "GET", "query": { "name": "  Ada " }, "headers": { "accept-language": "fr" }
            } }))
            .returns(value!({
                "status": 200,
                "headers": { "content-type": "application/json" },
                "body": { "name": "Ada", "language": "fr" }
            })),
        case("Data: the API handler answers 400 when a field is missing", &greet)
            .variables(value!({ "request": { "method": "GET", "query": {}, "headers": {} } }))
            .returns(value!({ "status": 400, "body": { "error": "name is required", "field": "query.name" } })),
        case("Data: a loop totals an order's lines, to the end of the list", &order_total)
            .variables(order(value!([
                { "sku": "pen", "price": 1.5, "qty": 4 },
                { "sku": "pad", "price": 12, "qty": 2 },
                { "sku": "ink", "price": 20, "qty": 1 }
            ])))
            .returns(value!({
                "customer": "Ada", "skus": ["pen", "pad"], "lines": 3, "subtotal": 50, "shipping": 0, "total": 50
            })),
        case("Data: the same order with one line: ?[1]?.sku is null", &order_total)
            .variables(order(value!([{ "sku": "pen", "price": 1.5, "qty": 4 }])))
            .returns(value!({
                "customer": "Ada", "skus": ["pen", null], "lines": 1, "subtotal": 6, "shipping": 5, "total": 11
            })),
        case("Data: a computed key looks a name up; ?[ for one that may be missing", &value!({ "list": [
            { "x": "prices[first]" },
            { "x": "prices?[second] ?? 0" }
        ] }))
            .variables(value!({ "prices": { "tea": 3, "cake": 4.5 }, "first": "cake", "second": "scone" }))
            .returns(value!([4.5, 0])),
        case("Data: val is a constant; obj works out its fields", &value!({ "list": [
            { "val": { "sum": { "x": "1 + 1" } } },
            { "obj": { "sum": { "x": "1 + 1" } } }
        ] }))
            .returns(value!([{ "sum": { "x": "1 + 1" } }, { "sum": 2 }])),
        case("Data: a CodyScript function returns an object", &value!({
            "fn": { "size": { "obj": { "width": { "ref": "width" }, "height": { "ref": "height" } } } },
            "list": [
                { "call": { "size": { "width": { "x": "image.width" }, "height": { "x": "image.height" } } } },
                { "call": { "size": { "width": { "x": "image.width / 2" }, "height": { "x": "image.height / 2" } } } }
            ]
        }))
            .variables(value!({ "image": { "width": 640, "height": 480 } }))
            .returns(value!([{ "width": 640, "height": 480 }, { "width": 320, "height": 240 }])),
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
            .debug(&["step_into", "step_into", "step_into", "step_over"], &[
                ("/do/0", "breakpoint"), ("/do/0/set/name", "step"),
                ("/do/0/set/name/call/text.clean/value", "step"), ("/fn/text.clean", "step"), ("/do/1", "step")
            ])
            .returns(value!("Ada")),
        case("Debugging: step over a whole loop, on to the step after it", &sum)
            .variables(value!({ "n": 100 }))
            .breakpoints([Breakpoint::path("/do/2")])
            .debug(&["step_over"], &[("/do/2", "breakpoint"), ("/do/3", "step")])
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
        case("Debugging: step into an obj, one field at a time, then over a list", &profile)
            .variables(value!({ "user": { "name": "Ada" }, "first": "admin", "second": "beta" }))
            .breakpoints([Breakpoint::path("/do/0")])
            .debug(&["step_into", "step_into", "step_into", "step_over"], &[
                ("/do/0", "breakpoint"), ("/do/0/ret", "step"),
                ("/do/0/ret/obj/name", "step"), ("/do/0/ret/obj/tags", "step")
            ])
            .returns(value!({ "name": "Ada", "tags": ["admin", "beta"] })),
        // Every kind of error.
        case("A missing variable", &readme).expect(value!({ "ok": false, "error": {
            "code": "undefined_variable", "path": "/do/0/set/name/call/text.trim/value"
        } })),
        case("An if whose test isn't a boolean", &value!({ "do": [{ "if": { "ref": "n" }, "do": [] }] }))
            .variables(value!({ "n": 1 }))
            .fails("type_error", r#""if" needs a boolean, got number at /do/0/if"#),
        case("Division by zero", &value!({ "x": "1 / z" }))
            .variables(value!({ "z": 0 }))
            .fails("type_error", r#""/" divided by zero in "1 / z" at /x"#),
        case("A field that isn't there (a typo)", &value!({ "x": "user.nmae" }))
            .variables(value!({ "user": { "name": "Ada" } }))
            .fails("type_error", r#"Object has no field "nmae" in "user.nmae" at /x"#),
        case("An item past the end of a list, inside an obj", &value!({ "obj": { "items": { "list": [
            { "x": "items[0]" }, { "x": "items[5]" }
        ] } } }))
            .variables(value!({ "items": [1, 2] }))
            .fails("type_error", r#"Index 5 is out of range for a list of 2 in "items[5]" at /obj/items/list/1/x"#),
        case("?. excuses only what's missing, not the wrong type", &value!({ "x": "n?.a" }))
            .variables(value!({ "n": 1 }))
            .fails("type_error", r#"".a" needs an object, got number in "n?.a" at /x"#),
        case("A throwing function", &value!({ "ext": { "fail": ["throw"] },
            "do": [{ "set": { "x": { "call": { "fail.throw": {} }, "arg": "boom", "tag": "n1" } } }] }))
            .fails("function_failed", r#"Function "fail.throw" failed: boom at /do/0/set/x (tag n1)"#),
        case("A rejecting function", &value!({ "ext": { "fail": ["reject"] }, "call": { "fail.reject": {} }, "arg": "nope" }))
            .fails("function_failed", r#"Function "fail.reject" failed: nope"#),
        case("An endless loop stops at max_steps", &value!({ "do": [{ "for": true, "do": [], "tag": "spin" }] }))
            .limits(value!({ "max_steps": 50 }))
            .fails("step_limit", "Program exceeded 50 steps at /do/0/for"),
        case("Endless recursion stops at max_depth", &value!({
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
        case("A 100 000-term expression stops at max_steps, not a crash", &value!({ "x": vec!["1"; 100_000].join("+") }))
            .expect(value!({ "ok": false, "error": { "code": "step_limit" } })),
        case("A 100 000-iteration loop finishes", &sum)
            .variables(value!({ "n": 100_000 }))
            .limits(value!({ "max_steps": 10_000_000 }))
            .returns(value!(5_000_050_000_u64)),
        case("10 000-deep recursion finishes: calls don't use the stack", &recursive_sum)
            .variables(value!({ "n": 9_999 }))
            .limits(value!({ "max_depth": 10_000, "max_steps": 10_000_000 }))
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
