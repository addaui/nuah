// SPDX-License-Identifier: Apache-2.0
//! Data Adapters as plugins.

use crate::{memory::MemoryDb, pg::PgDb, sqlite::SqliteDb};
use nuah_kernel::*;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct AdapterMemory;
impl Plugin for AdapterMemory {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id":"db-memory","version":"0.2.0","name":"Memory database","description":"Keeps data in memory; gone when the Core stops","fills":"db"})).unwrap()
    }
    fn fill(&self, _s: &Value) -> Result<Option<PortImpl>> {
        Ok(Some(PortImpl::Db(Arc::new(MemoryDb::new()))))
    }
}

pub struct AdapterSqlite;
impl Plugin for AdapterSqlite {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({
            "id":"db-sqlite","version":"0.2.0","name":"SQLite database","description":"Keeps data in one SQLite file","fills":"db",
            "settings_schema":{"type":"object","properties":{"path":{"type":"string","minLength":1,"x-tier":"bootstrap","x-apply":"restart"},"readers":{"type":"integer","minimum":0,"maximum":128,"x-tier":"bootstrap","x-apply":"restart"},"backup_dir":{"type":"string","minLength":1,"x-tier":"bootstrap","x-apply":"restart"}},"additionalProperties":false}
        })).unwrap()
    }
    fn fill(&self, s: &Value) -> Result<Option<PortImpl>> {
        let path = s.get("path").and_then(|p| p.as_str()).unwrap_or(":memory:");
        let readers = s.get("readers").and_then(|r| r.as_u64()).map(|r| r as usize).unwrap_or_else(crate::sqlite::default_readers);
        let mut db = SqliteDb::open_with(path, readers)?;
        if let Some(dir) = s.get("backup_dir").and_then(|d| d.as_str()) {
            db = db.with_backup_dir(dir);
        }
        Ok(Some(PortImpl::Db(Arc::new(db))))
    }
}

pub struct AdapterPostgres;
impl Plugin for AdapterPostgres {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({
            "id":"db-postgres","version":"0.1.0","name":"PostgreSQL database","description":"Keeps data in PostgreSQL; one Nuah process per database","fills":"db",
            "settings_schema":{"type":"object","required":["url"],"properties":{
                "url":{"type":"string","minLength":1,"x-tier":"bootstrap","x-apply":"restart"},
                "schema":{"type":"string","minLength":1,"maxLength":48,"x-tier":"bootstrap","x-apply":"restart"},
                "readers":{"type":"integer","minimum":0,"maximum":128,"x-tier":"bootstrap","x-apply":"restart"}},"additionalProperties":false}
        })).unwrap()
    }
    fn fill(&self, s: &Value) -> Result<Option<PortImpl>> {
        let url = s.get("url").and_then(|u| u.as_str()).ok_or_else(|| Error::invalid("url", "the Postgres Adapter needs a url"))?;
        let schema = s.get("schema").and_then(|u| u.as_str());
        let readers = s.get("readers").and_then(|r| r.as_u64()).map(|r| r as usize).unwrap_or(4);
        Ok(Some(PortImpl::Db(Arc::new(PgDb::open(url, schema, readers)?))))
    }
}
