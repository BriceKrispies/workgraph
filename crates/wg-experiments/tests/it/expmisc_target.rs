//! The out-of-range `target` message of `contract.check` carries Python's float repr, so that
//! exponent forms such as `1e-05` match the Python text.

use wg_experiments::contract::{Fit, Problems, decide};

fn clean() -> Problems {
    Problems {
        missing: Vec::new(),
        not_delegable: Vec::new(),
    }
}

#[test]
fn target_error_uses_python_float_repr_with_exponent() {
    let error = decide(clean(), || Fit::NoFit, "m", -1e-05).err();
    assert_eq!(
        error.as_deref(),
        Some("target must be between 0 and 1, got -1e-05")
    );
}

#[test]
fn target_error_keeps_a_point_zero_on_whole_numbers() {
    let error = decide(clean(), || Fit::NoFit, "m", 2.0).err();
    assert_eq!(
        error.as_deref(),
        Some("target must be between 0 and 1, got 2.0")
    );
}
