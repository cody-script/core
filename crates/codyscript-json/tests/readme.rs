//! The README's first `json` example, run as written, so the docs can't
//! drift from the code.

#[path = "../../codyscript-core/tests/common/mod.rs"]
mod common;
mod support;

use codyscript_core::{value, Engine, ExecutorCall, ExecutorResult, RunOptions, Value};
use common::*;

fn text(call: ExecutorCall) -> ExecutorResult {
    let value = match call.input.get("value") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => codyscript_json::to_string(other)?,
    };
    match call.op.as_str() {
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
    let engine = Engine::builder().executor("text", text).build();
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
