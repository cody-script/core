mod common;

use codyscript_core::{Engine, Error, ErrorCode, RunOptions};
use codyscript_core::{Value, value};
use common::*;

#[test]
fn accepts_every_valid_program() {
    let engine = engine();
    for (name, program) in valid() {
        match engine.compile(&program) {
            Ok(_) => {}
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
    assert_eq!(issue_paths(&error), ["/do/0/set", "/do/1/ret"]);
}

/// The paths of every issue an error lists.
fn issue_paths(error: &Error) -> Vec<&str> {
    error
        .issues()
        .unwrap()
        .iter()
        .map(|i| i.path.as_str())
        .collect()
}

#[test]
fn messages_say_what_is_wrong() {
    let cases = [
        (
            value!({ "rf": "x" }),
            r#"Invalid program: Expected an input with exactly one of "val", "ref", "x", "call", "do""#,
        ),
        (
            value!({ "ref": "a", "val": 1 }),
            r#"Invalid program: Expected an input with exactly one of "val", "ref", "x", "call", "do", found "val" and "ref""#,
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
            value!({ "call": {} }),
            "Invalid program: Expected exactly one key, found 0 at /call",
        ),
        (
            m(vec![value!({ "if": 1 })]),
            "Invalid program: Expected a list of instructions at /do/0/do",
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
            uses_math(value!({ "call": { "math.add": 1 } })),
            "Invalid program: Expected an object at /call/math.add",
        ),
        (
            value!({ "x": "1 +" }),
            r#"Invalid program: Expression ends too early in "1 +" at /x"#,
        ),
        (
            value!({ "fn": 1, "do": [] }),
            "Invalid program: Expected an object at /fn",
        ),
        (
            with_fn(m(vec![]), [("", value!(1))]),
            "Invalid program: Expected a non-empty name at /fn/",
        ),
        (
            with_fn(call("f", value!({ "": 1 })), [("f", value!(1))]),
            "Invalid program: Expected a non-empty name at /call/f/",
        ),
        (
            value!({ "ext": { "math": "add" }, "do": [] }),
            "Invalid program: Expected a list of function names at /ext/math",
        ),
        (
            value!({ "ext": { "ma.th": ["add"] }, "do": [] }),
            "Invalid program: Expected an executor name without a dot at /ext/ma.th",
        ),
        (
            value!({ "ext": { "math": ["add", "add"] }, "do": [] }),
            r#"Invalid program: Duplicate function "math.add" at /ext/math/1"#,
        ),
        (
            call("nope", value!({})),
            r#"Invalid program: Unknown function "nope" at /call/nope"#,
        ),
        (
            call("math.add", value!({})),
            r#"Invalid program: Function "math.add" isn't declared in "ext" at /call/math.add"#,
        ),
        (
            with_fn(uses_math(m(vec![])), [("math.add", value!(1))]),
            r#"Invalid program: "math.add" is both defined in "fn" and declared in "ext" at /fn/math.add"#,
        ),
        (
            with_fn(call_with("f", value!({}), value!(1)), [("f", value!(1))]),
            r#"Invalid program: "arg" is only for external functions at /arg"#,
        ),
        (
            with_ext(m(vec![]), value!({ "math": ["sub"] })),
            r#"Unknown function "math.sub" at /ext/math/0"#,
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
fn every_external_function_must_be_one_the_engine_provides() {
    let program = with_ext(
        m(vec![]),
        value!({ "math": ["add", "sub", "round.half.up", "round"], "text": ["trim"] }),
    );
    let error = compile_error(&engine(), &program);
    assert_eq!(error.code(), ErrorCode::UnknownFunction);
    assert_eq!(error.code().as_str(), "unknown_function");
    assert_eq!(error.path(), "/ext/math/1");
    assert_eq!(error.tag(), None);
    // Exactly: a function the engine provides, not just its executor.
    assert_eq!(
        issue_paths(&error),
        ["/ext/math/1", "/ext/math/3", "/ext/text/0"]
    );
    let messages: Vec<&str> = error
        .issues()
        .unwrap()
        .iter()
        .map(|i| i.message.as_str())
        .collect();
    assert_eq!(
        messages,
        [
            r#"Unknown function "math.sub""#,
            r#"Unknown function "math.round""#,
            r#"Unknown function "text.trim""#
        ]
    );
}

#[test]
fn external_function_names_are_looked_up_exactly() {
    let engine = engine();
    for (exe, op) in [
        ("math", "Add"),
        ("math", "add "),
        ("math", "round"),
        ("math", "round.half"),
        ("Math", "add"),
        ("constructor", "add"),
        ("toString", "add"),
        ("hasOwnProperty", "add"),
        ("math", "toString"),
        ("math", "constructor"),
    ] {
        let program = with_ext(m(vec![]), obj([(exe, value!([op]))]));
        let error = compile_error(&engine, &program);
        assert_eq!(error.code(), ErrorCode::UnknownFunction, "{exe}.{op}");
        assert!(!engine.has_function(&format!("{exe}.{op}")), "{exe}.{op}");
    }
    for name in MATH {
        assert!(engine.has_function(&format!("math.{name}")), "{name}");
    }
    assert!(!engine.has_function("math"));
    assert!(!engine.has_function(""));
}

#[test]
fn a_call_must_name_a_function_the_program_defines_or_declares() {
    let cases = [
        (tagged(call("nope", value!({})), "t"), "/call/nope"),
        // Names are looked up exactly.
        (
            with_fn(call("F", value!({})), [("f", value!(1))]),
            "/call/F",
        ),
        // The engine providing a function isn't enough: it must be declared.
        (call("math.add", value!({})), "/call/math.add"),
        (
            with_ext(call("math.add", value!({})), value!({ "math": ["lt"] })),
            "/call/math.add",
        ),
        (m(vec![ret(call("f", value!({})))]), "/do/0/ret/call/f"),
        // A function can't call the program's body: it has no name.
        (
            with_fn(m(vec![]), [("f", call("main", value!({})))]),
            "/fn/f/call/main",
        ),
    ];
    for (program, path) in &cases {
        let error = compile_error(&engine(), program);
        assert_eq!(error.code(), ErrorCode::InvalidProgram, "{program:?}");
        assert_eq!(error.path(), *path);
    }
    let program = tagged(call("nope", value!({ "a": x("1 +") })), "n3");
    let error = compile_error(&engine(), &program);
    assert_eq!(
        error.message(),
        r#"Invalid program: Unknown function "nope" at /call/nope (tag n3)"#
    );
    assert_eq!(error.tag(), Some("n3"));
    // Its arguments are checked all the same.
    assert_eq!(issue_paths(&error), ["/call/nope", "/call/nope/a/x"]);
}

#[test]
fn a_name_is_defined_in_fn_or_declared_in_ext_never_both() {
    let program = with_fn(
        uses_math(call("math.add", value!({}))),
        [("math.add", value!(1)), ("math.twice", value!(2))],
    );
    let error = compile_error(&engine(), &program);
    assert_eq!(error.code(), ErrorCode::InvalidProgram);
    // Only the clash: the call itself resolves, so it isn't reported too.
    assert_eq!(issue_paths(&error), ["/fn/math.add"]);
}

#[test]
fn only_external_functions_take_arg() {
    let program = with_fn(
        uses_math(m(vec![
            set("a", call_with("f", value!({}), value!({ "x": 1 }))),
            set("b", call_with("math.add", value!({}), value!({ "x": 1 }))),
        ])),
        [("f", value!(1))],
    );
    let error = compile_error(&engine(), &program);
    assert_eq!(issue_paths(&error), ["/do/0/set/a/arg"]);
    let program = uses_math(call_with("math.add", value!({}), value!({ "x": 1 })));
    assert!(engine().compile(&program).is_ok());
}

#[test]
fn shape_problems_and_unknown_functions_are_reported_together() {
    // Whichever comes first decides the code: `ext` is checked first.
    let program = with_ext(
        m(vec![set("x", call("nope", value!({}))), set("", value!(1))]),
        value!({ "text": ["trim"] }),
    );
    let error = compile_error(&engine(), &program);
    assert_eq!(error.code(), ErrorCode::UnknownFunction);
    assert_eq!(
        issue_paths(&error),
        ["/ext/text/0", "/do/0/set/x/call/nope", "/do/1/set"]
    );
}

#[test]
fn every_function_is_checked_and_every_issue_reported() {
    let program = with_fn(
        m(vec![]),
        [
            ("f", m(vec![set("", value!(1))])),
            ("g", call("math.add", value!({}))),
            ("h", call("i", value!({}))),
        ],
    );
    let error = compile_error(&engine(), &program);
    assert_eq!(error.code(), ErrorCode::InvalidProgram);
    assert_eq!(
        issue_paths(&error),
        ["/fn/f/do/0/set", "/fn/g/call/math.add", "/fn/h/call/i"]
    );
}

#[test]
fn fn_and_ext_are_only_fields_of_the_root() {
    let program = m(vec![set("f", value!({ "fn": { "g": 1 }, "x": "1" }))]);
    let error = compile_error(&engine(), &program);
    assert_eq!(
        error.message(),
        r#"Invalid program: Unknown field "fn" at /do/0/set/f/fn"#
    );
    // In a constant, they're just keys.
    let program = val(value!({ "fn": { "f": 1 }, "ext": { "math": ["add"] } }));
    assert_eq!(
        run(&engine(), &program).unwrap().value,
        value!({ "fn": { "f": 1 }, "ext": { "math": ["add"] } })
    );
    // A literal root has nowhere to put them, and needs neither.
    assert_eq!(run(&engine(), &value!(1)).unwrap().value, value!(1));
}

#[test]
fn fn_and_ext_count_towards_max_depth() {
    // The root is one level, `fn` and `ext` a second; a function's body and
    // an executor's list start at the third.
    let shallow = Engine::builder().max_depth(3).build();
    assert!(
        shallow
            .compile(&with_fn(m(vec![]), [("f", var("a"))]))
            .is_ok()
    );
    let error = compile_error(
        &shallow,
        &with_fn(m(vec![]), [("f", val(value!({ "a": 1 })))]),
    );
    assert_eq!(error.path(), "/fn/f/val");
    let tiny = Engine::builder().max_depth(2).build();
    let error = compile_error(&tiny, &value!({ "ext": { "math": [] }, "x": "1" }));
    assert_eq!(error.path(), "/ext/math");
}

#[test]
fn changing_the_source_after_compile_doesnt_change_the_program() {
    let mut source = m(vec![ret(val(value!(1)))]);
    let compiled = engine().compile(&source).unwrap();
    source["do"][0]["ret"]["val"] = value!("smuggled");
    assert_eq!(compiled_value(&compiled, RunOptions::new()), value!(1));
    drop(source);
    assert_eq!(compiled_value(&compiled, RunOptions::new()), value!(1));
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
    let program = m(vec![set("a/b~c", call("n/o~pe", value!({})))]);
    let error = compile_error(&engine(), &program);
    assert_eq!(error.path(), "/do/0/set/a~1b~0c/call/n~1o~0pe");
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
fn a_call_of_an_unknown_function_still_checks_its_arguments_and_arg() {
    let mut program = value!({ "call": { "nope": { "a": [1] } } });
    program["arg"] = obj([("n", Value::from(f64::NAN))]);
    let error = compile_error(&engine(), &program);
    assert_eq!(
        issue_paths(&error),
        ["/call/nope", "/call/nope/a", "/arg/n"]
    );
}

#[test]
fn compiled_program_debug_shows_its_tree() {
    let compiled = engine()
        .compile(&value!({ "ref": "a", "tag": "t" }))
        .unwrap();
    let debug = format!("{compiled:?}");
    assert!(
        debug.starts_with("CompiledProgram { tree: Program { body: Input {"),
        "{debug}"
    );
    assert!(debug.contains(r#"kind: Ref("a")"#), "{debug}");
}

#[test]
fn engine_debug_lists_the_functions_it_provides() {
    assert_eq!(
        format!("{:?}", engine()),
        r#"Engine { functions: ["math.add", "math.eq", "math.lt", "math.round.half.up"], limits: Limits { max_steps: 100000, max_depth: 128 } }"#
    );
}
