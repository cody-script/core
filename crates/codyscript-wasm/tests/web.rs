//! CodyScript, compiled to WebAssembly, run in a JavaScript engine.
//!
//! `cargo test -p codyscript-wasm --target wasm32-unknown-unknown` runs these
//! in Node; with `WASM_BINDGEN_USE_BROWSER=1` (and a WebDriver), in a
//! headless browser. They cover what differs from native: JavaScript
//! executors (sync, async, throwing) through the bindings, the event loop
//! driving a run, the core's own blocking runner, and stack use under
//! WebAssembly's smaller stack.

#![cfg(target_arch = "wasm32")]

use codyscript_core::{Engine, ErrorCode, RunOptions, Signal, Value, value};
use js_sys::{Function, Object, Reflect};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::wasm_bindgen_test;

// ── Helpers ─────────────────────────────────────────────────────────────────

/// A JavaScript function of `call`.
fn js(body: &str) -> Function {
    Function::new_with_args("call", body)
}

/// An object of these entries.
fn object(entries: &[(&str, &JsValue)]) -> Object {
    let object = Object::new();
    for (key, value) in entries {
        Reflect::set(&object, &JsValue::from_str(key), value).unwrap();
    }
    object
}

/// An executors object: `{ id: { function: (call) => value, … }, … }`.
fn executors(entries: &[(&str, &[(&str, Function)])]) -> JsValue {
    let executors = Object::new();
    for (id, functions) in entries {
        let functions: Vec<(&str, &JsValue)> = functions
            .iter()
            .map(|(name, f)| (*name, f.as_ref()))
            .collect();
        Reflect::set(&executors, &JsValue::from_str(id), &object(&functions)).unwrap();
    }
    executors.into()
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

const README_RULE: &str = r#"{ "ext": { "text": ["trim", "empty"] }, "do": [
  { "set": { "name": { "call": { "text.trim": { "value": { "ref": "Name" } } } } } },
  { "if": { "call": { "text.empty": { "value": { "ref": "name" } } } },
    "do": [{ "ret": { "val": { "error": "Name is required" } } }], "tag": "name-required" },
  { "ret": { "ref": "name" } } ] }"#;

/// `text.trim` and `text.empty`.
fn text_executor() -> JsValue {
    executors(&[(
        "text",
        &[
            ("trim", js("return String(call.input.value ?? '').trim();")),
            (
                "empty",
                js("return String(call.input.value ?? '').length === 0;"),
            ),
        ],
    )])
}

// ── Through the JavaScript bindings ─────────────────────────────────────────

