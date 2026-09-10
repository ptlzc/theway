//! `network.fetch`: HTTP(S) GET/POST from the QuickJS worker under the network
//! permission, with the response body bounded to [`MAX_NETWORK_BYTES`] and
//! redirects capped at 5 hops.

use std::sync::Arc;

use serde_json::{Value, json};
use theway_contract::extension::{
    ExtensionAuditOperation, ExtensionAuditOutcome, ExtensionPermission,
};

use super::ActiveBrokerInvocation;
use super::BrokerError;
use super::BrokerRuntime;
use super::arguments::NetworkArguments;
use super::guards::cancelled;

const MAX_NETWORK_BYTES: usize = 1024 * 1024;

impl BrokerRuntime {
    pub(super) fn network_fetch(
        &self,
        arguments: NetworkArguments,
        active: ActiveBrokerInvocation,
    ) -> Result<Value, BrokerError> {
        self.require(ExtensionPermission::NetworkConnect)?;
        let url = reqwest::Url::parse(&arguments.url)
            .map_err(|_| BrokerError::contract("network URL is invalid"))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(BrokerError::contract(
                "network URL must use HTTP or HTTPS with a host",
            ));
        }
        let target = format!(
            "{}://{}{}",
            url.scheme(),
            url.host_str().unwrap_or_default(),
            url.port()
                .map(|port| format!(":{port}"))
                .unwrap_or_default()
        );
        let cancellation = Arc::clone(&active.cancellation);
        let deadline = active.deadline;
        let result = self.services.block_on(async move {
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::limited(5))
                .build()
                .map_err(|error| format!("HTTP client initialization failed: {error}"))?;
            let mut request = match arguments.method.as_deref().unwrap_or("GET") {
                "GET" => client.get(url),
                "POST" => client.post(url),
                _ => return Err("network method must be GET or POST".into()),
            };
            for (name, value) in arguments.headers {
                request = request.header(name, value);
            }
            if let Some(body) = arguments.body {
                request = request.body(body);
            }
            let mut response = tokio::select! {
                response = request.send() => response.map_err(|error| format!("network request failed: {error}"))?,
                () = cancelled(Arc::clone(&cancellation), deadline) => return Err("cancelled".into()),
            };
            let status = response.status().as_u16();
            let mut body = Vec::new();
            loop {
                let chunk = tokio::select! {
                    chunk = response.chunk() => chunk.map_err(|error| format!("network response failed: {error}"))?,
                    () = cancelled(Arc::clone(&cancellation), deadline) => return Err("cancelled".into()),
                };
                let Some(chunk) = chunk else { break };
                if body.len().saturating_add(chunk.len()) > MAX_NETWORK_BYTES {
                    return Err("network response exceeds the configured limit".into());
                }
                body.extend_from_slice(&chunk);
            }
            Ok((status, String::from_utf8_lossy(&body).into_owned()))
        });
        match result {
            Ok((status, body)) => {
                self.audit(
                    ExtensionAuditOperation::NetworkConnect,
                    ExtensionAuditOutcome::Succeeded,
                    Some(ExtensionPermission::NetworkConnect),
                    Some(&target),
                    ["headers".into(), "body".into(), "response".into()],
                );
                Ok(json!({"status": status, "body": body}))
            }
            Err(error) => self.async_failure(
                ExtensionAuditOperation::NetworkConnect,
                ExtensionPermission::NetworkConnect,
                &target,
                error,
            ),
        }
    }
}
