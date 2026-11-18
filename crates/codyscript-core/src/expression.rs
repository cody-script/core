//! The expression language `{ "x": "…" }` nodes use: numbers, 'text', true,
//! false, null, variable names, `+ - * /`, parentheses, one comparison
//! (`== != < <= > >=`), `&& ||` and `!` (on booleans only), and `??`.
//! Parsed once at compile time by hand and evaluated against the run's scope
//! by walking the tree.
//!
//! The tree is a flat arena and evaluation uses an explicit stack, so neither
//! running nor dropping a long chain like `a + a + … + a` recurses: an
//! untrusted expression can't overflow the host's stack.

use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

use crate::value::{Number, Value};

/// A parsed expression, with its source.
#[derive(Clone, Debug)]
pub(crate) struct Ast {
    /// The expression as written.
    pub source: Arc<str>,
    nodes: Vec<Node>,
    root: usize,
}

#[derive(Clone, Debug)]
enum Node {
    /// Text, `true`, `false` or `null`.
    Lit(Value),
    /// A number literal.
    Num(f64),
    /// A variable. (Its own copy of the name: reading it is the hot path.)
    Name(Box<str>),
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
    /// `??`: the left side unless it's `null`, else the right.
    Default,
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
            Op::Default => "??",
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

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Binary operators by precedence, loosest first.
const LEVELS: [&[Op]; 6] = [
    &[Op::Default],
    &[Op::Or],
    &[Op::And],
    &[Op::Eq, Op::Ne, Op::Lt, Op::Le, Op::Gt, Op::Ge],
    &[Op::Add, Op::Sub],
    &[Op::Mul, Op::Div],
];
const COMPARISON: usize = 3;

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

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The length of the run of `pred` characters starting `s`.
fn span(s: &str, pred: impl Fn(char) -> bool) -> usize {
    s.find(|c: char| !pred(c)).unwrap_or(s.len())
}

/// The token starting at `source[start..]`, which isn't whitespace, and
/// where it ends.
fn token_at(source: &str, start: usize) -> Result<(Token<'_>, usize), String> {
    let rest = &source[start..];
    let c = rest.chars().next().unwrap_or(' ');
    // A number: \d+(\.\d+)?
    if c.is_ascii_digit() {
        let mut end = start + span(rest, |c| c.is_ascii_digit());
        let tail = &source[end..];
        if tail.starts_with('.') && tail[1..].starts_with(|c: char| c.is_ascii_digit()) {
            end += 1 + span(&tail[1..], |c| c.is_ascii_digit());
        }
        let digits = &source[start..end];
        return match digits.parse::<f64>() {
            Ok(n) if n.is_finite() => Ok((Token::Num(n), end)),
            _ => Err(format!("Number \"{digits}\" is too large")),
        };
    }
    if c == '\'' {
        let (text, len) = text(rest)?;
        return Ok((Token::Lit(Value::String(text)), start + len));
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
        return Ok((token, end));
    }
    for op in ["==", "!=", "<=", ">=", "&&", "||", "??"] {
        if rest.starts_with(op) {
            return Ok((Token::Sym(&rest[..2]), start + 2));
        }
    }
    if "-+*/()<>!".contains(c) {
        return Ok((Token::Sym(&rest[..1]), start + 1));
    }
    Err(format!("Unexpected \"{c}\""))
}

/// Text in single quotes at the start of `rest`, and its length there. `\n`
/// is a line break, `\t` a tab, `\\` a backslash and `\'` a quote; any other
/// escape is refused.
fn text(rest: &str) -> Result<(String, usize), String> {
    let mut text = String::new();
    let mut chars = rest.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        match c {
            '\'' => return Ok((text, i + 1)),
            '\\' => text.push(match chars.next() {
                Some((_, 'n')) => '\n',
                Some((_, 't')) => '\t',
                Some((_, '\\')) => '\\',
                Some((_, '\'')) => '\'',
                Some((_, other)) => {
                    let other = other.escape_debug();
                    return Err(format!(
                        r#"Unknown escape "\{other}"; use \n, \t, \\ or \'"#
                    ));
                }
                None => break,
            }),
            c => text.push(c),
        }
    }
    Err("Text has no closing \"'\"".into())
}

