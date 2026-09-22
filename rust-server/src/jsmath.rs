//! JS-exact numeric primitives. Every later port depends on these matching V8
//! bit-for-bit; the reference tables in the tests were generated from Node.

use serde_json::{json, Value};

/// JSON.stringify of a number: integral values print without a decimal point
/// (`2` not `2.0`), non-integral use shortest round-trip (serde's Ryu matches
/// V8). Mirrors JS number serialization for parity.
pub fn js_num(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9e15 {
        json!(f as i64)
    } else {
        json!(f)
    }
}

/// ECMAScript ToUint32.
fn to_u32(x: i64) -> u32 {
    (x as i128).rem_euclid(1i128 << 32) as u32
}

/// ECMAScript ToInt32.
fn to_i32(x: i64) -> i32 {
    to_u32(x) as i32
}

/// `Math.imul` — 32-bit integer multiply, result as int32 (sign-extended to i64).
fn imul(a: i64, b: i64) -> i64 {
    to_i32(a).wrapping_mul(to_i32(b)) as i64
}

/// `Math.round`: round half **up toward +∞**. `Math.round(-1.5) === -1`, so this
/// is NOT `f64::round` (which rounds half away from zero → -2).
pub fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// `Number.prototype.toFixed(digits)`.
///
/// Operates on the magnitude then reattaches the sign, exactly as the spec does,
/// which is why `(-0.5).toFixed(0) === "-1"` while `Math.round(-0.5) === 0`.
///
/// The rounding must be on the f64's *exact* value, not `|x| * 10^d` — the latter
/// re-rounds (e.g. `0.615 * 100` lands on `61.5` in f64 and would give "0.62").
/// So we take a correctly-rounded decimal expansion with guard digits (Rust
/// formats correctly-rounded at any precision) and round half-**up** on it, which
/// matches V8's "ties to the larger n" against Rust's default half-to-even. This
/// gives `(1.005).toFixed(2) === "1.00"`, `(0.615).toFixed(2) === "0.61"`, and the
/// exact-tie cases `(0.25).toFixed(1) === "0.3"`, `(0.125).toFixed(2) === "0.13"`.
pub fn to_fixed(x: f64, digits: usize) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    let neg = x < 0.0;
    let mag = x.abs();

    // Correctly-rounded expansion, far enough past f64 precision that the digit at
    // position `digits+1` is exact.
    let full = format!("{:.*}", digits + 25, mag);
    let dot = full.find('.').unwrap();
    let int_part = &full[..dot];
    let frac = &full[dot + 1..];

    let round_up = frac.as_bytes()[digits] >= b'5';
    let mut digs: Vec<u8> = int_part.bytes().chain(frac[..digits].bytes()).map(|b| b - b'0').collect();
    if round_up {
        let mut i = digs.len();
        loop {
            if i == 0 {
                digs.insert(0, 1);
                break;
            }
            i -= 1;
            if digs[i] == 9 {
                digs[i] = 0;
            } else {
                digs[i] += 1;
                break;
            }
        }
    }

    let s: String = digs.iter().map(|d| (d + b'0') as char).collect();
    let out = if digits == 0 {
        s
    } else {
        let split = s.len() - digits;
        format!("{}.{}", &s[..split], &s[split..])
    };

    // No sign on a rounded-to-zero result: (-0.0001).toFixed(2) === "0.00".
    let zero = digs.iter().all(|&d| d == 0);
    if neg && !zero {
        format!("-{out}")
    } else {
        out
    }
}

/// `+x.toFixed(digits)` — round to `digits` places, return the number.
pub fn round_fixed(x: f64, digits: usize) -> f64 {
    to_fixed(x, digits).parse().unwrap_or(f64::NAN)
}

/// `+x.toPrecision(sig)` — round to `sig` significant figures, as a number.
/// V8 rounds half-away-from-zero on the exact f64, matching `to_fixed`, so this
/// reduces toPrecision to a `to_fixed` at the right decimal place (and, for
/// large magnitudes, a divide-round-multiply through `to_fixed`).
pub fn to_precision(x: f64, sig: usize) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let exp = x.abs().log10().floor() as i32;
    let d = sig as i32 - 1 - exp;
    if d >= 0 {
        to_fixed(x, d as usize).parse().unwrap_or(x)
    } else {
        let scale = 10f64.powi(-d);
        to_fixed(x / scale, 0).parse().unwrap_or(0.0) * scale
    }
}

