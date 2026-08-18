//! Additional tests for `install_skill::fetch` — kept in a separate bridged module so the
//! original mirrored suite stays untouched (see docs/rust-test-files.md).

use super::super::*;

#[test]
fn is_private_or_local_host_trims_brackets_and_lowercases() {
    for host in ["[::1]", "[LOCALHOST]", "[127.0.0.1]"] {
        assert!(
            is_private_or_local_host(host),
            "host {host} should be rejected"
        );
    }
    assert!(!is_private_or_local_host("[EXAMPLE.COM]"));
}

#[test]
fn is_private_or_local_host_rejects_ipv6_ula_range() {
    for host in ["fc00::1", "fd00::1"] {
        assert!(
            is_private_or_local_host(host),
            "host {host} should be rejected"
        );
    }
}
