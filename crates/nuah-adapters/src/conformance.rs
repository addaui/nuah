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
    mixed_types(db);
    atomicity(db);
    nested(db);
    change_log(db);
    concurrent_writers(db);
    scan_all(db);
    features(db);
}

/// `scan` walks every document, tombstones included, in byte order of `(collection, id)`, a page at a time.
fn scan_all(db: &dyn Db) {
    db.write(&mut |w| {
        for (c, id) in [("sc", "b"), ("sc", "a"), ("sc", "B"), ("sc2", "a"), ("sc", "ab")] {
            put(w, c, id, json!({"k": id}));
        }
        w.delete("sc", "a", Expect::Any, 1).unwrap();
        Ok(())
    })
    .unwrap();
    db.read(&mut |r| {
        let mut got = vec![];
        let (mut c, mut i) = (String::new(), String::new());
        loop {
            let page = r.scan(&c, &i, 2)?;
            let Some(last) = page.last() else { break };
            (c, i) = (last.collection.clone(), last.id.clone());
            got.extend(page.into_iter().filter(|d| d.collection.starts_with("sc")).map(|d| (d.collection, d.id, d.deleted)));
        }
        let want: Vec<(String, String, bool)> = [("sc", "B", false), ("sc", "a", true), ("sc", "ab", false), ("sc", "b", false), ("sc2", "a", false)].iter().map(|(c, i, d)| (c.to_string(), i.to_string(), *d)).collect();
        assert_eq!(got, want, "bytes order, tombstones included");
        Ok(())
    })
    .unwrap();
}

/// What the Adapter claims, tested; and what it does not claim, refused with `unsupported`.
fn features(db: &dyn Db) {
    let f = db.features();
    for name in f.have.keys() {
        assert!([feature::INDEX_UNIQUE, feature::INDEX_COMPOSITE, feature::BACKUP_SNAPSHOT].contains(&name.as_str()), "unknown feature {name}");
    }
    if f.has(feature::INDEX_UNIQUE) {
        index_unique(db);
    } else {
        assert_eq!(db.ensure_index(&IndexSpec::on("nu", "k").unique()).unwrap_err().category, Category::Unsupported);
    }
    if f.has(feature::INDEX_COMPOSITE) {
        index_composite(db);
    }
    if !f.has(feature::BACKUP_SNAPSHOT) {
        assert_eq!(db.snapshot("/nonexistent/never").unwrap_err().category, Category::Unsupported);
    }
}

fn conflict_unique(r: Result<Doc>) {
    let e = r.expect_err("a unique index must refuse this");
    assert_eq!((e.category, e.reason.as_deref()), (Category::Conflict, Some("kernel.unique")), "{e:?}");
}

