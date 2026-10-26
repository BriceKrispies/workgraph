//! Float comparisons shared by the `wg-experiments` statistics tests.
//!
//! User decision Q9 (docs/rust-rewrite.md section 11): statistics computed through `exp` or `log`
//! are compared to a relative tolerance of `1e-12`, because CPython and Rust can reach different
//! libm implementations and differ in the last ulp. Every other float is compared with `==`.

#![allow(dead_code)]

/// Relative tolerance for statistics computed through `exp`, `log` or `log1p` (Q9).
pub const TRANSCENDENTAL_RELATIVE_TOLERANCE: f64 = 1e-12;

/// Whether `got` equals `want` within [`TRANSCENDENTAL_RELATIVE_TOLERANCE`] relative to `want`.
/// Identical values (including infinities and NaN paired with NaN) always match.
pub fn close_relative(got: f64, want: f64) -> bool {
    if got == want || (got.is_nan() && want.is_nan()) {
        return true;
    }
    (got - want).abs() <= TRANSCENDENTAL_RELATIVE_TOLERANCE * want.abs()
}

/// Panics unless `got` is [`close_relative`] to `want`.
pub fn assert_close_relative(got: f64, want: f64, what: &str) {
    assert!(
        close_relative(got, want),
        "{what}: got {got:?}, want {want:?} (relative diff {:e})",
        (got - want).abs() / want.abs()
    );
}

/// Panics unless the slices have the same length and each pair is [`close_relative`].
pub fn assert_close_relative_slice(got: &[f64], want: &[f64], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: length");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert_close_relative(*g, *w, &format!("{what}[{i}]"));
    }
}
