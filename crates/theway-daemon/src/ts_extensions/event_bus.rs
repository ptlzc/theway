//! Event bridge dispatch semantics (issue #84 §3): the five Cordis-style
//! dispatch modes exposed to plugins — emit (broadcast, ignore returns),
//! parallel (run all concurrently), serial (await in order until the first
//! bail value), bail (sync until the first bail value), and waterfall
//! (onion composition around a mandatory `next` continuation).

use std::sync::Arc;

use serde_json::Value;

pub(super) fn emit_mode(
    listeners: &[Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync>],
    payload: Value,
) -> Result<Vec<Value>, Vec<String>> {
    let mut outputs = Vec::new();
    let mut errors = Vec::new();
    for listener in listeners {
        match listener(payload.clone()) {
            Ok(output) => outputs.push(output),
            Err(error) => errors.push(error),
        }
    }
    if errors.is_empty() {
        Ok(outputs)
    } else {
        Err(errors)
    }
}

pub(super) async fn parallel_mode(
    listeners: &[Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync>],
    payload: Value,
) -> Result<Vec<Value>, Vec<String>> {
    let mut handles = Vec::with_capacity(listeners.len());
    for listener in listeners {
        let listener = Arc::clone(listener);
        let payload = payload.clone();
        handles.push(tokio::spawn(async move { listener(payload) }));
    }
    let mut outputs = Vec::with_capacity(handles.len());
    let mut errors = Vec::new();
    for handle in handles {
        match handle.await {
            Ok(Ok(output)) => outputs.push(output),
            Ok(Err(error)) => errors.push(error),
            Err(error) => errors.push(error.to_string()),
        }
    }
    if errors.is_empty() {
        Ok(outputs)
    } else {
        Err(errors)
    }
}

pub(super) async fn serial_mode(
    listeners: &[Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync>],
    payload: Value,
) -> Result<Option<Value>, Vec<String>> {
    let mut errors = Vec::new();
    for listener in listeners {
        match listener(payload.clone()) {
            Ok(Value::Null) => {}
            Ok(value) => return Ok(Some(value)),
            Err(error) => errors.push(error),
        }
    }
    if errors.is_empty() {
        Ok(None)
    } else {
        Err(errors)
    }
}

pub(super) fn bail_mode(
    listeners: &[Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync>],
    payload: Value,
) -> Result<Option<Value>, Vec<String>> {
    let mut errors = Vec::new();
    for listener in listeners {
        match listener(payload.clone()) {
            Ok(Value::Null) => {}
            Ok(value) => return Ok(Some(value)),
            Err(error) => errors.push(error),
        }
    }
    if errors.is_empty() {
        Ok(None)
    } else {
        Err(errors)
    }
}

/// Waterfall listener type: `(payload, next) -> Result<Value, String>` where
/// `next` runs the rest of the chain. Omitting `next()` short-circuits.
pub(super) type WaterfallFn = Arc<
    dyn Fn(
            Value,
            Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync>,
        ) -> Result<Value, String>
        + Send
        + Sync,
>;

pub(super) fn waterfall_mode(
    listeners: &[WaterfallFn],
    payload: Value,
    innermost: Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync>,
) -> Result<Value, String> {
    // Build the chain inside-out: each listener wraps the rest.
    let mut chain: Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync> = innermost;
    for listener in listeners.iter().rev() {
        let listener = Arc::clone(listener);
        let rest = Arc::clone(&chain);
        chain = Arc::new(move |value: Value| listener(value, Arc::clone(&rest)));
    }
    chain(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn listener(returns: Value) -> Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync> {
        Arc::new(move |_| Ok(returns.clone()))
    }

    #[tokio::test]
    async fn emit_runs_all_listeners_and_ignores_returns() {
        let listeners: Vec<_> = vec![listener(Value::Null), listener(Value::Null)];
        let outputs = emit_mode(&listeners, Value::Null).unwrap();
        assert_eq!(outputs.len(), 2);
    }

    #[tokio::test]
    async fn serial_and_bail_stop_at_first_non_null() {
        let listeners: Vec<_> = vec![
            listener(Value::Null),
            listener(Value::Bool(true)),
            listener(Value::Null),
        ];
        assert_eq!(
            serial_mode(&listeners, Value::Null).await.unwrap(),
            Some(Value::Bool(true))
        );
        assert_eq!(
            bail_mode(&listeners, Value::Null).unwrap(),
            Some(Value::Bool(true))
        );

        let never: Vec<_> = vec![listener(Value::Null), listener(Value::Null)];
        assert_eq!(serial_mode(&never, Value::Null).await.unwrap(), None);
        assert_eq!(bail_mode(&never, Value::Null).unwrap(), None);
    }

    #[tokio::test]
    async fn parallel_runs_concurrently() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut listeners: Vec<Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync>> =
            Vec::new();
        for _ in 0..4 {
            let counter = Arc::clone(&counter);
            listeners.push(Arc::new(move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(Value::Null)
            }));
        }
        parallel_mode(&listeners, Value::Null).await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn waterfall_chain_requires_next() {
        let chain: Vec<WaterfallFn> = vec![Arc::new(
            |input: Value, next: Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync>| {
                let downstream = next(input)?;
                Ok(serde_json::json!({"wrapped": downstream}))
            },
        )];
        let innermost: Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync> =
            Arc::new(|input: Value| Ok(input));
        let out = waterfall_mode(&chain, serde_json::json!({"v": 1}), innermost).unwrap();
        assert_eq!(out, serde_json::json!({"wrapped": {"v": 1}}));

        // Omitting next() short-circuits (gateway semantics).
        let gate: Vec<WaterfallFn> = vec![Arc::new(
            |_input: Value, _next: Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync>| {
                Ok(Value::Bool(false))
            },
        )];
        let innermost2: Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync> =
            Arc::new(|i: Value| Ok(i));
        let out2 = waterfall_mode(&gate, Value::Null, innermost2).unwrap();
        assert_eq!(out2, Value::Bool(false));
    }
}
