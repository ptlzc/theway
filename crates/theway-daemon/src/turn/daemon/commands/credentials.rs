// Per-session provider credential commands (memory-only secrets).
//
// `include!` fragment of the `turn::daemon` module — see `commands.rs`.

impl TurnHost {
    fn handle_set_credential(
        &mut self,
        request: WireSetCredentialRequest,
    ) -> Result<(), WireRpcError> {
        if request.session_id.trim().is_empty() {
            return Err(WireRpcError {
                code: "invalid_argument".into(),
                message: "session_id must not be empty".into(),
            });
        }
        if request.provider.trim().is_empty() {
            return Err(WireRpcError {
                code: "invalid_argument".into(),
                message: "provider must not be empty".into(),
            });
        }
        self.automation
            .services
            .session_execution
            .set_credential(&request.session_id, &request.provider, request.secret)
            .map_err(|error| match error {
                crate::session_execution::RegistryError::SessionNotRegistered(id) => WireRpcError {
                    code: "not_found".into(),
                    message: format!("session {id} is not registered; activate it first"),
                },
                other => WireRpcError {
                    code: "failed_precondition".into(),
                    message: other.to_string(),
                },
            })
    }

    fn handle_clear_credential(
        &mut self,
        request: WireClearCredentialRequest,
    ) -> Result<(), WireRpcError> {
        let session_id = request.session_id.trim();
        if session_id.is_empty() {
            return Err(WireRpcError {
                code: "invalid_argument".into(),
                message: "session_id must not be empty".into(),
            });
        }
        if self
            .automation
            .services
            .session_execution
            .get(session_id)
            .is_none()
        {
            return Err(WireRpcError {
                code: "not_found".into(),
                message: format!("session {session_id} is not registered; activate it first"),
            });
        }
        match request.provider.as_deref() {
            Some(provider) if provider.trim().is_empty() => Err(WireRpcError {
                code: "invalid_argument".into(),
                message: "provider must not be empty".into(),
            }),
            Some(provider) => {
                self.automation
                    .services
                    .session_execution
                    .clear_credential(session_id, provider);
                Ok(())
            }
            None => {
                self.automation
                    .services
                    .session_execution
                    .clear_credentials(session_id);
                Ok(())
            }
        }
    }
}
