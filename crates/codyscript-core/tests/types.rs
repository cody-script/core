//! Types: what values a program says it takes, and checking values against
//! them — with `is` and `problems` in a program, and
//! `CompiledProgram::problems` from a host.

mod common;

use codyscript_core::{Engine, ErrorCode, Issue, Limits, RunOptions, Value, value};
use common::*;

fn issue(path: &str, message: &str) -> Issue {
    Issue {
        path: path.into(),
        message: message.into(),
    }
}

/// `value` checked against `ty` in a program declaring `types`: whether it
/// is one, and its problems as the program sees them.
fn check(types: Value, value: Value, ty: Value) -> (bool, Value) {
    let program = value!({
        "types": types,
        "do": [
            { "set": { "is": { "is": { "val": (value.clone()) }, "type": (ty.clone()) } } },
            { "set": { "problems": { "problems": { "val": value }, "type": ty } } }
        ]
    });
    let result = run(&engine(), &program).unwrap();
    let is = result.variables.get("is").and_then(Value::as_bool).unwrap();
    (is, result.variables.get("problems").cloned().unwrap())
}

fn is(value: Value, ty: &str) -> bool {
    check(value!({}), value, value!(ty)).0
}

#[test]
fn built_in_types_take_what_their_names_say() {
    assert!(is(value!(1), "number") && is(value!(1.5), "number"));
    // A whole number, written either way.
    assert!(is(value!(2), "integer") && is(value!(2.0), "integer"));
    assert!(!is(value!(2.5), "integer"));
    assert!(is(value!("a"), "string") && !is(value!(1), "string"));
    assert!(is(value!(false), "boolean") && !is(value!(0), "boolean"));
    assert!(is(value!([1, "a"]), "list") && !is(value!({}), "list"));
    assert!(is(value!({ "a": 1 }), "object") && !is(value!([]), "object"));
    assert!(is(value!(null), "null") && !is(value!(0), "null"));
    assert!(is(value!(null), "any") && is(value!([{}]), "any"));
    // `?`: or null.
    assert!(is(value!(null), "string?") && is(value!("a"), "string?"));
    assert!(!is(value!(null), "string"));
}

#[test]
fn objects_have_their_fields_and_no_others_lists_and_maps_their_items() {
    let types = value!({
        "Todo": { "obj": {
            "id": "integer",
            "title": "string",
            "done": "boolean",
            "tags": { "list": "string" },
            "due": "string?"
        } },
        "Scores": { "map": "number" }
    });
    let todo = value!({ "id": 1, "title": "a", "done": false, "tags": ["x"] });
    // An optional field may be left out, or null.
    assert_eq!(
        check(types.clone(), todo, value!("Todo")),
        (true, value!([]))
    );
    let (is, problems) = check(
        types.clone(),
        value!({ "id": 1.5, "title": 3, "tags": ["a", 2], "due": null, "extra": true }),
        value!("Todo"),
    );
    assert!(!is);
    // Fields it shouldn't have, then those it lacks, then each it has, in
    // the type's order.
    assert_eq!(
        problems,
        value!([
            { "path": "/extra", "message": "Unknown field \"extra\"" },
            { "path": "", "message": "Missing field \"done\"" },
            { "path": "/id", "message": "Expected an integer, got number" },
            { "path": "/title", "message": "Expected a string, got number" },
            { "path": "/tags/1", "message": "Expected a string, got number" }
        ])
    );
    assert_eq!(
        check(types, value!({ "a": 1, "b/c": "x" }), value!("Scores")).1,
        value!([{ "path": "/b~1c", "message": "Expected a number, got string" }])
    );
}

