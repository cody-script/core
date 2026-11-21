//! The text functions, called from programs as a host would wire them.

use codyscript_core::{Engine, ErrorCode, ExecutorCall, Value, value};
use codyscript_text::{FUNCTIONS, text};

/// An engine with the text functions under `text`.
fn engine() -> Engine {
    Engine::builder().executor("text", FUNCTIONS, text).build()
}

/// Calls `text.<op>` with `args` from a program, which declares it.
fn call(op: &str, args: Value) -> Result<Value, codyscript_core::Error> {
    let mut program = value!({ "ext": { "text": [op] }, "call": {} });
    // Each argument as a literal input (`val`, so lists and objects are data).
    let args: Value = args
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, arg)| (name, value!({ "val": (arg.clone()) })))
        .collect();
    program["call"] = [(format!("text.{op}"), args)].into_iter().collect();
    engine()
        .run_blocking(&program, Default::default())
        .map(|result| result.value)
}

#[test]
fn each_function_does_what_strs_method_does() {
    for (op, args, expected) in [
        ("trim", value!({ "value": "  a b \n" }), value!("a b")),
        ("trim_start", value!({ "value": "  a  " }), value!("a  ")),
        ("trim_end", value!({ "value": "  a  " }), value!("  a")),
        ("upper", value!({ "value": "straße" }), value!("STRASSE")),
        ("lower", value!({ "value": "ÀB" }), value!("àb")),
        ("char_count", value!({ "value": "née" }), value!(3)),
        ("char_count", value!({ "value": "" }), value!(0)),
        ("is_empty", value!({ "value": "" }), value!(true)),
        ("is_empty", value!({ "value": " " }), value!(false)),
        (
            "contains",
            value!({ "value": "abc", "part": "bc" }),
            value!(true),
        ),
        (
            "contains",
            value!({ "value": "abc", "part": "" }),
            value!(true),
        ),
        (
            "starts_with",
            value!({ "value": "abc", "prefix": "ab" }),
            value!(true),
        ),
        (
            "ends_with",
            value!({ "value": "abc", "suffix": "ab" }),
            value!(false),
        ),
        (
            "replace",
            value!({ "value": "a-b-c", "from": "-", "to": "+" }),
            value!("a+b+c"),
        ),
        (
            "split",
            value!({ "value": "a,b,,c", "separator": "," }),
            value!(["a", "b", "", "c"]),
        ),
        (
            "join",
            value!({ "values": ["a", "b", "c"], "separator": ", " }),
            value!("a, b, c"),
        ),
        (
            "join",
            value!({ "values": [], "separator": "," }),
            value!(""),
        ),
    ] {
        assert_eq!(call(op, args.clone()).unwrap(), expected, "{op} {args:?}");
    }
}

#[test]
fn every_listed_function_is_one_it_has() {
    for op in FUNCTIONS {
        let result = text(ExecutorCall::new(op, Default::default()));
        let message = result.unwrap_err().to_string();
        assert!(message.starts_with("missing argument"), "{op}: {message}");
    }
    let unknown = text(ExecutorCall::new("reverse", Default::default())).unwrap_err();
    assert_eq!(unknown.to_string(), r#"text has no function "reverse""#);
}

#[test]
fn arguments_are_checked_strictly() {
    for (op, args, message) in [
        ("upper", value!({}), r#"missing argument "value""#),
        (
            "upper",
            value!({ "value": "a", "valeu": "b" }),
            r#"unexpected argument "valeu""#,
        ),
        (
            "upper",
            value!({ "value": 1 }),
            r#""value" must be a string, got number"#,
        ),
        (
            "contains",
            value!({ "value": "a", "part": null }),
            r#""part" must be a string, got null"#,
        ),
        (
            "split",
            value!({ "value": "a", "separator": "" }),
            r#""separator" must not be empty"#,
        ),
        (
            "join",
            value!({ "values": "a", "separator": "," }),
            r#""values" must be a list of strings, got string"#,
        ),
        (
            "join",
            value!({ "values": ["a", 2], "separator": "," }),
            r#""values" item 1 must be a string, got number"#,
        ),
        (
            "join",
            value!({ "values": [] }),
            r#"missing argument "separator""#,
        ),
    ] {
        let error = call(op, args.clone()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::FunctionFailed, "{op} {args:?}");
        assert_eq!(
            error.message(),
            format!("Function \"text.{op}\" failed: {message}"),
            "{op} {args:?}"
        );
    }
}

#[test]
fn a_program_that_uses_a_function_it_lacks_is_refused_before_it_runs() {
    let program = value!({ "ext": { "text": ["reverse"] }, "x": "1" });
    let error = engine().compile(&program).unwrap_err();
    assert_eq!(error.code(), ErrorCode::UnknownFunction);
    assert_eq!(error.path(), "/ext/text/0");
}

#[test]
fn any_id_will_do() {
    let engine = Engine::builder().executor("str", FUNCTIONS, text).build();
    let program = value!({
        "ext": { "str": ["upper"] },
        "call": { "str.upper": { "value": "ok" } }
    });
    assert_eq!(
        engine
            .run_blocking(&program, Default::default())
            .unwrap()
            .value,
        "OK"
    );
}
