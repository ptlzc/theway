//! Structured capability-broker error returned to the JS host module.

#[derive(Debug)]
pub(super) struct BrokerError {
    pub(super) code: &'static str,
    pub(super) message: std::borrow::Cow<'static, str>,
}

impl BrokerError {
    pub(super) fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message: std::borrow::Cow::Borrowed(message),
        }
    }

    /// Broker error with a runtime-owned message (String).
    pub(super) fn dynamic(code: &'static str, message: String) -> Self {
        Self {
            code,
            message: std::borrow::Cow::Owned(message),
        }
    }

    pub(super) fn contract(message: &'static str) -> Self {
        Self::new("invalid_arguments", message)
    }
}
