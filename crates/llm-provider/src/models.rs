//! Model registry. 1:1 stub of `packages/ai/src/models.ts`.
//!
//! TS exposes `getModel(provider, id)`, `listModels()`, custom-model registration, and
//! OpenAI-compat overrides. Here we provide just the surface; the data comes from
//! `models_generated.rs` (which is currently empty — populate via `build.rs` once we port
//! `scripts/generate-models.ts`).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::models_generated::BUILTIN_MODELS;
use crate::types::{Api, Model, Provider};

fn custom_registry() -> &'static Mutex<HashMap<String, Model>> {
    static CELL: OnceLock<Mutex<HashMap<String, Model>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(HashMap::new()))
}

fn key(provider: &Provider, id: &str) -> String {
    format!("{}/{}", provider.0, id)
}

pub fn get_model(provider: &Provider, id: &str) -> Option<Model> {
    let custom = custom_registry().lock().expect("registry poisoned");
    if let Some(m) = custom.get(&key(provider, id)) {
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
    out.extend(custom.values().cloned());
    out
}

pub fn register_custom_model(model: Model) {
    let k = key(&model.provider, &model.id);
    let mut reg = custom_registry().lock().expect("registry poisoned");
    reg.insert(k, model);
}

pub fn unregister_custom_model(provider: &Provider, id: &str) {
    let mut reg = custom_registry().lock().expect("registry poisoned");
    reg.remove(&key(provider, id));
}

pub fn list_apis() -> Vec<Api> {
    let mut out = std::collections::HashSet::new();
    for m in BUILTIN_MODELS.iter() {
        out.insert(m.api.clone());
    }
    out.into_iter().collect()
}
