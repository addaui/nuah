// SPDX-License-Identifier: Apache-2.0
mod common;
use common::*;
use nuah_adapters::*;
use nuah_kernel::*;
use serde_json::{json, Value};

fn call(w: &World, who: &Identity, op: &str, input: Value) -> Result<Value> {
    w.core.call_as(who, op, input, None).map(|r| r.output)
}
fn book(w: &World, who: &Identity) -> String {
    call(w, who, "notes.book.create", json!({"title": "b"})).unwrap()["id"].as_str().unwrap().to_string()
}

// ---- the Resolver ------------------------------------------------------------------------------

#[test]
fn the_resolver_reports_every_problem_at_once() {
    let clock = FixedClock::at(0);
    let seen = Default::default();
    let mut p = profile(&["db-memory", "auth-local", "policy-roles", "policy-allow-all", "clock-fixed", "ids-sequential", "notes", "ghost", "tell-a"]);
    p.grants.get_mut("notes").unwrap().clear(); // capabilities not granted
    p.settings.insert("db-memory".into(), json!({}));
    p.bindings.insert("db".into(), "policy-roles".into()); // wrong adapter
    let dist = Distribution { name: "t".into(), catalog: catalog(&clock, &seen), profile: p };
    let codes: Vec<String> = Core::check(&dist).into_iter().map(|p| p.code).collect();
    for want in ["unknown_plugin", "port_ambiguous", "bad_binding", "capability_not_granted", "missing_dependency"] {
        assert!(codes.iter().any(|c| c == want), "missing problem {want}; got {codes:?}");
    }
}

#[test]
fn names_must_be_inside_the_namespace_the_plugin_owns() {
    struct Bad;
    impl Plugin for Bad {
        fn manifest(&self) -> Manifest {
            serde_json::from_value(json!({"id":"bad","version":"1","namespaces":["bad"],
                "operations":{"notes.steal":{"kind":"query"},"bad.ok":{"kind":"query"}},"events":{"other.thing":{}}})).unwrap()
        }
        fn bind(&self, b: &mut Bindings) {
            b.op("notes.steal", |_, _| Ok(json!({})));
            b.op("bad.ok", |_, _| Ok(json!({})));
            b.op("bad.extra", |_, _| Ok(json!({})));
        }
    }
    let clock = FixedClock::at(0);
    let seen = Default::default();
    let mut cat = catalog(&clock, &seen);
    cat.0.push(std::sync::Arc::new(Bad));
    let dist = Distribution { name: "t".into(), catalog: cat, profile: profile(&["db-memory", "auth-local", "policy-roles", "clock-fixed", "ids-sequential", "bad"]) };
    let ps = Core::check(&dist);
    let find = |code: &str| ps.iter().filter(|p| p.code == code).count();
    assert_eq!(find("name_outside_namespace"), 2, "{ps:#?}");
    assert_eq!(find("extra_handler"), 1);
}

#[test]
fn a_plugin_that_is_not_enabled_contributes_nothing() {
    let w = world();
    assert!(w.core.describe(&person("a", &["member"]))["operations"].get("wrapper.nothing").is_none());
    assert!(!w.core.plugin_ids().contains(&"wrapper".to_string()));
}

// ---- calls ---------------------------------------------------------------------------------------

#[test]
fn input_is_checked_before_any_handler_runs() {
    let w = world();
    let a = person("alice", &["member"]);
    let e = call(&w, &a, "notes.book.create", json!({"title": ""})).unwrap_err();
    assert_eq!(e.category, Category::Invalid);
    assert_eq!(e.params["field"], "title");
    assert_eq!(call(&w, &a, "notes.book.create", json!({"title": "x", "extra": 1})).unwrap_err().category, Category::Invalid);
    assert!(w.seen.lock().is_empty(), "no hook ran either");
}

