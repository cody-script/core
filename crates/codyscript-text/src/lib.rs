#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use codyscript_core::{BoxError, ExecutorCall, ExecutorResult, Map, Value};

/// The functions [`text`] provides, to register it with.
pub const FUNCTIONS: [&str; 13] = [
    "trim",
    "trim_start",
    "trim_end",
    "upper",
    "lower",
    "char_count",
    "is_empty",
    "contains",
    "starts_with",
    "ends_with",
    "replace",
    "split",
    "join",
];

/// The text functions, as an executor: register it under the id programs
/// will use, with [`FUNCTIONS`] —
/// `Engine::builder().executor("text", FUNCTIONS, text)`.
///
/// It answers at once, so a call allocates nothing beyond its result.
pub fn text(call: ExecutorCall) -> ExecutorResult {
    let args = Args(&call.input);
    Ok(match call.op() {
        "trim" => args.texts(["value"])?[0].trim().into(),
        "trim_start" => args.texts(["value"])?[0].trim_start().into(),
        "trim_end" => args.texts(["value"])?[0].trim_end().into(),
        "upper" => args.texts(["value"])?[0].to_uppercase().into(),
        "lower" => args.texts(["value"])?[0].to_lowercase().into(),
        "char_count" => args.texts(["value"])?[0].chars().count().into(),
        "is_empty" => args.texts(["value"])?[0].is_empty().into(),
        "contains" => {
            let [value, part] = args.texts(["value", "part"])?;
            value.contains(part).into()
        }
        "starts_with" => {
            let [value, prefix] = args.texts(["value", "prefix"])?;
            value.starts_with(prefix).into()
        }
        "ends_with" => {
            let [value, suffix] = args.texts(["value", "suffix"])?;
            value.ends_with(suffix).into()
        }
        "replace" => {
            let [value, from, to] = args.texts(["value", "from", "to"])?;
            value.replace(from, to).into()
        }
        "split" => {
            let [value, separator] = args.texts(["value", "separator"])?;
            if separator.is_empty() {
                return Err(r#""separator" must not be empty"#.into());
            }
            value.split(separator).collect()
        }
        "join" => {
            args.exactly(&["values", "separator"])?;
            let values = args.list_of_texts("values")?;
            values.join(args.text("separator")?).into()
        }
        // The engine calls only the functions this executor was registered
        // with; registering it with others is the host's mistake.
        op => return Err(format!("text has no function {op:?}").into()),
    })
}

/// A call's arguments, checked as a function reads them.
struct Args<'a>(&'a Map);

impl<'a> Args<'a> {
    /// The arguments must be exactly `names`: none missing, none extra (a
    /// typo is an error, not an argument ignored).
    fn exactly(&self, names: &[&str]) -> Result<(), BoxError> {
        if let Some(missing) = names.iter().find(|name| !self.0.contains_key(name)) {
            return Err(format!("missing argument \"{missing}\"").into());
        }
        if let Some(extra) = self.0.keys().find(|key| !names.contains(key)) {
            return Err(format!("unexpected argument \"{extra}\"").into());
        }
        Ok(())
    }

    /// The arguments, which must be exactly `names`, each a string.
    fn texts<const N: usize>(&self, names: [&str; N]) -> Result<[&'a str; N], BoxError> {
        self.exactly(&names)?;
        let mut texts = [""; N];
        for (text, name) in texts.iter_mut().zip(names) {
            *text = self.text(name)?;
        }
        Ok(texts)
    }

    /// Argument `name`, a string.
    fn text(&self, name: &str) -> Result<&'a str, BoxError> {
        match &self.0[name] {
            Value::String(text) => Ok(text),
            other => Err(format!("\"{name}\" must be a string, got {}", kind(other)).into()),
        }
    }

    /// Argument `name`, a list of strings.
    fn list_of_texts(&self, name: &str) -> Result<Vec<&'a str>, BoxError> {
        let Value::Array(items) = &self.0[name] else {
            let got = kind(&self.0[name]);
            return Err(format!("\"{name}\" must be a list of strings, got {got}").into());
        };
        items
            .iter()
            .enumerate()
            .map(|(i, item)| match item {
                Value::String(text) => Ok(text.as_str()),
                other => {
                    Err(format!("\"{name}\" item {i} must be a string, got {}", kind(other)).into())
                }
            })
            .collect()
    }
}

/// What kind of value `value` is, as CodyScript's own messages name it.
fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "list",
        Value::Object(_) => "object",
    }
}
