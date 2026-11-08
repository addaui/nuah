// SPDX-License-Identifier: Apache-2.0
//! Adapters for the kernel's Ports: memory and SQLite data, local and password sign-in, allow-all and
//! role policy, system and fixed clock/ids. Also the data conformance suite every data Adapter passes.

pub mod auth;
pub mod conformance;
pub mod db_plugins;
pub mod log;
pub mod memory;
pub mod pg;
pub mod policy;
pub mod sqlite;
pub mod std_ports;
pub mod transfer;

pub use auth::{AuthLocal, AuthPassword};
pub use log::{LogConsole, LogFile};
pub use db_plugins::{AdapterMemory, AdapterPostgres, AdapterSqlite};
pub use memory::MemoryDb;
pub use pg::PgDb;
pub use policy::{PolicyAllowAll, PolicyRoles};
pub use sqlite::SqliteDb;
pub use transfer::{export, import};
pub use std_ports::*;