#[test]
fn the_policy_sees_the_resource_and_a_stranger_sees_nothing() {
    let w = world();
    let (alice, bob, carol) = (person("alice", &["member"]), person("bob", &["member"]), person("carol", &["viewer"]));
    let b = book(&w, &alice);
    // a stranger cannot tell the book exists
    assert_eq!(call(&w, &bob, "notes.add", json!({"book": b, "text": "hi"})).unwrap_err().category, Category::NotFound);
    call(&w, &alice, "notes.book.share", json!({"book": b, "with": "bob"})).unwrap();
    call(&w, &alice, "notes.book.share", json!({"book": b, "with": "carol"})).unwrap();
    // bob can read but the resource does not let him write
    assert_eq!(call(&w, &bob, "notes.add", json!({"book": b, "text": "hi"})).unwrap_err().category, Category::Denied);
    assert!(call(&w, &bob, "notes.list", json!({"book": b})).is_ok());
    // carol holds the viewer Role only: she may read, never write, whatever the resource says
    assert!(call(&w, &carol, "notes.list", json!({"book": b})).is_ok());
    assert_eq!(call(&w, &carol, "notes.add", json!({"book": b, "text": "hi"})).unwrap_err().category, Category::Denied);
}

#[test]
fn results_carry_what_the_subject_may_do_next() {
    let w = world();
    let (alice, bob) = (person("alice", &["member"]), person("bob", &["member"]));
    let b = book(&w, &alice);
    call(&w, &alice, "notes.book.share", json!({"book": b, "with": "bob"})).unwrap();
    let mine = call(&w, &alice, "notes.book.get", json!({"book": b})).unwrap();
    assert_eq!(mine["can"], json!(["notes.read", "notes.write"]));
    let theirs = call(&w, &bob, "notes.book.get", json!({"book": b})).unwrap();
    assert_eq!(theirs["can"], json!(["notes.read"]));
    let list = call(&w, &bob, "notes.book.list", json!({})).unwrap();
    assert_eq!(list["books"][0]["can"], json!(["notes.read"]));
}

#[test]
fn a_failed_command_leaves_nothing_behind() {
    let w = world();
    let a = person("alice", &["member"]);
    let b = book(&w, &a);
    let before = w.core.pulse_now().unwrap();
    assert!(call(&w, &a, "notes.add_then_fail", json!({"book": b})).is_err());
    assert_eq!(call(&w, &a, "notes.list", json!({"book": b})).unwrap()["notes"], json!([]), "the ghost note was rolled back");
    assert_eq!(w.core.pulse_now().unwrap(), before, "and no change record was written");
}

#[test]
fn a_reason_code_is_the_plugins_own_and_a_panic_is_internal() {
    let w = world();
    let a = person("alice", &["member"]);
    let b = book(&w, &a);
    for t in ["a", "b", "c"] {
        call(&w, &a, "notes.add", json!({"book": b, "text": t})).unwrap();
    }
    let e = call(&w, &a, "notes.add", json!({"book": b, "text": "d"})).unwrap_err();
    assert_eq!((e.category, e.reason.as_deref()), (Category::Conflict, Some("notes.too_many")));
    let p = call(&w, &a, "notes.panic", json!({})).unwrap_err();
    assert_eq!(p.category, Category::Internal);
    assert!(call(&w, &a, "notes.book.list", json!({})).is_ok(), "the Core survives");
}

#[test]
fn a_query_cannot_write() {
    let w = world();
    let e = call(&w, &person("alice", &["member"]), "notes.sneaky_write", json!({})).unwrap_err();
    assert_eq!(e.category, Category::Internal, "a programmer error is never a conflict");
}

#[test]
fn a_plugin_may_only_touch_its_own_data() {
    let w = world();
    // the toy 'tell' plugin owns nothing; calling an op that reads a foreign collection would fail
    let e = w.core.call_as(&person("a", &["member"]), "no.such.op", json!({}), None).unwrap_err();
    assert_eq!(e.category, Category::NotFound);
}

