//! CodyScript, compiled to WebAssembly, run in a JavaScript engine.
//!
//! `cargo test -p codyscript-wasm --target wasm32-unknown-unknown` runs these
//! in Node; with `WASM_BINDGEN_USE_BROWSER=1` (and a WebDriver), in a
//! headless browser. They cover what differs from native: JavaScript
//! executors (sync, async, throwing) through the bindings, the event loop
//! driving a run, the core's own blocking runner, and stack use under
//! WebAssembly's smaller stack.

#![cfg(target_arch = "wasm32")]

use codyscript_core::{value, Engine, ErrorCode, RunOptions, Signal, Value};
use js_sys::{Function, Object, Reflect};
use wasm_bindgen::JsValue;
use wasm_bindgen_test::wasm_bindgen_test;

// ── Helpers ─────────────────────────────────────────────────────────────────

/// A JavaScript function of `call`.
fn js(body: &str) -> Function {
    Function::new_with_args("call", body)
}

/// An executors object: `{ id: function }`.
fn executors(entries: &[(&str, Function)]) -> JsValue {
    let object = Object::new();
    for (id, function) in entries {
        Reflect::set(&object, &JsValue::from_str(id), function).unwrap();
    }
    object.into()
}

/// Runs through the exported binding; the outcome, parsed.
async fn run(program: &str, variables: &str, executors: JsValue, limits: &str) -> Value {
    let outcome = codyscript_wasm::run(program.into(), variables.into(), executors, limits.into())
        .await
        .unwrap_or_else(|_| panic!("run rejected"));
    codyscript_json::from_str(&outcome).unwrap()
}

fn to_json(value: &Value) -> String {
    codyscript_json::to_string(value).unwrap()
}

const README_RULE: &str = r#"{ "do": [
  { "set": { "name": { "op": { "text.trim": { "value": { "ref": "Name" } } } } } },
  { "if": { "op": { "text.empty": { "value": { "ref": "name" } } } },
    "do": [{ "ret": { "val": { "error": "Name is required" } } }], "tag": "name-required" },
  { "ret": { "ref": "name" } } ] }"#;