fn index_unique(db: &dyn Db) {
    // a rule over live documents; missing and null are exempt; types are not mixed up
    db.ensure_index(&IndexSpec::on("uq", "email").unique()).unwrap();
    db.ensure_index(&IndexSpec::on("uq", "email").unique()).expect("asking again is fine");
    db.write(&mut |w| {
        put(w, "uq", "a", json!({"email": "x"}));
        put(w, "uq", "b", json!({"email": "y"}));
        conflict_unique(w.put("uq", "c", "t", json!({"email": "x"}), Expect::Any, 1));
        put(w, "uq", "a", json!({"email": "x", "more": 1}));
        put(w, "uq", "n1", json!({}));
        put(w, "uq", "n2", json!({}));
        put(w, "uq", "n3", json!({"email": null}));
        put(w, "uq", "n4", json!({"email": null}));
        put(w, "uq", "one", json!({"email": 1}));
        put(w, "uq", "yes", json!({"email": true}));
        put(w, "uq", "str", json!({"email": "1"}));
        conflict_unique(w.put("uq", "one2", "t", json!({"email": 1}), Expect::Any, 1));
        conflict_unique(w.put("uq", "yes2", "t", json!({"email": true}), Expect::Any, 1));
        // a tombstone gives the value back
        w.delete("uq", "a", Expect::Any, 1).unwrap();
        put(w, "uq", "c", json!({"email": "x"}));
        Ok(())
    })
    .unwrap();
    // another collection is not affected
    db.write(&mut |w| {
        put(w, "uq_other", "z", json!({"email": "x"}));
        put(w, "uq_other", "z2", json!({"email": "x"}));
        Ok(())
    })
    .unwrap();
    // the failed put leaves nothing behind
    let r = db.write(&mut |w| {
        put(w, "uq", "fresh", json!({"email": "fresh"}));
        w.put("uq", "dup", "t", json!({"email": "fresh"}), Expect::Any, 1)?;
        Ok(())
    });
    assert_eq!(r.unwrap_err().reason.as_deref(), Some("kernel.unique"));
    db.read(&mut |r| {
        assert!(r.get("uq", "fresh")?.is_none());
        Ok(())
    })
    .unwrap();
    // an index cannot be made over data that already breaks it
    db.write(&mut |w| {
        put(w, "uqd", "a", json!({"k": 1}));
        put(w, "uqd", "b", json!({"k": 1}));
        Ok(())
    })
    .unwrap();
    assert!(db.ensure_index(&IndexSpec::on("uqd", "k").unique()).is_err());
    // names that look alike are different indexes
    for c in ["un.a", "un_a"] {
        db.ensure_index(&IndexSpec::on(c, "x.y").unique()).unwrap();
        db.ensure_index(&IndexSpec::on(c, "x_y").unique()).unwrap();
    }
    db.write(&mut |w| {
        for c in ["un.a", "un_a"] {
            put(w, c, "1", json!({"x": {"y": 1}, "x_y": 1}));
            conflict_unique(w.put(c, "2", "t", json!({"x": {"y": 1}}), Expect::Any, 1));
            conflict_unique(w.put(c, "3", "t", json!({"x_y": 1}), Expect::Any, 1));
        }
        Ok(())
    })
    .unwrap();
    // several fields together
    db.ensure_index(&IndexSpec::on("uq2", "a").and("b").unique()).unwrap();
    db.write(&mut |w| {
        put(w, "uq2", "1", json!({"a": 1, "b": 2}));
        put(w, "uq2", "2", json!({"a": 1, "b": 3}));
        put(w, "uq2", "3", json!({"a": 2, "b": 2}));
        put(w, "uq2", "4", json!({"a": 1}));
        put(w, "uq2", "5", json!({"a": 1}));
        conflict_unique(w.put("uq2", "6", "t", json!({"a": 1, "b": 2}), Expect::Any, 1));
        Ok(())
    })
    .unwrap();
}

fn index_composite(db: &dyn Db) {
    db.ensure_index(&IndexSpec::on("cx", "a").and("b.c")).unwrap();
    db.write(&mut |w| {
        for i in 0..10 {
            put(w, "cx", &format!("d{i}"), json!({"a": i % 2, "b": {"c": i % 3}}));
        }
        Ok(())
    })
    .unwrap();
    db.read(&mut |r| {
        assert_eq!(ids(&r.query("cx", &Query::new().eq("a", 1).eq("b.c", 0))?), ["d3", "d9"]);
        Ok(())
    })
    .unwrap();
    assert!(db.ensure_index(&IndexSpec { collection: "cx".into(), fields: vec![], unique: false }).is_err());
    assert!(db.ensure_index(&IndexSpec::on("cx", "a b")).is_err());
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
        assert!(r.query("q", &Query::new().eq("n.0", 1)).is_err(), "a segment of digits is refused");
        assert!(r.query("q", &Query::new().eq("a'); drop table docs; --", 1)).is_err());
        Ok(())
    })
    .unwrap();
}

