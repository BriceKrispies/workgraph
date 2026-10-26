//! The row helpers every store family reads through: run a statement on the session and decode
//! its rows. The statements themselves are built by the `wg_storecodec` `*_sql` modules.

use wg_storecodec::{Raw, Statement, TableRow};
use wg_types::WgError;

use crate::Session;

/// Runs `statement` and decodes the first row, if there is one.
pub fn one<T: TableRow>(
    session: &mut Session<'_>,
    statement: &Statement,
) -> Result<Option<T>, WgError> {
    session
        .rows(statement)?
        .first()
        .map(|values| T::from_raw(values))
        .transpose()
}

/// Runs `statement` and decodes every row, in the order the query returns them.
pub fn many<T: TableRow>(
    session: &mut Session<'_>,
    statement: &Statement,
) -> Result<Vec<T>, WgError> {
    session
        .rows(statement)?
        .iter()
        .map(|values| T::from_raw(values))
        .collect()
}

/// Runs `statement` and decodes every row with `decode`, in the order the query returns them.
pub fn many_with<T>(
    session: &mut Session<'_>,
    statement: &Statement,
    decode: impl Fn(&[Raw]) -> Result<T, WgError>,
) -> Result<Vec<T>, WgError> {
    session
        .rows(statement)?
        .iter()
        .map(|values| decode(values.as_slice()))
        .collect()
}
