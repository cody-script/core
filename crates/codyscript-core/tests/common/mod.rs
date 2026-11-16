//! Test builders and shared fixtures.
#![allow(dead_code)]

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::time::Duration;

use codyscript_core::{value, Map, Value};
use codyscript_core::{CompiledProgram, Engine, Error, ExecutorCall, ExecutorResult, RunOptions};

// ── Terse builders, so tests read like programs ─────────────────────────────
// (Built directly: `json!` with a non-literal goes through `to_value`, which
// copies, and recurses through, the whole value.)

/// An object with these entries.
pub fn obj<const N: usize>(entries: [(&str, Value); N]) -> Value {
    Value::Object(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    )
}

pub fn val(v: Value) -> Value {
    obj([("val", v)])
}

pub fn var(name: &str) -> Value {
    obj([("ref", name.into())])
}

pub fn x(source: &str) -> Value {
    obj([("x", source.into())])
}

/// A call with arguments (an object) and maybe a static `arg`.
pub fn op_with(exe: &str, id: &str, args: Value, arg: Option<Value>) -> Value {
    let mut node = obj([("op", obj([(&format!("{exe}.{id}"), args)]))]);
    if let Some(arg) = arg {
        node["arg"] = arg;
    }
    node
}

/// A call with arguments.
pub fn op(exe: &str, id: &str, args: Value) -> Value {
    op_with(exe, id, args, None)
}

/// A call with no arguments.
pub fn call(exe: &str, id: &str) -> Value {
    op_with(exe, id, value!({}), None)
}

/// A block.
pub fn m(body: Vec<Value>) -> Value {
    obj([("do", Value::Array(body))])
}

pub fn set(name: &str, to: Value) -> Value {
    obj([("set", obj([(name, to)]))])
}

pub fn ret(input: Value) -> Value {
    obj([("ret", input)])
}

pub fn when(cond: Value, then: Vec<Value>, otherwise: Option<Vec<Value>>) -> Value {
    let mut node = obj([("if", cond), ("do", Value::Array(then))]);
    if let Some(otherwise) = otherwise {
        node["el"] = Value::Array(otherwise);
    }
    node
}

pub fn lp(cond: Value, body: Vec<Value>) -> Value {
    obj([("for", cond), ("do", Value::Array(body))])
}

/// `node` with a `tag`.
pub fn tagged(mut node: Value, tag: &str) -> Value {
    node["tag"] = tag.into();
    node
}

/// An object's keys, in order.
pub fn keys(v: &Value) -> Vec<&str> {
    v.as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect()
}

/// An object's entries, for variables.
pub fn vars(v: Value) -> Map {
    match v {
        Value::Object(map) => map,
        _ => panic!("not an object"),
    }
}

pub fn with_vars(v: Value) -> RunOptions {
    RunOptions::from(vars(v))
}

// ── Executors and engines ───────────────────────────────────────────────────

fn number(v: Option<&Value>) -> f64 {
    v.and_then(Value::as_f64).unwrap_or(f64::NAN)
}

/// A tiny arithmetic/comparison executor: `op("math", "add", { a, b })`.
pub fn math(call: ExecutorCall) -> ExecutorResult {
    let (a, b) = (call.input.get("a"), call.input.get("b"));
    match call.op.as_str() {
        "add" => match (a.and_then(Value::as_i64), b.and_then(Value::as_i64)) {
            (Some(a), Some(b)) => Ok(value!(a + b)),
            _ => Ok(value!(number(a) + number(b))),
        },
        "lt" => Ok(value!(number(a) < number(b))),
        "eq" => Ok(value!(a == b)),
        op => Err(format!("unknown op {op}").into()),
    }
}

/// An engine with the `math` executor.
pub fn engine() -> Engine {
    Engine::builder().executor("math", math).build()
}

pub fn run(engine: &Engine, program: &Value) -> Result<codyscript_core::RunResult, Error> {
    engine.run_blocking(program, RunOptions::new())
}

/// The error `program` fails to compile with.
pub fn compile_error(engine: &Engine, program: &Value) -> Error {
    match engine.compile(program) {
        Ok(_) => panic!("expected compile to fail: {program:?}"),
        Err(error) => error,
    }
}

/// The error a run fails with.
pub fn failure(result: Result<codyscript_core::RunResult, Error>) -> Error {
    match result {
        Ok(result) => panic!("expected the run to fail, got {result:?}"),
        Err(error) => error,
    }
}

pub fn compiled_value(program: &CompiledProgram, options: RunOptions) -> Value {
    program.run_blocking(options).unwrap().value
}

/// A future that finishes after `ms` milliseconds, woken from another thread:
/// a real `Pending`, with no runtime needed.
pub fn sleep(ms: u64) -> impl Future<Output = ()> + Send {
    let state: Arc<Mutex<(bool, Option<Waker>)>> = Arc::new(Mutex::new((false, None)));
    let timer = Arc::clone(&state);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(ms));
        let mut state = timer.lock().unwrap();
        state.0 = true;
        if let Some(waker) = state.1.take() {
            waker.wake();
        }
    });
    std::future::poll_fn(move |cx| {
        let mut state = state.lock().unwrap();
        if state.0 {
            Poll::Ready(())
        } else {
            state.1 = Some(cx.waker().clone());
            Poll::Pending
        }
    })
}

// ── Fixtures: programs both the validator and schema.json must agree on ─────

