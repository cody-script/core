//! A validated program, as the engine runs it. Built by `validate`; every
//! node carries its JSON Pointer so a failure can name it without the engine
//! tracking paths as it runs.

use std::sync::Arc;

use crate::expression::Ast;
use crate::value::Value;

/// Something that produces a value.
#[derive(Clone, Debug)]
pub(crate) struct Input {
    pub path: String,
    pub tag: Option<String>,
    pub kind: InputKind,
}

#[derive(Clone, Debug)]
pub(crate) enum InputKind {
    /// A bare literal or a `val`: the value itself.
    Const(Value),
    /// `ref`: a variable.
    Ref(String),
    /// `x`: an expression.
    X(Expression),
    /// `op`: a call to an executor.
    Op(Operation),
    /// `do`: a block with its own scope.
    Do(Vec<Instruction>),
}

#[derive(Clone, Debug)]
pub(crate) struct Expression {
    pub source: String,
    pub ast: Arc<Ast>,
    /// Where failures point: the node's `/x`.
    pub path: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Operation {
    /// The executor id: the call up to its first dot.
    pub exe: String,
    /// The operation: the call after its first dot.
    pub op: String,
    /// The arguments to resolve, in order.
    pub args: Vec<(String, Input)>,
    pub arg: Option<Value>,
}

/// Something that does something.
#[derive(Clone, Debug)]
pub(crate) struct Instruction {
    pub path: String,
    pub tag: Option<String>,
    pub kind: InstructionKind,
}

#[derive(Clone, Debug)]
pub(crate) enum InstructionKind {
    Set {
        name: String,
        to: Input,
    },
    Ret(Input),
    If {
        test: Input,
        then: Vec<Instruction>,
        otherwise: Option<Vec<Instruction>>,
    },
    For {
        test: Input,
        body: Vec<Instruction>,
    },
}
