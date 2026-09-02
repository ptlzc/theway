//! Invocation-local budget shared by all capability brokers installed in one
//! QuickJS context. Broker adapters consume one unit before touching a daemon
//! resource, so a failed broker operation still counts toward the limit.

use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) struct BrokerOperationQuota {
    remaining: AtomicUsize,
}

impl BrokerOperationQuota {
    pub(super) fn new() -> Self {
        Self {
            remaining: AtomicUsize::new(0),
        }
    }

    pub(super) fn begin(&self, limit: usize) {
        self.remaining.store(limit, Ordering::Release);
    }

    pub(super) fn consume(&self) -> Result<(), &'static str> {
        self.remaining
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                remaining.checked_sub(1)
            })
            .map(|_| ())
            .map_err(|_| "extension broker operation quota exceeded")
    }
}

#[cfg(test)]
mod tests {
    use super::BrokerOperationQuota;

    #[test]
    fn broker_quota_rejects_operations_after_the_configured_limit() {
        let quota = BrokerOperationQuota::new();
        quota.begin(2);
        assert_eq!(quota.consume(), Ok(()));
        assert_eq!(quota.consume(), Ok(()));
        assert_eq!(
            quota.consume(),
            Err("extension broker operation quota exceeded")
        );
    }
}