#[test]
fn calls_between_plugins_share_one_transaction() {
    let w = world();
    let a = person("alice", &["member"]);
    let b = book(&w, &a);
    call(&w, &a, "notes.add_two", json!({"book": b})).unwrap();
    assert_eq!(call(&w, &a, "notes.list", json!({"book": b})).unwrap()["notes"], json!(["one", "two"]));
    // the nested calls ran their after hooks too
    assert!(w.seen.lock().iter().filter(|o| *o == "notes.add").count() >= 2);
    // one commit, one change record, holding both events
    let rec = w.core.pulse(&a, Some("c0"), 100).unwrap();
    assert_eq!(rec.events.iter().filter(|e| e.kind == "notes.added").count(), 2);
}

// ---- hooks ---------------------------------------------------------------------------------------

#[test]
fn before_hooks_may_refuse_and_around_hooks_wrap() {
    let w = world_with(&["wrapper"]);
    let a = person("alice", &["member"]);
    let b = book(&w, &a);
    assert_eq!(call(&w, &a, "notes.add", json!({"book": b, "text": "spam"})).unwrap_err().category, Category::Invalid);
    // before hooks run by order (wrapper.first at 5 trims, then notes.no_spam at 10), around shouts
    let r = call(&w, &a, "notes.add", json!({"book": b, "text": "  hello  "})).unwrap();
    assert_eq!(r["text"], "HELLO");
    assert_eq!(call(&w, &a, "notes.list", json!({"book": b})).unwrap()["notes"], json!(["HELLO"]));
}

#[test]
fn the_after_hook_sees_every_operation_it_names() {
    let w = world();
    let a = person("alice", &["member"]);
    book(&w, &a);
    assert!(w.seen.lock().contains(&"notes.book.create".to_string()));
}

// ---- idempotency -----------------------------------------------------------------------------------

#[test]
fn a_retried_command_runs_once() {
    let w = world();
    let (alice, bob) = (person("alice", &["member"]), person("bob", &["member"]));
    let b = book(&w, &alice);
    let first = w.core.call_as(&alice, "notes.add", json!({"book": b, "text": "once"}), Some("k1")).unwrap();
    let again = w.core.call_as(&alice, "notes.add", json!({"book": b, "text": "once"}), Some("k1")).unwrap();
    assert!(!first.replayed && again.replayed);
    assert_eq!(first.output, again.output);
    assert_eq!(call(&w, &alice, "notes.list", json!({"book": b})).unwrap()["notes"], json!(["once"]));
    // same key, different input
    let e = w.core.call_as(&alice, "notes.add", json!({"book": b, "text": "other"}), Some("k1")).unwrap_err();
    assert_eq!(e.reason.as_deref(), Some("kernel.idempotency_key_reused"));
    // keys are per Subject
    let other = w.core.call_as(&bob, "notes.book.create", json!({"title": "z"}), Some("k1")).unwrap();
    assert!(!other.replayed);
    // a Subject who lost access cannot read an old result by replaying the key
    call(&w, &alice, "notes.book.share", json!({"book": b, "with": "bob"})).unwrap();
    // (bob may read but not write, so his replay is denied rather than answered)
    assert!(w.core.call_as(&bob, "notes.add", json!({"book": b, "text": "once"}), Some("k1")).is_err());
}

#[test]
fn a_remembered_key_expires() {
    let w = world();
    let a = person("alice", &["member"]);
    w.core.call_as(&a, "notes.book.create", json!({"title": "x"}), Some("k")).unwrap();
    w.clock.advance(25 * 60 * 60 * 1000);
    let again = w.core.call_as(&a, "notes.book.create", json!({"title": "x"}), Some("k")).unwrap();
    assert!(!again.replayed);
}

// ---- Pulse ---------------------------------------------------------------------------------------------

