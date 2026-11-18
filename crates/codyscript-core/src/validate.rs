//! Checks a program in one walk — shape, unknown fields, finite numbers,
//! nesting depth, reserved keys, the functions it defines and declares, what
//! every call names and every expression's syntax — reporting every problem,
//! and builds the tree the engine runs.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;

use crate::engine::{Executors, provider};
use crate::error::{Details, Error, ErrorCode, Issue};
use crate::executor::Executor;
use crate::expression::{Ast, parse};
use crate::program::{
    Call, Expression, External, Function, Input, InputKind, Instruction, InstructionKind, Program,
};
use crate::value::{Map, Value};

/// Node types by the key that names them, with the other fields each may
/// carry besides `tag`. A node has exactly one type key; its position (input
/// or instruction) decides which set applies.
type Kinds = &'static [(&'static str, &'static [&'static str])];
const INPUTS: Kinds = &[
    ("val", &[]),
    ("ref", &[]),
    ("x", &[]),
    ("call", &["arg"]),
    ("do", &[]),
];
const INSTRUCTIONS: Kinds = &[
    ("set", &[]),
    ("ret", &[]),
    ("if", &["do", "el"]),
    ("for", &["do"]),
];

/// The fields only a program's root may carry, besides its own: the
/// CodyScript functions it defines, and the external functions it declares.
const FUNCTIONS: &str = "fn";
const EXTERNALS: &str = "ext";
const ROOT_FIELDS: &[&str] = &[FUNCTIONS, EXTERNALS];

/// Keys no object may use, as in the JavaScript engine, where they could
/// reach `Object.prototype`; refused so programs stay portable between the two.
const RESERVED: [&str; 3] = ["__proto__", "prototype", "constructor"];

/// Escapes one JSON Pointer (RFC 6901) reference token. Most keys need no
/// escaping, and aren't copied.
fn escape_key(key: &str) -> Cow<'_, str> {
    if key.contains(['~', '/']) {
        Cow::Owned(key.replace('~', "~0").replace('/', "~1"))
    } else {
        Cow::Borrowed(key)
    }
}

// Paths are kept in the program tree, so these make strings of exactly
// their length.

/// The JSON Pointer `path` extended by `key`.
fn child(path: &str, key: &str) -> String {
    let key = escape_key(key);
    let mut pointer = String::with_capacity(path.len() + 1 + key.len());
    pointer.push_str(path);
    pointer.push('/');
    pointer.push_str(&key);
    pointer
}

/// The JSON Pointer `path` extended by list index `i`.
fn child_at(path: &str, i: usize) -> String {
    let digits = i.checked_ilog10().unwrap_or(0) as usize + 1;
    let mut pointer = String::with_capacity(path.len() + 1 + digits);
    let _ = write!(pointer, "{path}/{i}"); // writing to a String can't fail
    pointer
}

/// JSON Pointer for a list of keys.
#[cfg(test)]
fn pointer(keys: &[&str]) -> String {
    keys.iter()
        .fold(String::new(), |path, key| child(&path, key))
}

static NULL: Value = Value::Null;

/// A node's field, or `null` when it's missing (which every check refuses
/// where a field is required).
fn field<'v>(node: &'v Map, key: &str) -> &'v Value {
    node.get(key).unwrap_or(&NULL)
}

/// Whether [`Walk::value`] could refuse `v`: it's a container, or a number
/// that isn't finite.
fn may_fail(v: &Value) -> bool {
    match v {
        Value::Array(_) | Value::Object(_) => true,
        Value::Number(n) => !n.is_finite(),
        Value::Null | Value::Bool(_) | Value::String(_) => false,
    }
}

/// An external function a program declares: where its full name's first
/// dot is, and the executor that provides it (`None` if the engine doesn't,
/// which is already an issue).
type Declared = (usize, Option<Arc<dyn Executor>>);

/// What a call's name refers to.
enum Target {
    /// One of the program's functions, by index.
    Function(usize),
    /// An external function the program declares, by its full name.
    External(String, Declared),
}

struct RawIssue {
    path: String,
    message: String,
    code: ErrorCode,
    tag: Option<String>,
}

