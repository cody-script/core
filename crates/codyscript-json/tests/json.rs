//! The JSON conventions: document order, numbers, errors, interop.

#[path = "../../codyscript-core/tests/common/mod.rs"]
mod common;

use codyscript_core::{Engine, ErrorCode, Map, RunOptions, Value, Variables, value};
use codyscript_json::{
    EngineJsonExt, from_reader, from_serde_json, from_slice, from_str, to_serde_json, to_string,
    to_string_pretty, to_vec, to_writer,
};
use common::*;

#[test]
fn objects_keep_document_order_and_round_trip() {
    let text = r#"{"z":1,"a":[true,null,"s"],"m":{"y":2,"b":3}}"#;
    let value: Value = from_str(text).unwrap();
    assert_eq!(keys(&value), ["z", "a", "m"]);
    assert_eq!(keys(&value["m"]), ["y", "b"]);
    assert_eq!(to_string(&value).unwrap(), text);
    assert_eq!(from_slice::<Value>(text.as_bytes()).unwrap(), value);
    assert_eq!(from_reader::<_, Value>(text.as_bytes()).unwrap(), value);
    assert_eq!(to_vec(&value).unwrap(), text.as_bytes());
    let mut written = Vec::new();
    to_writer(&mut written, &value).unwrap();
    assert_eq!(written, text.as_bytes());
    assert!(to_string_pretty(&value).unwrap().contains("\n  \"z\": 1"));
}

