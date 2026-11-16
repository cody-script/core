//! The expression language `{ "x": "…" }` nodes use: numbers, 'text', true,
//! false, null, variable names, `+ - * /`, parentheses, one comparison
//! (`== != < <= > >=`), `&& ||` and `!`. Parsed once at compile time by hand
//! and evaluated against the run's scope by walking the tree.
//!
//! The tree is a flat arena and evaluation uses an explicit stack, so neither
//! running nor dropping a long chain like `a + a + … + a` recurses: an
//! untrusted expression can't overflow the host's stack.

use std::cmp::Ordering;

use crate::error::Error;
use crate::value::{Number, Value};

/// A parsed expression.
#[derive(Clone, Debug)]
pub(crate) struct Ast {
    nodes: Vec<Node>,
    root: usize,
}

#[derive(Clone, Debug)]
enum Node {
    /// Text, `true`, `false` or `null`.
    Lit(Value),
    /// A number literal.
    Num(f64),
    /// A variable.
    Name(String),
    Unary(Unary, usize),
    Binary(Op, usize, usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unary {
    Not,
    Neg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
}

impl Op {
    fn as_str(self) -> &'static str {
        match self {
            Op::Or => "||",
            Op::And => "&&",
            Op::Eq => "==",
            Op::Ne => "!=",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Add => "+",
            Op::Sub => "-",
            Op::Mul => "*",
            Op::Div => "/",
        }
    }
}

/// Binary operators by precedence, loosest first.
const LEVELS: [&[Op]; 5] = [
    &[Op::Or],
    &[Op::And],
    &[Op::Eq, Op::Ne, Op::Lt, Op::Le, Op::Gt, Op::Ge],
    &[Op::Add, Op::Sub],
    &[Op::Mul, Op::Div],
];
const COMPARISON: usize = 2;

/// The operator at level `p` spelled `symbol`, if there is one.
fn level_op(p: usize, symbol: Option<&str>) -> Option<Op> {
    let symbol = symbol?;
    LEVELS[p].iter().copied().find(|op| op.as_str() == symbol)
}

// ── Tokens ──────────────────────────────────────────────────────────────────

enum Token<'s> {
    /// A number literal.
    Num(f64),
    /// Text, `true`, `false` or `null`.
    Lit(Value),
    /// A name or an operator.
    Sym(&'s str),
}

/// JavaScript's `\s`: WhiteSpace and LineTerminator.
fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// What JavaScript's `.` doesn't match.
fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The length of the run of `pred` characters starting `s`.
fn span(s: &str, pred: impl Fn(char) -> bool) -> usize {
    s.find(|c: char| !pred(c)).unwrap_or(s.len())
}

/// The token starting at `source[start..]` and where it ends, if one does.
fn token_at(source: &str, start: usize) -> Option<(Token<'_>, usize)> {
    let rest = &source[start..];
    let c = rest.chars().next()?;
    // A number: \d+(\.\d+)?
    if c.is_ascii_digit() {
        let mut end = start + span(rest, |c| c.is_ascii_digit());
        let tail = &source[end..];
        if tail.starts_with('.') && tail[1..].starts_with(|c: char| c.is_ascii_digit()) {
            end += 1 + span(&tail[1..], |c| c.is_ascii_digit());
        }
        let n = source[start..end].parse::<f64>().unwrap_or(f64::INFINITY);
        return Some((Token::Num(n), end));
    }
    // Text: '…', where \x stands for x (not a line break).
    if c == '\'' {
        let mut text = String::new();
        let mut chars = rest.char_indices().skip(1);
        while let Some((i, ch)) = chars.next() {
            match ch {
                '\'' => return Some((Token::Lit(Value::String(text)), start + i + 1)),
                '\\' => match chars.next() {
                    Some((_, escaped)) if !is_line_terminator(escaped) => text.push(escaped),
                    _ => return None,
                },
                _ => text.push(ch),
            }
        }
        return None;
    }
    // A name or keyword: [A-Za-z_]\w*
    if c.is_ascii_alphabetic() || c == '_' {
        let end = start + span(rest, is_word);
        let word = &source[start..end];
        let token = match word {
            "true" => Token::Lit(Value::Bool(true)),
            "false" => Token::Lit(Value::Bool(false)),
            "null" => Token::Lit(Value::Null),
            _ => Token::Sym(word),
        };
        return Some((token, end));
    }
    for op in ["==", "!=", "<=", ">=", "&&", "||"] {
        if rest.starts_with(op) {
            return Some((Token::Sym(&rest[..2]), start + 2));
        }
    }
    if "-+*/()<>!".contains(c) {
        return Some((Token::Sym(&rest[..1]), start + 1));
    }
    None
}

fn tokenize(source: &str) -> Result<Vec<Token<'_>>, String> {
    let mut tokens = Vec::new();
    let mut pos = 0;
    while pos < source.len() {
        let start = pos + span(&source[pos..], is_space);
        match token_at(source, start) {
            Some((token, end)) => {
                tokens.push(token);
                pos = end;
            }
            None => match source[start..].chars().next() {
                None => break, // only whitespace left
                Some(c) => return Err(format!("Unexpected \"{c}\"")),
            },
        }
    }
    Ok(tokens)
}

