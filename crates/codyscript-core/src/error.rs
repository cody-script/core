//! [`Error`], the one error type the engine raises, and its [`ErrorCode`]s.

use std::error::Error as StdError;
use std::fmt;
use std::sync::Arc;

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

/// Any error a host hands the engine: what an executor fails with, or an
/// abort reason. `Box<dyn Error>` converts from `&str`, `String` and every
/// error type, so executors can `?` or write `Err("…".into())`.
pub type BoxError = Box<dyn StdError + Send + Sync + 'static>;

/// A shared, cloneable [`BoxError`]: an [`Error`]'s cause.
pub type Cause = Arc<dyn StdError + Send + Sync + 'static>;

/// What went wrong. Adding a code is a public API change.
///
/// As data, a code is its [`as_str`](ErrorCode::as_str) form:
/// `"invalid_program"`, …
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The program isn't a well-formed CodyScript program.
    InvalidProgram,
    /// The program calls an executor the engine wasn't given.
    UnknownExecutor,
    /// A `ref` or an expression read a variable that isn't defined.
    UndefinedVariable,
    /// An expression mixed types (`1 + 'a'`), compared a list or object, or
    /// divided by zero.
    TypeError,
    /// An executor returned an error.
    ExecutorFailed,
    /// The run used more than `max_steps` steps.
    StepLimit,
    /// The run's [`Signal`](crate::Signal) was aborted.
    Aborted,
}

impl ErrorCode {
    /// The code as written in JSON and the documentation: `"invalid_program"`, …
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidProgram => "invalid_program",
            ErrorCode::UnknownExecutor => "unknown_executor",
            ErrorCode::UndefinedVariable => "undefined_variable",
            ErrorCode::TypeError => "type_error",
            ErrorCode::ExecutorFailed => "executor_failed",
            ErrorCode::StepLimit => "step_limit",
            ErrorCode::Aborted => "aborted",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One problem found while compiling a program.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Issue {
    /// JSON Pointer to the offending node.
    pub path: String,
    /// What's wrong there.
    pub message: String,
}

/// Optional context for [`Error::with_details`].
#[derive(Clone, Default)]
pub struct Details {
    /// JSON Pointer into the program (`/do/2/set/x`); empty for the root.
    pub path: Option<String>,
    /// The node's `tag`.
    pub tag: Option<String>,
    /// Every problem found, for `invalid_program` / `unknown_executor`.
    pub issues: Option<Vec<Issue>>,
    /// What caused this error, reported as its [`source`](StdError::source).
    pub cause: Option<Cause>,
}

impl fmt::Debug for Details {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Details")
            .field("path", &self.path)
            .field("tag", &self.tag)
            .field("issues", &self.issues)
            .field("cause", &self.cause.as_ref().map(|c| c.to_string()))
            .finish()
    }
}

/// Every failure the engine raises. [`path`](Error::path) is a JSON Pointer
/// into the program (`/do/2/set/x`) and [`tag`](Error::tag) the node's tag, so
/// a host can point at the culprit.
///
/// Its message (and `Display`) reads `"<message> at <path> (tag <tag>)"`,
/// leaving out the parts that are empty.
#[derive(Clone)]
pub struct Error {
    code: ErrorCode,
    message: String,
    path: String,
    tag: Option<String>,
    issues: Option<Vec<Issue>>,
    cause: Option<Cause>,
}

impl Error {
    /// An error with no path, tag, issues or cause. Executors may return one
    /// (boxed) to fail a run with exactly this error instead of
    /// `executor_failed`.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::with_details(code, message, Details::default())
    }

    /// An error with context; the message gets `" at <path>"` and
    /// `" (tag <tag>)"` appended when they aren't empty.
    pub fn with_details(code: ErrorCode, message: impl Into<String>, details: Details) -> Self {
        let mut message = message.into();
        if let Some(path) = details.path.as_deref().filter(|p| !p.is_empty()) {
            message.push_str(" at ");
            message.push_str(path);
        }
        if let Some(tag) = details.tag.as_deref().filter(|t| !t.is_empty()) {
            message.push_str(" (tag ");
            message.push_str(tag);
            message.push(')');
        }
        Error {
            code,
            message,
            path: details.path.unwrap_or_default(),
            tag: details.tag,
            issues: details.issues,
            cause: details.cause,
        }
    }

    /// What went wrong.
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    /// The full message, including where.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// JSON Pointer to the node that failed; `""` for the program's root.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The failing node's `tag`, if it has one.
    pub fn tag(&self) -> Option<&str> {
        self.tag.as_deref()
    }

    /// For `invalid_program` / `unknown_executor`: every problem found, not
    /// just the first.
    pub fn issues(&self) -> Option<&[Issue]> {
        self.issues.as_deref()
    }

    /// What caused this error: an executor's error for `executor_failed`, the
    /// signal's reason for `aborted`.
    pub fn cause(&self) -> Option<&Cause> {
        self.cause.as_ref()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("Error");
        d.field("code", &self.code)
            .field("message", &self.message)
            .field("path", &self.path);
        if let Some(tag) = &self.tag {
            d.field("tag", tag);
        }
        if let Some(issues) = &self.issues {
            d.field("issues", issues);
        }
        if let Some(cause) = &self.cause {
            d.field("cause", &cause.to_string());
        }
        d.finish()
    }
}

impl Serialize for Error {
    /// `{ code, message, path, tag?, issues? }`: what a host might send an
    /// editor. The cause is left out; it's host-side detail.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let fields = 3 + usize::from(self.tag.is_some()) + usize::from(self.issues.is_some());
        let mut error = serializer.serialize_struct("Error", fields)?;
        error.serialize_field("code", &self.code)?;
        error.serialize_field("message", &self.message)?;
        error.serialize_field("path", &self.path)?;
        if let Some(tag) = &self.tag {
            error.serialize_field("tag", tag)?;
        }
        if let Some(issues) = &self.issues {
            error.serialize_field("issues", issues)?;
        }
        error.end()
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.cause
            .as_deref()
            .map(|c| c as &(dyn StdError + 'static))
    }
}
