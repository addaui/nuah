// SPDX-License-Identifier: Apache-2.0
//! The HTTP and SSE Projection of the Link ([Link spec]), as a plugin (`http`, ADR 0031). It knows nothing
//! about any product: it exposes whatever contract the Core describes. A product that needs more routes
//! hands them to [`Http::with_routes`]; it never edits this crate. Sessions come from the `auth.*` Operations ([ADR 0010]);
//! a browser Face holds its session in an HttpOnly cookie, any other Face sends a bearer token.
//!
//!   GET  /healthz                   liveness, nothing else
//!   GET  /link/describe             public before sign-in, full after
//!   POST /link/call/{op}            run an Operation; the JSON body is the input
//!   GET  /pulse                     server-sent events; resume with ?after=<cursor> or Last-Event-ID
//!   GET  /link/openapi.json | tools.json | client.ts   generated from the contract (sign-in required)
//!
//! [Link spec]: https://docs.addaui.com/nuah/specs/link/
//! [ADR 0010]: https://docs.addaui.com/nuah/adr/0010-sign-in-is-an-operation-sessions-expire/

use axum::body::Bytes;
use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as Sse, KeepAlive};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{extract::Path, Json, Router};
use nuah_kernel::{CallRequest, Category, Core, Error};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod plugin;
pub use plugin::Http;

pub const COOKIE: &str = "nuah_session";

#[derive(Clone, Debug)]
pub struct Config {
    /// Host names the server answers to; anything else gets 421 (DNS rebinding).
    pub hosts: Vec<String>,
    /// Add `Secure` to the session cookie (set when a TLS proxy fronts the server).
    pub secure_cookie: bool,
    /// Static Faces to host: (URL prefix such as `/faces/web`, directory).
    pub faces: Vec<(String, std::path::PathBuf)>,
    /// Where the Faces of plugins are: `<faces_dir>/<plugin id>`, for each enabled plugin that publishes one and has no entry in `faces`.
    pub faces_dir: std::path::PathBuf,
    /// The Face `/` shows, by name (`web` for `/faces/web`), without redirecting. Default: `web` when it is hosted, else the first.
    pub root: Option<String>,
    /// Failed sign-ins allowed per account name within `throttle_window` (ADR 0015).
    pub sign_in_failures_per_name: usize,
    /// Failed sign-ins allowed overall within `throttle_window`.
    pub sign_in_failures_overall: usize,
    pub throttle_window: Duration,
    /// Open Pulse streams one person may hold; opening another closes their oldest ([ADR 0017]).
    ///
    /// [ADR 0017]: https://docs.addaui.com/nuah/adr/0017-each-person-has-a-limited-number-of-pulse-streams-the-newest-wins/
    pub pulse_streams_per_person: usize,
    /// Open Pulse streams the whole server serves; past it a new stream is refused with 429.
    pub pulse_streams_total: usize,
    /// Where to listen (`host:port`); port 0 picks a free one.
    pub listen: String,
    /// Tokio worker threads; 0 means one per core.
    pub worker_threads: usize,
}

impl Config {
    /// Read the plugin's Settings (checked against its schema by the Resolver) on top of the defaults.
    pub fn from_settings(v: &Value) -> Result<Config, String> {
        let mut c = Config::default();
        if let Some(l) = v["listen"].as_str() {
            c.listen = l.into();
        }
        c.hosts.extend(v["hosts"].as_array().into_iter().flatten().filter_map(|h| h.as_str()).map(|h| h.to_lowercase()));
        c.secure_cookie = v["secure_cookie"].as_bool().unwrap_or(false);
        if let Some(d) = v["faces_dir"].as_str() {
            c.faces_dir = d.into();
        }
        c.root = v["root"].as_str().map(|r| r.trim_matches('/').to_string()).filter(|r| !r.is_empty());
        for (prefix, dir) in v["faces"].as_object().into_iter().flatten() {
            if !prefix.starts_with('/') {
                return Err(format!("the face prefix {prefix} must start with /"));
            }
            c.faces.push((prefix.clone(), dir.as_str().unwrap_or("").into()));
        }
        let n = |k: &str, d: usize| v[k].as_u64().map(|n| n as usize).unwrap_or(d);
        c.sign_in_failures_per_name = n("sign_in_failures_per_name", c.sign_in_failures_per_name);
        c.sign_in_failures_overall = n("sign_in_failures_overall", c.sign_in_failures_overall);
        c.throttle_window = Duration::from_secs(v["throttle_window_secs"].as_u64().unwrap_or(c.throttle_window.as_secs()));
        c.pulse_streams_per_person = n("pulse_streams_per_person", c.pulse_streams_per_person);
        c.pulse_streams_total = n("pulse_streams_total", c.pulse_streams_total);
        c.worker_threads = n("worker_threads", 0);
        Ok(c)
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hosts: ["localhost", "127.0.0.1", "[::1]"].iter().map(|s| s.to_string()).collect(),
            secure_cookie: false,
            faces: vec![],
            faces_dir: "faces".into(),
            root: None,
            sign_in_failures_per_name: 5,
            sign_in_failures_overall: 100,
            throttle_window: Duration::from_secs(300),
            pulse_streams_per_person: 8,
            pulse_streams_total: 1000,
            listen: "127.0.0.1:3000".into(),
            worker_threads: 0,
        }
    }
}