// ── Parser ──────────────────────────────────────────────────────────────────

struct Parser<'s> {
    tokens: Vec<Token<'s>>,
    i: usize,
    depth: usize,
    max_depth: usize,
    nodes: Vec<Node>,
}

impl<'s> Parser<'s> {
    /// The symbol at the cursor, if the token there is one.
    fn peek(&self) -> Option<&'s str> {
        match self.tokens.get(self.i) {
            Some(Token::Sym(s)) => Some(s),
            _ => None,
        }
    }

    fn push(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    fn level(&mut self, p: usize) -> Result<usize, String> {
        if p == LEVELS.len() {
            return self.unary();
        }
        let mut left = self.level(p + 1)?;
        while let Some(op) = level_op(p, self.peek()) {
            self.i += 1;
            let right = self.level(p + 1)?;
            left = self.push(Node::Binary(op, left, right));
            if p == COMPARISON {
                if level_op(p, self.peek()).is_some() {
                    return Err("Comparisons can't be chained; use &&".into());
                }
                break;
            }
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<usize, String> {
        self.depth += 1;
        if self.depth > self.max_depth {
            return Err(format!(
                "Expression nests deeper than {} levels",
                self.max_depth
            ));
        }
        let token = self.tokens.get_mut(self.i).map(|t| match t {
            Token::Num(n) => Token::Num(*n),
            Token::Lit(v) => Token::Lit(v.take()),
            Token::Sym(s) => Token::Sym(s),
        });
        self.i += 1;
        let ast = match token {
            None => return Err("Expression ends too early".into()),
            Some(Token::Num(n)) => self.push(Node::Num(n)),
            Some(Token::Lit(v)) => self.push(Node::Lit(v)),
            Some(Token::Sym("!")) => {
                let a = self.unary()?;
                self.push(Node::Unary(Unary::Not, a))
            }
            Some(Token::Sym("-")) => {
                let a = self.unary()?;
                self.push(Node::Unary(Unary::Neg, a))
            }
            Some(Token::Sym("(")) => {
                let ast = self.level(0)?;
                if self.peek() != Some(")") {
                    return Err("Missing \")\"".into());
                }
                self.i += 1;
                ast
            }
            Some(Token::Sym(s)) if s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') => {
                self.push(Node::Name(s.to_owned()))
            }
            Some(Token::Sym(s)) => return Err(format!("Unexpected \"{s}\"")),
        };
        self.depth -= 1;
        Ok(ast)
    }
}

/// Parses `source`; the error says what's wrong.
pub(crate) fn parse(source: &str, max_depth: usize) -> Result<Ast, String> {
    let mut parser = Parser {
        tokens: tokenize(source)?,
        i: 0,
        depth: 0,
        max_depth,
        nodes: Vec::new(),
    };
    let root = parser.level(0)?;
    if let Some(token) = parser.tokens.get(parser.i) {
        return Err(match token {
            Token::Sym(s) => format!("Unexpected \"{s}\""),
            Token::Num(n) => format!("Unexpected {}", stringify_number(*n)),
            Token::Lit(v) => format!("Unexpected {}", stringify_literal(v)),
        });
    }
    Ok(Ast {
        nodes: parser.nodes,
        root,
    })
}

// ── Evaluation ──────────────────────────────────────────────────────────────

/// What `evaluate` needs from the engine.
pub(crate) trait Context<'v> {
    /// A variable's value, if it's defined.
    fn get(&self, name: &str) -> Option<&'v Value>;
    /// Counts one step; fails past the limit or once aborted.
    fn step(&mut self) -> Result<(), Error>;
}