#[wasm_bindgen_test]
async fn the_readme_rule_runs_with_javascript_functions() {
    let ok = run(README_RULE, r#"{"Name":"  Ada "}"#, text_executor(), "").await;
    assert_eq!(ok["ok"], true);
    assert_eq!(ok["value"], "Ada");
    assert_eq!(ok["steps"], 8);
    let blank = run(README_RULE, r#"{"Name":"  "}"#, text_executor(), "").await;
    assert_eq!(blank["value"], value!({ "error": "Name is required" }));
    let missing = run(README_RULE, "", text_executor(), "").await;
    assert_eq!(missing["ok"], false);
    assert_eq!(missing["error"]["code"], "undefined_variable");
    assert_eq!(
        missing["error"]["path"],
        "/do/0/set/name/call/text.trim/value"
    );
}

#[wasm_bindgen_test]
async fn an_executors_functions_are_the_keys_of_its_object() {
    // `text` provides trim and empty, so a program declaring text.upper is
    // refused when compiled: the host's object is the list.
    let program = r#"{ "ext": { "text": ["trim", "upper"] }, "x": "1" }"#;
    let outcome = run(program, "", text_executor(), "").await;
    assert_eq!(outcome["error"]["code"], "unknown_function");
    assert_eq!(outcome["error"]["path"], "/ext/text/1");
    // A function name may contain dots; the call splits at the first.
    let echo = executors(&[("echo", &[("do.it", js("return call.op;"))])]);
    let program = r#"{ "ext": { "echo": ["do.it"] }, "call": { "echo.do.it": {} } }"#;
    let outcome = run(program, "", echo, "").await;
    assert_eq!(outcome["value"], "do.it");
}

#[wasm_bindgen_test]
async fn async_functions_are_awaited_on_the_event_loop_in_order() {
    let slow = js(r#"return new Promise((resolve) => setTimeout(() => {
                       (globalThis.__cody_log ??= []).push(call.arg);
                       resolve(call.arg);
                     }, call.arg === "a" ? 20 : 0));"#);
    let pair = js("return call.input;");
    let program = r#"{ "ext": { "slow": ["x"], "pair": ["x"] }, "call": { "pair.x": {
        "first": { "call": { "slow.x": {} }, "arg": "a" },
        "second": { "call": { "slow.x": {} }, "arg": "b" } } } }"#;
    let outcome = run(
        program,
        "",
        executors(&[("slow", &[("x", slow)]), ("pair", &[("x", pair)])]),
        "",
    )
    .await;
    assert_eq!(to_json(&outcome["value"]), r#"{"first":"a","second":"b"}"#);
    let log = Reflect::get(&js_sys::global(), &"__cody_log".into()).unwrap();
    assert_eq!(js_sys::JSON::stringify(&log).unwrap(), r#"["a","b"]"#);
}

#[wasm_bindgen_test]
async fn functions_receive_op_arg_and_input_as_plain_objects() {
    let echo = executors(&[("echo", &[("it", js("return call;"))])]);
    let program = r#"{ "ext": { "echo": ["it"] },
        "call": { "echo.it": { "a": 1, "b": { "x": "n * 2" } } }, "arg": { "cfg": true } }"#;
    let outcome = run(program, r#"{"n":21}"#, echo, "").await;
    assert_eq!(
        to_json(&outcome["value"]),
        r#"{"op":"it","arg":{"cfg":true},"input":{"a":1,"b":42}}"#
    );
    // A tagged call says which it is.
    let echo = executors(&[("echo", &[("it", js("return call;"))])]);
    let program = r#"{ "ext": { "echo": ["it"] }, "call": { "echo.it": {} }, "tag": "b7" }"#;
    let outcome = run(program, "", echo, "").await;
    assert_eq!(
        to_json(&outcome["value"]),
        r#"{"op":"it","input":{},"tag":"b7"}"#
    );
}

#[wasm_bindgen_test]
async fn javascript_and_codyscript_functions_call_each_other() {
    let program = r#"{
        "ext": { "text": ["trim"] },
        "fn": { "text.clean": { "call": { "text.trim": { "value": { "ref": "value" } } } } },
        "call": { "text.clean": { "value": { "ref": "Name" } } } }"#;
    let outcome = run(program, r#"{"Name":"  Ada "}"#, text_executor(), "").await;
    assert_eq!(outcome["value"], "Ada");
}

#[wasm_bindgen_test]
async fn what_a_function_returns_comes_back_as_json() {
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
            r#"{ "ext": { "f": ["x"] }, "call": { "f.x": {} } }"#,
            "",
            executors(&[("f", &[("x", js(body))])]),
            "",
        )
        .await;
        assert_eq!(to_json(&outcome["value"]), expected, "{body}");
    }
}

