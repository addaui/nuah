// SPDX-License-Identifier: Apache-2.0
//! What every data Adapter must do. The suite tests the guarantees the kernel depends on (snapshot
//! reads, atomic writes, nested undo, tombstones, ordering, the change log), not any mechanism.

use nuah_kernel::*;
use serde_json::{json, Value};

fn put(w: &mut dyn DbWrite, c: &str, id: &str, body: Value) -> Doc {
    w.put(c, id, "t", body, Expect::Any, 1000).unwrap()
}

fn ids(p: &Page) -> Vec<String> {
    p.items.iter().map(|d| d.id.clone()).collect()
}

pub fn run(db: &dyn Db) {
    envelope(db);
    expectations(db);
    queries(db);
    atomicity(db);
    nested(db);
    change_log(db);
}

fn envelope(db: &dyn Db) {
    db.write(&mut |w| {
        let a = put(w, "e", "a", json!({"x": 1}));
        assert_eq!((a.rev, a.deleted, a.owner.as_str()), (1, false, "t"));
        let b = put(w, "e", "a", json!({"x": 2}));
        assert_eq!(b.rev, 2);
        let t = w.delete("e", "a", Expect::Any, 2000).unwrap().unwrap();
        assert_eq!((t.rev, t.deleted), (3, true));
        assert!(w.delete("e", "a", Expect::Any, 2000).unwrap().is_none(), "deleting twice does nothing");
        let again = put(w, "e", "a", json!({"x": 3}));
        assert_eq!(again.rev, 4, "a tombstone keeps counting");
        Ok(())
    })
    .unwrap();
    db.read(&mut |r| {
        assert_eq!(r.get("e", "a")?.unwrap().body["x"], 3);
        assert!(r.get("e", "missing")?.is_none());
        Ok(())
    })
    .unwrap();
}

fn expectations(db: &dyn Db) {
    db.write(&mut |w| {
        put(w, "x", "k", json!({}));
        assert_eq!(w.put("x", "k", "t", json!({}), Expect::Absent, 1).unwrap_err().category, Category::Conflict);
        assert_eq!(w.put("x", "k", "t", json!({}), Expect::Rev(7), 1).unwrap_err().category, Category::Conflict);
        w.put("x", "k", "t", json!({"ok": true}), Expect::Rev(1), 1).unwrap();
        w.delete("x", "k", Expect::Any, 1).unwrap();
        w.put("x", "k", "t", json!({}), Expect::Absent, 1).expect("absent after a tombstone");
        Ok(())
    })
    .unwrap();
}

fn queries(db: &dyn Db) {
    db.write(&mut |w| {
        for (i, (s, p)) in [("todo", 3), ("doing", 5), ("todo", 1), ("done", 9), ("todo", 7)].iter().enumerate() {
            put(w, "q", &format!("d{i}"), json!({"s": s, "p": p, "n": {"deep": i}, "t": i % 2 == 0}));
        }
        put(w, "q", "gone", json!({"s": "todo", "p": 0}));
        w.delete("q", "gone", Expect::Any, 1).unwrap();
        put(w, "other", "o", json!({"s": "todo"}));
        Ok(())
    })
    .unwrap();
    db.write(&mut |w| {
        put(w, "tg", "a", json!({"tags": ["x", "y"]}));
        put(w, "tg", "b", json!({"tags": ["y"]}));
        put(w, "tg", "c", json!({"tags": []}));
        put(w, "tg", "d", json!({"tags": "y"}));
        Ok(())
    })
    .unwrap();
    db.read(&mut |r| {
        assert_eq!(ids(&r.query("tg", &Query::new().has("tags", "y"))?), ["a", "b"]);
        assert_eq!(ids(&r.query("tg", &Query::new().has("tags", "x"))?), ["a"]);
        Ok(())
    })
    .unwrap();
    db.read(&mut |r| {
        let todo = r.query("q", &Query::new().eq("s", "todo"))?;
        assert_eq!(ids(&todo), ["d0", "d2", "d4"], "default order is by id; tombstones and other collections are invisible");
        assert_eq!(ids(&r.query("q", &Query::new().eq("s", "todo").desc("p"))?), ["d4", "d0", "d2"]);
        assert_eq!(ids(&r.query("q", &Query::new().gte("p", 5).asc("p"))?), ["d1", "d4", "d3"]);
        assert_eq!(ids(&r.query("q", &Query::new().gt("p", 5).lt("p", 9))?), ["d4"]);
        assert_eq!(ids(&r.query("q", &Query::new().is_in("s", vec![json!("done"), json!("doing")]))?), ["d1", "d3"]);
        assert_eq!(ids(&r.query("q", &Query::new().ne("s", "todo"))?), ["d1", "d3"]);
        assert_eq!(ids(&r.query("q", &Query::new().eq("n.deep", 3))?), ["d3"]);
        assert_eq!(ids(&r.query("q", &Query::new().eq("t", true))?), ["d0", "d2", "d4"]);
        assert_eq!(ids(&r.query("q", &Query::new().eq("missing", Value::Null))?).len(), 5, "missing behaves like null");
        assert!(r.query("q", &Query::new().gt("missing", 0))?.items.is_empty(), "ordered comparisons never match a missing field");
        let p1 = r.query("q", &Query::new().asc("p").limit(2))?;
        assert_eq!((ids(&p1), p1.next), (vec!["d2".to_string(), "d0".to_string()], Some(2)));
        let p3 = r.query("q", &Query::new().asc("p").limit(2).offset(4))?;
        assert_eq!((ids(&p3), p3.next), (vec!["d3".to_string()], None));
        assert_eq!(r.count("q", &[])?, 5);
        assert!(r.query("q", &Query::new().has("tags", "a"))?.items.is_empty(), "a missing array contains nothing");
        assert_eq!(r.count("q", &[Cond { field: "s".into(), op: Cmp::Eq, value: json!("todo") }])?, 3);
        assert!(r.query("q", &Query::new().eq("a b", 1)).is_err(), "field paths are checked");
        assert!(r.query("q", &Query::new().eq("a'); drop table docs; --", 1)).is_err());
        Ok(())
    })
    .unwrap();
}

