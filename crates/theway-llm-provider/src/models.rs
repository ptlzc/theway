//! Model registry. 1:1 stub of `packages/ai/src/models.ts`.
//!
//! TS exposes `getModel(provider, id)`, `listModels()`, custom-model registration, and
//! OpenAI-compat overrides. Here we provide just the surface; the data comes from
//! `models_generated.rs` (which is currently empty — populate via `build.rs` once we port
//! `scripts/generate-models.ts`).

use std::sync::{Mutex, OnceLock};

use crate::models_generated::BUILTIN_MODELS;
use crate::types::{Api, Model, Provider};

fn custom_registry() -> &'static Mutex<Vec<Model>> {
    static CELL: OnceLock<Mutex<Vec<Model>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn get_model(provider: &Provider, id: &str) -> Option<Model> {
    let custom = custom_registry().lock().expect("registry poisoned");
    if let Some(m) = custom
        .iter()
        .find(|m| m.provider == *provider && m.id == id)
    {
        return Some(m.clone());
    }
    BUILTIN_MODELS
        .iter()
        .find(|m| m.provider == *provider && m.id == id)
        .cloned()
}

pub fn list_models() -> Vec<Model> {
    let custom = custom_registry().lock().expect("registry poisoned");
    let mut out: Vec<Model> = BUILTIN_MODELS.iter().cloned().collect();
    out.extend(custom.iter().cloned());
    out
}

pub fn list_custom_models() -> Vec<Model> {
    custom_registry().lock().expect("registry poisoned").clone()
}

pub fn register_custom_model(model: Model) {
    let mut reg = custom_registry().lock().expect("registry poisoned");
    if let Some(existing) = reg
        .iter_mut()
        .find(|m| m.provider == model.provider && m.id == model.id)
    {
        *existing = model;
    } else {
        reg.push(model);
    }
}

pub fn unregister_custom_model(provider: &Provider, id: &str) {
    let mut reg = custom_registry().lock().expect("registry poisoned");
    reg.retain(|m| !(m.provider == *provider && m.id == id));
}

pub fn list_apis() -> Vec<Api> {
    let mut out = std::collections::HashSet::new();
    for m in BUILTIN_MODELS.iter() {
        out.insert(m.api.clone());
    }
    out.into_iter().collect()
}