/// Programs both the engine's validator and `schema.json` must accept (given
/// a `math` executor).
pub fn valid() -> Vec<(&'static str, Value)> {
    vec![
        ("a bare number", value!(5)),
        ("a bare string", value!("text")),
        ("a bare boolean", value!(false)),
        ("null", value!(null)),
        ("a constant object", val(value!({ "a": [1, "x", null] }))),
        (
            "a constant object whose keys look like nodes",
            val(value!({ "ref": "x", "op": {} })),
        ),
        ("a reference", var("x")),
        ("an expression", x("a * 2 > 1 && !done")),
        ("a tagged expression", value!({ "x": "1", "tag": "t" })),
        ("a call with no arguments", call("math", "now")),
        (
            "a call with arguments and arg",
            op_with(
                "math",
                "add",
                value!({ "a": 1, "b": var("x") }),
                Some(value!({ "round": true })),
            ),
        ),
        (
            "an operation whose name has dots",
            call("math", "round.half.up"),
        ),
        ("an empty block", m(vec![])),
        (
            "a loop",
            m(vec![lp(value!(true), vec![value!({ "ret": 1 })])]),
        ),
        (
            "every node type, tagged",
            tagged(
                m(vec![
                    value!({ "set": { "a": { "val": 1, "tag": "v" } }, "tag": "s" }),
                    tagged(
                        when(
                            var("a"),
                            vec![],
                            Some(vec![value!({ "for": false, "do": [], "tag": "f" })]),
                        ),
                        "i",
                    ),
                    value!({
                        "ret": tagged(op("math", "add", value!({ "a": 1, "b": 2 })), "o"),
                        "tag": "r"
                    }),
                ]),
                "root",
            ),
        ),
    ]
}

/// Programs both must refuse, with the path the validator reports first.
pub fn invalid() -> Vec<(&'static str, Value, &'static str)> {
    vec![
        ("an input with no type key", value!({ "rf": "x" }), ""),
        (
            "an input with two type keys",
            value!({ "ref": "a", "val": 1 }),
            "",
        ),
        (
            "an instruction in an input position",
            value!({ "if": 1, "do": [] }),
            "/if",
        ),
        (
            "an instruction with two type keys",
            m(vec![value!({ "if": 1, "for": 1, "do": [] })]),
            "/do/0",
        ),
        (
            "an instruction that is only a do",
            m(vec![value!({ "do": [] })]),
            "/do/0",
        ),
        (
            "an input in an instruction position",
            m(vec![var("x")]),
            "/do/0",
        ),
        ("the 0.2 format", value!({ "$": "ref", "ref": "x" }), "/$"),
        ("a typo'd field", value!({ "val": 1, "tga": "x" }), "/tga"),
        (
            "a missing do on if",
            m(vec![value!({ "if": 1 })]),
            "/do/0/do",
        ),
        (
            "an el on a loop",
            m(vec![value!({ "for": 1, "do": [], "el": [] })]),
            "/do/0/el",
        ),
        (
            "an el that isn't a list",
            m(vec![value!({ "if": 1, "do": [], "el": {} })]),
            "/do/0/el",
        ),
        (
            "an instruction that isn't an object",
            m(vec![value!("set")]),
            "/do/0",
        ),
        (
            "a literal where an instruction belongs",
            m(vec![value!(1)]),
            "/do/0",
        ),
        ("a non-string tag", value!({ "val": 1, "tag": 1 }), "/tag"),
        (
            "a list as a bare input",
            m(vec![set("x", value!([1]))]),
            "/do/0/set/x",
        ),
        (
            "set with no variable",
            m(vec![value!({ "set": {} })]),
            "/do/0/set",
        ),
        (
            "set with two variables",
            m(vec![value!({ "set": { "a": 1, "b": 2 } })]),
            "/do/0/set",
        ),
        (
            "set that isn't an object",
            m(vec![value!({ "set": "x" })]),
            "/do/0/set",
        ),
        (
            "an empty variable name",
            m(vec![set("", value!(1))]),
            "/do/0/set",
        ),
        ("an empty reference", var(""), "/ref"),
        ("an empty expression", x(""), "/x"),
        (
            "an expression that isn't a string",
            value!({ "x": 1 }),
            "/x",
        ),
        (
            "an expression with an extra field",
            value!({ "x": "a", "y": 1 }),
            "/y",
        ),
        ("op with no call", value!({ "op": {} }), "/op"),
        (
            "op with two calls",
            value!({ "op": { "math.a": {}, "math.b": {} } }),
            "/op",
        ),
        (
            "op that isn't an object",
            value!({ "op": "math.add" }),
            "/op",
        ),
        (
            "a call without an operation",
            value!({ "op": { "math.": {} } }),
            "/op/math.",
        ),
        (
            "a call without an executor",
            value!({ "op": { ".add": {} } }),
            "/op/.add",
        ),
        (
            "a call without a dot at all",
            value!({ "op": { "add": {} } }),
            "/op/add",
        ),
        (
            "arguments that are a list",
            value!({ "op": { "math.add": [] } }),
            "/op/math.add",
        ),
        (
            "a bad argument",
            op("math", "add", value!({ "a": [1] })),
            "/op/math.add/a",
        ),
        (
            "the reserved key __proto__",
            value!({ "val": { "__proto__": 1 } }),
            "/val/__proto__",
        ),
        (
            "the reserved key prototype",
            val(value!({ "prototype": 1 })),
            "/val/prototype",
        ),
        (
            "a reserved variable name",
            value!({ "do": [{ "set": { "__proto__": 1 } }] }),
            "/do/0/set/__proto__",
        ),
        (
            "a reserved argument name",
            value!({ "op": { "math.add": { "constructor": 1 } } }),
            "/op/math.add/constructor",
        ),
    ]
}