#[derive(Clone)]
struct App {
    core: Core,
    cfg: Arc<Config>,
    throttle: Arc<Mutex<Throttle>>,
    streams: Arc<Mutex<Streams>>,
    shutdown: tokio::sync::watch::Receiver<bool>,
}

/// The open Pulse streams: per person, oldest first ([ADR 0017]).
///
/// [ADR 0017]: https://docs.addaui.com/nuah/adr/0017-each-person-has-a-limited-number-of-pulse-streams-the-newest-wins/
#[derive(Default)]
struct Streams {
    by_person: HashMap<String, std::collections::VecDeque<(u64, Arc<tokio::sync::Notify>)>>,
    total: usize,
    next: u64,
}

/// Held by a stream while it is open; dropping it frees the place.
struct StreamSlot {
    streams: Arc<Mutex<Streams>>,
    person: String,
    id: u64,
    /// Fires when a newer stream of the same person takes this one's place.
    replaced: Arc<tokio::sync::Notify>,
}

impl Streams {
    fn open(this: &Arc<Mutex<Streams>>, person: &str, cfg: &Config) -> Option<StreamSlot> {
        let mut s = this.lock().unwrap_or_else(|e| e.into_inner());
        if s.total >= cfg.pulse_streams_total {
            return None;
        }
        s.next += 1;
        let id = s.next;
        let replaced = Arc::new(tokio::sync::Notify::new());
        let mut evicted = vec![];
        {
            let list = s.by_person.entry(person.to_string()).or_default();
            while list.len() >= cfg.pulse_streams_per_person.max(1) {
                if let Some((_, n)) = list.pop_front() {
                    evicted.push(n);
                }
            }
            list.push_back((id, replaced.clone()));
        }
        s.total = s.total + 1 - evicted.len();
        for n in evicted {
            n.notify_one();
        }
        Some(StreamSlot { streams: this.clone(), person: person.to_string(), id, replaced })
    }
}

impl Drop for StreamSlot {
    fn drop(&mut self) {
        let mut s = self.streams.lock().unwrap_or_else(|e| e.into_inner());
        let Some(list) = s.by_person.get_mut(&self.person) else { return };
        let before = list.len();
        list.retain(|(i, _)| *i != self.id);
        let removed = before - list.len();
        if list.is_empty() {
            s.by_person.remove(&self.person);
        }
        // An evicted stream was already taken out of the count.
        s.total -= removed;
    }
}

/// Failed sign-in times, per account name and overall ([ADR 0015]). In memory; a restart clears it.
///
/// [ADR 0015]: https://docs.addaui.com/nuah/adr/0015-sign-in-attempts-are-throttled-and-local-sign-in-stays-on-loopback/
#[derive(Default)]
struct Throttle {
    names: HashMap<String, Vec<Instant>>,
    overall: Vec<Instant>,
}

const MAX_TRACKED_NAMES: usize = 10_000;

impl Throttle {
    fn key(name: &str) -> String {
        name.trim().to_lowercase().chars().take(200).collect()
    }

