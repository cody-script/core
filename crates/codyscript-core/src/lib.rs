#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod block_on;
mod config;
pub mod debug;
mod engine;
mod error;
pub mod executor;
mod expression;
mod program;
mod run;
mod validate;
pub mod value;

pub use config::RunConfig;
pub use engine::{
    CompiledProgram, Engine, EngineBuilder, Limits, RunOptions, RunResult, Variables,
};
pub use error::{BoxError, Cause, Details, Error, ErrorCode, Issue};
pub use executor::{BoxFuture, Executor, ExecutorCall, ExecutorFuture, ExecutorResult, Signal};
pub use value::{Map, Number, Value};
