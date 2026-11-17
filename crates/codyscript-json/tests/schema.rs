//! `schema.json` is what editors check hand-written programs against; it must
//! accept and refuse the same programs as the engine's own validator.

#[path = "../../codyscript-core/tests/common/mod.rs"]
mod common;
mod support;

use codyscript_core::Value;
use codyscript_json::{SCHEMA, to_serde_json};

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