fn text_executor() -> Function {
    js(r#"const value = String(call.input.value ?? "");
          switch (call.op) {
            case "trim": return value.trim();
            case "empty": return value.length === 0;
            default: throw new Error(`text has no "${call.op}"`);
          }"#)
}

// ── Through the JavaScript bindings ─────────────────────────────────────────

#[wasm_bindgen_test]
async fn the_readme_rule_runs_with_a_javascript_executor() {
    let exe = || executors(&[("text", text_executor())]);
    let ok = run(README_RULE, r#"{"Name":"  Ada "}"#, exe(), "").await;
    assert_eq!(ok["ok"], true);
    assert_eq!(ok["value"], "Ada");
    assert_eq!(ok["steps"], 8);
    let blank = run(README_RULE, r#"{"Name":"  "}"#, exe(), "").await;
    assert_eq!(blank["value"], value!({ "error": "Name is required" }));
    let missing = run(README_RULE, "", exe(), "").await;
    assert_eq!(missing["ok"], false);
    assert_eq!(missing["error"]["code"], "undefined_variable");
    assert_eq!(
        missing["error"]["path"],
        "/do/0/set/name/op/text.trim/value"
    );
}

#[wasm_bindgen_test]
async fn async_executors_are_awaited_on_the_event_loop_in_order() {
    let slow = js(r#"return new Promise((resolve) => setTimeout(() => {
                       (globalThis.__cody_log ??= []).push(call.arg);
                       resolve(call.arg);
                     }, call.arg === "a" ? 20 : 0));"#);
    let pair = js("return call.input;");
    let program = r#"{ "op": { "pair.x": {
        "first": { "op": { "slow.x": {} }, "arg": "a" },
        "second": { "op": { "slow.x": {} }, "arg": "b" } } } }"#;
    let outcome = run(
        program,
        "",
        executors(&[("slow", slow), ("pair", pair)]),
        "",
    )
    .await;
    assert_eq!(to_json(&outcome["value"]), r#"{"first":"a","second":"b"}"#);
    let log = Reflect::get(&js_sys::global(), &"__cody_log".into()).unwrap();
    assert_eq!(js_sys::JSON::stringify(&log).unwrap(), r#"["a","b"]"#);
}

#[wasm_bindgen_test]
async fn executors_receive_op_arg_and_input_as_plain_objects() {
    let echo = js("return call;");
    let program =
        r#"{ "op": { "echo.do.it": { "a": 1, "b": { "x": "n * 2" } } }, "arg": { "cfg": true } }"#;
    let outcome = run(program, r#"{"n":21}"#, executors(&[("echo", echo)]), "").await;
    assert_eq!(
        to_json(&outcome["value"]),
        r#"{"op":"do.it","arg":{"cfg":true},"input":{"a":1,"b":42}}"#
    );
}

#[wasm_bindgen_test]
async fn what_an_executor_returns_comes_back_as_json() {
    let cases = [
        ("return undefined;", "null"),
        ("return NaN;", "null"),
        ("return () => 1;", "null"),
        (
            "return [1, 'x', null, { a: true }];",
            r#"[1,"x",null,{"a":true}]"#,
        ),
        ("return Promise.resolve(2.5);", "2.5"),
    ];
    for (body, expected) in cases {
        let outcome = run(
            r#"{ "op": { "f.x": {} } }"#,
            "",
            executors(&[("f", js(body))]),
            "",
        )
        .await;
        assert_eq!(to_json(&outcome["value"]), expected, "{body}");
    }
}

#[wasm_bindgen_test]
async fn a_throwing_or_rejecting_executor_fails_the_run() {
    let cases = [
        ("throw new Error('boom');", ": boom"),
        ("return Promise.reject('nope');", ": nope"),
        ("throw { code: 7 };", r#": {"code":7}"#),
    ];
    for (body, suffix) in cases {
        let program = r#"{ "do": [{ "set": { "x": { "op": { "bad.go": {} }, "tag": "n1" } } }] }"#;
        let outcome = run(program, "", executors(&[("bad", js(body))]), "").await;
        let error = &outcome["error"];
        assert_eq!(error["code"], "executor_failed", "{body}");
        assert_eq!(error["path"], "/do/0/set/x");
        assert_eq!(error["tag"], "n1");
        let expected = format!(r#"Executor "bad" failed on "go"{suffix} at /do/0/set/x (tag n1)"#);
        assert_eq!(error["message"], expected.as_str());
    }
}

#[wasm_bindgen_test]
async fn compile_errors_limits_and_expressions() {
    let nope = run(r#"{ "op": { "nope.x": {} } }"#, "", JsValue::NULL, "").await;
    assert_eq!(nope["error"]["code"], "unknown_executor");

    let not_json = run(r#"{ "x": "1 + 2" "#, "", JsValue::NULL, "").await;
    assert_eq!(not_json["error"]["code"], "invalid_program");

    let endless = r#"{ "do": [{ "for": true, "do": [] }] }"#;
    let limited = run(endless, "", JsValue::NULL, r#"{"maxSteps":50}"#).await;
    assert_eq!(
        limited["error"]["message"],
        "Program exceeded 50 steps at /do/0/for"
    );

    let math = run(
        r#"{ "x": "(a + 2) * 3 / 4 > 1 && s == 'ok'" }"#,
        r#"{"a":2,"s":"ok"}"#,
        JsValue::NULL,
        "",
    )
    .await;
    assert_eq!(math["value"], true);
    let divide = run(r#"{ "x": "1 / z" }"#, r#"{"z":0}"#, JsValue::NULL, "").await;
    assert_eq!(
        divide["error"]["message"],
        r#""/" gave Infinity (division by zero?) in "1 / z" at /x"#
    );
}

#[wasm_bindgen_test]
async fn wrong_arguments_reject() {
    let run = |variables: &str, executors: JsValue, limits: &str| {
        codyscript_wasm::run("1".into(), variables.into(), executors, limits.into())
    };
    assert!(run("{", JsValue::NULL, "").await.is_err());
    assert!(run("[1]", JsValue::NULL, "").await.is_err());
    assert!(run("", JsValue::NULL, "nope").await.is_err());
    assert!(run("", JsValue::from(3), "").await.is_err());
    let not_a_function = Object::new();
    Reflect::set(&not_a_function, &"f".into(), &JsValue::from(1)).unwrap();
    assert!(run("", not_a_function.into(), "").await.is_err());
}

#[wasm_bindgen_test]
fn version_and_schema() {
    assert_eq!(codyscript_wasm::version(), env!("CARGO_PKG_VERSION"));
    let schema: Value = codyscript_json::from_str(&codyscript_wasm::schema()).unwrap();
    assert_eq!(schema["title"], "CodyScript program");
}

// ── Stack use: WebAssembly's stack is 1 MiB by default ─────────────────────
//
// Running a program doesn't recurse, however deep it is; compiling does, up
// to `max_depth` levels. Measured on a 1 MiB stack, compile + run handles
// about 590 levels in a debug build (nested `if`s, the costliest shape) and
// 2,000 in release; the default `max_depth` is 128.

/// Blocks nested `levels` deep: `{ "do": [{ "ret": { "do": [ … ] } }] }`,
/// three nesting levels each.
fn nested_blocks(levels: usize) -> Value {
    let mut program = value!(1);
    for _ in 0..levels {
        program = value!({ "do": [{ "ret": program }] });
    }
    program
}

/// `if`s nested `levels` deep, two nesting levels each: the costliest shape
/// per level.
fn nested_ifs(levels: usize) -> Value {
    let mut list = value!([{ "ret": 1 }]);
    for _ in 0..levels {
        list = value!([{ "if": true, "do": list }]);
    }
    value!({ "do": list })
}

#[wasm_bindgen_test]
async fn the_deepest_programs_the_default_limit_allows_run() {
    // 42 blocks is 126 levels; 43 is past both max_depth and the JSON
    // parser's own limit of 128.
    let outcome = run(&to_json(&nested_blocks(42)), "", JsValue::NULL, "").await;
    assert_eq!(outcome["value"], 1);
    let too_deep = run(&to_json(&nested_blocks(43)), "", JsValue::NULL, "").await;
    assert_eq!(too_deep["error"]["code"], "invalid_program");
    let outcome = run(&to_json(&nested_ifs(62)), "", JsValue::NULL, "").await;
    assert_eq!(outcome["value"], 1);
}

#[wasm_bindgen_test]
async fn a_raised_max_depth_fits_the_stack() {
    // 510 levels, within what a debug build handles.
    let engine = Engine::builder().max_depth(512).build();
    let result = engine
        .run(&nested_blocks(170), RunOptions::new())
        .await
        .unwrap();
    assert_eq!(result.value, 1);
    let result = engine
        .run(&nested_ifs(250), RunOptions::new())
        .await
        .unwrap();
    assert_eq!(result.value, 1);
}

#[wasm_bindgen_test]
async fn hostile_input_fails_cleanly_rather_than_overflowing() {
    let chain = vec!["1"; 200_000].join(" + ");
    let outcome = run(&to_json(&value!({ "x": chain })), "", JsValue::NULL, "").await;
    assert_eq!(outcome["error"]["code"], "step_limit");

    let deep_json = format!("{}1{}", "[".repeat(100_000), "]".repeat(100_000));
    let outcome = run(&deep_json, "", JsValue::NULL, "").await;
    assert_eq!(outcome["error"]["code"], "invalid_program");

    let parens = format!("{}1{}", "(".repeat(10_000), ")".repeat(10_000));
    let outcome = run(&to_json(&value!({ "x": parens })), "", JsValue::NULL, "").await;
    assert_eq!(outcome["error"]["code"], "invalid_program");
}

// ── The core, directly ──────────────────────────────────────────────────────

#[wasm_bindgen_test]
fn the_blocking_runner_works_with_sync_executors() {
    let engine = Engine::builder()
        .executor("math", |call| {
            let a = call.input.get("a").and_then(Value::as_f64).unwrap_or(0.0);
            Ok(value!(a * 2.0))
        })
        .build();
    let program = value!({ "do": [
        { "set": { "d": { "op": { "math.double": { "a": { "ref": "n" } } } } } },
        { "ret": { "x": "d + 1" } }
    ] });
    let options = RunOptions::from(value!({ "n": 20 }).into_object().unwrap());
    assert_eq!(engine.run_blocking(&program, options).unwrap().value, 41);
}

#[wasm_bindgen_test]
fn a_signal_aborts_a_run() {
    let signal = Signal::new();
    let engine = Engine::builder()
        .executor("stop", |call| {
            call.signal.expect("the run's signal").abort_with("enough");
            Ok(value!(true))
        })
        .build();
    let program = value!({ "do": [{ "for": { "op": { "stop.x": {} } }, "do": [] }] });
    let error = engine
        .run_blocking(&program, RunOptions::new().with_signal(signal))
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::Aborted);
    assert_eq!(error.cause().unwrap().to_string(), "enough");
}

#[wasm_bindgen_test]
fn maps_and_numbers_behave_the_same() {
    // Map's hash index (past eight keys) and float formatting, under wasm32.
    let map: codyscript_core::Map = (0..50).map(|i| (format!("k{i}"), i)).collect();
    assert_eq!(map.get("k42"), Some(&value!(42)));
    assert_eq!(map.keys().next().map(String::as_str), Some("k0"));
    let v: Value = codyscript_json::from_str("[0.1, 1e21, -0, 18446744073709551615]").unwrap();
    assert_eq!(to_json(&v), "[0.1,1e+21,-0.0,18446744073709551615]");
}
