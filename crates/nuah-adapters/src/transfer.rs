// SPDX-License-Identifier: Apache-2.0
//! Export and import in a neutral format, through the data Port alone, so data can leave any Adapter and enter
//! any other: one JSON value per line. The first line is a header, then documents (tombstones and the
//! kernel's own `_meta` included), then the change log.
//!
//! ```text
//! {"nuah_export":1,"latest_change":42}
//! {"doc":{...}}
//! {"change":{...}}
//! ```

use nuah_kernel::*;
use serde_json::{json, Value};
use std::io::{BufRead, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub docs: u64,
    pub changes: u64,
}

fn io(e: std::io::Error) -> Error {
    Error::internal(format!("transfer: {e}"))
}

const PAGE: usize = 500;

/// Write everything in `db` as it was at one moment.
pub fn export(db: &dyn Db, out: &mut dyn Write) -> Result<Counts> {
    let mut counts = Counts::default();
    db.read(&mut |r| {
        counts = Counts::default();
        let latest = r.latest_change()?;
        writeln!(out, "{}", json!({"nuah_export": 1, "latest_change": latest})).map_err(io)?;
        let (mut c, mut i) = (String::new(), String::new());
        loop {
            let page = r.scan(&c, &i, PAGE)?;
            let Some(last) = page.last() else { break };
            (c, i) = (last.collection.clone(), last.id.clone());
            for d in &page {
                writeln!(out, "{}", json!({"doc": d})).map_err(io)?;
                counts.docs += 1;
            }
        }
        let mut after = r.oldest_change()?.saturating_sub(1);
        loop {
            let page = r.changes_after(after, PAGE)?;
            let Some(last) = page.last() else { break };
            after = last.seq;
            for rec in &page {
                writeln!(out, "{}", json!({"change": rec})).map_err(io)?;
                counts.changes += 1;
            }
        }
        Ok(())
    })?;
    Ok(counts)
}

/// Read an export into an empty database, in batches. The target must be empty (nothing stored, no change numbers
/// used); an import that fails part-way leaves a partial database, so start again from an empty one.
pub fn import(db: &dyn Db, input: &mut dyn BufRead) -> Result<Counts> {
    let mut empty = false;
    db.read(&mut |r| {
        empty = r.scan("", "", 1)?.is_empty() && r.latest_change()? == 0;
        Ok(())
    })?;
    if !empty {
        return Err(Error::invalid("target", "the database is not empty; import into a new one"));
    }
    let mut lines = input.lines();
    let header: Value = serde_json::from_str(&lines.next().ok_or_else(|| Error::invalid("export", "the file is empty"))?.map_err(io)?)
        .map_err(|e| Error::invalid("export", &format!("the first line is not JSON: {e}")))?;
    if header["nuah_export"] != 1 {
        return Err(Error::invalid("export", "not a Nuah export, or a newer format"));
    }
    let latest = header["latest_change"].as_u64().unwrap_or(0);
    let mut counts = Counts::default();
    let mut batch: Vec<Value> = vec![];
    let flush = |batch: &mut Vec<Value>, counts: &mut Counts| -> Result<()> {
        let items = std::mem::take(batch);
        let (mut docs, mut changes) = (0, 0);
        db.write(&mut |w| {
            docs = 0;
            changes = 0;
            for v in &items {
                if let Some(d) = v.get("doc") {
                    w.restore(serde_json::from_value(d.clone()).map_err(|e| Error::invalid("export", &format!("bad document: {e}")))?)?;
                    docs += 1;
                } else if let Some(c) = v.get("change") {
                    w.restore_change(serde_json::from_value(c.clone()).map_err(|e| Error::invalid("export", &format!("bad change record: {e}")))?)?;
                    changes += 1;
                } else {
                    return Err(Error::invalid("export", "a line is neither a document nor a change record"));
                }
            }
            Ok(())
        })?;
        counts.docs += docs;
        counts.changes += changes;
        Ok(())
    };
    for line in lines {
        let line = line.map_err(io)?;
        if line.trim().is_empty() {
            continue;
        }
        batch.push(serde_json::from_str(&line).map_err(|e| Error::invalid("export", &format!("a line is not JSON: {e}")))?);
        if batch.len() >= PAGE {
            flush(&mut batch, &mut counts)?;
        }
    }
    flush(&mut batch, &mut counts)?;
    db.write(&mut |w| w.restore_latest_change(latest))?;
    Ok(counts)
}