/// Port of `seededServer` (server/index.js:338-340): FNV-1a seed over the UTF-16
/// code units, then a mulberry32-style step. Returns a closure yielding the same
/// f64 stream as the JS generator. `h` is modelled as the JS double does — it
/// accumulates unmasked across calls and is only reduced mod 2^32 where JS applies
/// a bitwise op — so it stays exact well past the handful of calls per contract.
pub fn seeded_server(seed: &str) -> impl FnMut() -> f64 {
    let mut h: i64 = 2166136261;
    for c in seed.encode_utf16() {
        h = imul((to_i32(h) ^ c as i32) as i64, 16777619);
    }
    move || {
        h += 0x6d2b79f5;
        let t0 = h;
        // t = Math.imul(t ^ (t >>> 15), t | 1)
        let a = (to_i32(t0) ^ (to_u32(t0) >> 15) as i32) as i64;
        let b = (to_i32(t0) | 1) as i64;
        let t1 = imul(a, b);
        // t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
        let c = (to_i32(t1) ^ (to_u32(t1) >> 7) as i32) as i64;
        let d = (to_i32(t1) | 61) as i64;
        let inner = imul(c, d);
        let sum = to_i32(t1) as i64 + inner;
        let t2 = (to_i32(t1) ^ to_i32(sum)) as i64;
        // ((t ^ (t >>> 14)) >>> 0) / 4294967296
        let r = to_u32((to_i32(t2) ^ (to_u32(t2) >> 14) as i32) as i64);
        r as f64 / 4294967296.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_precision_matches_node() {
        let golden: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/toprecision-golden.json")).unwrap();
        for c in golden.as_array().unwrap() {
            let x = c["x"].as_f64().unwrap();
            let expect = c["p4"].as_f64().unwrap();
            assert_eq!(to_precision(x, 4), expect, "toPrecision(4) of {x}");
        }
    }

    #[test]
    fn to_fixed_2() {
        // (value, expected) generated from Node's (x).toFixed(2)
        let cases = [
            (0.0, "0.00"),
            (0.5, "0.50"),
            (1.005, "1.00"),
            (0.615, "0.61"),
            (2.675, "2.67"),
            (1.255, "1.25"),
            (-0.5, "-0.50"),
            (-1.5, "-1.50"),
            (123.456, "123.46"),
            (0.1, "0.10"),
            (0.7044610297307372, "0.70"),
            (99.995, "100.00"),
            (0.125, "0.13"),
            (-0.005, "-0.01"),
            (1e-7, "0.00"),
            (100.0, "100.00"),
        ];
        for (x, want) in cases {
            assert_eq!(to_fixed(x, 2), want, "toFixed({x}, 2)");
        }
    }

    #[test]
    fn to_fixed_1_and_0() {
        for (x, want) in [(0.05, "0.1"), (0.15, "0.1"), (0.25, "0.3"), (0.35, "0.3"), (0.45, "0.5"), (2.25, "2.3"), (-0.15, "-0.1"), (1.05, "1.1")] {
            assert_eq!(to_fixed(x, 1), want, "toFixed({x}, 1)");
        }
        for (x, want) in [(0.5, "1"), (1.5, "2"), (2.5, "3"), (-0.5, "-1"), (-1.5, "-2"), (2.4999, "2")] {
            assert_eq!(to_fixed(x, 0), want, "toFixed({x}, 0)");
        }
    }

    #[test]
    fn to_fixed_carry_propagation() {
        // Rounding that ripples an extra integer digit — from Node.
        for (x, want) in [(9.999, "10.00"), (99.996, "100.00"), (0.999, "1.00"), (9.995, "9.99")] {
            assert_eq!(to_fixed(x, 2), want, "toFixed({x}, 2)");
        }
    }

    #[test]
    fn js_round_half_up() {
        for (x, want) in [(-2.5, -2.0), (-1.5, -1.0), (-0.5, 0.0), (0.5, 1.0), (1.5, 2.0), (2.5, 3.0), (2.4, 2.0), (2.6, 3.0), (0.4999999999999999, 0.0)] {
            assert_eq!(js_round(x), want, "js_round({x})");
        }
    }

    #[test]
    fn prng_reference_stream() {
        // Node: seededServer("0xdac17f958d2ee523a2206206994597c13d831ec7")
        let mut r = seeded_server("0xdac17f958d2ee523a2206206994597c13d831ec7");
        let got = [r(), r(), r()];
        let want = [0.7044610297307372, 0.39827295090071857, 0.6513761680107564];
        for i in 0..3 {
            assert_eq!(got[i], want[i], "prng[{i}]");
        }
    }
}
