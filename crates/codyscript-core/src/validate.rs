//! Checks a program in one walk — shape, unknown fields, finite numbers,
//! nesting depth, reserved keys, executor ids and every expression's syntax —
//! reporting every problem, and builds the tree the engine runs.

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{Details, Error, ErrorCode, Issue};
use crate::expression::{parse, Ast};
use crate::program::{Expression, Input, InputKind, Instruction, InstructionKind, Operation};
use crate::value::{Map, Value};

/// Node types by the key that names them, with the other fields each may
/// carry besides `tag`. A node has exactly one type key; its position (input
/// or instruction) decides which set applies.
type Kinds = &'static [(&'static str, &'static [&'static str])];
const INPUTS: Kinds = &[
    ("val", &[]),
    ("ref", &[]),
    ("x", &[]),
    ("op", &["arg"]),
    ("do", &[]),
];
const INSTRUCTIONS: Kinds = &[
    ("set", &[]),
    ("ret", &[]),
    ("if", &["do", "el"]),
    ("for", &["do"]),
];

/// Keys no object may use, as in the JavaScript engine, where they could
/// reach `Object.prototype`; refused so programs stay portable between the two.
const RESERVED: [&str; 3] = ["__proto__", "prototype", "constructor"];

/// Escapes one JSON Pointer (RFC 6901) reference token.
pub(crate) fn escape_key(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// The JSON Pointer `path` extended by `key`.
pub(crate) fn child(path: &str, key: &str) -> String {
    format!("{path}/{}", escape_key(key))
}

/// JSON Pointer for a list of keys.
#[cfg(test)]
pub(crate) fn pointer(keys: &[&str]) -> String {
    keys.iter()
        .fold(String::new(), |path, key| child(&path, key))
}

static NULL: Value = Value::Null;

/// A node's field, or `null` when it's missing (which every check refuses
/// where a field is required).
fn field<'v>(node: &'v Map, key: &str) -> &'v Value {
    node.get(key).unwrap_or(&NULL)
}

struct RawIssue {
    path: String,
    message: String,
    code: ErrorCode,
    tag: Option<String>,
}

/// Validates `program` and builds its tree. Fails with `invalid_program` or
/// `unknown_executor` (whichever problem comes first), listing every issue.
pub(crate) fn validate(
    program: &Value,
    max_depth: usize,
    has_executor: &dyn Fn(&str) -> bool,
) -> Result<Input, Error> {
    let mut walk = Walk {
        max_depth,
        has_executor,
        expressions: HashMap::new(),
        issues: Vec::new(),
    };
    let root = walk.input(program, "", 0);
    if let Some(first) = walk.issues.first() {
        let prefix = if first.code == ErrorCode::InvalidProgram {
            "Invalid program: "
        } else {
            ""
        };
        return Err(Error::with_details(
            first.code,
            format!("{prefix}{}", first.message),
            Details {
                path: Some(first.path.clone()),
                tag: first.tag.clone(),
                issues: Some(
                    walk.issues
                        .iter()
                        .map(|i| Issue {
                            path: i.path.clone(),
                            message: i.message.clone(),
                        })
                        .collect(),
                ),
                cause: None,
            },
        ));
    }
    // Every failure to build a node records an issue, so this can't miss.
    root.ok_or_else(|| Error::new(ErrorCode::InvalidProgram, "Invalid program"))
}

struct Walk<'a> {
    max_depth: usize,
    has_executor: &'a dyn Fn(&str) -> bool,
    /// Every expression's parse, by its source: each is parsed once.
    expressions: HashMap<String, Arc<Ast>>,
    issues: Vec<RawIssue>,
}

