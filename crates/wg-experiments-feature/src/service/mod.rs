//! Shell: the experiment readers, free functions over `&mut Session`, and the experiment entry
//! points of the application (`experiments`, free functions over `&mut App`).

pub mod experiments;
pub mod reads;

pub use experiments::{experiment_list, experiment_show};