    fn recent(list: &mut Vec<Instant>, now: Instant, window: Duration) {
        list.retain(|t| now.duration_since(*t) < window);
    }

    /// How long until this name may try again; `None` if it may try now.
    fn blocked(&mut self, name: &str, cfg: &Config) -> Option<Duration> {
        let now = Instant::now();
        Self::recent(&mut self.overall, now, cfg.throttle_window);
        let wait = |list: &Vec<Instant>, limit: usize| -> Option<Duration> {
            (list.len() >= limit).then(|| list.first().map(|t| cfg.throttle_window.saturating_sub(now.duration_since(*t))).unwrap_or_default())
        };
        let by_name = match self.names.get_mut(&Self::key(name)) {
            Some(list) => {
                Self::recent(list, now, cfg.throttle_window);
                wait(list, cfg.sign_in_failures_per_name)
            }
            None => None,
        };
        by_name.into_iter().chain(wait(&self.overall, cfg.sign_in_failures_overall)).max()
    }

    fn failed(&mut self, name: &str, cfg: &Config) {
        let now = Instant::now();
        self.overall.push(now);
        self.names.retain(|_, v| {
            Self::recent(v, now, cfg.throttle_window);
            !v.is_empty()
        });
        if self.names.len() < MAX_TRACKED_NAMES {
            self.names.entry(Self::key(name)).or_default().push(now);
        }
    }

    fn succeeded(&mut self, name: &str) {
        self.names.remove(&Self::key(name));
    }
}

/// An Error on its way to becoming a response: internals stay in the log, the caller gets a reference.
pub struct ApiError(pub Error);

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut err = self.0;
        let status = StatusCode::from_u16(err.category.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        // Internals (SQL messages, panics) stay in the server log; the caller gets a reference to quote.
        if err.category == Category::Internal {
            // The Core already logged this error under its request id; only a failure of the server itself is logged here.
            let reference = match err.params.get("ref").and_then(|r| r.as_str()) {
                Some(r) => r.to_string(),
                None => {
                    let r = format!("{:08x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0));
                    eprintln!("[nuah-http] error ref {r}: {}", err.message);
                    r
                }
            };
            err = Error::internal(format!("Something went wrong on the server (ref {reference})"));
        }
        (status, [(header::CACHE_CONTROL, "no-store")], Json(json!({"ok": false, "error": err}))).into_response()
    }
}

pub fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(String::from)
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    header_str(headers, "authorization").and_then(|v| v.strip_prefix("Bearer ").map(|t| t.trim().to_string()))
}

fn cookie(headers: &HeaderMap) -> Option<String> {
    let all = header_str(headers, "cookie")?;
    all.split(';').filter_map(|p| p.trim().split_once('=')).find(|(k, _)| *k == COOKIE).map(|(_, v)| v.to_string())
}

/// A bearer token wins over the cookie, so a script on a machine with a browser session is not confused.
pub fn credential(headers: &HeaderMap) -> Option<String> {
    bearer(headers).or_else(|| cookie(headers))
}

/// Storage calls never run on the async worker threads.
pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| ApiError(Error::internal(format!("task failed: {e}"))))
}

pub fn no_store<T: IntoResponse>(t: T) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], t).into_response()
}

async fn describe(State(app): State<App>, headers: HeaderMap) -> Result<Response, ApiError> {
    let (core, cred) = (app.core.clone(), credential(&headers));
    let body = blocking(move || match core.authenticate(cred.as_deref()) {
        Ok(who) => core.describe(&who),
        Err(_) => core.describe_public(),
    })
    .await?;
    Ok(no_store(Json(body)))
}

