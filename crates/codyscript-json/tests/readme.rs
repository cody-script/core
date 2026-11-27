//! The README's first `json` example, and the core README's functions,
//! types and tests examples, run as written, so the docs can't drift from
//! the code.

#[path = "../../codyscript-core/tests/common/mod.rs"]
mod common;
mod support;

use codyscript_core::{Engine, ErrorCode, ExecutorCall, ExecutorResult, RunOptions, Value, value};
use common::*;

fn text(call: ExecutorCall) -> ExecutorResult {
    let value = match call.input.get("value") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => codyscript_json::to_string(other)?,
    };
    match call.op() {
        "trim" => Ok(value.trim().into()),
        "empty" => Ok(value.is_empty().into()),
        op => Err(format!("text has no \"{op}\"").into()),
    }
}

#[test]
fn the_readme_example_works() {
    let blocks = support::readme_json_blocks("README.md");
    let (lang, rule) = blocks.into_iter().next().expect("a json block");
    assert_eq!(lang, "json");
    let engine = Engine::builder()
        .executor("text", ["trim", "empty"], text)
        .build();
    let compiled = engine.compile(&rule).unwrap();
    assert_eq!(
        compiled_value(&compiled, with_vars(value!({ "Name": "  Ada " }))),
        "Ada"
    );
    assert_eq!(
        compiled_value(&compiled, with_vars(value!({ "Name": "  " }))),
        value!({ "error": "Name is required" })
    );
    assert!(compiled.run_blocking(RunOptions::new()).is_err());
}

#[test]
fn the_core_readmes_functions_example_works() {
    let program = support::readme_json_blocks("crates/codyscript-core/README.md")
        .into_iter()
        .map(|(_, json)| json)
        .find(|json| json.get("fn").is_some())
        .expect("a program with functions");
    // Its `ext` is checked against the host: this one provides no `text`.
    let error = Engine::new().compile(&program).unwrap_err();
    assert_eq!(error.code(), ErrorCode::UnknownFunction);
    assert_eq!(error.path(), "/ext/text/0");

    let engine = Engine::builder()
        .executor("text", ["trim", "empty"], text)
        .build();
    let compiled = engine.compile(&program).unwrap();
    let row = |name: &str, amount: i64| with_vars(value!({ "Name": name, "Amount": amount }));
    let result = compiled.run_blocking(row(" Ada ", 1000)).unwrap();
    assert_eq!(result.value, 30);
    // The body's variables come back; the function's `fee` doesn't.
    assert_eq!(
        Value::Object(result.variables),
        value!({ "Name": " Ada ", "Amount": 1000, "name": "Ada" })
    );
    assert_eq!(compiled_value(&compiled, row("Ada", 20000)), 300);
    assert_eq!(
        compiled_value(&compiled, row("  ", 1000)),
        value!({ "error": "Name is required" })
    );
}

/// The core README's first program with `key` at its root.
fn core_readme_program(key: &str) -> Value {
    support::readme_json_blocks("crates/codyscript-core/README.md")
        .into_iter()
        .map(|(_, json)| json)
        .find(|json| json.get(key).is_some())
        .unwrap_or_else(|| panic!("a program with {key}"))
}

#[test]
fn the_core_readmes_types_example_works() {
    let compiled = Engine::new()
        .compile(&core_readme_program("types"))
        .unwrap();
    let todo = value!({ "title": "a", "done": false, "tags": [], "owner": { "name": "b" } });
    let created = compiled_value(&compiled, with_vars(value!({ "body": todo })));
    assert_eq!(created["status"], 201);
    // The body and problems it says.
    let body = value!({ "title": 7, "tags": ["a", 2], "urgent": true });
    let refused = compiled_value(&compiled, with_vars(value!({ "body": body })));
    assert_eq!(
        refused,
        value!({ "status": 400, "problems": [
            { "path": "/urgent", "message": "Unknown field \"urgent\"" },
            { "path": "", "message": "Missing field \"done\"" },
            { "path": "/title", "message": "Expected a string, got number" },
            { "path": "/tags/1", "message": "Expected a string, got number" }
        ] })
    );
}

#[test]
fn the_core_readmes_tests_example_works() {
    let compiled = Engine::new()
        .compile(&core_readme_program("tests"))
        .unwrap();
    assert_eq!(compiled.tests().len(), 3);
    for test in compiled.tests() {
        let outcome = test.run_blocking(RunOptions::new());
        assert!(outcome.is_ok(), "{}: {outcome:?}", test.name());
    }
    // As a program, it charges its fee.
    let charged = compiled_value(&compiled, with_vars(value!({ "Amount": 20000 })));
    assert_eq!(charged, 300);
}
