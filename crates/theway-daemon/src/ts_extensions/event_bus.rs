//! Live event bridge dispatch semantics (issue #84 §3): the five
//! dispatch modes exposed to plugins — emit (broadcast, ignore returns),
//! parallel (run all concurrently), serial (await in order until the first
//! bail value), bail (ordered until the first bail value), and waterfall
//! (each listener receives the previous listener's returned value).
//!
//! Listener futures are supplied by the session host: every invocation is an
//! isolated QuickJS call through the engine pool, so `bail` still awaits the
//! selected listeners in order before short-circuiting.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

pub(super) type LiveEventFuture = Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;
pub(super) type LiveEventListener = Arc<dyn Fn(Value) -> LiveEventFuture + Send + Sync>;

fn is_bail_value(value: &Value) -> bool {
    !matches!(value, Value::Null | Value::Bool(false))
}

pub(super) async fn emit_mode(
    listeners: &[LiveEventListener],
    payload: Value,
) -> Result<Vec<Value>, Vec<String>> {
    let mut outputs = Vec::with_capacity(listeners.len());
    let mut errors = Vec::new();
    for listener in listeners {
        match listener(payload.clone()).await {
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
    listeners: &[LiveEventListener],
    payload: Value,
) -> Result<Vec<Value>, Vec<String>> {
    let mut tasks = tokio::task::JoinSet::new();
    for listener in listeners {
        let listener = Arc::clone(listener);
        let payload = payload.clone();
        tasks.spawn(async move { listener(payload).await });
    }
    let mut outputs = Vec::with_capacity(listeners.len());
    let mut errors = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        match joined {
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
    listeners: &[LiveEventListener],
    payload: Value,
) -> Result<Option<Value>, Vec<String>> {
    ordered_short_circuit(listeners, payload).await
}

pub(super) async fn bail_mode(
    listeners: &[LiveEventListener],
    payload: Value,
) -> Result<Option<Value>, Vec<String>> {
    ordered_short_circuit(listeners, payload).await
}

async fn ordered_short_circuit(
    listeners: &[LiveEventListener],
    payload: Value,
) -> Result<Option<Value>, Vec<String>> {
    let mut errors = Vec::new();
    for listener in listeners {
        match listener(payload.clone()).await {
            Ok(value) if is_bail_value(&value) => return Ok(Some(value)),
            Ok(_) => {}
            Err(error) => errors.push(error),
        }
    }
    if errors.is_empty() {
        Ok(None)
    } else {
        Err(errors)
    }
}

/// Sequential transform chain: each listener receives the previous listener's
/// returned value; the final value is the dispatch result.
pub(super) async fn waterfall_mode(
    listeners: &[LiveEventListener],
    payload: Value,
) -> Result<Value, String> {
    let mut current = payload;
    for listener in listeners {
        current = listener(current).await?;
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listener(returns: Value) -> LiveEventListener {
        Arc::new(move |_| {
            let value = returns.clone();
            Box::pin(async move { Ok(value) })
        })
    }

    fn collect(
        listener: LiveEventListener,
        target: Arc<std::sync::Mutex<Vec<Value>>>,
    ) -> LiveEventListener {
        Arc::new(move |value| {
            target.lock().unwrap().push(value.clone());
            let listener = Arc::clone(&listener);
            Box::pin(async move { listener(value).await })
        })
    }

    #[tokio::test]
    async fn emit_runs_all_listeners_and_ignores_returns() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let listeners: Vec<_> = (0..2)
            .map(|_| collect(listener(Value::Bool(true)), Arc::clone(&seen)))
            .collect();
        let outputs = emit_mode(&listeners, Value::Null).await.unwrap();
        assert_eq!(outputs.len(), 2);
        assert_eq!(seen.lock().unwrap().len(), 2);
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
            bail_mode(&listeners, Value::Null).await.unwrap(),
            Some(Value::Bool(true))
        );

        let never: Vec<_> = vec![listener(Value::Null), listener(Value::Null)];
        assert_eq!(serial_mode(&never, Value::Null).await.unwrap(), None);
        assert_eq!(bail_mode(&never, Value::Null).await.unwrap(), None);
    }

    #[tokio::test]
    async fn parallel_runs_all_listeners() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let listeners: Vec<_> = (0..4)
            .map(|index| collect(listener(Value::from(index)), Arc::clone(&seen)))
            .collect();
        parallel_mode(&listeners, Value::Null).await.unwrap();
        assert_eq!(seen.lock().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn waterfall_chains_returned_values() {
        let wrap: LiveEventListener = Arc::new(|input: Value| {
            Box::pin(async move { Ok(serde_json::json!({"wrapped": input})) })
        });
        let listeners = vec![wrap.clone(), wrap];
        let out = waterfall_mode(&listeners, serde_json::json!({"v": 1}))
            .await
            .unwrap();
        assert_eq!(out, serde_json::json!({"wrapped": {"wrapped": {"v": 1}}}));
    }
}