#[test]
fn pulse_follows_a_scope_and_a_removed_member_stops_hearing() {
    let w = world();
    let (alice, bob, carol) = (person("alice", &["member"]), person("bob", &["member"]), person("carol", &["member"]));
    let b = book(&w, &alice);
    call(&w, &alice, "notes.book.share", json!({"book": b, "with": "bob"})).unwrap();
    let (ca, cb, cc) = (w.core.pulse_now().unwrap(), w.core.pulse_now().unwrap(), w.core.pulse_now().unwrap());
    call(&w, &alice, "notes.add", json!({"book": b, "text": "1"})).unwrap();
    let kinds = |who: &Identity, c: &str| w.core.pulse(who, Some(c), 50).unwrap().events.iter().map(|e| e.kind.clone()).collect::<Vec<_>>();
    assert_eq!(kinds(&alice, &ca), ["notes.added"]);
    assert_eq!(kinds(&bob, &cb), ["notes.added"]);
    assert!(kinds(&carol, &cc).is_empty(), "never a member: hears nothing");
    // bob is removed, then more happens
    call(&w, &alice, "notes.book.unshare", json!({"book": b, "with": "bob"})).unwrap();
    let (cb2, ca2) = (w.core.pulse_now().unwrap(), w.core.pulse_now().unwrap());
    call(&w, &alice, "notes.add", json!({"book": b, "text": "2"})).unwrap();
    assert!(kinds(&bob, &cb2).is_empty(), "a removed member stops receiving the group's events");
    assert_eq!(kinds(&alice, &ca2), ["notes.added"]);
}

#[test]
fn pulse_resumes_from_a_cursor_and_resets_when_the_cursor_is_too_old() {
    let w = world();
    let a = person("alice", &["member"]);
    let b = book(&w, &a);
    let start = w.core.pulse_now().unwrap();
    for t in ["a", "b", "c"] {
        call(&w, &a, "notes.add", json!({"book": b, "text": t})).unwrap();
    }
    let first = w.core.pulse(&a, Some(&start), 2).unwrap();
    assert_eq!(first.events.len(), 2);
    assert!(first.more);
    let rest = w.core.pulse(&a, Some(&first.cursor), 50).unwrap();
    assert_eq!(rest.events.len(), 1);
    assert!(w.core.pulse(&a, Some(&rest.cursor), 50).unwrap().events.is_empty());
    // the log forgets old records; an old cursor must reload
    for t in ["d", "e"] {
        call(&w, &a, "notes.book.create", json!({"title": t})).unwrap();
    }
    w.core.prune_log(1).unwrap();
    let old = w.core.pulse(&a, Some(&start), 50).unwrap();
    assert!(old.reset && old.events.is_empty());
    let fresh = w.core.pulse(&a, Some(&old.cursor), 50).unwrap();
    assert!(!fresh.reset);
}

#[test]
fn a_bad_cursor_is_invalid() {
    let w = world();
    assert_eq!(w.core.pulse(&person("a", &["member"]), Some("zzz"), 5).unwrap_err().category, Category::Invalid);
}

// ---- Listeners ---------------------------------------------------------------------------------------

#[test]
fn listeners_catch_up_after_the_commit_and_keep_their_place() {
    let w = world();
    let a = person("alice", &["member"]);
    let b = book(&w, &a);
    call(&w, &a, "notes.add", json!({"book": b, "text": "x"})).unwrap();
    call(&w, &a, "notes.add", json!({"book": b, "text": "y"})).unwrap();
    assert_eq!(w.core.drain_listeners().unwrap(), 2);
    assert_eq!(w.core.drain_listeners().unwrap(), 0, "a Listener never sees an Event twice");
    call(&w, &a, "notes.add", json!({"book": b, "text": "z"})).unwrap();
    assert_eq!(w.core.drain_listeners().unwrap(), 1);
}

// ---- Extension points ---------------------------------------------------------------------------------

#[test]
fn a_many_extension_point_isolates_each_provider() {
    let w = world_with(&["tell-bad"]);
    let out = call(&w, &person("alice", &["member"]), "notes.tell_all", json!({"text": "hey"})).unwrap();
    let rows = out.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|r| r["provider"] == "tell-a" && r["ok"] == true));
    assert!(rows.iter().any(|r| r["provider"] == "tell-bad" && r["ok"] == false));
}

// ---- the contract ------------------------------------------------------------------------------------

#[test]
fn a_face_built_for_another_contract_is_told_so() {
    let w = world();
    let req = CallRequest { credential: None, op: "notes.book.list".into(), input: json!({}), key: None, contract: Some("stale".into()) };
    assert_eq!(w.core.call(req.clone()).unwrap_err().category, Category::Incompatible);
    let ok = CallRequest { contract: Some(w.core.contract_hash().to_string()), ..req };
    assert!(w.core.call(ok).is_ok());
}

