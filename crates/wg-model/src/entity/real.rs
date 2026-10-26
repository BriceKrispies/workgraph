//! [`Real`]: a `FLOAT` column value that can derive `Eq`.

/// A `FLOAT` column (`backoff_base_seconds`, `cost_usd`, ...). Equality compares the bit
/// patterns, so the entity value is `Eq` and a stored float is compared exactly, as the crate
/// documentation's rule for floating-point fields requires.
#[derive(Debug, Clone, Copy)]
pub struct Real(pub f64);

impl PartialEq for Real {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Real {}

impl serde::Serialize for Real {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.0)
    }
}