impl Walk<'_> {
    fn fail(&mut self, path: String, message: impl Into<String>) {
        self.issues.push(RawIssue {
            path,
            message: message.into(),
            code: ErrorCode::InvalidProgram,
            tag: None,
        });
    }

    /// An object or array we may descend into: not too deep, no reserved keys.
    fn container(&mut self, v: &Value, path: &str, depth: usize) -> bool {
        if depth >= self.max_depth {
            let message = format!("Program nests deeper than {} levels", self.max_depth);
            self.fail(path.into(), message);
            return false;
        }
        if let Value::Object(map) = v {
            if let Some(key) = map.keys().find(|k| RESERVED.contains(&k.as_str())) {
                let message = format!("\"{key}\" is a reserved key");
                self.fail(child(path, key), message);
                return false;
            }
        }
        true
    }

    /// An object with only `allowed` fields (any, if `None`).
    fn record<'v>(
        &mut self,
        v: &'v Value,
        path: &str,
        depth: usize,
        allowed: Option<&[&str]>,
    ) -> Option<&'v Map> {
        let Value::Object(map) = v else {
            self.fail(path.into(), "Expected an object");
            return None;
        };
        if !self.container(v, path, depth) {
            return None;
        }
        if let Some(allowed) = allowed {
            for key in map.keys() {
                if !allowed.contains(&key.as_str()) {
                    self.fail(child(path, key), format!("Unknown field \"{key}\""));
                }
            }
        }
        Some(map)
    }

    fn name<'v>(&mut self, v: &'v Value, path: &str) -> Option<&'v str> {
        match v.as_str() {
            Some(s) if !s.is_empty() => Some(s),
            _ => {
                self.fail(path.into(), "Expected a non-empty string");
                None
            }
        }
    }

    /// Any JSON value: finite numbers, no reserved keys, not too deep.
    fn value(&mut self, v: &Value, path: &str, depth: usize) {
        match v {
            Value::Null | Value::Bool(_) | Value::String(_) => {}
            Value::Number(n) => {
                if !n.is_finite() {
                    self.fail(path.into(), "Expected a finite number");
                }
            }
            Value::Array(items) => {
                if self.container(v, path, depth) {
                    for (i, item) in items.iter().enumerate() {
                        self.value(item, &child(path, &i.to_string()), depth + 1);
                    }
                }
            }
            Value::Object(map) => {
                if self.container(v, path, depth) {
                    for (key, item) in map {
                        self.value(item, &child(path, key), depth + 1);
                    }
                }
            }
        }
    }

    fn block(&mut self, v: &Value, path: &str, depth: usize) -> Option<Vec<Instruction>> {
        let Value::Array(items) = v else {
            self.fail(path.into(), "Expected a list of instructions");
            return None;
        };
        if !self.container(v, path, depth) {
            return None;
        }
        let mut list = Some(Vec::with_capacity(items.len()));
        for (i, item) in items.iter().enumerate() {
            // Check every instruction, even after one fails.
            let ins = self.instruction(item, &child(path, &i.to_string()), depth + 1);
            match (ins, list.as_mut()) {
                (Some(ins), Some(list)) => list.push(ins),
                _ => list = None,
            }
        }
        list
    }

    /// A one-key object (`set`, `op`): its single entry.
    fn single<'v>(
        &mut self,
        v: &'v Value,
        path: &str,
        depth: usize,
    ) -> Option<(&'v String, &'v Value)> {
        let Value::Object(map) = v else {
            self.fail(path.into(), "Expected an object");
            return None;
        };
        if !self.container(v, path, depth) {
            return None;
        }
        if map.len() == 1 {
            return map.iter().next();
        }
        let message = format!("Expected exactly one key, found {}", map.len());
        self.fail(path.into(), message);
        None
    }

    /// A node's type key and fields: an object with exactly one of `kinds`'
    /// type keys, only that type's fields and a string `tag`.
    fn header<'v>(
        &mut self,
        v: &'v Value,
        path: &str,
        depth: usize,
        kinds: Kinds,
        what: &str,
    ) -> Option<(&'static str, &'v Map)> {
        let Value::Object(map) = v else {
            self.fail(path.into(), format!("Expected {what} object"));
            return None;
        };
        let types: Vec<&'static str> = kinds
            .iter()
            .map(|(k, _)| *k)
            .filter(|k| map.contains_key(k))
            .collect();
        let [ty] = types[..] else {
            let quote = |k: &&str| format!("\"{k}\"");
            let expected: Vec<String> = kinds.iter().map(|(k, _)| quote(k)).collect();
            let found = if types.is_empty() {
                String::new()
            } else {
                let found: Vec<String> = types.iter().map(quote).collect();
                format!(", found {}", found.join(" and "))
            };
            let message = format!(
                "Expected {what} with exactly one of {}{found}",
                expected.join(", ")
            );
            self.fail(path.into(), message);
            return None;
        };
        let extra = kinds
            .iter()
            .find(|(k, _)| *k == ty)
            .map_or(&[][..], |(_, e)| e);
        let mut allowed = vec![ty, "tag"];
        allowed.extend_from_slice(extra);
        let node = self.record(v, path, depth, Some(&allowed))?;
        if let Some(tag) = node.get("tag") {
            if !tag.is_string() {
                self.fail(child(path, "tag"), "Expected a string");
            }
        }
        Some((ty, node))
    }

    /// A literal (`null`, boolean, finite number, string) or an input node.
    fn input(&mut self, v: &Value, path: &str, depth: usize) -> Option<Input> {
        if !(v.is_object() || v.is_array()) {
            self.value(v, path, depth);
            return Some(Input {
                path: path.into(),
                tag: None,
                kind: InputKind::Const(v.clone()),
            });
        }
        let (ty, node) = self.header(v, path, depth, INPUTS, "an input")?;
        let tag = node.get("tag").and_then(Value::as_str).map(str::to_owned);
        let kind = match ty {
            "val" => {
                let val = field(node, "val");
                self.value(val, &child(path, "val"), depth + 1);
                Some(InputKind::Const(val.clone()))
            }
            "ref" => self
                .name(field(node, "ref"), &child(path, "ref"))
                .map(|name| InputKind::Ref(name.to_owned())),
            "x" => self.expression(field(node, "x"), child(path, "x")),
            "op" => self
                .operation(node, path, depth, tag.as_deref())
                .map(InputKind::Op),
            _ => self
                .block(field(node, "do"), &child(path, "do"), depth + 1)
                .map(InputKind::Do),
        }?;
        Some(Input {
            path: path.into(),
            tag,
            kind,
        })
    }

    fn expression(&mut self, v: &Value, path: String) -> Option<InputKind> {
        let source = self.name(v, &path)?;
        if !self.expressions.contains_key(source) {
            match parse(source, self.max_depth) {
                Ok(ast) => {
                    self.expressions.insert(source.to_owned(), Arc::new(ast));
                }
                Err(message) => {
                    self.fail(path, format!("{message} in \"{source}\""));
                    return None;
                }
            }
        }
        let ast = self.expressions.get(source)?.clone();
        Some(InputKind::X(Expression {
            source: source.to_owned(),
            ast,
            path,
        }))
    }

    fn operation(
        &mut self,
        node: &Map,
        path: &str,
        depth: usize,
        tag: Option<&str>,
    ) -> Option<Operation> {
        let op_path = child(path, "op");
        let mut operation = None;
        if let Some((call, args)) = self.single(field(node, "op"), &op_path, depth + 1) {
            let at = child(&op_path, call);
            // "<executor>.<operation>", split at the first dot; neither empty.
            let dot = call.find('.').filter(|&d| d >= 1 && d != call.len() - 1);
            match dot {
                None => self.fail(at.clone(), "Expected \"<executor>.<operation>\""),
                Some(d) if !(self.has_executor)(&call[..d]) => self.issues.push(RawIssue {
                    path: at.clone(),
                    message: format!("Unknown executor \"{}\"", &call[..d]),
                    code: ErrorCode::UnknownExecutor,
                    tag: tag.map(str::to_owned),
                }),
                Some(_) => {}
            }
            let mut inputs = None;
            if let Some(map) = self.record(args, &at, depth + 2, None) {
                let mut list = Some(Vec::with_capacity(map.len()));
                for (arg, from) in map {
                    let input = self.input(from, &child(&at, arg), depth + 3);
                    match (input, list.as_mut()) {
                        (Some(input), Some(list)) => list.push((arg.clone(), input)),
                        _ => list = None,
                    }
                }
                inputs = list;
            }
            if let (Some(d), Some(args)) = (dot, inputs) {
                operation = Some(Operation {
                    exe: call[..d].to_owned(),
                    op: call[d + 1..].to_owned(),
                    args,
                    arg: None,
                });
            }
        }
        if let Some(arg) = node.get("arg") {
            self.value(arg, &child(path, "arg"), depth + 1);
            if let Some(operation) = operation.as_mut() {
                operation.arg = Some(arg.clone());
            }
        }
        operation
    }

    fn instruction(&mut self, v: &Value, path: &str, depth: usize) -> Option<Instruction> {
        let (ty, node) = self.header(v, path, depth, INSTRUCTIONS, "an instruction")?;
        let tag = node.get("tag").and_then(Value::as_str).map(str::to_owned);
        let kind = match ty {
            "set" => {
                let at = child(path, "set");
                let (name, to) = self.single(field(node, "set"), &at, depth + 1)?;
                if name.is_empty() {
                    self.fail(at, "Expected a non-empty string");
                    return None;
                }
                let to = self.input(to, &child(&at, name), depth + 2)?;
                InstructionKind::Set {
                    name: name.clone(),
                    to,
                }
            }
            "ret" => InstructionKind::Ret(self.input(
                field(node, "ret"),
                &child(path, "ret"),
                depth + 1,
            )?),
            _ => {
                // "if" or "for". An `el` on a loop is already an unknown
                // field, but it's checked all the same.
                let test = self.input(field(node, ty), &child(path, ty), depth + 1);
                let body = self.block(field(node, "do"), &child(path, "do"), depth + 1);
                let otherwise = node
                    .get("el")
                    .map(|el| self.block(el, &child(path, "el"), depth + 1));
                let (test, body) = (test?, body?);
                if ty == "if" {
                    InstructionKind::If {
                        test,
                        then: body,
                        otherwise: match otherwise {
                            Some(el) => Some(el?),
                            None => None,
                        },
                    }
                } else {
                    InstructionKind::For { test, body }
                }
            }
        };
        Some(Instruction {
            path: path.into(),
            tag,
            kind,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_escapes_tilde_and_slash() {
        assert_eq!(pointer(&["a/b", "c~d", "0"]), "/a~1b/c~0d/0");
        assert_eq!(pointer(&[]), "");
        assert_eq!(escape_key("~/"), "~0~1");
    }
}
