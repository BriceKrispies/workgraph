//! wg-archive: backup and restore zips, JSON Lines, CSV and Parquet export.
//!
//! The shell half of SH-20. The formats and the statements live in `wg-archivespec` (pure); this
//! crate reads and writes files and runs them against a [`wg_store::Store`]:
//!
//! * [`backup`]: `create_backup` and `restore_backup` (F10).
//! * [`transfer`]: `export_workspace` and `import_workspace` (F14).
//! * [`analytics`]: `export_dataset`, the CSV, JSON and Parquet files (F15, DV6).

pub mod analytics;
pub mod backup;
pub mod transfer;

mod scratch;