/// The tokens of `source`, each with its text as written.
fn tokenize(source: &str) -> Result<Vec<(Token<'_>, &str)>, String> {
    let mut tokens = Vec::new();
    let mut pos = 0;
    loop {
        pos += span(&source[pos..], char::is_whitespace);
        if pos == source.len() {
            return Ok(tokens);
        }
        let (token, end) = token_at(source, pos)?;
        tokens.push((token, &source[pos..end]));
        pos = end;
    }
}

// ── Parser ──────────────────────────────────────────────────────────────────

struct Parser<'s> {
    /// Each with its text as written, for messages.
    tokens: Vec<(Token<'s>, &'s str)>,
    i: usize,
    depth: usize,
    max_depth: usize,
    nodes: Vec<Node>,
}

impl<'s> Parser<'s> {
    /// The symbol at the cursor, if the token there is one.
    fn peek(&self) -> Option<&'s str> {
        match self.tokens.get(self.i) {
            Some((Token::Sym(s), _)) => Some(s),
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
        let token = self.tokens.get_mut(self.i).map(|(t, _)| match t {
            Token::Num(n) => Token::Num(*n),
            Token::Lit(v) => Token::Lit(v.take()),
            Token::Sym(s) => Token::Sym(s),
        });
        self.i += 1;
        let Some(token) = token else {
            return Err("Expression ends too early".into());
        };
        let ast = match token {
            Token::Num(n) => self.push(Node::Num(n)),
            Token::Lit(v) => self.push(Node::Lit(v)),
            Token::Sym("!") => {
                let a = self.unary()?;
                self.push(Node::Unary(Unary::Not, a))
            }
            Token::Sym("-") => {
                let a = self.unary()?;
                self.push(Node::Unary(Unary::Neg, a))
            }
            Token::Sym("(") => {
                let ast = self.level(0)?;
                if self.peek() != Some(")") {
                    return Err("Missing \")\"".into());
                }
                self.i += 1;
                ast
            }
            Token::Sym(s) if s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') => {
                self.push(Node::Name(s.into()))
            }
            Token::Sym(s) => return Err(format!("Unexpected \"{s}\"")),
        };
        self.depth -= 1;
        Ok(ast)
    }
}

