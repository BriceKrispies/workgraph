//! The Parquet bytes of an analytics dataset (F15, DV6). Column types follow [`column_types`]
//! (the Python `_arrow_table` inference); DV6 compares the files by reading them back, so the
//! encoder's own choices (row groups, page sizes, compression) are not part of the contract.

use std::sync::Arc;

use arrow_array::{ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use wg_rows::Raw;
use wg_types::WgError;

use crate::{ColumnType, Dataset, cell_text, column_types};

/// The dataset as one Parquet file, in memory.
pub fn parquet_bytes(dataset: &Dataset) -> Result<Vec<u8>, WgError> {
    let types = column_types(dataset);
    let mut fields = Vec::with_capacity(types.len());
    let mut arrays: Vec<ArrayRef> = Vec::with_capacity(types.len());
    for (index, (name, column_type)) in dataset.columns.iter().zip(&types).enumerate() {
        let cells = dataset
            .rows
            .iter()
            .map(|row| row.get(index).unwrap_or(&Raw::Null));
        let (data_type, array): (DataType, ArrayRef) = match column_type {
            ColumnType::Int64 => {
                let values: Vec<Option<i64>> = cells
                    .map(|cell| match cell {
                        Raw::Int(number) => Some(*number),
                        _ => None,
                    })
                    .collect();
                (DataType::Int64, Arc::new(Int64Array::from(values)))
            }
            ColumnType::Float64 => {
                let values: Vec<Option<f64>> = cells
                    .map(|cell| match cell {
                        Raw::Real(number) => Some(*number),
                        Raw::Int(number) => Some(*number as f64),
                        _ => None,
                    })
                    .collect();
                (DataType::Float64, Arc::new(Float64Array::from(values)))
            }
            ColumnType::Utf8 => {
                let values: Vec<Option<String>> = cells.map(cell_text).collect();
                (DataType::Utf8, Arc::new(StringArray::from(values)))
            }
        };
        fields.push(Field::new(name.as_str(), data_type, true));
        arrays.push(array);
    }
    let schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(Arc::clone(&schema), arrays).map_err(parquet_failure)?;
    let mut buffer: Vec<u8> = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut buffer, schema, None).map_err(parquet_failure)?;
    writer.write(&batch).map_err(parquet_failure)?;
    writer.close().map_err(parquet_failure)?;
    Ok(buffer)
}

fn parquet_failure(err: impl std::fmt::Display) -> WgError {
    WgError::workgraph(format!("cannot write parquet: {err}"))
}