/// The contract documents describe what the caller may do, so they need a signed-in caller.
async fn generated(app: App, headers: HeaderMap, make: fn(&Value) -> Response) -> Result<Response, ApiError> {
    let (core, cred) = (app.core.clone(), credential(&headers));
    let d = blocking(move || core.authenticate(cred.as_deref()).map(|who| core.describe(&who))).await??;
    Ok(make(&d))
}
async fn openapi_doc(State(app): State<App>, headers: HeaderMap) -> Result<Response, ApiError> {
    generated(app, headers, |d| no_store(Json(nuah_kernel::contract::openapi(d)))).await
}
async fn tools_doc(State(app): State<App>, headers: HeaderMap) -> Result<Response, ApiError> {
    generated(app, headers, |d| no_store(Json(nuah_kernel::contract::tool_definitions(d)))).await
}
async fn client_ts(State(app): State<App>, headers: HeaderMap) -> Result<Response, ApiError> {
    generated(app, headers, |d| {
        ([(header::CONTENT_TYPE, "text/typescript; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], nuah_kernel::contract::typescript(d)).into_response()
    })
    .await
}

fn session_cookie(app: &App, token: &str, expires_at: i64) -> HeaderValue {
    let secs = ((expires_at - app.core.now()) / 1000).max(0);
    let secure = if app.cfg.secure_cookie { "; Secure" } else { "" };
    HeaderValue::from_str(&format!("{COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={secs}{secure}")).unwrap()
}

fn clearing_cookie(app: &App) -> HeaderValue {
    let secure = if app.cfg.secure_cookie { "; Secure" } else { "" };
    HeaderValue::from_str(&format!("{COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0{secure}")).unwrap()
}

async fn call(State(app): State<App>, Path(op): Path<String>, headers: HeaderMap, body: Bytes) -> Result<Response, ApiError> {
    let mut input: Value = if body.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&body).map_err(|e| Error::invalid("body", &format!("is not valid JSON: {e}")))?
    };
    let cookie_mode = header_str(&headers, "x-nuah-session").as_deref() == Some("cookie");
    let held = cookie(&headers);
    // A browser Face never sees its session token, so it cannot name it: the server fills it in.
    if matches!(op.as_str(), "auth.refresh" | "auth.sign_out") {
        if let (Some(obj), Some(t)) = (input.as_object_mut(), held.as_ref()) {
            obj.entry("token").or_insert_with(|| json!(t));
        }
    }
    // Failed sign-ins are counted; past the limit the attempt is refused before any password is checked.
    let attempt = (op == "auth.sign_in").then(|| input["name"].as_str().unwrap_or_default().to_string());
    if let Some(name) = &attempt {
        let wait = app.throttle.lock().unwrap_or_else(|e| e.into_inner()).blocked(name, &app.cfg);
        if let Some(wait) = wait {
            let secs = wait.as_secs() + 1;
            let mut r = refuse(StatusCode::TOO_MANY_REQUESTS, "denied", "auth.too_many_attempts", &format!("Too many failed sign-ins. Try again in {secs} seconds."));
            r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(secs));
            return Ok(r);
        }
    }
    let req = CallRequest { credential: credential(&headers), op: op.clone(), input, key: header_str(&headers, "idempotency-key"), contract: header_str(&headers, "x-nuah-contract") };
    let core = app.core.clone();
    let result = blocking(move || core.call(req)).await?;
    if let Some(name) = &attempt {
        let mut t = app.throttle.lock().unwrap_or_else(|e| e.into_inner());
        match &result {
            Ok(_) => t.succeeded(name),
            Err(e) if e.reason.as_deref() == Some("auth.bad_credentials") => t.failed(name, &app.cfg),
            Err(_) => {}
        }
    }
    let mut res = result?;
    let mut cookie_header = None;
    match op.as_str() {
        "auth.sign_in" if cookie_mode => {
            if let (Some(t), Some(exp)) = (res.output["token"].as_str().map(String::from), res.output["expires_at"].as_i64()) {
                cookie_header = Some(session_cookie(&app, &t, exp));
                res.output.as_object_mut().map(|o| o.remove("token"));
            }
        }
        "auth.refresh" if held.is_some() && bearer(&headers).is_none() => {
            if let (Some(t), Some(exp)) = (held, res.output["expires_at"].as_i64()) {
                cookie_header = Some(session_cookie(&app, &t, exp));
            }
        }
        "auth.sign_out" if held.is_some() => cookie_header = Some(clearing_cookie(&app)),
        _ => {}
    }
    let mut out = no_store(Json(json!({"ok": true, "output": res.output, "replayed": res.replayed})));
    if let Some(c) = cookie_header {
        out.headers_mut().append(header::SET_COOKIE, c);
    }
    Ok(out)
}

const BATCH: usize = 200;

