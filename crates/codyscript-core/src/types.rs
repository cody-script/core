//! Types: what a program says its values are. A program declares them by
//! name in `types` — objects, lists and maps of other types, written in the
//! shapes of the data they describe — and checks values against them with
//! `is` and `problems`; a host checks values against them with
//! [`CompiledProgram::problems`](crate::CompiledProgram::problems).
//!
//! Checking walks the value with an explicit stack, as running does: a value
//! however deep (a host's, or one a program built in a loop) can't overflow
//! it.

use std::sync::Arc;

use crate::error::Issue;
use crate::expression::kind;
use crate::value::Value;

/// Problems a check reports, at most: past them, it stops looking.
pub(crate) const MAX_PROBLEMS: usize = 100;

/// The types every program has, by name.
pub(crate) const BUILT_IN: &[(&str, Type)] = &[
    ("any", Type::Any),
    ("null", Type::Null),
    ("boolean", Type::Boolean),
    ("number", Type::Number),
    ("integer", Type::Integer),
    ("string", Type::String),
    ("list", Type::List),
    ("object", Type::Object),
];

/// A type, as a program writes it: a name, `T?`, or an `obj`, `list` or
/// `map` of other types.
#[derive(Clone, Debug)]
pub(crate) enum Type {
    /// Any value at all, `null` included.
    Any,
    Null,
    Boolean,
    Number,
    /// A whole number (`2`, or `2.0`).
    Integer,
    String,
    /// Any list.
    List,
    /// Any object.
    Object,
    /// A type the program declares, by its index in [`TypeDef`]s.
    Named(usize),
    /// `T?`: a `T` or `null` — and, as an object's field, one it may leave out.
    Optional(Box<Type>),
    /// `{ "obj": { <field>: <type>, … } }`: an object with these fields, and
    /// no others.
    Obj(Vec<(Arc<str>, Type)>),
    /// `{ "list": <type> }`: a list whose every item is one.
    ListOf(Box<Type>),
    /// `{ "map": <type> }`: an object whose every field's value is one.
    MapOf(Box<Type>),
}

/// A type the program declares (`types`), by name.
#[derive(Clone, Debug)]
pub(crate) struct TypeDef {
    pub name: String,
    pub ty: Type,
}

/// What `ty` is under its names and `?`s, and whether it may be `null`.
fn resolve<'t>(defs: &'t [TypeDef], mut ty: &'t Type) -> (&'t Type, bool) {
    // A declared type is never only itself (`validate` refuses one that
    // is), so this ends.
    let mut nullable = false;
    loop {
        match ty {
            Type::Named(i) => ty = &defs[*i].ty,
            Type::Optional(inner) => {
                nullable = true;
                ty = inner;
            }
            Type::Any | Type::Null => return (ty, true),
            _ => return (ty, nullable),
        }
    }
}

/// What a value of `ty` is, for messages: `a string`, `Todo`, `a list or null`.
fn describe(defs: &[TypeDef], ty: &Type) -> String {
    match ty {
        Type::Any => "anything".into(),
        Type::Null => "null".into(),
        Type::Boolean => "a boolean".into(),
        Type::Number => "a number".into(),
        Type::Integer => "an integer".into(),
        Type::String => "a string".into(),
        Type::List | Type::ListOf(_) => "a list".into(),
        Type::Object | Type::Obj(_) | Type::MapOf(_) => "an object".into(),
        Type::Named(i) => defs[*i].name.clone(),
        Type::Optional(inner) => format!("{} or null", describe(defs, inner)),
    }
}

/// Where a value is, inside the one checked: a JSON Pointer, kept as a chain
/// of steps and only written out for a problem.
enum Step<'v> {
    Root,
    Field(usize, &'v str),
    Item(usize, usize),
}

fn pointer(steps: &[Step<'_>], mut at: usize) -> String {
    let mut parts = Vec::new();
    loop {
        match steps[at] {
            Step::Root => break,
            Step::Field(parent, key) => {
                parts.push(key.replace('~', "~0").replace('/', "~1"));
                at = parent;
            }
            Step::Item(parent, i) => {
                parts.push(i.to_string());
                at = parent;
            }
        }
    }
    parts.iter().rev().map(|part| format!("/{part}")).collect()
}

/// What's wrong with `value` as a `ty` — nothing, if it is one — in the
/// order found (each field in the order its type lists them), at most
/// [`MAX_PROBLEMS`]. `step` is called once for each value looked at, and may
/// stop the check (a run's step limit).
pub(crate) fn problems<E>(
    defs: &[TypeDef],
    value: &Value,
    ty: &Type,
    mut step: impl FnMut() -> Result<(), E>,
) -> Result<Vec<Issue>, E> {
    let mut found = Vec::new();
    let mut steps = vec![Step::Root];
    let mut pending = vec![(value, ty, 0)];
    while let Some((value, written, at)) = pending.pop() {
        step()?;
        let (ty, nullable) = resolve(defs, written);
        if nullable && value.is_null() {
            continue;
        }
        let fits = match (ty, value) {
            (Type::Any, _)
            | (Type::Null, Value::Null)
            | (Type::Boolean, Value::Bool(_))
            | (Type::Number, Value::Number(_))
            | (Type::String, Value::String(_))
            | (Type::List, Value::Array(_))
            | (Type::Object, Value::Object(_)) => true,
            (Type::Integer, Value::Number(n)) => {
                let n = n.as_f64();
                n.is_finite() && n.fract() == 0.0
            }
            (Type::ListOf(item), Value::Array(items)) => {
                for (i, value) in items.iter().enumerate().rev() {
                    steps.push(Step::Item(at, i));
                    pending.push((value, item, steps.len() - 1));
                }
                true
            }
            (Type::MapOf(of), Value::Object(map)) => {
                let fields: Vec<_> = map.iter().collect();
                for (key, value) in fields.into_iter().rev() {
                    steps.push(Step::Field(at, key));
                    pending.push((value, of, steps.len() - 1));
                }
                true
            }
            (Type::Obj(fields), Value::Object(map)) => {
                // Fields it has that the type hasn't, then those it lacks,
                // then what's in each it has, in the type's order (the first
                // pushed last, so checked first).
                for (key, _) in map {
                    if !fields.iter().any(|(name, _)| &**name == key) {
                        steps.push(Step::Field(at, key));
                        found.push(Issue {
                            path: pointer(&steps, steps.len() - 1),
                            message: format!("Unknown field \"{key}\""),
                        });
                    }
                }
                for (name, field) in fields {
                    // An optional field may be left out.
                    if !map.contains_key(name) && !resolve(defs, field).1 {
                        found.push(Issue {
                            path: pointer(&steps, at),
                            message: format!("Missing field \"{name}\""),
                        });
                    }
                }
                for (name, field) in fields.iter().rev() {
                    if let Some(value) = map.get(name) {
                        steps.push(Step::Field(at, name));
                        pending.push((value, field, steps.len() - 1));
                    }
                }
                true
            }
            _ => false,
        };
        if !fits {
            found.push(Issue {
                path: pointer(&steps, at),
                message: format!("Expected {}, got {}", describe(defs, written), kind(value)),
            });
        }
        if found.len() >= MAX_PROBLEMS {
            found.truncate(MAX_PROBLEMS);
            break;
        }
    }
    Ok(found)
}

/// A problem as a value, for `problems`: `{ "path": …, "message": … }`.
pub(crate) fn as_value(issue: Issue) -> Value {
    let mut map = crate::value::Map::new();
    map.insert("path", Value::String(issue.path));
    map.insert("message", Value::String(issue.message));
    Value::Object(map)
}
