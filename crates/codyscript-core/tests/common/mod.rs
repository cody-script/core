//! Test builders and shared fixtures.
#![allow(
    dead_code,
    reason = "shared by several test crates (and codyscript-json's), each using some"
)]

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::time::Duration;

use codyscript_core::{CompiledProgram, Engine, Error, ExecutorCall, ExecutorResult, RunOptions};
use codyscript_core::{Map, Value, value};

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

/// A call of function `name` (the program's own, or an external one it
/// declares), with arguments (an object).
pub fn call(name: &str, args: Value) -> Value {
    obj([("call", obj([(name, args)]))])
}

/// A call of an external function with a static `arg`.
pub fn call_with(name: &str, args: Value, arg: Value) -> Value {
    obj([("call", obj([(name, args)])), ("arg", arg)])
}

/// `program`, declaring external functions: `ext` at its root.
pub fn with_ext(mut program: Value, ext: Value) -> Value {
    program["ext"] = ext;
    program
}

/// `program`, defining CodyScript functions: `fn` at its root.
pub fn with_fn<const N: usize>(mut program: Value, functions: [(&str, Value); N]) -> Value {
    program["fn"] = obj(functions);
    program
}

/// `program`, declaring every function of the `math` executor.
pub fn uses_math(program: Value) -> Value {
    with_ext(program, obj([("math", MATH.into_iter().collect())]))
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
#[track_caller]
pub fn keys(v: &Value) -> Vec<&str> {
    v.as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect()
}

/// An object's entries, for variables.
#[track_caller]
pub fn vars(v: Value) -> Map {
    let Value::Object(map) = v else {
        panic!("not an object: {v:?}")
    };
    map
}

#[track_caller]
pub fn with_vars<'d>(v: Value) -> RunOptions<'d> {
    RunOptions::from(vars(v))
}

// ── Executors and engines ───────────────────────────────────────────────────

fn number(v: Option<&Value>) -> f64 {
    v.and_then(Value::as_f64).unwrap_or(f64::NAN)
}

/// The functions of the `math` executor.
pub const MATH: [&str; 4] = ["add", "lt", "eq", "round.half.up"];

/// A tiny arithmetic/comparison executor: `call("math.add", { a, b })`.
pub fn math(call: ExecutorCall) -> ExecutorResult {
    let (a, b) = (call.input.get("a"), call.input.get("b"));
    match call.op.as_str() {
        "add" => match (a.and_then(Value::as_i64), b.and_then(Value::as_i64)) {
            (Some(a), Some(b)) => Ok(value!(a + b)),
            _ => Ok(value!(number(a) + number(b))),
        },
        "lt" => Ok(value!(number(a) < number(b))),
        "eq" => Ok(value!(a == b)),
        "round.half.up" => Ok(value!((number(a) + 0.5).floor())),
        op => Err(format!("unknown op {op}").into()),
    }
}

/// An engine with the `math` executor.
pub fn engine() -> Engine {
    Engine::builder().executor("math", MATH, math).build()
}

pub fn run(engine: &Engine, program: &Value) -> Result<codyscript_core::RunResult, Error> {
    engine.run_blocking(program, RunOptions::new())
}

/// The error `program` fails to compile with.
#[track_caller]
pub fn compile_error(engine: &Engine, program: &Value) -> Error {
    match engine.compile(program) {
        Ok(_) => panic!("expected compile to fail: {program:?}"),
        Err(error) => error,
    }
}

/// The error a run fails with.
#[track_caller]
pub fn failure(result: Result<codyscript_core::RunResult, Error>) -> Error {
    match result {
        Ok(result) => panic!("expected the run to fail, got {result:?}"),
        Err(error) => error,
    }
}

#[track_caller]
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
// (What only the validator can check — names a call uses, clashes between
// `fn` and `ext`, `arg` on a CodyScript function, what the host provides — is
// tested in validate.rs.)

/// Programs both the engine's validator and `schema.json` must accept (given
/// the `math` executor).
pub fn valid() -> Vec<(&'static str, Value)> {
    vec![
        ("a bare number", value!(5)),
        ("a bare string", value!("text")),
        ("a bare boolean", value!(false)),
        ("null", value!(null)),
        ("a constant object", val(value!({ "a": [1, "x", null] }))),
        (
            "a constant object whose keys look like nodes",
            val(value!({ "ref": "x", "call": {}, "fn": {}, "ext": [] })),
        ),
        (
            "keys that are special elsewhere (JavaScript's)",
            value!({
                "fn": { "constructor": { "ref": "prototype" } },
                "ext": { "math": ["add"] },
                "do": [
                    { "set": { "__proto__": { "val": { "prototype": 1 } } } },
                    { "ret": { "call": { "math.add": { "constructor": 1 } } } }
                ]
            }),
        ),
        ("a reference", var("x")),
        ("an expression", x("a * 2 > 1 && !done")),
        ("a tagged expression", value!({ "x": "1", "tag": "t" })),
        ("an empty block", m(vec![])),
        (
            "a loop",
            m(vec![lp(value!(true), vec![value!({ "ret": 1 })])]),
        ),
        (
            "a call of an external function with no arguments",
            uses_math(call("math.add", value!({}))),
        ),
        (
            "a call with arguments and arg",
            uses_math(call_with(
                "math.add",
                value!({ "a": 1, "b": var("x") }),
                value!({ "round": true }),
            )),
        ),
        (
            "an external function whose name has dots",
            uses_math(call("math.round.half.up", value!({ "a": 1.5 }))),
        ),
        (
            "external functions declared but not called",
            with_ext(m(vec![]), value!({ "math": ["add", "lt"] })),
        ),
        (
            "an executor declared with no functions",
            with_ext(m(vec![]), value!({ "math": [] })),
        ),
        (
            "every node type, tagged",
            tagged(
                uses_math(m(vec![
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
                        "ret": tagged(call("math.add", value!({ "a": 1, "b": 2 })), "o"),
                        "tag": "r"
                    }),
                ])),
                "root",
            ),
        ),
        (
            "functions calling each other, defined in any order",
            with_fn(
                m(vec![ret(call("double", value!({ "n": 2 })))]),
                [
                    ("double", call("twice", value!({ "n": var("n") }))),
                    ("twice", x("n * 2")),
                ],
            ),
        ),
        (
            "a function with no arguments",
            with_fn(call("one", value!({})), [("one", value!(1))]),
        ),
        (
            "a recursive function, and a tagged root",
            tagged(
                with_fn(
                    call("down", value!({ "n": 3 })),
                    [(
                        "down",
                        m(vec![
                            when(
                                x("n > 0"),
                                vec![ret(tagged(call("down", value!({ "n": x("n - 1") })), "c"))],
                                None,
                            ),
                            ret(var("n")),
                        ]),
                    )],
                ),
                "program",
            ),
        ),
        (
            "a namespaced function beside an executor of the same name",
            with_fn(
                uses_math(call(
                    "math.double",
                    value!({ "a": call("math.add", value!({ "a": 1, "b": 2 })) }),
                )),
                [("math.double", x("a * 2"))],
            ),
        ),
        (
            "functions named like the root's fields",
            with_fn(
                call("call", value!({})),
                [("call", value!(1)), ("fn", value!(2)), ("ext", value!(3))],
            ),
        ),
        (
            "every root field, on an expression",
            value!({ "ext": { "math": ["add"] }, "fn": { "f": 1 }, "x": "1", "tag": "t" }),
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
        (
            "op, which 0.6 replaced with call",
            value!({ "op": { "math.add": {} } }),
            "",
        ),
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
        ("call with no function", value!({ "call": {} }), "/call"),
        (
            "call with two functions",
            uses_math(value!({ "call": { "math.add": {}, "math.lt": {} } })),
            "/call",
        ),
        (
            "call that isn't an object",
            value!({ "call": "math.add" }),
            "/call",
        ),
        (
            "arguments that are a list",
            uses_math(value!({ "call": { "math.add": [] } })),
            "/call/math.add",
        ),
        (
            "a bad argument",
            uses_math(call("math.add", value!({ "a": [1] }))),
            "/call/math.add/a",
        ),
        (
            "an empty argument name",
            with_fn(call("f", value!({ "": 1 })), [("f", value!(1))]),
            "/call/f/",
        ),
        // The root's `fn` and `ext`.
        (
            "functions with no program",
            value!({ "fn": { "f": 1 } }),
            "",
        ),
        (
            "fn that isn't an object",
            value!({ "fn": [], "do": [] }),
            "/fn",
        ),
        (
            "an empty function name",
            with_fn(m(vec![]), [("", value!(1))]),
            "/fn/",
        ),
        (
            "a function that isn't an input",
            with_fn(m(vec![]), [("f", value!([1]))]),
            "/fn/f",
        ),
        (
            "a function that is an instruction",
            with_fn(m(vec![]), [("f", ret(value!(1)))]),
            "/fn/f",
        ),
        (
            "a bad instruction in a function",
            with_fn(m(vec![]), [("f", m(vec![set("", value!(1))]))]),
            "/fn/f/do/0/set",
        ),
        (
            "ext that isn't an object",
            value!({ "ext": [], "do": [] }),
            "/ext",
        ),
        (
            "an executor's functions that aren't a list",
            value!({ "ext": { "math": "add" }, "do": [] }),
            "/ext/math",
        ),
        (
            "an external function name that isn't a string",
            value!({ "ext": { "math": [1] }, "do": [] }),
            "/ext/math/0",
        ),
        (
            "an empty external function name",
            value!({ "ext": { "math": [""] }, "do": [] }),
            "/ext/math/0",
        ),
        (
            "an executor name with a dot",
            value!({ "ext": { "ma.th": ["add"] }, "do": [] }),
            "/ext/ma.th",
        ),
        (
            "an empty executor name",
            value!({ "ext": { "": ["add"] }, "do": [] }),
            "/ext/",
        ),
        (
            "an external function declared twice",
            value!({ "ext": { "math": ["add", "add"] }, "do": [] }),
            "/ext/math/1",
        ),
        (
            "functions defined below the root",
            m(vec![set("f", value!({ "fn": { "g": 1 }, "x": "1" }))]),
            "/do/0/set/f/fn",
        ),
        (
            "external functions declared below the root",
            m(vec![ret(value!({ "ext": { "math": ["add"] }, "do": [] }))]),
            "/do/0/ret/ext",
        ),
        (
            "a main function, which 0.6 has no need of",
            value!({ "fn": { "main": { "do": [] } } }),
            "",
        ),
    ]
}