async fn pulse(
    State(app): State<App>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let cred = credential(&headers);
    let core = app.core.clone();
    let c = cred.clone();
    let who = blocking({
        let core = core.clone();
        move || core.authenticate(c.as_deref())
    })
    .await??;
    // EventSource resends the last id it saw as Last-Event-ID when it reconnects but keeps the
    // original URL, so the header can be newer than ?after. Cursors are opaque and only ever grow
    // with the log, so the one that was delivered last is the header.
    let after = header_str(&headers, "last-event-id").or_else(|| q.get("after").cloned());
    let start = match after {
        Some(a) => a,
        None => blocking({
            let core = core.clone();
            move || core.pulse_now()
        })
        .await??,
    };
    let Some(slot) = Streams::open(&app.streams, &who.subject, &app.cfg) else {
        let mut r = refuse(StatusCode::TOO_MANY_REQUESTS, "denied", "pulse.too_many_streams", "The server has as many live streams as it allows. Try again shortly.");
        r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(5u32));
        return Ok(r);
    };
    let mut wake = core.subscribe();
    let mut shutdown = app.shutdown.clone();
    let who0 = who.subject.clone();
    let stream = async_stream::stream! {
        let slot = slot;
        let mut cursor = start.clone();
        yield Ok::<_, Infallible>(Sse::default().event("ready").data(json!({"cursor": cursor, "subject": who0}).to_string()));
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            loop {
                let (c, cr, from) = (core.clone(), cred.clone(), cursor.clone());
                // The session is checked again on every round: a stream ends when its session does.
                let r = tokio::task::spawn_blocking(move || {
                    let who = c.authenticate(cr.as_deref())?;
                    c.pulse(&who, Some(&from), BATCH)
                })
                .await;
                let batch = match r {
                    Ok(Ok(b)) => b,
                    Ok(Err(e)) => {
                        yield Ok(Sse::default().event("closed").data(json!({"category": e.category, "key": e.key}).to_string()));
                        return;
                    }
                    Err(_) => return,
                };
                if batch.reset {
                    cursor = batch.cursor.clone();
                    yield Ok(Sse::default().event("reset").data(json!({"cursor": cursor}).to_string()));
                    break;
                }
                for e in &batch.events {
                    yield Ok(Sse::default().id(e.cursor.clone()).event(e.kind.clone()).data(serde_json::to_string(e).unwrap_or_default()));
                }
                cursor = batch.cursor;
                if !batch.more { break; }
            }
            // A commit wakes the stream at once; the tick covers another process writing the same database.
            tokio::select! {
                r = wake.changed() => if r.is_err() { return },
                _ = tick.tick() => {}
                _ = async {
                    // The server is stopping. With no shutdown signal wired (a sender that is gone) this never fires.
                    if shutdown.wait_for(|down| *down).await.is_err() { std::future::pending::<()>().await }
                } => { return; }
                _ = slot.replaced.notified() => {
                    yield Ok(Sse::default().event("closed").data(json!({"category": "denied", "key": "pulse.replaced"}).to_string()));
                    return;
                }
            }
        }
    };
    Ok(axum::response::sse::Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))).into_response())
}

/// Liveness only: it says nothing about the data.
async fn health() -> Json<Value> {
    Json(json!({"ok": true}))
}

