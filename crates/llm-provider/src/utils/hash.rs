//! Fast deterministic hash for shortening cache keys. 1:1 port of
//! `packages/ai/src/utils/hash.ts`. Uses cjk-friendly UTF-16 code units to keep wire-compatible
//! output with the JS version.

/// `Math.imul` polyfill — 32-bit integer multiplication with wrap-around.
#[inline]
fn imul(a: u32, b: u32) -> u32 {
    a.wrapping_mul(b)
}

pub fn short_hash(s: &str) -> String {
    let mut h1: u32 = 0xdead_beef;
    let mut h2: u32 = 0x41c6_ce57;
    // `String.prototype.charCodeAt(i)` returns a UTF-16 code unit. Iterating over
    // `s.encode_utf16()` matches that exactly.
    for ch in s.encode_utf16() {
        let ch = ch as u32;
        h1 = imul(h1 ^ ch, 2_654_435_761);
        h2 = imul(h2 ^ ch, 1_597_334_677);
    }
    h1 = imul(h1 ^ (h1 >> 16), 2_246_822_507) ^ imul(h2 ^ (h2 >> 13), 3_266_489_909);
    h2 = imul(h2 ^ (h2 >> 16), 2_246_822_507) ^ imul(h1 ^ (h1 >> 13), 3_266_489_909);
    format!("{}{}", to_base36(h2), to_base36(h1))
}

fn to_base36(mut n: u32) -> String {
    if n == 0 {
        return "0".into();
    }
    const ALPHA: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut buf = Vec::with_capacity(7);
    while n > 0 {
        buf.push(ALPHA[(n % 36) as usize]);
        n /= 36;
    }
    buf.reverse();
    String::from_utf8(buf).expect("base36 alpha is ascii")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_deterministic_output() {
        let h1 = short_hash("hello");
        let h2 = short_hash("hello");
        assert_eq!(h1, h2);
        assert_ne!(short_hash("hello"), short_hash("hellp"));
    }
}