/// Parses `source`; the error says what's wrong.
pub(crate) fn parse(source: &str, max_depth: usize) -> Result<Ast, String> {
    let tokens = tokenize(source)?;
    // Every node consumes a token, so this is room enough.
    let nodes = Vec::with_capacity(tokens.len());
    let mut parser = Parser {
        tokens,
        i: 0,
        depth: 0,
        max_depth,
        nodes,
    };
    let root = parser.level(0)?;
    if let Some((_, text)) = parser.tokens.get(parser.i) {
        return Err(format!("Unexpected \"{text}\""));
    }
    Ok(Ast {
        source: source.into(),
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
    fn step(&mut self) -> Result<(), Stop>;
}

/// Why a step was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    Aborted,
    StepLimit,
}

/// Why an evaluation failed.
#[derive(Debug)]
pub(crate) enum Fault {
    /// This variable isn't defined.
    Undefined(String),
    /// A type error, with its message.
    Type(String),
    /// A step was refused.
    Stop(Stop),
}

/// A value during evaluation: borrowed from the tree or the scope where
/// possible. A number remembers the [`Number`] it was read from, if any, so
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

/// Equality of values that aren't lists or objects: the same type, and
/// equal (numbers numerically, so `1 == 1.0`).
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
/// integers, so `1 + 2` is `3` (not `3.0`) and `1e18` is an exact integer.
/// Anything else stays a float — including a non-finite one, which only a
/// host's variable can bring in (`-x` of an infinite `x`).
pub(crate) fn number(n: f64) -> Value {
    // 2^63: every whole f64 below it converts to an i64 exactly.
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    if n.is_finite() && n.fract() == 0.0 && n.abs() < LIMIT {
        Value::from(n as i64)
    } else {
        Value::Number(Number::from_f64(n))
    }
}

/// What's left to do once an operand is evaluated.
#[derive(Clone, Copy)]
enum Task {
    Unary(Unary),
    /// The left operand is on the value stack; evaluate the right one.
    Right(Op, usize),
    Apply(Op),
    /// The right operand of `&&` or `||` is on the value stack, and must be
    /// a boolean.
    Boolean(Op),
}

/// Evaluates `ast`. `&&`, `||` and `??` short-circuit: their right side is
/// evaluated only when it decides the result. Every node evaluated is one
/// step, counted before its operands.
pub(crate) fn evaluate<'v>(ast: &'v Ast, cx: &mut impl Context<'v>) -> Result<Value, Fault> {
    let mut tasks = Stack::new(Task::Apply(Op::Add)); // (only a filler)
    let mut values = Stack::new(V::Null);
    let pop = |values: &mut Stack<V<'v>>| values.pop().unwrap_or(V::Null);
    // The node to evaluate now; once there's none, the next task.
    let mut next = Some(ast.root);
    loop {
        if let Some(i) = next.take() {
            cx.step().map_err(Fault::Stop)?;
            match &ast.nodes[i] {
                Node::Lit(v) => values.push(V::of(v)),
                Node::Num(n) => values.push(V::Num(*n, None)),
                Node::Name(name) => match cx.get(name) {
                    Some(v) => values.push(V::of(v)),
                    None => return Err(Fault::Undefined(name.to_string())),
                },
                Node::Unary(op, a) => {
                    tasks.push(Task::Unary(*op));
                    next = Some(*a);
                }
                Node::Binary(op, a, c) => {
                    tasks.push(Task::Right(*op, *c));
                    next = Some(*a);
                }
            }
            continue;
        }
        let Some(task) = tasks.pop() else { break };
        match task {
            Task::Unary(op) => {
                let a = pop(&mut values);
                values.push(match (op, a) {
                    (Unary::Not, V::Bool(b)) => V::Bool(!b),
                    (Unary::Neg, V::Num(n, _)) => V::Num(-n, None),
                    (Unary::Not, _) => return Err(needs("!", "a boolean", a)),
                    (Unary::Neg, _) => return Err(needs("-", "a number", a)),
                });
            }
            Task::Right(op @ (Op::And | Op::Or), c) => {
                let a = values.last().unwrap_or(V::Null);
                let V::Bool(b) = a else {
                    return Err(needs(op.as_str(), "booleans", a));
                };
                // `false && …` and `true || …` are decided: the left side.
                if (op == Op::And) == b {
                    values.pop();
                    tasks.push(Task::Boolean(op));
                    next = Some(c);
                }
            }
            Task::Right(Op::Default, c) => {
                if let Some(V::Null) = values.last() {
                    values.pop();
                    next = Some(c);
                }
            }
            Task::Right(op, c) => {
                tasks.push(Task::Apply(op));
                next = Some(c);
            }
            Task::Apply(op) => {
                let c = pop(&mut values);
                let a = pop(&mut values);
                values.push(binary(op, a, c).map_err(Fault::Type)?);
            }
            Task::Boolean(op) => {
                let c = values.last().unwrap_or(V::Null);
                if !matches!(c, V::Bool(_)) {
                    return Err(needs(op.as_str(), "booleans", c));
                }
            }
        }
    }
    Ok(pop(&mut values).into_value())
}

/// How many items a [`Stack`] holds before it needs the heap: enough for
/// any ordinary expression.
const INLINE: usize = 4;

/// A stack for `evaluate` that keeps its first [`INLINE`] items in place,
/// so evaluating an ordinary expression doesn't allocate; only a long chain
/// spills onto the heap.
struct Stack<T> {
    inline: [T; INLINE],
    len: usize,
    /// What doesn't fit inline; only used once `inline` is full.
    spill: Vec<T>,
}

impl<T: Copy> Stack<T> {
    /// An empty stack; `filler` only initializes the unused slots.
    fn new(filler: T) -> Self {
        Stack {
            inline: [filler; INLINE],
            len: 0,
            spill: Vec::new(),
        }
    }

    fn push(&mut self, item: T) {
        if self.len < INLINE {
            self.inline[self.len] = item;
            self.len += 1;
        } else {
            self.spill.push(item);
        }
    }

    fn pop(&mut self) -> Option<T> {
        if let Some(item) = self.spill.pop() {
            return Some(item);
        }
        self.len = self.len.checked_sub(1)?;
        Some(self.inline[self.len])
    }

    fn last(&self) -> Option<T> {
        match self.spill.last() {
            Some(item) => Some(*item),
            None => self.len.checked_sub(1).map(|i| self.inline[i]),
        }
    }
}

