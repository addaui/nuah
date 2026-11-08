// SPDX-License-Identifier: Apache-2.0
use nuah_adapters::transfer::Counts;
use nuah_adapters::*;
use nuah_kernel::*;
use serde_json::json;

fn fill(db: &dyn Db) {
    db.write(&mut |w| {
        for i in 0..1200 {
            w.put("c", &format!("d{i:04}"), "t", json!({"i": i, "s": (if i % 2 == 0 { "x" } else { "y" }), "o": {"k": null}}), Expect::Any, 7)?;
        }
        w.put("_meta", "version:t", "kernel", json!({"version": 3}), Expect::Any, 7)?;
        w.delete("c", "d0005", Expect::Any, 8)?;
        w.put("c", "d0006", "t", json!({"again": true}), Expect::Any, 9)?;
        Ok(())
    })
    .unwrap();
    for i in 0..30 {
        db.write(&mut |w| {
            w.append_change(ChangeRecord { seq: 0, at: i, actor: "a".into(), request: format!("r{i}"), operation: "op".into(), docs: vec![DocRef { collection: "c".into(), id: "d0006".into(), rev: 2, kind: ChangeKind::Updated }], events: vec![Event { kind: "k".into(), audience: Audience::subject("s"), data: json!({"i": i}) }] })?;
            Ok(())
        })
        .unwrap();
    }
    db.write(&mut |w| w.prune_changes(10)).unwrap();
}

#[derive(Debug, PartialEq)]
struct Everything {
    docs: Vec<Doc>,
    changes: Vec<ChangeRecord>,
    latest: u64,
    oldest: u64,
}

fn everything(db: &dyn Db) -> Everything {
    let mut e = Everything { docs: vec![], changes: vec![], latest: 0, oldest: 0 };
    db.read(&mut |r| {
        e.docs.clear();
        let (mut c, mut i) = (String::new(), String::new());
        loop {
            let page = r.scan(&c, &i, 100)?;
            let Some(last) = page.last() else { break };
            (c, i) = (last.collection.clone(), last.id.clone());
            e.docs.extend(page);
        }
        e.changes = r.changes_after(0, 1000)?;
        e.latest = r.latest_change()?;
        e.oldest = r.oldest_change()?;
        Ok(())
    })
    .unwrap();
    e
}

fn through(from: &dyn Db, to: &dyn Db) -> Counts {
    let mut buf = vec![];
    let out = export(from, &mut buf).unwrap();
    let back = import(to, &mut std::io::BufReader::new(&buf[..])).unwrap();
    assert_eq!(out, back);
    back
}

#[test]
fn data_goes_round_between_adapters_unchanged() {
    let a = MemoryDb::new();
    fill(&a);
    let want = everything(&a);
    assert_eq!(want.latest, 30, "the log was pruned but the numbers were not reused");
    assert_eq!(want.changes.len(), 10);
    let dir = std::env::temp_dir().join(format!("nuah-transfer-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let b = SqliteDb::open(dir.join("b.db").to_str().unwrap()).unwrap();
    let counts = through(&a, &b);
    assert_eq!((counts.docs, counts.changes), (1201, 10));
    assert_eq!(everything(&b), want);
    let c = MemoryDb::new();
    through(&b, &c);
    assert_eq!(everything(&c), want);
    // numbers go on from where they were
    b.write(&mut |w| w.append_change(ChangeRecord { seq: 0, at: 0, actor: "a".into(), request: "r".into(), operation: "o".into(), docs: vec![], events: vec![] }).map(|n| assert_eq!(n, 31))).unwrap();
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn an_empty_log_that_had_numbers_keeps_them() {
    let a = MemoryDb::new();
    fill(&a);
    a.write(&mut |w| w.prune_changes(0)).unwrap();
    let b = SqliteDb::open(":memory:").unwrap();
    through(&a, &b);
    assert_eq!(everything(&b), everything(&a));
    assert_eq!(everything(&b).latest, 30);
}

#[test]
fn import_refuses_a_database_that_is_not_empty_and_bad_files() {
    let a = MemoryDb::new();
    fill(&a);
    let mut buf = vec![];
    export(&a, &mut buf).unwrap();
    let e = import(&a, &mut std::io::BufReader::new(&buf[..])).unwrap_err();
    assert_eq!(e.category, Category::Invalid);
    for bad in ["", "not json\n", "{\"nuah_export\":2}\n", "{\"nuah_export\":1,\"latest_change\":0}\n{\"x\":1}\n"] {
        let fresh = MemoryDb::new();
        assert_eq!(import(&fresh, &mut std::io::BufReader::new(bad.as_bytes())).unwrap_err().category, Category::Invalid, "{bad:?}");
    }
}

#[test]
fn sqlite_snapshot_is_a_whole_copy_that_opens() {
    let dir = std::env::temp_dir().join(format!("nuah-snap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = SqliteDb::open(dir.join("live.db").to_str().unwrap()).unwrap();
    fill(&db);
    let copy = dir.join("copy.db");
    db.snapshot(copy.to_str().unwrap()).unwrap();
    assert_eq!(db.snapshot(copy.to_str().unwrap()).unwrap_err().category, Category::Invalid, "never over an existing file");
    let opened = SqliteDb::open(copy.to_str().unwrap()).unwrap();
    assert_eq!(everything(&opened), everything(&db));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_database_from_a_newer_layout_is_refused() {
    let dir = std::env::temp_dir().join(format!("nuah-layout-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("n.db");
    drop(SqliteDb::open(path.to_str().unwrap()).unwrap());
    rusqlite_set_version(&path, 99);
    let e = SqliteDb::open(path.to_str().unwrap()).err().expect("must refuse");
    assert!(e.message.contains("99") && e.message.contains("newer"), "{}", e.message);
    std::fs::remove_dir_all(dir).ok();
}

fn rusqlite_set_version(path: &std::path::Path, v: u32) {
    let c = rusqlite::Connection::open(path).unwrap();
    c.pragma_update(None, "user_version", v).unwrap();
}
