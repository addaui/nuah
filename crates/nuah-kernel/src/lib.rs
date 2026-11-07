//! Nuah kernel v2: plugin-agnostic machinery. It knows nothing about any app.
//!
//! Documentation lives at <https://docs.addaui.com/nuah/>; start with the Architecture overview.

pub mod contract;
pub mod core;
pub mod db;
pub mod describe;
pub mod error;
pub mod manifest;
pub mod pipeline;
pub mod plugin;
pub mod ports;
pub mod pulse;
pub mod resolver;
pub mod schema;
pub mod setup;

pub use crate::core::{Core, Services};
pub use crate::db::*;
pub use crate::error::{Category, Error, Result};
pub use crate::manifest::*;
pub use crate::pipeline::{CallRequest, CallResponse};
pub use crate::pulse::{PulseBatch, PulseEvent};
pub use crate::plugin::*;
pub use crate::ports::*;
pub use crate::resolver::*;
