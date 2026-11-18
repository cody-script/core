//! `schema.json` is what editors check hand-written programs against; it must
//! accept and refuse the same programs as the engine's own validator.

#[path = "../../codyscript-core/tests/common/mod.rs"]
mod common;
mod support;

use codyscript_core::{RunConfig, Value};
use codyscript_json::{RUN_CONFIG_SCHEMA, SCHEMA, to_serde_json};

fn validator() -> jsonschema::Validator {
    let schema: serde_json::Value = serde_json::from_str(SCHEMA).expect("schema.json is JSON");
    jsonschema::draft202012::new(&schema).expect("schema.json is a valid schema")
}

fn errors(validator: &jsonschema::Validator, program: &Value) -> Vec<String> {
    validator
        .iter_errors(&to_serde_json(program))
        .map(|e| e.to_string())
        .collect()
}

#[test]
fn accepts_every_valid_program() {
    let validator = validator();
    for (name, program) in common::valid() {
        assert!(
            validator.is_valid(&to_serde_json(&program)),
            "{name}: {:?}",
            errors(&validator, &program)
        );
    }
}

#[test]
fn refuses_every_invalid_program() {
    let validator = validator();
    for (name, program, _) in common::invalid() {
        assert!(!validator.is_valid(&to_serde_json(&program)), "{name}");
    }
}

#[test]
fn accepts_every_program_in_the_readmes() {
    let validator = validator();
    let programs: Vec<Value> = ["README.md", "crates/codyscript-core/README.md"]
        .into_iter()
        .flat_map(support::readme_json_blocks)
        .map(|(_, json)| json)
        .filter(|json| json.get("do").is_some() || json.get("fn").is_some())
        .collect();
    assert_eq!(programs.len(), 3);
    for program in programs {
        assert!(
            validator.is_valid(&to_serde_json(&program)),
            "{:?}",
            errors(&validator, &program)
        );
    }
}

// ── Run configurations ──────────────────────────────────────────────────────

/// Run configurations, and whether they're well formed.
const RUN_CONFIGS: &[(&str, bool)] = &[
    ("{}", true),
    (r#"{ "variables": { "Name": " Ada " } }"#, true),
    (r#"{ "breakpoints": [] }"#, true),
    (
        r#"{ "breakpoints": [{ "tag": "fee" }, { "path": "/do/1" }, { "path": "" }] }"#,
        true,
    ),
    (
        r#"{ "variables": { "n": 1 }, "breakpoints": [{ "path": "/fn/a~1b/do/0" }] }"#,
        true,
    ),
    // Not objects, unknown fields, limits (the host's), wrong types.
    ("[]", false),
    (r#"{ "limits": { "max_steps": 10 } }"#, false),
    (r#"{ "variables": [1] }"#, false),
    (r#"{ "breakpoints": { "tag": "fee" } }"#, false),
    (r#"{ "breakpoints": ["/do/1"] }"#, false),
    // A breakpoint names a step one way: a tag or a path, not both, not neither.
    (
        r#"{ "breakpoints": [{ "tag": "fee", "path": "/do/1" }] }"#,
        false,
    ),
    (r#"{ "breakpoints": [{}] }"#, false),
    (r#"{ "breakpoints": [{ "line": 3 }] }"#, false),
    (r#"{ "breakpoints": [{ "tag": 3 }] }"#, false),
];

#[test]
fn the_run_config_schema_and_the_parser_agree() {
    let schema: serde_json::Value = serde_json::from_str(RUN_CONFIG_SCHEMA).expect("JSON");
    let validator = jsonschema::draft202012::new(&schema).expect("a valid schema");
    for &(json, valid) in RUN_CONFIGS {
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(validator.is_valid(&value), valid, "schema: {json}");
        let parsed = codyscript_json::from_str::<RunConfig>(json);
        assert_eq!(parsed.is_ok(), valid, "parser: {json}: {parsed:?}");
    }
    // A path that isn't a JSON Pointer can't name a step: the schema says
    // so as it's written, `check` when it meets a program.
    let not_a_pointer = r#"{ "breakpoints": [{ "path": "do/1" }] }"#;
    assert!(!validator.is_valid(&serde_json::from_str(not_a_pointer).unwrap()));
    assert!(codyscript_json::from_str::<RunConfig>(not_a_pointer).is_ok());
}
