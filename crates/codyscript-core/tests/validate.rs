mod common;

use codyscript_core::{value, Value};
use codyscript_core::{Engine, ErrorCode, RunOptions};
use common::*;

#[test]
fn accepts_every_valid_program() {
    let engine = engine();
    for (name, program) in valid() {
        match engine.compile(&program) {
            Ok(compiled) => assert_eq!(compiled.program(), &program, "{name}"),
            Err(error) => panic!("refused {name}: {error}"),
        }
    }
}

#[test]
fn refuses_every_invalid_program() {
    let engine = engine();
    for (name, program, path) in invalid() {
        let error = compile_error(&engine, &program);
        assert_eq!(error.code(), ErrorCode::InvalidProgram, "{name}");
        assert_eq!(error.path(), path, "{name}");
        assert!(!error.issues().unwrap_or_default().is_empty(), "{name}");
        assert!(error.message().starts_with("Invalid program: "), "{name}");
    }
}

#[test]
fn reports_every_issue_not_just_the_first() {
    let program = m(vec![set("", value!(1)), value!({ "ret": [1] })]);
    let error = compile_error(&engine(), &program);
    let paths: Vec<&str> = error
        .issues()
        .unwrap()
        .iter()
        .map(|i| i.path.as_str())
        .collect();
    assert_eq!(paths, ["/do/0/set", "/do/1/ret"]);
}

#[test]
fn messages_say_what_is_wrong() {
    let cases = [
        (
            value!({ "rf": "x" }),
            r#"Invalid program: Expected an input with exactly one of "val", "ref", "x", "op", "do""#,
        ),
        (
            value!({ "ref": "a", "val": 1 }),
            r#"Invalid program: Expected an input with exactly one of "val", "ref", "x", "op", "do", found "val" and "ref""#,
        ),
        (
            m(vec![value!(1)]),
            "Invalid program: Expected an instruction object at /do/0",
        ),
        (value!([1]), "Invalid program: Expected an input object"),
        (
            value!({ "val": 1, "tga": "x" }),
            r#"Invalid program: Unknown field "tga" at /tga"#,
        ),
        (
            value!({ "op": {} }),
            "Invalid program: Expected exactly one key, found 0 at /op",
        ),
        (
            value!({ "op": { "add": {} } }),
            r#"Invalid program: Expected "<executor>.<operation>" at /op/add"#,
        ),
        (
            m(vec![value!({ "if": 1 })]),
            "Invalid program: Expected a list of instructions at /do/0/do",
        ),
        (
            value!({ "val": { "constructor": 1 } }),
            r#"Invalid program: "constructor" is a reserved key at /val/constructor"#,
        ),
        (
            value!({ "ref": 1 }),
            "Invalid program: Expected a non-empty string at /ref",
        ),
        (
            value!({ "val": 1, "tag": null }),
            "Invalid program: Expected a string at /tag",
        ),
        (
            value!({ "op": { "math.add": 1 } }),
            "Invalid program: Expected an object at /op/math.add",
        ),
        (
            value!({ "x": "1 +" }),
            r#"Invalid program: Expression ends too early in "1 +" at /x"#,
        ),
    ];
    for (program, message) in cases {
        assert_eq!(
            compile_error(&engine(), &program).message(),
            message,
            "{program:?}"
        );
    }
}

#[test]
fn refuses_an_unknown_executor_wherever_it_is_with_its_nodes_tag() {
    let nope = call("nope", "x");
    let cases = [
        (tagged(nope.clone(), "n7"), "/op/nope.x"),
        (
            op("math", "add", value!({ "a": nope.clone() })),
            "/op/math.add/a/op/nope.x",
        ),
        (m(vec![set("x", nope.clone())]), "/do/0/set/x/op/nope.x"),
        (m(vec![ret(nope.clone())]), "/do/0/ret/op/nope.x"),
        (
            m(vec![when(nope.clone(), vec![], None)]),
            "/do/0/if/op/nope.x",
        ),
        (
            m(vec![when(value!(1), vec![set("x", nope.clone())], None)]),
            "/do/0/do/0/set/x/op/nope.x",
        ),
        (
            m(vec![when(
                value!(1),
                vec![],
                Some(vec![set("x", nope.clone())]),
            )]),
            "/do/0/el/0/set/x/op/nope.x",
        ),
        (m(vec![lp(nope.clone(), vec![])]), "/do/0/for/op/nope.x"),
        (
            m(vec![lp(value!(0), vec![set("x", nope.clone())])]),
            "/do/0/do/0/set/x/op/nope.x",
        ),
        (
            m(vec![set("x", m(vec![set("y", nope.clone())]))]),
            "/do/0/set/x/do/0/set/y/op/nope.x",
        ),
    ];
    for (program, path) in &cases {
        let error = compile_error(&engine(), program);
        assert_eq!(error.code(), ErrorCode::UnknownExecutor, "{program:?}");
        assert_eq!(error.path(), *path);
    }
    let error = compile_error(&engine(), &cases[0].0);
    assert_eq!(
        error.message(),
        r#"Unknown executor "nope" at /op/nope.x (tag n7)"#
    );
    assert_eq!(error.tag(), Some("n7"));
}