/// Validates `program` and builds its tree, finding each external function
/// it declares among `executors`. Fails with
/// `invalid_program` or `unknown_function` (whichever problem comes first),
/// listing every issue.
pub(crate) fn validate(
    program: &Value,
    max_depth: usize,
    executors: &Executors,
) -> Result<Program, Error> {
    let mut walk = Walk {
        max_depth,
        executors,
        expressions: HashMap::new(),
        functions: HashMap::new(),
        externals: HashMap::new(),
        issues: Vec::new(),
    };
    // Every name a call may use is known before any call is checked, so
    // functions may be defined in any order and call each other.
    let root = program.as_object();
    if let Some(ext) = root.and_then(|map| map.get(EXTERNALS)) {
        walk.externals(ext);
    }
    let table = root
        .and_then(|map| map.get(FUNCTIONS))
        .and_then(|table| walk.function_names(table));
    let body = walk.node(program, String::new(), 0, ROOT_FIELDS);
    let functions = match table {
        Some(table) => walk.function_bodies(table),
        None => Some(Vec::new()),
    };
    let root = body
        .zip(functions)
        .map(|(body, functions)| Program { body, functions });
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
    /// The engine's executors: where external functions are found.
    executors: &'a Executors,
    /// Every expression's parse, by its source: each is parsed once, and
    /// nodes with the same expression share its source and parse.
    expressions: HashMap<Arc<str>, Arc<Ast>>,
    /// The program's own functions (`fn`) by name, with their index.
    functions: HashMap<String, usize>,
    /// The external functions it declares (`ext`) by full name
    /// (`text.trim`).
    externals: HashMap<String, Declared>,
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
        if let Value::Object(map) = v
            && let Some(key) = map.keys().find(|k| RESERVED.contains(&k.as_str()))
        {
            let message = format!("\"{key}\" is a reserved key");
            self.fail(child(path, key), message);
            return false;
        }
        true
    }

    /// An object with only `allowed` fields (any, if `None`).
    fn record<'v>(
        &mut self,
        v: &'v Value,
        path: &str,
        depth: usize,
        allowed: Option<&dyn Fn(&str) -> bool>,
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
                if !allowed(key) {
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
            // Items that can't fail need no path, so none is built for them.
            Value::Array(items) => {
                if self.container(v, path, depth) {
                    for (i, item) in items.iter().enumerate() {
                        if may_fail(item) {
                            self.value(item, &child_at(path, i), depth + 1);
                        }
                    }
                }
            }
            Value::Object(map) => {
                if self.container(v, path, depth) {
                    for (key, item) in map {
                        if may_fail(item) {
                            self.value(item, &child(path, key), depth + 1);
                        }
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
            let ins = self.instruction(item, child_at(path, i), depth + 1);
            match (ins, list.as_mut()) {
                (Some(ins), Some(list)) => list.push(ins),
                _ => list = None,
            }
        }
        list
    }

    /// A node's `tag`, if it has one, must be a string.
    fn tag(&mut self, node: &Map, path: &str) {
        if let Some(tag) = node.get("tag")
            && !tag.is_string()
        {
            self.fail(child(path, "tag"), "Expected a string");
        }
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
    /// type keys, only that type's fields, a string `tag` and, where allowed
    /// (at the root), `extra` fields.
    fn header<'v>(
        &mut self,
        v: &'v Value,
        path: &str,
        depth: usize,
        (kinds, what): (Kinds, &str),
        extra: &[&'static str],
    ) -> Option<(&'static str, &'v Map)> {
        let Value::Object(map) = v else {
            self.fail(path.into(), format!("Expected {what} object"));
            return None;
        };
        let mut types = kinds.iter().filter(|(k, _)| map.contains_key(k));
        let (Some(&(ty, fields)), None) = (types.next(), types.next()) else {
            let quote = |(k, _): &(&str, _)| format!("\"{k}\"");
            let expected: Vec<String> = kinds.iter().map(quote).collect();
            let found: Vec<String> = kinds
                .iter()
                .filter(|(k, _)| map.contains_key(k))
                .map(quote)
                .collect();
            let found = if found.is_empty() {
                String::new()
            } else {
                format!(", found {}", found.join(" and "))
            };
            let message = format!(
                "Expected {what} with exactly one of {}{found}",
                expected.join(", ")
            );
            self.fail(path.into(), message);
            return None;
        };
        let allowed =
            |key: &str| key == ty || key == "tag" || fields.contains(&key) || extra.contains(&key);
        let node = self.record(v, path, depth, Some(&allowed))?;
        self.tag(node, path);
        Some((ty, node))
    }

    /// `ext`: `{ <executor>: [<function>, …], … }`, the external functions
    /// the program may call, as `<executor>.<function>`. Each must be one the
    /// engine provides: the only check that depends on the host.
    fn externals(&mut self, ext: &Value) {
        let path = child("", EXTERNALS);
        let Some(table) = self.record(ext, &path, 1, None) else {
            return;
        };
        for (exe, functions) in table {
            let at = child(&path, exe);
            let exe_ok = if exe.is_empty() {
                self.fail(at.clone(), "Expected a non-empty name");
                false
            } else if exe.contains('.') {
                self.fail(at.clone(), "Expected an executor name without a dot");
                false
            } else {
                true
            };
            let Value::Array(list) = functions else {
                self.fail(at, "Expected a list of function names");
                continue;
            };
            if !self.container(functions, &at, 2) {
                continue;
            }
            for (i, op) in list.iter().enumerate() {
                let op_path = child_at(&at, i);
                let Some(op) = self.name(op, &op_path) else {
                    continue;
                };
                if !exe_ok {
                    continue;
                }
                let name = format!("{exe}.{op}");
                if self.externals.contains_key(&name) {
                    self.fail(op_path, format!("Duplicate function \"{name}\""));
                    continue;
                }
                let executor = provider(self.executors, exe, op).cloned();
                if executor.is_none() {
                    self.issues.push(RawIssue {
                        path: op_path,
                        message: format!("Unknown function \"{name}\""),
                        code: ErrorCode::UnknownFunction,
                        tag: None,
                    });
                }
                self.externals.insert(name, (exe.len(), executor));
            }
        }
    }

    /// `fn`'s names: `{ <name>: <input>, … }`, the program's own functions.
    /// Returns the table, for [`function_bodies`](Self::function_bodies) to
    /// check once every name is known.
    fn function_names<'v>(&mut self, table: &'v Value) -> Option<&'v Map> {
        let path = child("", FUNCTIONS);
        let table = self.record(table, &path, 1, None)?;
        for (index, name) in table.keys().enumerate() {
            if name.is_empty() {
                self.fail(child(&path, name), "Expected a non-empty name");
                continue;
            }
            if self.externals.contains_key(name) {
                let message =
                    format!("\"{name}\" is both defined in \"fn\" and declared in \"ext\"");
                self.fail(child(&path, name), message);
            }
            self.functions.insert(name.clone(), index);
        }
        Some(table)
    }

    /// Every function in `fn`, each a program of its own, in order; `None`
    /// if any is invalid (each is checked, even after one fails).
    fn function_bodies(&mut self, table: &Map) -> Option<Vec<Function>> {
        let path = child("", FUNCTIONS);
        let mut functions = Some(Vec::with_capacity(table.len()));
        for (name, body) in table {
            let body = self.input(body, child(&path, name), 2);
            match (body, functions.as_mut()) {
                (Some(body), Some(list)) => list.push(Function {
                    name: name.clone(),
                    body,
                }),
                _ => functions = None,
            }
        }
        functions
    }

    /// A literal (`null`, boolean, finite number, string) or an input node.
    fn input(&mut self, v: &Value, path: String, depth: usize) -> Option<Input> {
        self.node(v, path, depth, &[])
    }

    /// An input, which may also carry the `extra` fields (the root's).
    fn node(
        &mut self,
        v: &Value,
        path: String,
        depth: usize,
        extra: &[&'static str],
    ) -> Option<Input> {
        if !(v.is_object() || v.is_array()) {
            self.value(v, &path, depth);
            return Some(Input {
                path,
                tag: None,
                kind: InputKind::Const(v.clone()),
            });
        }
        let (ty, node) = self.header(v, &path, depth, (INPUTS, "an input"), extra)?;
        let tag = node.get("tag").and_then(Value::as_str).map(str::to_owned);
        let kind = match ty {
            "val" => {
                let val = field(node, "val");
                self.value(val, &child(&path, "val"), depth + 1);
                Some(InputKind::Const(val.clone()))
            }
            "ref" => self
                .name(field(node, "ref"), &child(&path, "ref"))
                .map(|name| InputKind::Ref(name.to_owned())),
            "x" => self.expression(field(node, "x"), child(&path, "x")),
            "call" => self.call(node, &path, depth, tag.as_deref()),
            _ => self
                .block(field(node, "do"), &child(&path, "do"), depth + 1)
                .map(InputKind::Do),
        }?;
        Some(Input { path, tag, kind })
    }

    fn expression(&mut self, v: &Value, path: String) -> Option<InputKind> {
        let source = self.name(v, &path)?;
        if !self.expressions.contains_key(source) {
            match parse(source, self.max_depth) {
                Ok(ast) => {
                    self.expressions.insert(source.into(), Arc::new(ast));
                }
                Err(message) => {
                    self.fail(path, format!("{message} in \"{source}\""));
                    return None;
                }
            }
        }
        let (source, ast) = self.expressions.get_key_value(source)?;
        Some(InputKind::X(Expression {
            source: Arc::clone(source),
            ast: Arc::clone(ast),
            path,
        }))
    }

    /// `{ "call": { <function>: { <name>: <input>, … } }, "arg"?: <any> }`: a
    /// call of one of the program's functions (`fn`), whose variables the
    /// arguments become, or of an external one it declares (`ext`), which
    /// alone may take a static `arg`.
    fn call(
        &mut self,
        node: &Map,
        path: &str,
        depth: usize,
        tag: Option<&str>,
    ) -> Option<InputKind> {
        let call_path = child(path, "call");
        let mut call = None;
        if let Some((name, args)) = self.single(field(node, "call"), &call_path, depth + 1) {
            let at = child(&call_path, name);
            let target = self.target(name, &at, tag);
            let args = self.arguments(args, &at, depth + 2);
            call = target.zip(args);
        }
        let arg = node.get("arg");
        if let Some(arg) = arg {
            let at = child(path, "arg");
            self.value(arg, &at, depth + 1);
            if let Some((Target::Function(_), _)) = call {
                self.fail(at, "\"arg\" is only for external functions");
                return None;
            }
        }
        Some(match call? {
            (Target::Function(function), args) => InputKind::Call(Call { function, args }),
            (Target::External(name, (dot, executor)), args) => {
                InputKind::External(Box::new(External {
                    // An unknown function is already an issue.
                    executor: executor?,
                    name,
                    dot,
                    args,
                    arg: arg.cloned(),
                }))
            }
        })
    }

    /// What a call's name refers to: a function the program defines or one
    /// it declares.
    fn target(&mut self, name: &str, at: &str, tag: Option<&str>) -> Option<Target> {
        if let Some(&index) = self.functions.get(name) {
            return Some(Target::Function(index));
        }
        if let Some(declared) = self.externals.get(name) {
            return Some(Target::External(name.to_owned(), declared.clone()));
        }
        // Most likely a host function the program forgot to declare.
        let provided = name
            .split_once('.')
            .is_some_and(|(exe, op)| provider(self.executors, exe, op).is_some());
        let message = if provided {
            format!("Function \"{name}\" isn't declared in \"ext\"")
        } else {
            format!("Unknown function \"{name}\"")
        };
        self.issues.push(RawIssue {
            path: at.to_owned(),
            message,
            code: ErrorCode::InvalidProgram,
            tag: tag.map(str::to_owned),
        });
        None
    }

    /// A call's arguments, `{ <name>: <input>, … }`, in order.
    fn arguments(&mut self, args: &Value, at: &str, depth: usize) -> Option<Vec<(String, Input)>> {
        let map = self.record(args, at, depth, None)?;
        let mut list = Some(Vec::with_capacity(map.len()));
        for (arg, from) in map {
            let arg_path = child(at, arg);
            if arg.is_empty() {
                self.fail(arg_path.clone(), "Expected a non-empty name");
                list = None;
            }
            let input = self.input(from, arg_path, depth + 1);
            match (input, list.as_mut()) {
                (Some(input), Some(list)) => list.push((arg.clone(), input)),
                _ => list = None,
            }
        }
        list
    }

    fn instruction(&mut self, v: &Value, path: String, depth: usize) -> Option<Instruction> {
        let (ty, node) = self.header(v, &path, depth, (INSTRUCTIONS, "an instruction"), &[])?;
        let tag = node.get("tag").and_then(Value::as_str).map(str::to_owned);
        let kind = match ty {
            "set" => {
                let at = child(&path, "set");
                let (name, to) = self.single(field(node, "set"), &at, depth + 1)?;
                if name.is_empty() {
                    self.fail(at, "Expected a non-empty string");
                    return None;
                }
                let to = self.input(to, child(&at, name), depth + 2)?;
                InstructionKind::Set {
                    name: name.clone(),
                    to,
                }
            }
            "ret" => InstructionKind::Ret(self.input(
                field(node, "ret"),
                child(&path, "ret"),
                depth + 1,
            )?),
            _ => {
                // "if" or "for". An `el` on a loop is already an unknown
                // field, but it's checked all the same.
                let test = self.input(field(node, ty), child(&path, ty), depth + 1);
                let body = self.block(field(node, "do"), &child(&path, "do"), depth + 1);
                let otherwise = node
                    .get("el")
                    .map(|el| self.block(el, &child(&path, "el"), depth + 1));
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
        Some(Instruction { path, tag, kind })
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

    #[test]
    fn paths_are_exactly_their_length() {
        for (i, expected) in [(0, "/a/0"), (9, "/a/9"), (10, "/a/10"), (12345, "/a/12345")] {
            let path = child_at("/a", i);
            assert_eq!(path, expected);
            assert_eq!(path.capacity(), path.len());
        }
        let path = child("/a", "b/c");
        assert_eq!(path, "/a/b~1c");
        assert_eq!(path.capacity(), path.len());
    }
}
