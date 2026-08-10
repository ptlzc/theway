//! UUIDv7 helper. 1:1 port of `packages/agent/src/harness/session/uuid.ts`.

use uuid::Uuid;

/// Mint a fresh UUIDv7 string (lowercase, hyphenated).
pub fn uuidv7() -> String {
    Uuid::now_v7().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_time_ordered_ids() {
        let a = uuidv7();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = uuidv7();
        assert_ne!(a, b);
        assert!(a < b, "{a} should sort before {b}");
    }
}