#[test]
fn shape_problems_and_unknown_executors_are_reported_together() {
    let program = m(vec![set("x", call("nope", "x")), set("", value!(1))]);
    let error = compile_error(&engine(), &program);
    assert_eq!(error.code(), ErrorCode::UnknownExecutor);
    let paths: Vec<&str> = error
        .issues()
        .unwrap()
        .iter()
        .map(|i| i.path.as_str())
        .collect();
    assert_eq!(paths, ["/do/0/set/x/op/nope.x", "/do/1/set"]);
}

#[test]
fn executor_ids_are_looked_up_exactly() {
    for id in [
        "constructor",
        "toString",
        "__proto__",
        "hasOwnProperty",
        "Math",
        "math ",
    ] {
        assert_eq!(
            compile_error(&engine(), &call(id, "x")).code(),
            ErrorCode::UnknownExecutor
        );
    }
}

#[test]
fn changing_the_source_after_compile_doesnt_change_the_program() {
    let mut source = m(vec![ret(val(value!(1)))]);
    let compiled = engine().compile(&source).unwrap();
    source["do"][0]["ret"]["val"] = value!("smuggled");
    assert_eq!(compiled_value(&compiled, RunOptions::new()), value!(1));
    assert_eq!(compiled.program(), &m(vec![ret(val(value!(1)))]));
}

#[test]
fn refuses_a_program_nested_past_max_depth_before_walking_it() {
    let mut deep = value!(1);
    for _ in 0..100_000 {
        deep = m(vec![set("x", deep)]);
    }
    let error = compile_error(&engine(), &deep);
    assert_eq!(error.code(), ErrorCode::InvalidProgram);
    assert!(
        error.message().contains("deeper than 128 levels"),
        "{}",
        error.message()
    );
    // Values drop recursively; this one is too deep for a test
    // thread's stack, so leak it instead.
    std::mem::forget(deep);
}

#[test]
fn max_depth_is_configurable() {
    let shallow = Engine::builder().max_depth(3).build();
    assert!(shallow.compile(&val(value!({ "a": 1 }))).is_ok());
    let error = compile_error(&shallow, &val(value!({ "a": { "b": { "c": 1 } } })));
    assert_eq!(error.path(), "/val/a/b");
    assert_eq!(
        error.message(),
        "Invalid program: Program nests deeper than 3 levels at /val/a/b"
    );
}

#[test]
fn a_variable_named_after_an_object_prototype_member_is_just_a_variable() {
    let error = failure(run(&Engine::new(), &var("toString")));
    assert_eq!(error.code(), ErrorCode::UndefinedVariable);
    let result = run(&Engine::new(), &m(vec![set("toString", value!(1))])).unwrap();
    assert_eq!(result.variables.keys().collect::<Vec<_>>(), ["toString"]);
    assert_eq!(result.variables["toString"], value!(1));
}

#[test]
fn paths_escape_tilde_and_slash() {
    let program = m(vec![set("a/b~c", call("nope", "x"))]);
    let error = compile_error(&engine(), &program);
    assert_eq!(error.path(), "/do/0/set/a~1b~0c/op/nope.x");
}

#[test]
fn an_empty_tag_is_kept_but_not_shown() {
    let error = failure(run(&engine(), &value!({ "ref": "nope", "tag": "" })));
    assert_eq!(error.tag(), Some(""));
    assert_eq!(error.message(), r#"Variable "nope" is not defined"#);
}

#[test]
fn the_same_bad_expression_is_reported_wherever_it_is() {
    let program = m(vec![set("a", x("1 +")), set("b", x("1 +"))]);
    let error = compile_error(&engine(), &program);
    assert_eq!(error.issues().unwrap().len(), 2);
}

#[test]
fn an_op_with_a_bad_call_still_checks_its_arguments_and_arg() {
    let program = value!({ "op": { "nodot": { "a": [1] } }, "arg": { "prototype": 1 } });
    let error = compile_error(&engine(), &program);
    let paths: Vec<&str> = error
        .issues()
        .unwrap()
        .iter()
        .map(|i| i.path.as_str())
        .collect();
    assert_eq!(paths, ["/op/nodot", "/op/nodot/a", "/arg/prototype"]);
}

#[test]
fn compiled_program_debug_shows_the_program() {
    let compiled = engine().compile(&value!(1)).unwrap();
    assert_eq!(
        format!("{compiled:?}"),
        "CompiledProgram { program: Number(1) }"
    );
    let _: Value = compiled.program().clone();
}
