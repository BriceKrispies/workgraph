//! Shell: the experiment readers, free functions over `&mut Session`, and the experiment entry
//! points of the application (`experiments`, `lifecycle`, `planning`, `predicting`: free
//! functions over `&mut App`).

pub mod exec;
pub mod experiments;
pub mod lifecycle;
pub mod planning;
pub mod predicting;
pub mod reads;
pub mod replay;
pub mod runner;
pub mod trials;

pub use experiments::{experiment_list, experiment_show};
pub use lifecycle::{experiment_close, experiment_create, experiment_freeze};
pub use planning::{experiment_next, experiment_report};
pub use predicting::{experiment_fit, experiment_predict};