/// A comparison or arithmetic operator applied to two evaluated operands.
fn binary<'v>(op: Op, a: V<'v>, c: V<'v>) -> Result<V<'v>, String> {
    let pair = || format!("{} and {}", a.kind(), c.kind());
    match op {
        Op::Eq | Op::Ne => {
            if a.is_list() || c.is_list() {
                return Err(format!("\"{op}\" can't compare {}", pair()));
            }
            Ok(V::Bool(strict_eq(a, c) == (op == Op::Eq)))
        }
        Op::Lt | Op::Le | Op::Gt | Op::Ge => {
            let order = match (a, c) {
                (V::Num(x, _), V::Num(y, _)) => x.partial_cmp(&y),
                // Text orders by Unicode code point.
                (V::Str(x), V::Str(y)) => Some(x.cmp(y)),
                _ => {
                    return Err(format!(
                        "\"{op}\" needs two numbers or two texts, got {}",
                        pair()
                    ));
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
                return Err(format!("\"{op}\" needs numbers, got {}", pair()));
            };
            let result = match op {
                Op::Add => x + y,
                Op::Sub => x - y,
                Op::Mul => x * y,
                _ => x / y,
            };
            if op == Op::Div && y == 0.0 {
                return Err("\"/\" divided by zero".into());
            }
            if !result.is_finite() {
                return Err(format!("\"{op}\" gave {}", non_finite(result)));
            }
            Ok(V::Num(result, None))
        }
        // Short-circuited by `evaluate`; never applied.
        Op::Default | Op::And | Op::Or => Ok(a),
    }
}

/// A type error: operator `op` needs `what`, and got `got`.
fn needs(op: &str, what: &str, got: V<'_>) -> Fault {
    Fault::Type(format!("\"{op}\" needs {what}, got {}", got.kind()))
}

/// What kind of value `value` is, for messages: `null`, `boolean`,
/// `number`, `string`, `list` or `object`.
pub(crate) fn kind(value: &Value) -> &'static str {
    V::of(value).kind()
}

/// The name of a number that isn't finite, for messages.
fn non_finite(n: f64) -> &'static str {
    match n {
        f64::INFINITY => "Infinity",
        f64::NEG_INFINITY => "-Infinity",
        _ => "NaN",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_numbers_are_named() {
        assert_eq!(non_finite(f64::INFINITY), "Infinity");
        assert_eq!(non_finite(f64::NEG_INFINITY), "-Infinity");
        assert_eq!(non_finite(f64::NAN), "NaN");
    }

    #[test]
    fn number_is_an_integer_when_it_can_be() {
        assert_eq!(number(3.0).as_i64(), Some(3));
        assert_eq!(number(-0.0).as_i64(), Some(0));
        assert_eq!(number(1e18).as_i64(), Some(1_000_000_000_000_000_000));
        assert!(number(2.5).as_number().unwrap().is_f64());
        assert!(
            number(-9_223_372_036_854_775_808.0)
                .as_number()
                .unwrap()
                .is_f64()
        );
        assert_eq!(number(f64::INFINITY).as_f64(), Some(f64::INFINITY));
    }

    #[test]
    fn the_stack_is_last_in_first_out_past_its_inline_part() {
        let mut stack = Stack::new(0);
        for i in 0..INLINE * 3 {
            stack.push(i);
            assert_eq!(stack.last(), Some(i));
        }
        for i in (0..INLINE * 3).rev() {
            assert_eq!(stack.last(), Some(i));
            assert_eq!(stack.pop(), Some(i));
        }
        assert_eq!((stack.pop(), stack.last()), (None, None));
    }

    #[test]
    fn deep_nesting_spills_operands_in_order() {
        // `1-(2-(3-…))`: every left operand waits on the stack.
        let mut source = String::new();
        for i in 1..100 {
            source.push_str(&format!("{i}-("));
        }
        source.push_str("100");
        source.push_str(&")".repeat(99));
        let ast = parse(&source, 128).unwrap();
        struct Cx;
        impl<'v> Context<'v> for Cx {
            fn get(&self, _: &str) -> Option<&'v Value> {
                None
            }
            fn step(&mut self) -> Result<(), Stop> {
                Ok(())
            }
        }
        let expected = (1..=100).rev().fold(0, |acc, i| i - acc);
        assert_eq!(evaluate(&ast, &mut Cx).unwrap(), Value::from(expected));
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
            fn step(&mut self) -> Result<(), Stop> {
                Ok(())
            }
        }
        assert_eq!(evaluate(&ast, &mut Cx).unwrap(), Value::from(200_000));
        drop(ast);
    }
}
