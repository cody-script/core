#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::io;
use std::sync::Arc;

use codyscript_core::{CompiledProgram, Details, Engine, Error, ErrorCode, Issue, Number, Value};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// The core crate, re-exported so its version always matches this one.
pub use codyscript_core;
/// `serde_json`, re-exported for its error type and for interop.
pub use serde_json;

/// A JSON Schema (draft 2020-12) for the language, for editors to check
/// hand-written programs against. It checks shape only;
/// [`Engine::compile`] also checks what each call names, the external
/// functions the host provides, nesting depth, expression syntax and that
/// numbers are finite.
pub const SCHEMA: &str = include_str!("../schema.json");

// ── Reading ─────────────────────────────────────────────────────────────────

/// Parses JSON text: a program or any other [`Value`], or [`Variables`]
/// (which must be an object), or any type that implements `Deserialize`.
/// Objects keep their keys in document order; a repeated key keeps its first
/// place and its last value, as `JSON.parse` does.
///
/// Nesting is limited to 128 levels, as `compile`'s default `max_depth` is.
///
/// [`Variables`]: codyscript_core::Variables
pub fn from_str<T: DeserializeOwned>(json: &str) -> serde_json::Result<T> {
    serde_json::from_str(json)
}

/// [`from_str`], from UTF-8 bytes.
pub fn from_slice<T: DeserializeOwned>(json: &[u8]) -> serde_json::Result<T> {
    serde_json::from_slice(json)
}

/// [`from_str`], from a reader. Buffer it (`BufReader`) if it isn't already.
pub fn from_reader<R: io::Read, T: DeserializeOwned>(reader: R) -> serde_json::Result<T> {
    serde_json::from_reader(reader)
}

// ── Writing ─────────────────────────────────────────────────────────────────

/// Writes a value, a [`RunResult`](codyscript_core::RunResult), an
/// [`Error`] or anything else serializable as compact JSON. Numbers that
/// JSON can't hold (`NaN`, `Infinity`) are written as `null`, as
/// `JSON.stringify` does.
pub fn to_string<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    serde_json::to_string(value)
}

/// [`to_string`], indented.
pub fn to_string_pretty<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    serde_json::to_string_pretty(value)
}

/// [`to_string`], as UTF-8 bytes.
pub fn to_vec<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec(value)
}

/// [`to_string`], to a writer.
pub fn to_writer<W: io::Write, T: Serialize + ?Sized>(
    writer: W,
    value: &T,
) -> serde_json::Result<()> {
    serde_json::to_writer(writer, value)
}

// ── Engine ──────────────────────────────────────────────────────────────────

/// JSON text straight into an [`Engine`].
pub trait EngineJsonExt {
    /// Parses `json` and compiles it. Text that isn't JSON fails like any
    /// other malformed program: `invalid_program`, with the parser's message
    /// (which says where) and the parser's error as the cause.
    fn compile_json(&self, json: &str) -> Result<CompiledProgram, Error>;
}

impl EngineJsonExt for Engine {
    fn compile_json(&self, json: &str) -> Result<CompiledProgram, Error> {
        let program: Value = from_str(json).map_err(syntax_error)?;
        self.compile_owned(program)
    }
}

/// A JSON syntax error as the engine's `invalid_program`.
fn syntax_error(error: serde_json::Error) -> Error {
    let message = error.to_string();
    Error::with_details(
        ErrorCode::InvalidProgram,
        format!("Invalid program: {message}"),
        Details {
            path: Some(String::new()),
            tag: None,
            issues: Some(vec![Issue {
                path: String::new(),
                message,
            }]),
            cause: Some(Arc::new(error)),
        },
    )
}

// ── serde_json interop ──────────────────────────────────────────────────────

/// A `serde_json::Value` as a CodyScript [`Value`]; keys keep their order
/// if `serde_json`'s `preserve_order` feature is on, and are sorted if not.
pub fn from_serde_json(value: serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(b),
        serde_json::Value::Number(n) => Value::Number(number_from_serde_json(&n)),
        serde_json::Value::String(s) => Value::String(s),
        serde_json::Value::Array(items) => {
            Value::Array(items.into_iter().map(from_serde_json).collect())
        }
        serde_json::Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, from_serde_json(v)))
                .collect(),
        ),
    }
}

fn number_from_serde_json(n: &serde_json::Number) -> Number {
    if let Some(n) = n.as_u64() {
        Number::from(n)
    } else if let Some(n) = n.as_i64() {
        Number::from(n)
    } else {
        Number::from_f64(n.as_f64().unwrap_or(f64::NAN))
    }
}

/// A CodyScript [`Value`] as a `serde_json::Value`. Numbers JSON can't hold
/// (`NaN`, `Infinity`) become `null`.
pub fn to_serde_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Number(n) => number_to_serde_json(n),
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Array(items) => serde_json::Value::Array(items.iter().map(to_serde_json).collect()),
        Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), to_serde_json(v)))
                .collect(),
        ),
    }
}

fn number_to_serde_json(n: &Number) -> serde_json::Value {
    if let Some(n) = n.as_u64() {
        n.into()
    } else if let Some(n) = n.as_i64() {
        n.into()
    } else {
        serde_json::Number::from_f64(n.as_f64()).map_or(serde_json::Value::Null, Into::into)
    }
}

/// The README at the repository's root, whose Rust examples run as doc tests.
#[cfg(doctest)]
#[doc = include_str!("../../../README.md")]
pub struct RepositoryReadme;
