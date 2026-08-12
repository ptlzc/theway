//! Tool input validation. TODO: 1:1 port of `packages/ai/src/utils/validation.ts`.
//!
//! TS uses typebox schemas — Rust uses raw `serde_json::Value` schemas per Q3:A. Validation is
//! a thin wrapper over `jsonschema` (TBD) once we add the dep.

use serde_json::Value;

#[derive(Clone, Debug)]
pub struct ValidationError {
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<ValidationError>,
}

/// Validate `input` against `schema`. Currently a stub that accepts everything; the real
/// implementation will plug in the `jsonschema` crate.
pub fn validate(_input: &Value, _schema: &Value) -> ValidationResult {
    ValidationResult {
        valid: true,
        errors: vec![],
    }
}