#[test]
fn types_name_each_other_and_themselves_and_say_so_when_a_value_isnt_one() {
    let types = value!({
        "Person": { "obj": {
            "name": "string",
            "friends": { "list": "Person" },
            "boss": "Person?"
        } }
    });
    let deep = value!({
        "name": "a",
        "friends": [{ "name": "b", "friends": [{ "name": 3, "friends": [] }] }],
        "boss": 7
    });
    assert_eq!(
        check(types, deep, value!("Person")).1,
        value!([
            { "path": "/friends/0/friends/0/name", "message": "Expected a string, got number" },
            { "path": "/boss", "message": "Expected Person or null, got number" }
        ])
    );
    // A type written in place, rather than named.
    assert_eq!(
        check(
            value!({}),
            value!([1, null, "x"]),
            value!({ "list": "number?" })
        )
        .1,
        value!([{ "path": "/2", "message": "Expected a number or null, got string" }])
    );
}

#[test]
fn a_check_says_at_most_100_problems_and_counts_its_steps() {
    let many: Vec<Value> = (0..150).map(|_| value!("x")).collect();
    let (_, problems) = check(
        value!({}),
        Value::Array(many.clone()),
        value!({ "list": "number" }),
    );
    assert_eq!(problems.as_array().map(Vec::len), Some(100));
    // Each value looked at is a step, which the limit stops.
    let program = value!({ "is": { "val": Value::Array(many) }, "type": { "list": "string" } });
    let checked = run(&engine(), &program).unwrap();
    assert_eq!(checked.value, true);
    assert_eq!(checked.steps, 1 + 1 + 151);
    let tight = Engine::builder()
        .executor("math", MATH, math)
        .limits(Limits {
            max_steps: 50,
            ..Limits::default()
        })
        .build();
    let error = failure(run(&tight, &program));
    assert_eq!(error.code(), ErrorCode::StepLimit);
    assert_eq!(error.path(), "");
}

#[test]
fn hosts_check_values_against_a_programs_types() {
    let program = engine()
        .compile(&value!({
            "types": {
                "Name": "string",
                "Todo": { "obj": { "title": "Name", "done": "boolean?" } }
            },
            "do": []
        }))
        .unwrap();
    assert_eq!(program.types().collect::<Vec<_>>(), ["Name", "Todo"]);
    assert_eq!(
        program.problems(&value!({ "title": "a" }), "Todo"),
        Some(vec![])
    );
    assert_eq!(
        program.problems(&value!({ "title": 1, "done": 1 }), "Todo"),
        Some(vec![
            issue("/title", "Expected Name, got number"),
            issue("/done", "Expected a boolean or null, got number"),
        ])
    );
    assert_eq!(program.problems(&value!(1), "Nope"), None);
}

#[test]
fn a_deep_value_is_checked_without_recursing() {
    // A list in a list, 5 000 deep: checked on WebAssembly's 1 MiB stack.
    let checked = std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            let mut deep = value!(1);
            for _ in 0..5_000 {
                deep = Value::Array(vec![deep]);
            }
            let program = engine()
                .compile(&value!({ "types": { "Nest": { "list": "Nest" } }, "do": [] }))
                .unwrap();
            let found = program.problems(&deep, "Nest").unwrap();
            // Taken apart a level at a time: dropping it recurses too.
            while let Value::Array(mut items) = deep {
                deep = items.pop().unwrap_or_default();
            }
            found
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(checked.len(), 1);
    assert_eq!(checked[0].message, "Expected Nest, got number");
    assert_eq!(checked[0].path.matches("/0").count(), 5_000);
}

#[test]
fn checks_are_steps_a_debugger_names() {
    let program = engine()
        .compile(&value!({
            "types": { "T": "string" },
            "do": [{ "ret": { "is": 1, "type": "T", "tag": "check" } }]
        }))
        .unwrap();
    let steps = program.steps();
    let check = steps.iter().find(|s| s.tag == Some("check")).unwrap();
    assert_eq!(check.kind.as_str(), "is");
    assert_eq!(check.name, Some("T"));
    let options = RunOptions::new();
    assert_eq!(program.run_blocking(options).unwrap().value, false);
}