#[wasm_bindgen_test]
async fn a_throwing_or_rejecting_function_fails_the_run() {
    let cases = [
        ("throw new Error('boom');", ": boom"),
        ("return Promise.reject('nope');", ": nope"),
        ("throw { code: 7 };", r#": {"code":7}"#),
    ];
    for (body, suffix) in cases {
        let program = r#"{ "ext": { "bad": ["go"] },
            "do": [{ "set": { "x": { "call": { "bad.go": {} }, "tag": "n1" } } }] }"#;
        let outcome = run(program, "", executors(&[("bad", &[("go", js(body))])]), "").await;
        let error = &outcome["error"];
        assert_eq!(error["code"], "function_failed", "{body}");
        assert_eq!(error["path"], "/do/0/set/x");
        assert_eq!(error["tag"], "n1");
        let expected = format!(r#"Function "bad.go" failed{suffix} at /do/0/set/x (tag n1)"#);
        assert_eq!(error["message"], expected.as_str());
    }
}

#[wasm_bindgen_test]
async fn compile_errors_limits_and_expressions() {
    let nope = r#"{ "ext": { "nope": ["x"] }, "call": { "nope.x": {} } }"#;
    let nope = run(nope, "", JsValue::NULL, "").await;
    assert_eq!(nope["error"]["code"], "unknown_function");
    let undeclared = run(
        r#"{ "call": { "text.trim": {} } }"#,
        "",
        text_executor(),
        "",
    )
    .await;
    assert_eq!(
        undeclared["error"]["message"],
        r#"Invalid program: Function "text.trim" isn't declared in "ext" at /call/text.trim"#
    );

    let not_json = run(r#"{ "x": "1 + 2" "#, "", JsValue::NULL, "").await;
    assert_eq!(not_json["error"]["code"], "invalid_program");

    let endless = r#"{ "do": [{ "for": true, "do": [] }] }"#;
    let limited = run(endless, "", JsValue::NULL, r#"{"max_steps":50}"#).await;
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
        r#""/" divided by zero in "1 / z" at /x"#
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
    // An executor must be an object of functions: not a number, not a
    // function (the 0.5 shape), and each of its values a function.
    let number = object(&[("f", &JsValue::from(1))]);
    assert!(run("", number.into(), "").await.is_err());
    let function = object(&[("f", js("return 1;").as_ref())]);
    assert!(run("", function.into(), "").await.is_err());
    let not_a_function = object(&[("f", object(&[("x", &JsValue::from(1))]).as_ref())]);
    assert!(run("", not_a_function.into(), "").await.is_err());
}

// ── Compiled once, run many times ───────────────────────────────────────────

/// Runs a compiled program; the outcome, parsed.
/// The README's rule, with a type for what it's given, and tests of it.
const TESTED_RULE: &str = r#"{
  "ext": { "text": ["trim", "empty"] },
  "types": { "Row": { "obj": { "Name": "string" } } },
  "tests": {
    "trims the name": { "do": [{ "expect": { "run": { "Name": " Ada " } }, "eq": "Ada" }] },
    "a blank name is refused": { "do": [
      { "expect": { "run": { "Name": "  " } }, "eq": { "val": { "error": "Name is required" } }, "tag": "blank" }
    ] },
    "a row is a Row": { "do": [{ "expect": { "is": { "val": { "Name": "a" } }, "type": "Row" } }] }
  },
  "do": [
    { "set": { "name": { "call": { "text.trim": { "value": { "ref": "Name" } } } } } },
    { "if": { "call": { "text.empty": { "value": { "ref": "name" } } } },
      "do": [{ "ret": { "val": { "error": "Name is required" } } }] },
    { "ret": { "ref": "name" } } ] }"#;

#[wasm_bindgen_test]
async fn a_programs_tests_are_listed_and_run_with_javascript_functions() {
    let listed =
        codyscript_wasm::tests(TESTED_RULE.into(), text_executor(), String::new()).unwrap();
    assert_eq!(
        codyscript_json::from_str::<Value>(&listed).unwrap(),
        value!({ "ok": true, "tests": ["trims the name", "a blank name is refused", "a row is a Row"] })
    );
    for name in [
        "trims the name",
        "a blank name is refused",
        "a row is a Row",
    ] {
        let outcome = codyscript_wasm::test(
            TESTED_RULE.into(),
            name.into(),
            String::new(),
            text_executor(),
            String::new(),
        )
        .await
        .unwrap_or_else(|_| panic!("test rejected"));
        let outcome: Value = codyscript_json::from_str(&outcome).unwrap();
        assert_eq!(outcome["ok"], true, "{name}: {outcome:?}");
    }
    // A test that fails says where, and why.
    let failing = TESTED_RULE.replace("\"eq\": \"Ada\"", "\"eq\": \"Bob\"");
    let outcome = codyscript_wasm::test(
        failing,
        "trims the name".into(),
        String::new(),
        text_executor(),
        String::new(),
    )
    .await
    .unwrap_or_else(|_| panic!("test rejected"));
    let outcome: Value = codyscript_json::from_str(&outcome).unwrap();
    assert_eq!(outcome["error"]["code"], "expectation_failed");
    assert_eq!(outcome["error"]["path"], "/tests/trims the name/do/0");
    // No such test: the arguments are wrong.
    assert!(
        codyscript_wasm::test(
            TESTED_RULE.into(),
            "nope".into(),
            String::new(),
            text_executor(),
            String::new(),
        )
        .await
        .is_err()
    );
}

#[wasm_bindgen_test]
fn values_are_checked_against_a_programs_types() {
    let checked = codyscript_wasm::problems(
        TESTED_RULE.into(),
        "Row".into(),
        r#"{ "Name": 1, "Age": 3 }"#.into(),
        text_executor(),
        String::new(),
    )
    .unwrap();
    assert_eq!(
        codyscript_json::from_str::<Value>(&checked).unwrap(),
        value!({ "ok": true, "problems": [
            { "path": "/Age", "message": "Unknown field \"Age\"" },
            { "path": "/Name", "message": "Expected a string, got number" }
        ] })
    );
    assert!(
        codyscript_wasm::problems(
            TESTED_RULE.into(),
            "Nope".into(),
            "1".into(),
            text_executor(),
            String::new(),
        )
        .is_err()
    );
}

async fn run_compiled(program: &codyscript_wasm::Program, variables: &str) -> Value {
    let outcome = JsFuture::from(program.run(variables.into()))
        .await
        .unwrap_or_else(|_| panic!("run rejected"));
    codyscript_json::from_str(&outcome.as_string().unwrap()).unwrap()
}

/// A property of a thrown JavaScript value.
fn property(thrown: &JsValue, key: &str) -> JsValue {
    Reflect::get(thrown, &key.into()).unwrap()
}

#[wasm_bindgen_test]
async fn a_compiled_program_runs_many_times() {
    let exe = text_executor();
    let program = codyscript_wasm::compile(README_RULE, exe.clone(), "").unwrap();
    for (name, expected) in [("  Ada ", value!("Ada")), ("Grace", value!("Grace"))] {
        let variables = to_json(&value!({ "Name": name }));
        let outcome = run_compiled(&program, &variables).await;
        assert_eq!(outcome["value"], expected);
        // The same outcome as compiling and running in one go.
        assert_eq!(outcome, run(README_RULE, &variables, exe.clone(), "").await);
    }
    let blank = run_compiled(&program, r#"{"Name":"  "}"#).await;
    assert_eq!(blank["value"], value!({ "error": "Name is required" }));
    // Errors while running resolve, as from `run`.
    let missing = run_compiled(&program, "").await;
    assert_eq!(missing["ok"], false);
    assert_eq!(missing["error"]["code"], "undefined_variable");
}

#[wasm_bindgen_test]
async fn compiling_throws_a_codyscript_error_for_an_invalid_program() {
    let nope = r#"{ "ext": { "nope": ["x"] }, "call": { "nope.x": {} } }"#;
    let Err(unknown) = codyscript_wasm::compile(nope, JsValue::NULL, "") else {
        panic!("compiled");
    };
    assert!(unknown.is_instance_of::<js_sys::Error>());
    assert_eq!(property(&unknown, "name"), "CodyScriptError");
    assert_eq!(property(&unknown, "code"), "unknown_function");
    assert_eq!(property(&unknown, "path"), "/ext/nope/0");
    assert_eq!(
        property(&unknown, "message"),
        r#"Unknown function "nope.x" at /ext/nope/0"#
    );
    let issues = js_sys::JSON::stringify(&property(&unknown, "issues")).unwrap();
    assert_eq!(
        issues,
        r#"[{"path":"/ext/nope/0","message":"Unknown function \"nope.x\""}]"#
    );

    let Err(not_json) = codyscript_wasm::compile("{", JsValue::NULL, "") else {
        panic!("compiled");
    };
    assert_eq!(property(&not_json, "code"), "invalid_program");
}

#[wasm_bindgen_test]
async fn wrong_arguments_to_a_compiled_program_throw_or_reject() {
    // Limits that aren't JSON, executors that aren't objects of functions:
    // a plain Error, with no CodyScript code.
    let Err(limits) = codyscript_wasm::compile("1", JsValue::NULL, "nope") else {
        panic!("compiled");
    };
    assert!(property(&limits, "code").is_undefined());
    assert!(codyscript_wasm::compile("1", JsValue::from(3), "").is_err());

    let program = codyscript_wasm::compile("1", JsValue::NULL, "").unwrap();
    assert!(JsFuture::from(program.run("[1]".into())).await.is_err());
    assert_eq!(run_compiled(&program, "").await["value"], 1);
}

#[wasm_bindgen_test]
async fn runs_of_one_program_may_overlap_and_outlive_it() {
    let slow = js(r#"return new Promise((resolve) =>
                       setTimeout(() => resolve(call.input.n), call.input.n));"#);
    let program = codyscript_wasm::compile(
        r#"{ "ext": { "slow": ["x"] }, "call": { "slow.x": { "n": { "ref": "n" } } } }"#,
        executors(&[("slow", &[("x", slow)])]),
        "",
    )
    .unwrap();
    let first = JsFuture::from(program.run(r#"{"n":30}"#.into()));
    let second = JsFuture::from(program.run(r#"{"n":1}"#.into()));
    // Freed while both are in flight: they keep what they need.
    drop(program);
    let value = |outcome: Result<JsValue, JsValue>| {
        let outcome: Value =
            codyscript_json::from_str(&outcome.unwrap().as_string().unwrap()).unwrap();
        outcome["value"].clone()
    };
    assert_eq!(value(second.await), 1);
    assert_eq!(value(first.await), 30);
}

#[wasm_bindgen_test]
fn version_and_schema() {
    assert_eq!(codyscript_wasm::version(), env!("CARGO_PKG_VERSION"));
    let schema: Value = codyscript_json::from_str(&codyscript_wasm::schema()).unwrap();
    assert_eq!(schema["title"], "CodyScript program");
}

// ── Debugging, through the JavaScript bindings ──────────────────────────────

/// A JavaScript function of `pause`.
fn on_pause(body: &str) -> Function {
    Function::new_with_args("pause", body)
}

/// Debugs through the exported binding, with a run configuration (JSON
/// text); the outcome, parsed, or why it rejected.
async fn debug(program: &str, config: &str, on_pause: Function) -> Result<Value, String> {
    let outcome = codyscript_wasm::debug(
        program.into(),
        config.into(),
        text_executor(),
        "".into(),
        on_pause,
    )
    .await
    .map_err(|e| String::from(js_sys::Error::from(JsValue::from(e)).message()))?;
    Ok(codyscript_json::from_str(&outcome).unwrap())
}

/// What `globalThis[name]` holds, as a CodyScript value.
fn global(name: &str) -> Value {
    let value = Reflect::get(&js_sys::global(), &name.into()).unwrap();
    let json = js_sys::JSON::stringify(&value).unwrap();
    codyscript_json::from_str(&String::from(json)).unwrap()
}

#[wasm_bindgen_test]
async fn javascript_sets_breakpoints_and_steps_taking_its_time() {
    // At the breakpoint, a step over, answered later, from a timer, as a
    // person clicking a button would — clearing the breakpoints too.
    let debugger = on_pause(
        r#"(globalThis.__cody_pauses ??= []).push(pause);
           if (pause.reason === "step") return "continue";
           return new Promise((resolve) =>
             setTimeout(() => resolve({ action: "step_over", breakpoints: [] }), 5));"#,
    );
    // The `if`, by its tag.
    let config =
        r#"{ "variables": { "Name": "  Ada " }, "breakpoints": [{ "tag": "name-required" }] }"#;
    let outcome = debug(README_RULE, config, debugger).await.unwrap();
    assert_eq!(outcome["ok"], true);
    assert_eq!(outcome["value"], "Ada");
    assert_eq!(outcome["steps"], 8);
    let pauses = global("__cody_pauses");
    assert_eq!(pauses.as_array().unwrap().len(), 2);
    assert_eq!(pauses[0]["path"], "/do/1");
    assert_eq!(pauses[0]["reason"], "breakpoint");
    assert_eq!(pauses[0]["kind"], "if");
    assert_eq!(pauses[0]["tag"], "name-required");
    assert_eq!(pauses[0]["depth"], 0);
    assert_eq!(
        pauses[0]["breakpoints"],
        value!([{ "tag": "name-required" }])
    );
    assert_eq!(
        pauses[0]["frames"],
        value!([{ "scopes": [{ "Name": "  Ada ", "name": "Ada" }] }])
    );
    assert_eq!(pauses[1]["path"], "/do/2");
    assert_eq!(pauses[1]["reason"], "step");
    assert_eq!(pauses[1]["breakpoints"], value!([]));
}

#[wasm_bindgen_test]
async fn with_no_breakpoints_it_never_pauses() {
    let never = on_pause("throw new Error('unreachable');");
    let config = r#"{ "variables": { "Name": "  Ada " } }"#;
    let outcome = debug(README_RULE, config, never).await;
    assert_eq!(outcome.unwrap()["value"], "Ada");
}

#[wasm_bindgen_test]
async fn stepping_into_a_codyscript_function_shows_the_call_stack() {
    let program = r#"{
        "fn": { "double": { "do": [{ "ret": { "x": "v * 2" } }] } },
        "do": [{ "ret": { "call": { "double": { "v": { "ref": "n" } } } } }] }"#;
    let debugger = on_pause(
        r#"(globalThis.__cody_stack ??= []).push(pause.frames);
           return pause.path.startsWith("/fn/") ? "continue" : "step_into";"#,
    );
    let config = r#"{ "variables": { "n": 21 }, "breakpoints": [{ "path": "/do/0" }] }"#;
    let outcome = debug(program, config, debugger).await.unwrap();
    assert_eq!(outcome["value"], 42);
    let stacks = global("__cody_stack");
    let last = &stacks[stacks.as_array().unwrap().len() - 1];
    assert_eq!(
        *last,
        value!([
            { "scopes": [{ "n": 21 }] },
            { "function": "double", "call": "/do/0/ret", "scopes": [{ "v": 21 }] }
        ])
    );
}

#[wasm_bindgen_test]
async fn stop_ends_the_run_with_aborted() {
    let config = r#"{ "variables": { "Name": "x" }, "breakpoints": [{ "path": "/do/0" }] }"#;
    let outcome = debug(README_RULE, config, on_pause(r#"return "stop";"#))
        .await
        .unwrap();
    assert_eq!(outcome["ok"], false);
    assert_eq!(outcome["error"]["code"], "aborted");
    assert_eq!(
        outcome["error"]["message"],
        "Run stopped by the debugger at /do/0"
    );
}

#[wasm_bindgen_test]
async fn a_failing_or_confused_debugger_rejects() {
    let at = r#"{ "breakpoints": [{ "path": "/do/0" }] }"#;
    let thrown = debug(README_RULE, at, on_pause("throw new Error('oops');")).await;
    assert_eq!(thrown.unwrap_err(), "onPause: oops");
    let rejected = debug(README_RULE, at, on_pause("return Promise.reject('no');")).await;
    assert_eq!(rejected.unwrap_err(), "onPause: no");
    for (body, got) in [
        ("return 'jump';", r#""jump""#),
        ("return;", "undefined"),
        ("return { breakpoints: [] };", r#"{"breakpoints":[]}"#),
    ] {
        let confused = debug(README_RULE, at, on_pause(body)).await.unwrap_err();
        assert_eq!(
            confused,
            format!(
                r#"onPause: expected "step_over", "step_into", "continue", "stop" or {{ action, breakpoints? }}, got {got}"#
            ),
            "{body}"
        );
    }
    // A program that doesn't compile never pauses: it's an outcome.
    let never = on_pause("throw new Error('unreachable');");
    let outcome = debug(r#"{ "x": "1 +" }"#, at, never).await.unwrap();
    assert_eq!(outcome["error"]["code"], "invalid_program");
}

#[wasm_bindgen_test]
async fn a_run_configuration_must_fit_its_program() {
    // A breakpoint that names no step: an outcome, listing each.
    let never = || on_pause("throw new Error('unreachable');");
    let config = r#"{ "breakpoints": [{ "tag": "name-requird" }, { "path": "/do/1" }, { "path": "/do/9" }] }"#;
    let outcome = debug(README_RULE, config, never()).await.unwrap();
    assert_eq!(outcome["ok"], false);
    assert_eq!(outcome["error"]["code"], "invalid_config");
    assert_eq!(
        outcome["error"]["issues"],
        value!([
            { "path": "/breakpoints/0", "message": "No step has tag \"name-requird\"" },
            { "path": "/breakpoints/2", "message": "No step at \"/do/9\"" }
        ])
    );
    // `check` says the same without running it.
    let checked = codyscript_wasm::check(
        README_RULE.into(),
        config.into(),
        text_executor(),
        "".into(),
    );
    let checked: Value = codyscript_json::from_str(&checked.unwrap()).unwrap();
    assert_eq!(checked["error"], outcome["error"]);
    let fine = r#"{ "breakpoints": [{ "tag": "name-required" }] }"#;
    let checked =
        codyscript_wasm::check(README_RULE.into(), fine.into(), text_executor(), "".into());
    assert_eq!(checked.unwrap(), r#"{"ok":true}"#);
    // Or one set at a pause: the run fails when it goes on.
    let bad = on_pause(r#"return { action: "continue", breakpoints: [{ path: "/nope" }] };"#);
    let outcome = debug(README_RULE, at_start(), bad).await.unwrap();
    assert_eq!(outcome["error"]["code"], "invalid_config");
    // What isn't a run configuration at all is a wrong argument.
    for config in [
        "[]",
        r#"{ "breakpoints": ["/do/1"] }"#,
        r#"{ "limits": {} }"#,
    ] {
        let rejected = debug(README_RULE, config, never()).await.unwrap_err();
        assert!(rejected.starts_with("config: "), "{config}: {rejected}");
    }
    let schema: Value = codyscript_json::from_str(&codyscript_wasm::run_config_schema()).unwrap();
    assert_eq!(schema["title"], "CodyScript run configuration");
}

/// A run configuration with a breakpoint at the first step.
fn at_start() -> &'static str {
    r#"{ "breakpoints": [{ "path": "/do/0" }] }"#
}

#[wasm_bindgen_test]
fn steps_lists_what_breakpoints_can_name() {
    let listed = codyscript_wasm::steps(README_RULE.into(), text_executor(), "".into()).unwrap();
    let listed: Value = codyscript_json::from_str(&listed).unwrap();
    assert_eq!(listed["ok"], true);
    let steps = listed["steps"].as_array().unwrap();
    // Every step, taken or not: a name that isn't blank skips the two of
    // the `if`'s `ret`.
    assert_eq!(steps.len(), 10);
    assert_eq!(
        steps[0],
        value!({ "path": "/do/0", "kind": "set", "name": "name", "depth": 0 })
    );
    assert_eq!(
        steps[1],
        value!({ "path": "/do/0/set/name", "kind": "call", "name": "text.trim", "depth": 1 })
    );
    assert_eq!(steps[3]["tag"], "name-required");
    let broken = codyscript_wasm::steps(
        r#"{ "ext": { "nope": ["x"] }, "x": "1" }"#.into(),
        JsValue::NULL,
        "".into(),
    )
    .unwrap();
    let broken: Value = codyscript_json::from_str(&broken).unwrap();
    assert_eq!(broken["error"]["code"], "unknown_function");
    assert!(codyscript_wasm::steps("1".into(), JsValue::NULL, "nope".into()).is_err());
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

#[wasm_bindgen_test]
async fn deep_recursion_doesnt_use_the_stack() {
    // Calls are entries on the machine's own stacks: 10 000 nested calls
    // need no more of WebAssembly's stack than one.
    let program = value!({ "call": { "sum": { "n": 9_999 } }, "fn": {
        "sum": { "do": [
            { "if": { "x": "n == 0" }, "do": [{ "ret": 0 }] },
            { "set": { "rest": { "call": { "sum": { "n": { "x": "n - 1" } } } } } },
            { "ret": { "x": "n + rest" } }
        ] }
    } });
    let limits = r#"{ "max_depth": 10000, "max_steps": 10000000 }"#;
    let outcome = run(&to_json(&program), "", JsValue::NULL, limits).await;
    assert_eq!(outcome["value"], 49_995_000);
    let outcome = run(&to_json(&program), "", JsValue::NULL, "").await;
    assert_eq!(outcome["error"]["code"], "depth_limit");
}

// ── The core, directly ──────────────────────────────────────────────────────

#[wasm_bindgen_test]
fn the_blocking_runner_works_with_sync_executors() {
    let engine = Engine::builder()
        .executor("math", ["double"], |call| {
            let a = call.input.get("a").and_then(Value::as_f64).unwrap_or(0.0);
            Ok(value!(a * 2.0))
        })
        .build();
    let program = value!({ "ext": { "math": ["double"] }, "do": [
        { "set": { "d": { "call": { "math.double": { "a": { "ref": "n" } } } } } },
        { "ret": { "x": "d + 1" } }
    ] });
    let options = RunOptions::from(value!({ "n": 20 }).into_object().unwrap());
    assert_eq!(engine.run_blocking(&program, options).unwrap().value, 41);
}

#[wasm_bindgen_test]
fn a_signal_aborts_a_run() {
    let signal = Signal::new();
    let engine = Engine::builder()
        .executor("stop", ["x"], |call| {
            call.signal()
                .expect("the run's signal")
                .abort_with("enough");
            Ok(value!(true))
        })
        .build();
    let program = value!({ "ext": { "stop": ["x"] },
        "do": [{ "for": { "call": { "stop.x": {} } }, "do": [] }] });
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
    assert_eq!(map.keys().next(), Some("k0"));
    let v: Value = codyscript_json::from_str("[0.1, 1e21, -0, 18446744073709551615]").unwrap();
    assert_eq!(to_json(&v), "[0.1,1e+21,-0.0,18446744073709551615]");
}
