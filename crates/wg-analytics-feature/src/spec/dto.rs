//! The DTO of the analytics source extraction (SH-14).

use wg_storecodec::Raw;

/// One table's copy: the columns read (in the order the rows hold them) and the rows, in rowid
/// order.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalyticsSource {
    pub table: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Raw>>,
}