#[test]
fn a_repeated_key_keeps_its_first_place_and_last_value_like_json_parse() {
    let value: Value = from_str(r#"{"a":1,"b":2,"a":3}"#).unwrap();
    assert_eq!(keys(&value), ["a", "b"]);
    assert_eq!(value["a"], 3);
}

#[test]
fn numbers_keep_their_form() {
    // Out of a float's range: refused by the parser, not turned into Infinity.
    assert!(from_str::<Value>("1e400").is_err());
    let value: Value = from_str("[1, -1, 1.0, 2.5, 18446744073709551615]").unwrap();
    assert_eq!(
        to_string(&value).unwrap(),
        "[1,-1,1.0,2.5,18446744073709551615]"
    );
}

#[test]
fn numbers_json_cant_hold_are_written_as_null() {
    let value = value!([f64::NAN, f64::INFINITY, 1]);
    assert_eq!(to_string(&value).unwrap(), "[null,null,1]");
    assert_eq!(to_serde_json(&value), serde_json::json!([null, null, 1]));
}

#[test]
fn results_and_errors_are_written_as_json() {
    let engine = engine();
    let result = engine
        .run_blocking(
            &m(vec![set("a", value!(1)), ret(x("a / 2"))]),
            RunOptions::new(),
        )
        .unwrap();
    assert_eq!(
        to_string(&result).unwrap(),
        r#"{"value":0.5,"variables":{"a":1},"steps":7}"#
    );
    let error = engine
        .compile_json(r#"{ "call": { "nope.x": {} }, "tag": "n1" }"#)
        .unwrap_err();
    assert_eq!(
        to_string(&error).unwrap(),
        r#"{"code":"invalid_program","message":"Invalid program: Unknown function \"nope.x\" at /call/nope.x (tag n1)","path":"/call/nope.x","tag":"n1","issues":[{"path":"/call/nope.x","message":"Unknown function \"nope.x\""}]}"#
    );
    let error = engine
        .compile_json(r#"{ "ext": { "nope": ["x"] }, "call": { "nope.x": {} } }"#)
        .unwrap_err();
    assert_eq!(
        to_string(&error).unwrap(),
        r#"{"code":"unknown_function","message":"Unknown function \"nope.x\" at /ext/nope/0","path":"/ext/nope/0","issues":[{"path":"/ext/nope/0","message":"Unknown function \"nope.x\""}]}"#
    );
}

#[test]
fn pauses_steps_and_actions_are_written_as_json() {
    use codyscript_core::debug::{self, Action, Breakpoint};
    let program = engine()
        .compile(&with_fn(
            m(vec![
                set("a", value!(1)),
                tagged(set("b", call("twice", value!({ "v": var("a") }))), "t"),
            ]),
            [("twice", m(vec![ret(x("v * 2"))]))],
        ))
        .unwrap();
    let mut pauses = Vec::new();
    let debugger = debug::from_fn(|pause| {
        pauses.push(to_string(&*pause).unwrap());
        if pause.step().path == "/do/0" {
            pause
                .breakpoints_mut()
                .push(Breakpoint::path("/fn/twice/do/0/ret"));
            Action::StepInto
        } else {
            Action::Continue
        }
    });
    program
        .run_blocking(RunOptions::new().with_debugger([Breakpoint::path("/do/0")], debugger))
        .unwrap();
    assert_eq!(
        pauses,
        [
            r#"{"reason":"breakpoint","path":"/do/0","kind":"set","name":"a","depth":0,"steps":0,"frames":[{"scopes":[{}]}],"breakpoints":[{"path":"/do/0"}]}"#,
            r#"{"reason":"step","path":"/do/0/set/a","kind":"val","depth":1,"steps":1,"frames":[{"scopes":[{}]}],"breakpoints":[{"path":"/do/0"},{"path":"/fn/twice/do/0/ret"}]}"#,
            r#"{"reason":"breakpoint","path":"/fn/twice/do/0/ret","kind":"x","name":"v * 2","function":"twice","depth":1,"steps":6,"frames":[{"scopes":[{"a":1}]},{"function":"twice","call":"/do/1/set/b","scopes":[{"v":1}]}],"breakpoints":[{"path":"/do/0"},{"path":"/fn/twice/do/0/ret"}]}"#,
        ]
    );
    let steps = to_string(&program.steps()).unwrap();
    assert!(steps.starts_with(
        r#"[{"path":"/do/0","kind":"set","name":"a","depth":0},{"path":"/do/0/set/a","kind":"val","depth":1},{"path":"/do/1","kind":"set","name":"b","tag":"t","depth":0},"#
    ));
    assert!(steps.ends_with(
        r#"{"path":"/fn/twice/do/0","kind":"ret","function":"twice","depth":0},{"path":"/fn/twice/do/0/ret","kind":"x","name":"v * 2","function":"twice","depth":1}]"#
    ));
    let actions: Vec<Action> = from_str(r#"["stepOver","stepInto","continue","stop"]"#).unwrap();
    assert_eq!(
        actions,
        [
            Action::StepOver,
            Action::StepInto,
            Action::Continue,
            Action::Stop
        ]
    );
}

#[test]
fn run_configurations_are_read_and_written_as_json() {
    use codyscript_core::RunConfig;
    use codyscript_core::debug::Breakpoint;
    let text = r#"{"variables":{"amount":200},"breakpoints":[{"tag":"fee"},{"path":"/do/1"}]}"#;
    let config: RunConfig = from_str(text).unwrap();
    assert_eq!(config.variables, vars(value!({ "amount": 200 })));
    assert_eq!(
        config.breakpoints,
        [Breakpoint::tag("fee"), Breakpoint::path("/do/1")]
    );
    assert_eq!(to_string(&config).unwrap(), text);
    assert_eq!(from_str::<RunConfig>("{}").unwrap(), RunConfig::default());
    let typo = from_str::<RunConfig>(r#"{ "breakpoint": [] }"#).unwrap_err();
    assert!(
        typo.to_string().starts_with("unknown field `breakpoint`"),
        "{typo}"
    );
}

#[test]
fn compile_json_compiles_and_reports_syntax_errors_as_invalid_programs() {
    let engine = engine();
    let compiled = engine.compile_json(r#"{ "x": "1 + 2" }"#).unwrap();
    assert_eq!(compiled_value(&compiled, RunOptions::new()), 3);

    let error = engine.compile_json(r#"{ "x": "1 + 2" "#).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidProgram);
    assert_eq!(error.path(), "");
    assert!(
        error
            .message()
            .starts_with("Invalid program: EOF while parsing"),
        "{}",
        error.message()
    );
    assert!(error.message().contains("line 1"));
    assert_eq!(error.issues().map(<[_]>::len), Some(1));
    assert!(
        error
            .cause()
            .unwrap()
            .downcast_ref::<serde_json::Error>()
            .is_some()
    );

    let error = engine.compile_json(r#"{ "x": "1 +" }"#).unwrap_err();
    assert_eq!(
        error.message(),
        r#"Invalid program: Expression ends too early in "1 +" at /x"#
    );
}

#[test]
fn variables_must_be_an_object() {
    let vars: Variables = from_str(r#"{ "n": 2 }"#).unwrap();
    assert_eq!(vars, value!({ "n": 2 }).into_object().unwrap());
    assert!(from_str::<Variables>("[1]").is_err());
    let _: Map = vars;
}

#[test]
fn serde_json_values_convert_both_ways() {
    let json = serde_json::json!({ "a": [1, -2, 2.5, u64::MAX, "s", null, true], "b": {} });
    let value = from_serde_json(json.clone());
    assert_eq!(
        value,
        value!({ "a": [1, -2, 2.5, u64::MAX, "s", null, true], "b": {} })
    );
    assert_eq!(to_serde_json(&value), json);
    assert!(value["a"][2].as_number().unwrap().is_f64());
}

#[test]
fn deep_json_is_refused_by_the_parser_before_compile() {
    let deep = format!("{}1{}", "[".repeat(200), "]".repeat(200));
    assert!(from_str::<Value>(&deep).is_err());
    assert!(Engine::new().compile_json(&deep).is_err());
}