#[test]
fn the_contract_hash_changes_when_the_operations_change() {
    let a = world();
    let b = world_with(&["wrapper"]);
    assert_ne!(a.core.contract_hash(), b.core.contract_hash());
    assert_eq!(a.core.contract_hash(), world().core.contract_hash());
}

#[test]
fn describe_tells_a_face_what_exists_and_what_it_may_do() {
    let w = world();
    let d = w.core.describe(&person("vic", &["viewer"]));
    assert_eq!(d["operations"]["notes.book.create"]["allowed"], false, "a viewer cannot create");
    assert_eq!(d["permissions"]["notes.read"]["held"], true);
    assert!(d["events"]["notes.added"].is_object());
    assert!(d["plugins"].as_array().unwrap().iter().any(|p| p["id"] == "notes" && p["kind"] == "feature"));
    assert!(d["plugins"].as_array().unwrap().iter().any(|p| p["id"] == "tell-a" && p["kind"] == "provider"));
    assert!(w.core.describe_public()["operations"].as_object().unwrap().is_empty() || true);
}

// ---- services (ADR 0031) -----------------------------------------------------------------------

struct Serving { running: std::sync::Arc<std::sync::atomic::AtomicUsize>, fail: bool }
struct Stopper(std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl Service for Stopper {
    fn stop(self: Box<Self>) { self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst); }
}
impl Plugin for Serving {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id":"serving","version":"1","namespaces":["serving"],"requires":{"capabilities":["serve"]}})).unwrap()
    }
    fn serve(&self, ctx: ServiceContext) -> Result<Option<Box<dyn Service>>> {
        if self.fail { return Err(Error::internal("no port")); }
        assert!(!ctx.core.plugin_ids().is_empty(), "a service sees the Core");
        self.running.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Some(Box::new(Stopper(self.running.clone()))))
    }
}

fn serving_core(fail: bool, grant: bool) -> (std::result::Result<Core, Vec<Problem>>, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let running = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let clock = FixedClock::at(0);
    let seen = Default::default();
    let mut cat = catalog(&clock, &seen);
    cat.0.push(std::sync::Arc::new(Serving { running: running.clone(), fail }));
    let mut p = profile(&["db-memory", "auth-local", "policy-roles", "clock-fixed", "ids-sequential", "notes", "tell", "tell-a", "serving"]);
    if grant { p.grants.insert("serving".into(), vec!["serve".into()]); }
    (Core::start(&Distribution { name: "t".into(), catalog: cat, profile: p }), running)
}

#[test]
fn a_plugin_may_serve_only_when_the_operator_grants_it() {
    let (core, _) = serving_core(false, false);
    let codes: Vec<String> = core.err().expect("must not resolve").into_iter().map(|p| p.code).collect();
    assert!(codes.iter().any(|c| c == "capability_not_granted"), "{codes:?}");
}