/// One field holding values of every JSON type: equality and order follow the Port's rule, never the
/// engine's own (a stored `true` is not `1`, an array is not a string).
fn mixed_types(db: &dyn Db) {
    let vals = [json!(null), json!(false), json!(true), json!(0), json!(1), json!(2.5), json!("1"), json!("a"), json!([1]), json!(["a"]), json!({"k": 1})];
    db.write(&mut |w| {
        for (i, v) in vals.iter().enumerate() {
            put(w, "mx", &format!("v{i:02}"), json!({ "f": v }));
        }
        put(w, "mx", "v99", json!({}));
        Ok(())
    })
    .unwrap();
    db.read(&mut |r| {
        let q = |qq: Query| ids(&r.query("mx", &qq).unwrap());
        assert_eq!(q(Query::new().eq("f", 1)), ["v04"], "1 is not true");
        assert_eq!(q(Query::new().eq("f", true)), ["v02"], "true is not 1");
        assert_eq!(q(Query::new().eq("f", "1")), ["v06"], "\"1\" is not 1");
        assert_eq!(q(Query::new().eq("f", json!([1]))), ["v08"]);
        assert_eq!(q(Query::new().eq("f", Value::Null)), ["v00", "v99"]);
        assert_eq!(q(Query::new().ne("f", 1)).len(), 11);
        assert_eq!(q(Query::new().is_in("f", vec![json!(1), json!("a")])), ["v04", "v07"]);
        assert_eq!(q(Query::new().gt("f", 0)), ["v04", "v05", "v06", "v07", "v08", "v09", "v10"], "bool < number < string < array < object");
        assert_eq!(q(Query::new().gte("f", 0)), ["v03", "v04", "v05", "v06", "v07", "v08", "v09", "v10"]);
        assert_eq!(q(Query::new().lt("f", 1)), ["v01", "v02", "v03"], "bool, then numbers below 1; null and missing never match");
        assert_eq!(q(Query::new().lt("f", "1")), ["v01", "v02", "v03", "v04", "v05"]);
        assert_eq!(q(Query::new().gt("f", "a")), ["v08", "v09", "v10"]);
        assert_eq!(q(Query::new().lte("f", false)), ["v01"]);
        assert_eq!(q(Query::new().gt("f", Value::Null)).len(), 10, "everything present and not null");
        let asc = q(Query::new().asc("f"));
        assert_eq!(asc[..8], ["v00", "v99", "v01", "v02", "v03", "v04", "v05", "v06"], "null, bool, numbers, then strings");
        assert_eq!((&asc[8..9], &asc[11..]), (&["v07".to_string()][..], &["v10".to_string()][..]));
        assert!(asc[9..11].contains(&"v08".to_string()) && asc[9..11].contains(&"v09".to_string()), "arrays come before objects; their order among themselves is not specified");
        assert_eq!(q(Query::new().desc("f"))[0], "v10");
        Ok(())
    })
    .unwrap();
    db.write(&mut |w| {
        put(w, "mh", "a", json!({"t": [1, true, "1", null, [2]]}));
        Ok(())
    })
    .unwrap();
    db.read(&mut |r| {
        for (v, hit) in [(json!(1), true), (json!(true), true), (json!("1"), true), (json!(null), true), (json!([2]), true), (json!(false), false), (json!(2), false), (json!("x"), false)] {
            assert_eq!(r.query("mh", &Query::new().has("t", v.clone()))?.items.len() == 1, hit, "has {v}");
        }
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

/// Several threads write at once. The change log still comes out in commit order: every number is used once,
/// the numbers have no gaps, and a reader that follows the log with a cursor sees every record exactly once,
/// in order, however the writes interleave. (A log numbered before commit would let a reader skip a record
/// that commits late.)
fn concurrent_writers(db: &dyn Db) {
    const THREADS: usize = 4;
    const EACH: usize = 40;
    let mut base = 0;
    db.read(&mut |r| {
        base = r.latest_change()?;
        Ok(())
    })
    .unwrap();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let mut assigned: Vec<u64> = vec![];
    let mut followed: Vec<u64> = vec![];
    std::thread::scope(|sc| {
        let writers: Vec<_> = (0..THREADS)
            .map(|t| {
                let done = &done;
                sc.spawn(move || {
                    let mut got = vec![];
                    for i in 0..EACH {
                        let mut seq = 0;
                        db.write(&mut |w| {
                            w.put("cw", &format!("t{t}"), "t", json!({"i": i}), Expect::Any, 1)?;
                            seq = w.append_change(ChangeRecord { seq: 0, at: 1, actor: format!("t{t}"), request: format!("r{t}-{i}"), operation: "op".into(), docs: vec![], events: vec![] })?;
                            Ok(())
                        })
                        .unwrap();
                        got.push(seq);
                    }
                    done.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    got
                })
            })
            .collect();
        let reader = sc.spawn(|| {
            let mut cursor = base;
            let mut seen = vec![];
            loop {
                let finished = done.load(std::sync::atomic::Ordering::SeqCst) == THREADS;
                db.read(&mut |r| {
                    for c in r.changes_after(cursor, 25)? {
                        cursor = c.seq;
                        seen.push(c.seq);
                    }
                    Ok(())
                })
                .unwrap();
                if finished && seen.len() == THREADS * EACH {
                    break;
                }
                if finished {
                    // one more pass after the writers ended picks up the rest
                    let mut more = 0;
                    db.read(&mut |r| {
                        more = r.changes_after(cursor, 1)?.len();
                        Ok(())
                    })
                    .unwrap();
                    if more == 0 {
                        break;
                    }
                }
            }
            seen
        });
        for w in writers {
            assigned.extend(w.join().unwrap());
        }
        followed = reader.join().unwrap();
    });
    let all: Vec<u64> = (base + 1..=base + (THREADS * EACH) as u64).collect();
    let mut sorted = assigned.clone();
    sorted.sort();
    assert_eq!(sorted, all, "every number is used once, with no gaps");
    assert_eq!(followed, all, "a reader following the log sees every record once, in order");
}

/// Random documents whose one field holds any JSON type, queried with every operator: `b` must give exactly the
/// answers of `a`. Meant for `a` = the in-memory Adapter, which applies the Port's rule directly. The order of
/// arrays and objects among themselves is not specified, so ordered operators only use plain values.
pub fn differential(a: &dyn Db, b: &dyn Db, seed: u64) {
    let flat = [json!(null), json!(false), json!(true), json!(0), json!(1), json!(2), json!(-3), json!(1.5), json!(""), json!("0"), json!("1"), json!("a"), json!("B"), json!("é")];
    let nested = [json!([]), json!([1]), json!(["a"]), json!([true]), json!({}), json!({"k": 1}), json!({"k": "a"})];
    let all: Vec<Value> = flat.iter().chain(nested.iter()).cloned().collect();
    let mut s = seed.wrapping_mul(0x9E3779B97F4A7C15) | 1;
    let mut next = move |n: usize| {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s % n as u64) as usize
    };
    for i in 0..60 {
        let wide = match next(8) {
            0 => json!({}),
            1 => json!({"o": {"f": all[next(all.len())].clone()}}),
            2 => json!({"f": [all[next(all.len())].clone(), flat[next(flat.len())].clone()]}),
            _ => json!({"f": all[next(all.len())].clone()}),
        };
        let plain = if next(6) == 0 { json!({}) } else { json!({"f": flat[next(flat.len())].clone()}) };
        for db in [a, b] {
            db.write(&mut |w| {
                w.put("wide", &format!("d{i:02}"), "t", wide.clone(), Expect::Any, 5)?;
                w.put("flat", &format!("d{i:02}"), "t", plain.clone(), Expect::Any, 5)?;
                Ok(())
            })
            .unwrap();
        }
    }
    let same = |coll: &str, q: &Query| {
        let (mut ra, mut rb) = (None, None);
        a.read(&mut |r| { ra = Some((r.query(coll, q)?, r.count(coll, &q.conds)?)); Ok(()) }).unwrap();
        b.read(&mut |r| { rb = Some((r.query(coll, q)?, r.count(coll, &q.conds)?)); Ok(()) }).unwrap();
        assert_eq!(ra, rb, "{} vs {}, seed {seed}, {coll}, {q:?}", a.name(), b.name());
    };
    for field in ["f", "o.f"] {
        for v in &all {
            same("wide", &Query::new().eq(field, v.clone()));
            same("wide", &Query::new().ne(field, v.clone()));
            same("wide", &Query::new().has(field, v.clone()));
            same("wide", &Query::new().is_in(field, vec![v.clone(), all[next(all.len())].clone()]));
        }
        for v in &flat {
            for coll in ["wide", "flat"] {
                same(coll, &Query::new().lt(field, v.clone()));
                same(coll, &Query::new().lte(field, v.clone()));
                same(coll, &Query::new().gt(field, v.clone()));
                same(coll, &Query::new().gte(field, v.clone()));
            }
            same("flat", &Query::new().gte(field, v.clone()).desc(field));
            same("flat", &Query::new().lt(field, v.clone()).asc(field).limit(7).offset(2));
        }
    }
    same("flat", &Query::new().asc("f"));
    same("flat", &Query::new().desc("f"));
    // an index changes speed, never answers
    for db in [b] {
        db.ensure_index(&IndexSpec::on("flat", "f")).unwrap();
        db.ensure_index(&IndexSpec::on("wide", "f")).unwrap();
    }
    for v in &flat {
        same("flat", &Query::new().gt("f", v.clone()).asc("f"));
        same("flat", &Query::new().eq("f", v.clone()));
        same("wide", &Query::new().eq("f", v.clone()));
    }
}