/// Why an evaluation failed.
#[derive(Debug)]
pub(crate) enum Fault {
    /// This variable isn't defined.
    Undefined(String),
    /// A type error, with its message.
    Type(String),
    /// A step failed: step limit or abort.
    Stop(Error),
}

/// A value during evaluation: borrowed from the tree or the scope where
/// possible. A number remembers the JSON number it was read from, if any, so
/// `a || 0` returns `a` exactly.
#[derive(Clone, Copy)]
enum V<'v> {
    Null,
    Bool(bool),
    Num(f64, Option<&'v Number>),
    Str(&'v str),
    List(&'v Value),
    Obj(&'v Value),
}

impl<'v> V<'v> {
    fn of(value: &'v Value) -> Self {
        match value {
            Value::Null => V::Null,
            Value::Bool(b) => V::Bool(*b),
            Value::Number(n) => V::Num(n.as_f64(), Some(n)),
            Value::String(s) => V::Str(s),
            Value::Array(_) => V::List(value),
            Value::Object(_) => V::Obj(value),
        }
    }

    fn kind(self) -> &'static str {
        match self {
            V::Null => "null",
            V::Bool(_) => "boolean",
            V::Num(..) => "number",
            V::Str(_) => "string",
            V::List(_) => "list",
            V::Obj(_) => "object",
        }
    }

    /// JavaScript truthiness.
    fn truthy(self) -> bool {
        match self {
            V::Null => false,
            V::Bool(b) => b,
            V::Num(n, _) => n != 0.0 && !n.is_nan(),
            V::Str(s) => !s.is_empty(),
            V::List(_) | V::Obj(_) => true,
        }
    }

    /// An array or object: not comparable with `==` or ordering.
    fn is_list(self) -> bool {
        matches!(self, V::List(_) | V::Obj(_))
    }

    fn into_value(self) -> Value {
        match self {
            V::Null => Value::Null,
            V::Bool(b) => Value::Bool(b),
            V::Num(_, Some(n)) => Value::Number(*n),
            V::Num(n, None) => number(n),
            V::Str(s) => Value::String(s.to_owned()),
            V::List(v) | V::Obj(v) => v.clone(),
        }
    }
}

/// JavaScript's `===` on values that aren't lists or objects.
fn strict_eq(a: V<'_>, c: V<'_>) -> bool {
    match (a, c) {
        (V::Null, V::Null) => true,
        (V::Bool(x), V::Bool(y)) => x == y,
        (V::Num(x, _), V::Num(y, _)) => x == y,
        (V::Str(x), V::Str(y)) => x == y,
        _ => false,
    }
}

/// A computed number as a value: whole numbers that fit an `i64` become
/// integers, so `1 + 2` is `3` (not `3.0`) and `1e18` is an exact integer, as
/// JavaScript would write them. Anything else stays a float — including
/// `Infinity`, which only an overflowing literal can produce.
pub(crate) fn number(n: f64) -> Value {
    // 2^63: every whole f64 below it converts to an i64 exactly.
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    if n.is_finite() && n.fract() == 0.0 && n.abs() < LIMIT {
        Value::from(n as i64)
    } else {
        Value::Number(Number::from_f64(n))
    }
}

enum Task {
    Eval(usize),
    Unary(Unary),
    /// The left operand is on the value stack; evaluate the right one.
    Right(Op, usize),
    Apply(Op),
}

/// Evaluates `ast`. `&&` and `||` short-circuit and return an operand, as in
/// JavaScript. Every node evaluated is one step, counted before its operands.
pub(crate) fn evaluate<'v>(ast: &'v Ast, cx: &mut dyn Context<'v>) -> Result<Value, Fault> {
    let mut tasks = vec![Task::Eval(ast.root)];
    let mut values: Vec<V<'v>> = Vec::new();
    let pop = |values: &mut Vec<V<'v>>| values.pop().unwrap_or(V::Null);
    while let Some(task) = tasks.pop() {
        match task {
            Task::Eval(i) => {
                cx.step().map_err(Fault::Stop)?;
                match &ast.nodes[i] {
                    Node::Lit(v) => values.push(V::of(v)),
                    Node::Num(n) => values.push(V::Num(*n, None)),
                    Node::Name(name) => match cx.get(name) {
                        Some(v) => values.push(V::of(v)),
                        None => return Err(Fault::Undefined(name.clone())),
                    },
                    Node::Unary(op, a) => {
                        tasks.push(Task::Unary(*op));
                        tasks.push(Task::Eval(*a));
                    }
                    Node::Binary(op, a, c) => {
                        tasks.push(Task::Right(*op, *c));
                        tasks.push(Task::Eval(*a));
                    }
                }
            }
            Task::Unary(op) => {
                let a = pop(&mut values);
                values.push(match (op, a) {
                    (Unary::Not, _) => V::Bool(!a.truthy()),
                    (Unary::Neg, V::Num(n, _)) => V::Num(-n, None),
                    (Unary::Neg, _) => {
                        return Err(Fault::Type(format!(
                            "\"-\" needs a number, got {}",
                            a.kind()
                        )))
                    }
                });
            }
            Task::Right(op @ (Op::And | Op::Or), c) => {
                let a = values.last().copied().unwrap_or(V::Null);
                if (op == Op::And) == a.truthy() {
                    values.pop();
                    tasks.push(Task::Eval(c));
                }
            }
            Task::Right(op, c) => {
                tasks.push(Task::Apply(op));
                tasks.push(Task::Eval(c));
            }
            Task::Apply(op) => {
                let c = pop(&mut values);
                let a = pop(&mut values);
                values.push(binary(op, a, c).map_err(Fault::Type)?);
            }
        }
    }
    Ok(pop(&mut values).into_value())
}

/// A comparison or arithmetic operator applied to two evaluated operands.
fn binary<'v>(op: Op, a: V<'v>, c: V<'v>) -> Result<V<'v>, String> {
    let sym = op.as_str();
    let pair = || format!("{} and {}", a.kind(), c.kind());
    match op {
        Op::Eq | Op::Ne => {
            if a.is_list() || c.is_list() {
                return Err(format!("\"{sym}\" can't compare {}", pair()));
            }
            Ok(V::Bool(strict_eq(a, c) == (op == Op::Eq)))
        }
        Op::Lt | Op::Le | Op::Gt | Op::Ge => {
            let order = match (a, c) {
                (V::Num(x, _), V::Num(y, _)) => x.partial_cmp(&y),
                // JavaScript orders text by UTF-16 code units.
                (V::Str(x), V::Str(y)) => Some(x.encode_utf16().cmp(y.encode_utf16())),
                _ => {
                    return Err(format!(
                        "\"{sym}\" needs two numbers or two texts, got {}",
                        pair()
                    ))
                }
            };
            Ok(V::Bool(match order {
                None => false, // NaN
                Some(o) => match op {
                    Op::Lt => o == Ordering::Less,
                    Op::Le => o != Ordering::Greater,
                    Op::Gt => o == Ordering::Greater,
                    _ => o != Ordering::Less,
                },
            }))
        }
        Op::Add | Op::Sub | Op::Mul | Op::Div => {
            let (V::Num(x, _), V::Num(y, _)) = (a, c) else {
                return Err(format!("\"{sym}\" needs numbers, got {}", pair()));
            };
            let result = match op {
                Op::Add => x + y,
                Op::Sub => x - y,
                Op::Mul => x * y,
                _ => x / y,
            };
            if !result.is_finite() {
                return Err(format!(
                    "\"{sym}\" gave {} (division by zero?)",
                    js_number(result)
                ));
            }
            Ok(V::Num(result, None))
        }
        // Short-circuited by `evaluate`; never applied.
        Op::And | Op::Or => Ok(a),
    }
}

/// A number literal as JavaScript's `JSON.stringify` writes it in a message:
/// `null` when it overflowed to `Infinity`.
fn stringify_number(n: f64) -> String {
    if n.is_finite() {
        js_number(n)
    } else {
        "null".into()
    }
}

/// A text, boolean or `null` literal as JavaScript's `JSON.stringify` writes
/// it in a message.
fn stringify_literal(v: &Value) -> String {
    match v {
        Value::String(s) => quote(s),
        Value::Bool(b) => b.to_string(),
        _ => "null".into(),
    }
}

/// `s` in double quotes, escaped as `JSON.stringify` escapes it.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str(r#"\""#),
            '\\' => out.push_str(r"\\"),
            '\n' => out.push_str(r"\n"),
            '\r' => out.push_str(r"\r"),
            '\t' => out.push_str(r"\t"),
            '\u{8}' => out.push_str(r"\b"),
            '\u{c}' => out.push_str(r"\f"),
            c if c < ' ' => out.push_str(&format!(r"\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Formats a number as JavaScript's `String(n)` does.
pub(crate) fn js_number(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if n == 0.0 {
        return "0".into();
    }
    let sign = if n < 0.0 { "-" } else { "" };
    // Rust's `{:e}` gives the shortest round-trip digits, as JavaScript does.
    let sci = format!("{:e}", n.abs());
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    // The decimal point sits after `point` digits.
    let point = exp.parse::<i32>().unwrap_or(0) + 1;
    let body = if k <= point && point <= 21 {
        format!("{digits}{}", "0".repeat((point - k) as usize))
    } else if 0 < point && point <= 21 {
        format!(
            "{}.{}",
            &digits[..point as usize],
            &digits[point as usize..]
        )
    } else if -6 < point && point <= 0 {
        format!("0.{}{digits}", "0".repeat((-point) as usize))
    } else {
        let e = point - 1;
        let e = if e < 0 {
            format!("-{}", -e)
        } else {
            format!("+{e}")
        };
        if k == 1 {
            format!("{digits}e{e}")
        } else {
            format!("{}.{}e{e}", &digits[..1], &digits[1..])
        }
    };
    format!("{sign}{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_number_matches_javascript() {
        for (n, s) in [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-2.5, "-2.5"),
            (0.1, "0.1"),
            (0.000001, "0.000001"),
            (0.0000001, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (123456789.0, "123456789"),
            (1e20, "100000000000000000000"),
            (1e21, "1e+21"),
            (1.25e22, "1.25e+22"),
            (f64::MAX, "1.7976931348623157e+308"),
            (5e-324, "5e-324"),
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
            (f64::NAN, "NaN"),
        ] {
            assert_eq!(js_number(n), s, "{n:e}");
        }
    }

    #[test]
    fn number_is_an_integer_when_it_can_be() {
        assert_eq!(number(3.0).as_i64(), Some(3));
        assert_eq!(number(-0.0).as_i64(), Some(0));
        assert_eq!(number(1e18).as_i64(), Some(1_000_000_000_000_000_000));
        assert!(number(2.5).as_number().unwrap().is_f64());
        assert!(number(-9_223_372_036_854_775_808.0)
            .as_number()
            .unwrap()
            .is_f64());
        assert_eq!(number(f64::INFINITY).as_f64(), Some(f64::INFINITY));
    }

    #[test]
    fn literals_in_messages_are_written_as_json_stringify_writes_them() {
        let text = Value::from("a\"b\\c\n\t\u{1}\u{8}\u{c}\r");
        assert_eq!(stringify_literal(&text), r#""a\"b\\c\n\t\u0001\b\f\r""#);
        assert_eq!(stringify_literal(&Value::Bool(true)), "true");
        assert_eq!(stringify_literal(&Value::Null), "null");
        assert_eq!(stringify_number(f64::INFINITY), "null");
        assert_eq!(stringify_number(1.5), "1.5");
    }

    #[test]
    fn a_long_chain_neither_overflows_nor_recurses_on_drop() {
        let source = vec!["1"; 200_000].join("+");
        let ast = parse(&source, 128).unwrap();
        struct Cx;
        impl<'v> Context<'v> for Cx {
            fn get(&self, _: &str) -> Option<&'v Value> {
                None
            }
            fn step(&mut self) -> Result<(), Error> {
                Ok(())
            }
        }
        assert_eq!(evaluate(&ast, &mut Cx).unwrap(), Value::from(200_000));
        drop(ast);
    }
}