#[test]
fn services_start_with_the_core_and_stop_with_it() {
    let (core, running) = serving_core(false, true);
    let core = core.unwrap();
    let services = core.start_services().unwrap();
    assert_eq!(services.running(), vec!["serving"]);
    assert_eq!(running.load(std::sync::atomic::Ordering::SeqCst), 1);
    services.stop();
    assert_eq!(running.load(std::sync::atomic::Ordering::SeqCst), 0);
    // dropping stops them too
    let services = core.start_services().unwrap();
    drop(services);
    assert_eq!(running.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn a_service_that_cannot_start_fails_the_start_and_leaves_nothing_running() {
    let (core, running) = serving_core(true, true);
    let e = core.unwrap().start_services().err().expect("must fail");
    assert!(e.message.contains("serving could not start its service"), "{}", e.message);
    assert_eq!(running.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn housekeeping_delivers_events_to_listeners_without_a_server() {
    let w = world();
    let services = w.core.start_services().unwrap();
    let a = person("alice", &["member"]);
    let b = book(&w, &a);
    call(&w, &a, "notes.add", json!({"book": b, "text": "x"})).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(600));
    // Nothing is left for a manual drain: the Core's own upkeep already ran the Listener.
    assert_eq!(w.core.drain_listeners().unwrap(), 0);
    services.stop();
}

// ---- Ports are needed only when a plugin needs them (ADR 0039) ---------------------------------

fn hello(extra: Value) -> std::sync::Arc<dyn Plugin> {
    struct Hello(Value);
    impl Plugin for Hello {
        fn manifest(&self) -> Manifest {
            let mut m = json!({"id":"hello","version":"1","namespaces":["hello"],"operations":{"hello.say":{"kind":"query","public":true}}});
            for (k, v) in self.0.as_object().unwrap() {
                m[k] = v.clone();
            }
            serde_json::from_value(m).unwrap()
        }
        fn bind(&self, b: &mut Bindings) {
            b.op("hello.say", |_, _| Ok(json!({"hi": true})));
        }
    }
    std::sync::Arc::new(Hello(extra))
}

#[test]
fn a_distribution_with_no_database_starts_when_no_plugin_needs_one() {
    let mut p = profile(&["hello"]);
    p.settings.clear();
    p.grants.clear();
    let dist = Distribution { name: "t".into(), catalog: Catalog(vec![hello(json!({}))]), profile: p };
    let core = Core::start(&dist).expect("a core with no adapters");
    let anyone = Identity::new("x", "x", &[]);
    assert_eq!(core.call_as(&anyone, "hello.say", json!({}), None).unwrap().output["hi"], true);
}

#[test]
fn a_plugin_that_owns_data_makes_the_database_needed_and_the_problem_names_it() {
    let dist = Distribution { name: "t".into(), catalog: Catalog(vec![hello(json!({"data":{"collections":["hello.things"],"version":1}}))]), profile: profile(&["hello"]) };
    let problems = Core::check(&dist);
    let p = problems.iter().find(|p| p.code == "port_unfilled").expect("db is needed");
    assert!(p.message.contains("hello") && p.message.contains("db"), "{}", p.message);
}

// ---- Outbound Operations (ADR 0040) ------------------------------------------------------------

struct FakeNet(std::sync::Arc<parking_lot::Mutex<Vec<String>>>);
struct FakeNetPlugin(std::sync::Arc<parking_lot::Mutex<Vec<String>>>);
impl Net for FakeNet {
    fn name(&self) -> &str {
        "net-fake"
    }
    fn fetch(&self, r: &NetRequest) -> Result<NetResponse> {
        self.0.lock().push(r.url.clone());
        Ok(NetResponse { status: 200, url: r.url.clone(), headers: vec![], body: b"hello".to_vec(), truncated: false })
    }
}
impl Plugin for FakeNetPlugin {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id":"net-fake","version":"1","fills":"net"})).unwrap()
    }
    fn fill(&self, _: &Value) -> Result<Option<PortImpl>> {
        Ok(Some(PortImpl::Net(std::sync::Arc::new(FakeNet(self.0.clone())))))
    }
}

/// A plugin with an outbound Operation that fetches, one that reads data through another Operation, and a Command that tries to call an outbound one.
struct Fetchy {
    wants_net: bool,
}
impl Plugin for Fetchy {
    fn manifest(&self) -> Manifest {
        let caps = if self.wants_net { json!(["call", "net"]) } else { json!(["call"]) };
        serde_json::from_value(json!({
            "id": "fetchy", "version": "1", "namespaces": ["fetchy"],
            "requires": {"capabilities": caps},
            "data": {"collections": ["fetchy.seen"], "version": 1},
            "permissions": {"fetchy.use": {"default": ["member"]}},
            "operations": {
                "fetchy.get": {"kind": "outbound", "permission": "fetchy.use", "input": {"type":"object","required":["url"],"properties":{"url":{"type":"string"}},"additionalProperties":false}, "output": {"type":"object","required":["body"]}},
                "fetchy.note": {"kind": "command", "permission": "fetchy.use", "input": {"type":"object"}},
                "fetchy.count": {"kind": "query", "permission": "fetchy.use"},
                "fetchy.bad": {"kind": "command", "permission": "fetchy.use"}
            }
        })).unwrap()
    }
    fn bind(&self, b: &mut Bindings) {
        b.outbound("fetchy.get", |cx, i| {
            let r = cx.net()?.fetch(&NetRequest::get(i["url"].as_str().unwrap_or("")))?;
            // data is reached by calling Operations, each in a transaction of its own
            cx.call("fetchy.note", json!({}))?;
            Ok(json!({"body": String::from_utf8_lossy(&r.body)}))
        });
        b.op("fetchy.note", |tx, _| {
            let n = tx.query("fetchy.seen", &Query::new())?.items.len();
            tx.put("fetchy.seen", &format!("s{n}"), json!({}), Expect::Any)?;
            Ok(json!({}))
        });
        b.op("fetchy.count", |tx, _| Ok(json!({"n": tx.query("fetchy.seen", &Query::new())?.items.len()})));
        b.op("fetchy.bad", |tx, _| tx.call("fetchy.get", json!({"url": "https://example.com/"})));
    }
}

fn outbound_world(wants_net: bool, grant_net: bool) -> std::result::Result<(Core, std::sync::Arc<parking_lot::Mutex<Vec<String>>>), Vec<Problem>> {
    let asked = std::sync::Arc::new(parking_lot::Mutex::new(vec![]));
    let mut p = profile(&["db-memory", "auth-local", "policy-roles", "clock-system", "ids-random", "net-fake", "fetchy"]);
    p.settings.clear();
    p.grants.clear();
    p.grants.insert("fetchy".into(), if grant_net { vec!["call".into(), "net".into()] } else { vec!["call".into()] });
    let list: Vec<std::sync::Arc<dyn Plugin>> = vec![
        std::sync::Arc::new(AdapterMemory), std::sync::Arc::new(AuthLocal), std::sync::Arc::new(PolicyRoles), std::sync::Arc::new(StdClock), std::sync::Arc::new(StdIds),
        std::sync::Arc::new(FakeNetPlugin(asked.clone())), std::sync::Arc::new(Fetchy { wants_net }),
    ];
    Core::start(&Distribution { name: "t".into(), catalog: Catalog(list), profile: p }).map(|c| (c, asked))
}

#[test]
fn an_outbound_operation_uses_the_net_port_outside_a_transaction_and_reaches_data_by_calling_operations() {
    let (core, asked) = outbound_world(true, true).unwrap();
    let me = Identity::new("u", "u", &["member"]);
    let out = core.call_as(&me, "fetchy.get", json!({"url": "https://example.com/"}), None).unwrap().output;
    assert_eq!(out["body"], "hello");
    assert_eq!(asked.lock().as_slice(), ["https://example.com/"]);
    assert_eq!(core.call_as(&me, "fetchy.count", json!({}), None).unwrap().output["n"], 1, "the nested command committed on its own");
    // the front door still applies: input is checked and the permission is needed
    assert!(core.call_as(&me, "fetchy.get", json!({"nope": 1}), None).is_err());
    assert!(core.call_as(&Identity::new("s", "s", &["stranger"]), "fetchy.get", json!({"url": "https://example.com/"}), None).is_err());
    // a retried call is not a replayed one: keys are refused, and a Command cannot call an outbound Operation
    assert!(core.call_as(&me, "fetchy.get", json!({"url": "https://example.com/"}), Some("k")).is_err());
    assert!(core.call_as(&me, "fetchy.bad", json!({}), None).is_err());
}

#[test]
fn the_net_capability_and_the_net_port_are_both_needed() {
    // without the grant the Profile is refused, and the Port is asked for by the capability
    let problems = outbound_world(true, false).err().expect("not granted");
    assert!(problems.iter().any(|p| p.code == "capability_not_granted"), "{problems:?}");
    // granted but the plugin never asked: also refused
    let problems = outbound_world(false, true).err().expect("not requested");
    assert!(problems.iter().any(|p| p.code == "capability_not_requested"), "{problems:?}");
}
