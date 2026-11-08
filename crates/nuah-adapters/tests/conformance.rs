// SPDX-License-Identifier: Apache-2.0
use nuah_adapters::*;
use nuah_kernel::*;
use serde_json::json;

#[test]
fn memory_conforms() {
    conformance::run(&MemoryDb::new());
}

#[test]
fn sqlite_memory_conforms() {
    conformance::run(&SqliteDb::open(":memory:").unwrap());
}

#[test]
fn sqlite_file_conforms() {
    let dir = std::env::temp_dir().join(format!("nuah-conf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.db");
    conformance::run(&SqliteDb::open(path.to_str().unwrap()).unwrap());
    std::fs::remove_dir_all(dir).ok();
}

/// A deterministic pseudo-random sequence of writes, checked to give the same answers on both Adapters.
#[test]
fn memory_and_sqlite_agree_on_random_sessions() {
    for seed in 1..=5u64 {
        let (a, b) = (MemoryDb::new(), SqliteDb::open(":memory:").unwrap());
        let mut s = seed.wrapping_mul(0x9E3779B97F4A7C15);
        let mut next = move |n: u64| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s % n
        };
        for _ in 0..300 {
            let (id, status, pri, op) = (format!("d{}", next(12)), ["todo", "doing", "done"][next(3) as usize], next(10) as i64, next(5));
            for db in [&a as &dyn Db, &b as &dyn Db] {
                let _ = db.write(&mut |w| {
                    match op {
                        0 | 1 | 2 => {
                            w.put("c", &id, "t", json!({"s": status, "p": pri, "o": {"k": pri % 3}}), Expect::Any, 5)?;
                        }
                        3 => {
                            w.delete("c", &id, Expect::Any, 5)?;
                        }
                        _ => {
                            let _ = w.put("c", &id, "t", json!({"s": status}), Expect::Absent, 5);
                        }
                    }
                    Ok(())
                });
            }
        }
        let qs = [
            Query::new(),
            Query::new().eq("s", "todo").desc("p"),
            Query::new().gte("p", 4).asc("p").limit(5).offset(1),
            Query::new().is_in("s", vec![json!("done"), json!("doing")]),
            Query::new().ne("s", "todo").asc("o.k"),
            Query::new().lt("p", 5).desc("s"),
        ];
        for q in &qs {
            let (mut ra, mut rb) = (None, None);
            a.read(&mut |r| { ra = Some(r.query("c", q)?); Ok(()) }).unwrap();
            b.read(&mut |r| { rb = Some(r.query("c", q)?); Ok(()) }).unwrap();
            assert_eq!(ra, rb, "seed {seed}, query {q:?}");
        }
    }
}

/// Many threads read while one writes: reads never block on the writer, every read sees a whole commit,
/// and a read that starts after a commit sees it.
#[test]
fn sqlite_readers_run_beside_the_writer() {
    let dir = std::env::temp_dir().join(format!("nuah-pool-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("p.db");
    let db = std::sync::Arc::new(SqliteDb::open_with(path.to_str().unwrap(), 4).unwrap());
    db.write(&mut |w| { w.put("c", "counter", "t", json!({"n": 0, "m": 0}), Expect::Any, 1).map(|_| ()) }).unwrap();
    let writer = { let db = db.clone(); std::thread::spawn(move || for i in 1..=200 { db.write(&mut |w| { w.put("c", "counter", "t", json!({"n": i, "m": i}), Expect::Any, 1).map(|_| ()) }).unwrap(); }) };
    let readers: Vec<_> = (0..8).map(|_| { let db = db.clone(); std::thread::spawn(move || for _ in 0..200 {
        db.read(&mut |r| { let d = r.get("c", "counter")?.unwrap(); assert_eq!(d.body["n"], d.body["m"], "a read saw half a commit"); Ok(()) }).unwrap();
    }) }).collect();
    writer.join().unwrap();
    for r in readers { r.join().unwrap(); }
    db.read(&mut |r| { assert_eq!(r.get("c", "counter")?.unwrap().body["n"], 200); Ok(()) }).unwrap();
    drop(db);
    std::fs::remove_dir_all(dir).ok();
}
