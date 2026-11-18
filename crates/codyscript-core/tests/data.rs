//! The data model is format-independent: programs and results travel through
//! any serde format, and everything a format can express is handled.

mod common;

use codyscript_core::{Engine, Error, ErrorCode, Limits, RunOptions, RunResult, Value, value};
use common::*;
use serde_test::{
    Token, assert_de_tokens, assert_de_tokens_error, assert_ser_tokens, assert_tokens,
};

// ── Non-finite numbers: some formats (YAML, CBOR) can express them ─────────

#[test]
fn refuses_numbers_that_arent_finite() {
    let nan = Value::from(f64::NAN);
    let cases = [
        ("NaN", nan.clone(), ""),
        ("Infinity", val(Value::from(f64::INFINITY)), "/val"),
        (
            "a nested NaN",
            val(value!({ "a": [1, nan.clone()] })),
            "/val/a/1",
        ),
        (
            "a non-finite arg",
            uses_math(call_with(
                "math.add",
                value!({}),
                Value::from(f64::NEG_INFINITY),
            )),
            "/arg",
        ),
        (
            "a non-finite argument",
            uses_math(call("math.add", value!({ "a": nan }))),
            "/call/math.add/a",
        ),
    ];
    for (name, program, path) in cases {
        let error = compile_error(&engine(), &program);
        assert_eq!(error.code(), ErrorCode::InvalidProgram, "{name}");
        assert_eq!(error.path(), path, "{name}");
        assert!(
            error.message().contains("Expected a finite number"),
            "{name}"
        );
    }
}

#[test]
fn an_overflowing_literal_is_refused_so_programs_stay_finite() {
    let huge = format!("1{}", "0".repeat(400));
    let error = compile_error(&Engine::new(), &x(&format!("-{huge}")));
    assert_eq!(error.code(), ErrorCode::InvalidProgram);
    assert_eq!(error.path(), "/x");
    assert!(
        error
            .message()
            .contains(&format!(r#"Number "{huge}" is too large"#))
    );
}

#[test]
fn a_repeated_key_is_an_error_in_any_format() {
    // Serde tokens stand in for any format: CBOR, YAML, …
    assert_de_tokens_error::<Value>(
        &[
            Token::Map { len: Some(2) },
            Token::Str("a"),
            Token::I64(1),
            Token::Str("a"),
            Token::I64(2),
            Token::MapEnd,
        ],
        r#"repeated key "a""#,
    );
}

// ── A non-JSON format, end to end ──────────────────────────────────────────

fn to_cbor<T: serde::Serialize>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).unwrap();
    bytes
}

fn from_cbor<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> T {
    ciborium::from_reader(bytes).unwrap()
}

#[test]
fn a_program_stored_as_cbor_compiles_and_runs() {
    let source = m(vec![
        set("fee", x("amount * 0.03")),
        when(x("fee > 300"), vec![set("fee", value!(300))], None),
        ret(value!({ "val": { "fee": 0, "note": "capped?" } })),
    ]);
    let bytes = to_cbor(&source);
    let program: Value = from_cbor(&bytes);
    assert_eq!(program, source);

    let compiled = engine().compile(&program).unwrap();
    let result = compiled
        .run_blocking(with_vars(value!({ "amount": 20000 })))
        .unwrap();
    assert_eq!(result.variables["fee"], 300);

    // Results travel back the same way, with object keys in order.
    let back: Value = from_cbor(&to_cbor(&result));
    assert_eq!(back["steps"], result.steps);
    assert_eq!(keys(&back["variables"]), ["amount", "fee"]);
    assert_eq!(keys(&back["value"]), ["fee", "note"]);
}

#[test]
fn cbor_can_carry_a_nan_which_compile_refuses() {
    let program: Value = from_cbor(&to_cbor(&val(Value::from(f64::NAN))));
    let error = compile_error(&engine(), &program);
    assert_eq!(error.path(), "/val");
}

// ── The host types as data ──────────────────────────────────────────────────

#[test]
fn error_codes_are_their_snake_case_names() {
    assert_tokens(
        &ErrorCode::InvalidProgram,
        &[Token::UnitVariant {
            name: "ErrorCode",
            variant: "invalid_program",
        }],
    );
    assert_tokens(
        &ErrorCode::StepLimit,
        &[Token::UnitVariant {
            name: "ErrorCode",
            variant: "step_limit",
        }],
    );
    assert_tokens(
        &ErrorCode::UnknownFunction,
        &[Token::UnitVariant {
            name: "ErrorCode",
            variant: "unknown_function",
        }],
    );
    assert_tokens(
        &ErrorCode::FunctionFailed,
        &[Token::UnitVariant {
            name: "ErrorCode",
            variant: "function_failed",
        }],
    );
    assert_tokens(
        &ErrorCode::DepthLimit,
        &[Token::UnitVariant {
            name: "ErrorCode",
            variant: "depth_limit",
        }],
    );
}

#[test]
fn an_error_serializes_for_an_editor() {
    let error = compile_error(&engine(), &tagged(call("nope", value!({})), "n7"));
    assert_ser_tokens(
        &error,
        &[
            Token::Struct {
                name: "Error",
                len: 5,
            },
            Token::Str("code"),
            Token::UnitVariant {
                name: "ErrorCode",
                variant: "invalid_program",
            },
            Token::Str("message"),
            Token::Str(r#"Invalid program: Unknown function "nope" at /call/nope (tag n7)"#),
            Token::Str("path"),
            Token::Str("/call/nope"),
            Token::Str("tag"),
            Token::Str("n7"),
            Token::Str("issues"),
            Token::Seq { len: Some(1) },
            Token::Struct {
                name: "Issue",
                len: 2,
            },
            Token::Str("path"),
            Token::Str("/call/nope"),
            Token::Str("message"),
            Token::Str(r#"Unknown function "nope""#),
            Token::StructEnd,
            Token::SeqEnd,
            Token::StructEnd,
        ],
    );
    let plain = Error::new(ErrorCode::Aborted, "stop");
    assert_ser_tokens(
        &plain,
        &[
            Token::Struct {
                name: "Error",
                len: 3,
            },
            Token::Str("code"),
            Token::UnitVariant {
                name: "ErrorCode",
                variant: "aborted",
            },
            Token::Str("message"),
            Token::Str("stop"),
            Token::Str("path"),
            Token::Str(""),
            Token::StructEnd,
        ],
    );
}

#[test]
fn limits_read_from_configuration_with_defaults() {
    assert_de_tokens(
        &Limits {
            max_steps: 500,
            ..Limits::default()
        },
        &[
            Token::Struct {
                name: "Limits",
                len: 1,
            },
            Token::Str("max_steps"),
            Token::U64(500),
            Token::StructEnd,
        ],
    );
    let limits: Limits = from_cbor(&to_cbor(&value!({ "max_depth": 16 })));
    assert_eq!(
        limits,
        Limits {
            max_depth: 16,
            ..Limits::default()
        }
    );
    let engine = Engine::builder().limits(limits).build();
    assert_eq!(engine.limits().max_depth, 16);
}

#[test]
fn a_run_result_serializes_as_value_variables_steps() {
    let result: RunResult = Engine::new()
        .run_blocking(
            &m(vec![set("a", value!(1)), ret(x("a + 1"))]),
            RunOptions::new(),
        )
        .unwrap();
    let data: Value = from_cbor(&to_cbor(&result));
    assert_eq!(
        data,
        value!({ "value": 2, "variables": { "a": 1 }, "steps": 7 })
    );
    assert_eq!(keys(&data), ["value", "variables", "steps"]);
}
