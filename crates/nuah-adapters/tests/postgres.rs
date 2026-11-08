// SPDX-License-Identifier: Apache-2.0
//! The Postgres Adapter against a real server. Set `NUAH_TEST_PG_URL` (for example
//! `postgres://postgres:secret@127.0.0.1:55432/postgres`); without it these tests say so and pass.
use nuah_adapters::*;
use nuah_kernel::*;
use serde_json::json;
use std::sync::atomic::{AtomicU32, Ordering};

static N: AtomicU32 = AtomicU32::new(0);

fn url() -> Option<String> {
    match std::env::var("NUAH_TEST_PG_URL") {
        Ok(u) => Some(u),
        Err(_) => {
            eprintln!("NUAH_TEST_PG_URL is not set: skipping the Postgres tests");
            None
        }
    }
}

/// A schema of its own for one test, dropped at the end.
struct Schema(String, String);
impl Schema {
    fn new(url: &str) -> Schema {
        Schema(format!("nt_{}_{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)), url.to_string())
    }
    fn open(&self, readers: usize) -> Result<PgDb> {
        PgDb::open(&self.1, Some(&self.0), readers)
    }
}
impl Drop for Schema {
    fn drop(&mut self) {
        if let Ok(mut c) = postgres::Client::connect(&self.1, postgres::NoTls) {
            let _ = c.batch_execute(&format!("DROP SCHEMA IF EXISTS {} CASCADE", self.0));
        }
    }
}

#[test]
fn postgres_conforms() {
    let Some(u) = url() else { return };
    let s = Schema::new(&u);
    conformance::run(&s.open(4).unwrap());
}

#[test]
fn postgres_conforms_with_reads_on_the_writer_connection() {
    let Some(u) = url() else { return };
    let s = Schema::new(&u);
    conformance::run(&s.open(0).unwrap());
}

#[test]
fn postgres_gives_the_answers_the_port_defines() {
    let Some(u) = url() else { return };
    for seed in 1..=4u64 {
        let s = Schema::new(&u);
        conformance::differential(&MemoryDb::new(), &s.open(2).unwrap(), seed);
    }
}

#[test]
fn a_second_process_on_the_same_database_is_refused_and_the_lock_goes_with_the_first() {
    let Some(u) = url() else { return };
    let s = Schema::new(&u);
    let first = s.open(1).unwrap();
    let e = s.open(1).err().expect("must refuse");
    assert!(e.message.contains("another Nuah process"), "{}", e.message);
    // a different schema is a different database as far as Nuah is concerned
    let other = Schema::new(&u);
    drop(other.open(1).unwrap());
    drop(first);
    s.open(1).expect("the lock is released when the Adapter is dropped");
}

#[test]
fn data_survives_a_restart_and_goes_round_to_other_adapters() {
    let Some(u) = url() else { return };
    let s = Schema::new(&u);
    let a = MemoryDb::new();
    a.write(&mut |w| {
        for i in 0..700 {
            w.put("c", &format!("d{i:04}"), "t", json!({"i": i, "t": [i, "x"]}), Expect::Any, 1)?;
        }
        w.delete("c", "d0003", Expect::Any, 2)?;
        for i in 0..5 {
            w.append_change(ChangeRecord { seq: 0, at: i, actor: "a".into(), request: "r".into(), operation: "op".into(), docs: vec![], events: vec![] })?;
        }
        w.prune_changes(2)
    })
    .unwrap();
    {
        let pg = s.open(2).unwrap();
        let mut buf = vec![];
        export(&a, &mut buf).unwrap();
        import(&pg, &mut std::io::BufReader::new(&buf[..])).unwrap();
    }
    let pg = s.open(2).unwrap();
    let (mut back, mut again) = (vec![], vec![]);
    export(&pg, &mut back).unwrap();
    export(&a, &mut again).unwrap();
    // jsonb keeps the same values, though not the key order inside text; compare as parsed lines
    let parse = |b: &[u8]| String::from_utf8_lossy(b).lines().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()).collect::<Vec<_>>();
    assert_eq!(parse(&back), parse(&again));
    pg.write(&mut |w| w.append_change(ChangeRecord { seq: 0, at: 0, actor: "a".into(), request: "r".into(), operation: "o".into(), docs: vec![], events: vec![] }).map(|n| assert_eq!(n, 6))).unwrap();
}

#[test]
fn a_database_from_a_newer_layout_is_refused() {
    let Some(u) = url() else { return };
    let s = Schema::new(&u);
    drop(s.open(1).unwrap());
    let mut c = postgres::Client::connect(&u, postgres::NoTls).unwrap();
    c.batch_execute(&format!("UPDATE {}.nuah_state SET value=99 WHERE key='layout'", s.0)).unwrap();
    let e = s.open(1).err().expect("must refuse");
    assert!(e.message.contains("99") && e.message.contains("newer"), "{}", e.message);
}

#[test]
fn unique_indexes_survive_a_restart_and_keep_refusing_without_poisoning_the_transaction() {
    let Some(u) = url() else { return };
    let s = Schema::new(&u);
    {
        let pg = s.open(1).unwrap();
        pg.ensure_index(&IndexSpec::on("u", "email").unique()).unwrap();
    }
    let pg = s.open(1).unwrap();
    pg.write(&mut |w| {
        w.put("u", "a", "t", json!({"email": "x"}), Expect::Any, 1)?;
        let e = w.put("u", "b", "t", json!({"email": "x"}), Expect::Any, 1).unwrap_err();
        assert_eq!(e.reason.as_deref(), Some("kernel.unique"));
        // the transaction is still usable
        w.put("u", "c", "t", json!({"email": "y"}), Expect::Any, 1)?;
        Ok(())
    })
    .unwrap();
}
