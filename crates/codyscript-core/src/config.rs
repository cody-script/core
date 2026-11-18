//! [`RunConfig`]: how to run a program, kept apart from the program.

use std::fmt;

use serde::de::value::MapAccessDeserializer;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::debug::Breakpoint;
use crate::engine::Variables;

/// How to run a program — its variables, and where a debugger pauses it —
/// kept apart from the program, which behaves the same whatever they are.
/// It's what an IDE keeps beside a program, as its launch configuration.
///
/// As data, `{ "variables": { … }, "breakpoints": [{ "tag": "…" }, {
/// "path": "…" }] }`; either field may be left out, and any other is
/// refused, so a typo is an error. `codyscript-json` has its JSON Schema.
///
/// Limits aren't part of it: they're the host's, for where it runs.
/// [`CompiledProgram::check`](crate::CompiledProgram::check) checks one
/// against its program, as a run with a debugger
/// ([`RunOptions::with_debugger`](crate::RunOptions::with_debugger)) does
/// before it starts.
///
/// ```
/// use codyscript_core::debug::{self, Action, Breakpoint};
/// use codyscript_core::{value, Engine, RunConfig, RunOptions, Value};
///
/// let program = Engine::new().compile(&value!({ "do": [
///     { "set": { "fee": { "x": "amount * 0.03" } }, "tag": "fee" },
///     { "ret": { "ref": "fee" } }
/// ] })).unwrap();
/// let config = RunConfig {
///     variables: value!({ "amount": 200 }).into_object().unwrap(),
///     breakpoints: vec![Breakpoint::tag("fee")],
/// };
/// program.check(&config).unwrap();
///
/// let mut paused = Vec::new();
/// let debugger = debug::from_fn(|pause| {
///     paused.push(pause.step().path.to_owned());
///     Action::Continue
/// });
/// let options = RunOptions::from(config.variables).with_debugger(config.breakpoints, debugger);
/// let result = program.run_blocking(options).unwrap();
/// assert_eq!(result.value, 6);
/// assert_eq!(paused, ["/do/0"]);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct RunConfig {
    /// The run's initial variables, visible to the whole program.
    pub variables: Variables,
    /// Where a debugger pauses it. Each must name a step of the program.
    pub breakpoints: Vec<Breakpoint>,
}

impl<'de> Deserialize<'de> for RunConfig {
    /// From an object only, as its schema says. (A derived implementation
    /// would also take a list of the fields, in order.)
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Default, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        struct Fields {
            variables: Variables,
            breakpoints: Vec<Breakpoint>,
        }

        struct Object;

        impl<'de> Visitor<'de> for Object {
            type Value = Fields;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a run configuration object")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Fields, A::Error> {
                Fields::deserialize(MapAccessDeserializer::new(map))
            }
        }

        let Fields {
            variables,
            breakpoints,
        } = deserializer.deserialize_map(Object)?;
        Ok(RunConfig {
            variables,
            breakpoints,
        })
    }
}