fn atomicity(db: &dyn Db) {
    let r = db.write(&mut |w| {
        put(w, "at", "a", json!({}));
        w.append_change(ChangeRecord { seq: 0, at: 1, actor: "x".into(), request: "r".into(), operation: "o".into(), docs: vec![], events: vec![] })?;
        Err(Error::internal("boom"))
    });
    assert!(r.is_err());
    db.read(&mut |r| {
        assert!(r.get("at", "a")?.is_none(), "a failed write leaves nothing");
        Ok(())
    })
    .unwrap();
    // a reader keeps one view even while a write goes on elsewhere is covered by the kernel tests
}

fn nested(db: &dyn Db) {
    db.write(&mut |w| {
        put(w, "n", "keep", json!({"v": 1}));
        w.begin_nested()?;
        put(w, "n", "gone", json!({"v": 2}));
        put(w, "n", "keep", json!({"v": 99}));
        w.end_nested(false)?;
        w.begin_nested()?;
        put(w, "n", "kept2", json!({"v": 3}));
        w.begin_nested()?;
        put(w, "n", "inner", json!({"v": 4}));
        w.end_nested(false)?;
        w.end_nested(true)?;
        assert!(w.end_nested(true).is_err(), "nothing left to close");
        Ok(())
    })
    .unwrap();
    db.read(&mut |r| {
        assert_eq!(r.get("n", "keep")?.unwrap().body["v"], 1);
        assert!(r.get("n", "gone")?.is_none());
        assert!(r.get("n", "kept2")?.is_some());
        assert!(r.get("n", "inner")?.is_none());
        Ok(())
    })
    .unwrap();
}

fn change_log(db: &dyn Db) {
    let mut base = 0;
    db.read(&mut |r| {
        base = r.latest_change()?;
        Ok(())
    })
    .unwrap();
    let mut seqs = vec![];
    for i in 0..5 {
        db.write(&mut |w| {
            seqs.truncate(i);
            seqs.push(w.append_change(ChangeRecord { seq: 0, at: i as i64, actor: "a".into(), request: format!("r{i}"), operation: "op".into(), docs: vec![], events: vec![Event { kind: "k".into(), audience: Audience::subject("s"), data: json!({"i": i}) }] })?);
            Ok(())
        })
        .unwrap();
    }
    assert_eq!(seqs, (base + 1..=base + 5).collect::<Vec<_>>(), "numbers grow by one, in commit order");
    db.read(&mut |r| {
        let got = r.changes_after(base + 2, 10)?;
        assert_eq!(got.iter().map(|c| c.seq).collect::<Vec<_>>(), vec![base + 3, base + 4, base + 5]);
        assert_eq!(got[0].events[0].data["i"], 2);
        assert_eq!(r.changes_after(base, 2)?.len(), 2);
        assert_eq!(r.latest_change()?, base + 5);
        Ok(())
    })
    .unwrap();
    db.write(&mut |w| w.prune_changes(2)).unwrap();
    db.read(&mut |r| {
        assert_eq!(r.latest_change()?, base + 5, "pruning never lowers the latest number");
        assert_eq!(r.oldest_change()?, base + 4);
        assert_eq!(r.changes_after(0, 10)?.len(), 2);
        Ok(())
    })
    .unwrap();
}
