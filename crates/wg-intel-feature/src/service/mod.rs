//! Shell: the classification and search readers, free functions over `&mut Session`, and the
//! entry points the commands call over `&mut App`.

pub mod classify;
pub mod embed;
pub mod entry;
pub mod llm;
pub mod ops;
pub mod search;
