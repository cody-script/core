//! A validated program, as the engine runs it. Built by `validate`; every
//! node carries its JSON Pointer so a failure can name it without the engine
//! tracking paths as it runs.

use std::fmt;
use std::sync::Arc;

use crate::executor::Executor;
use crate::expression::Ast;
use crate::value::Value;

/// A whole program: the input that runs, and the CodyScript functions
/// (`fn`) it may call. External functions (`ext`) need nothing here: a call
/// to one names its executor.
#[derive(Clone, Debug)]
pub(crate) struct Program {
    pub body: Input,
    /// Every function's body, in the order written; calls hold an index.
    pub functions: Vec<Input>,
}

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
    /// `call` of an external function: the host's executor runs it. Boxed,
    /// as the largest kind by far, so every other input stays small.
    External(Box<External>),
    /// `call` of one of the program's own functions.
    Call(Call),
    /// `do`: a block with its own scope.
    Do(Vec<Instruction>),
}

#[derive(Clone, Debug)]
pub(crate) struct Expression {
    pub source: Arc<str>,
    pub ast: Arc<Ast>,
    /// Where failures point: the node's `/x`.
    pub path: String,
}

#[derive(Clone)]
pub(crate) struct External {
    /// The executor, found when the program was compiled.
    pub executor: Arc<dyn Executor>,
    /// The executor id: the function's name up to its first dot.
    pub exe: String,
    /// The function's name within its executor: what follows the first dot
    /// (`trim` in `text.trim`).
    pub op: String,
    /// The arguments to resolve, in order.
    pub args: Vec<(String, Input)>,
    /// The static `arg`, passed as written.
    pub arg: Option<Value>,
}

impl fmt::Debug for External {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("External")
            .field("exe", &self.exe)
            .field("op", &self.op)
            .field("args", &self.args)
            .field("arg", &self.arg)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Call {
    /// The function's index in [`Program::functions`].
    pub function: usize,
    /// The arguments to resolve, in order: the function's variables.
    pub args: Vec<(String, Input)>,
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