/// Readiness: whether the Core can read its data. 200 when it can, 503 when it cannot.
async fn ready(State(core): State<Core>) -> Response {
    let h = tokio::task::spawn_blocking(move || core.health()).await;
    match h {
        Ok(h) => (if h.ready { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE }, [(header::CACHE_CONTROL, "no-store")], Json(nuah_kernel::telemetry::health_json(&h))).into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

fn host_only(h: &str) -> String {
    match h.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => host.to_lowercase(),
        _ => h.to_lowercase(),
    }
}

pub fn refuse(status: StatusCode, category: &str, key: &str, message: &str) -> Response {
    (status, Json(json!({"ok": false, "error": {"category": category, "key": key, "message": message}}))).into_response()
}

/// Browser-facing protections: Host allow-list (DNS rebinding), Origin must match Host (cross-site
/// writes), commands must be JSON with the Face header (a plain cross-site form cannot send both),
/// and security headers on every response.
pub async fn guard(State(cfg): State<Arc<Config>>, req: Request, next: Next) -> Response {
    let host_header = header_str(req.headers(), "host").unwrap_or_default();
    if !cfg.hosts.iter().any(|h| *h == host_only(&host_header)) {
        return refuse(StatusCode::MISDIRECTED_REQUEST, "invalid", "server.bad_host", "This host name is not allowed. Start the server with --allow-host to add it.");
    }
    if let Some(origin) = header_str(req.headers(), "origin") {
        if origin.split("://").nth(1).unwrap_or(&origin) != host_header {
            return refuse(StatusCode::FORBIDDEN, "denied", "server.bad_origin", "Cross-origin requests are not allowed.");
        }
    }
    if req.method() == Method::POST && (req.uri().path().starts_with("/link/call/") || req.uri().path().starts_with("/setup/")) {
        if !header_str(req.headers(), "content-type").is_some_and(|c| c.starts_with("application/json")) {
            return refuse(StatusCode::UNSUPPORTED_MEDIA_TYPE, "invalid", "server.not_json", "Send the input as application/json.");
        }
        if req.headers().get("x-nuah-face").is_none() {
            return refuse(StatusCode::BAD_REQUEST, "invalid", "server.no_face", "The X-Nuah-Face header is required.");
        }
    }
    let is_face = req.uri().path().starts_with("/faces/");
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    if is_face {
        // Faces are small and change with every release: always ask the server whether they changed.
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    }
    if !h.contains_key(header::CONTENT_SECURITY_POLICY) { h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; img-src 'self' data: https:; frame-src https://www.youtube-nocookie.com; frame-ancestors 'none'; base-uri 'none'; form-action 'self'")); }
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    res
}

/// Routes a product adds to the Projection. Called once with the Core, it returns its own router.
pub type RouteFn = Arc<dyn Fn(Core) -> Router + Send + Sync>;

/// The directory `/` serves: the named root Face, else `web`, else the first one hosted.
pub fn root_face(cfg: &Config) -> Option<std::path::PathBuf> {
    let by = |name: &str| cfg.faces.iter().find(|(p, _)| p.trim_start_matches("/faces/") == name).map(|(_, d)| d.clone());
    cfg.root.as_deref().and_then(by).or_else(|| by("web")).or_else(|| cfg.faces.first().map(|(_, d)| d.clone()))
}

pub fn router(core: Core, cfg: Config) -> Router {
    router_with(core, cfg, &[], tokio::sync::watch::channel(false).1)
}

/// The Link, the static Faces and a product's extra routes, behind the guard. `shutdown` ends Pulse streams
/// when it turns true so the server can stop.
pub fn router_with(core: Core, mut cfg: Config, extra: &[RouteFn], shutdown: tokio::sync::watch::Receiver<bool>) -> Router {
    // A plugin that publishes a Face is hosted from `<faces_dir>/<plugin id>` unless the Profile gave it a directory.
    for (name, plugin) in core.faces() {
        let prefix = format!("/faces/{name}");
        if !cfg.faces.iter().any(|(p, _)| *p == prefix) {
            cfg.faces.push((prefix, cfg.faces_dir.join(plugin)));
        }
    }
    let cfg = Arc::new(cfg);
    let mut r = Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(ready).with_state(core.clone()))
        .route("/link/describe", get(describe))
        .route("/link/call/{op}", post(call).layer(axum::extract::DefaultBodyLimit::max(7 * 1024 * 1024)))
        .route("/link/openapi.json", get(openapi_doc))
        .route("/link/tools.json", get(tools_doc))
        .route("/link/client.ts", get(client_ts))
        .route("/pulse", get(pulse))
        .with_state(App { core: core.clone(), cfg: cfg.clone(), throttle: Default::default(), streams: Default::default(), shutdown });
    for f in extra {
        r = r.merge(f(core.clone()));
    }
    for (prefix, dir) in &cfg.faces {
        r = r.nest_service(prefix, tower_http::services::ServeDir::new(dir));
    }
    // `/` shows the root Face where it is (no redirect), so a bare address opens the product.
    if let Some(dir) = root_face(&cfg) {
        r = r.fallback_service(tower_http::services::ServeDir::new(dir));
    }
    r.layer(middleware::from_fn_with_state(cfg, guard))
}
