//! Test support: the core's shared fixtures, and the READMEs' JSON blocks.
#![allow(dead_code)]

use codyscript_core::Value;

/// Every ```json / ```jsonc block in a README (relative to the workspace
/// root), parsed, with its language.
pub fn readme_json_blocks(readme: &str) -> Vec<(String, Value)> {
    let path = format!("{}/../../{readme}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut blocks = Vec::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find("```json") {
        let after = &rest[start + 3..];
        let line_end = after.find('\n').expect("a fence line");
        let lang = after[..line_end].trim().to_owned();
        let body = &after[line_end + 1..];
        let end = body.find("```").expect("a closing fence");
        let value: Value = codyscript_json::from_str(&body[..end])
            .unwrap_or_else(|e| panic!("a {lang} block in {readme} isn't JSON: {e}"));
        blocks.push((lang, value));
        rest = &body[end + 3..];
    }
    blocks
}
