// SPDX-License-Identifier: Apache-2.0
"use strict";
// The generated console. It knows nothing about tasks or projects: it reads /link/describe and builds
// a form for each Operation from its input schema. Anything a plugin adds shows up here on its own.
const $ = (id) => document.getElementById(id);
let D = null, es = null;

function h(tag, props, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (k === "class") e.className = v; else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else if (v != null && v !== false) e.setAttribute(k, v === true ? "" : v);
  }
  for (const kid of kids.flat(Infinity)) if (kid != null) e.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
  return e;
}
const uuid = () => (crypto.randomUUID ? crypto.randomUUID() : String(Date.now()) + Math.random());

async function call(op, input, extra) {
  const command = D && D.operations[op] && D.operations[op].kind === "command";
  const res = await fetch("/link/call/" + op, { method: "POST", credentials: "same-origin",
    headers: { "Content-Type": "application/json", "X-Nuah-Face": "console", ...(D ? { "X-Nuah-Contract": D.contract } : {}), ...(command ? { "Idempotency-Key": uuid() } : {}), ...extra }, body: JSON.stringify(input) });
  const body = await res.json();
  if (!body.ok) throw body.error;
  return body;
}

async function load() {
  D = await (await fetch("/link/describe", { credentials: "same-origin" })).json();
  const signedIn = !!D.who;
  $("signin").hidden = signedIn; $("app").hidden = !signedIn; $("out").hidden = !signedIn;
  $("who").textContent = signedIn ? `${D.who.display} · ${D.profile} · contract ${D.contract}` : "";
  if (!signedIn) { const op = D.operations["auth.sign_in"]; $("err").textContent = op ? "" : "This core has no sign-in."; stop(); return; }
  $("plugins").replaceChildren(...D.plugins.map((p) => h("li", {}, h("strong", {}, p.id), " ", h("span", { class: "muted" }, p.state === "active" ? `${p.kind} ${p.version}` : `inactive: ${p.reason}`))));
  const pts = Object.entries(D.extension_points || {});
  $("points").replaceChildren(...(pts.length ? pts.map(([n, p]) => h("div", {}, h("strong", {}, n), ` [${p.policy}] `, h("span", { class: "muted" }, "filled by: " + (p.providers.join(", ") || "nobody")))) : [h("span", { class: "muted" }, "none")]));
  $("events").textContent = Object.keys(D.events || {}).join(", ");
  renderOps(); connect();
  accounts(); config();
}

const typesOf = (s) => (Array.isArray(s.type) ? s.type : s.type ? [s.type] : []);

function field(name, s, required) {
  const ts = typesOf(s);
  let input, read;
  if (s.enum) { input = h("select", {}, h("option", { value: "" }, "(not set)"), s.enum.map((v) => h("option", { value: JSON.stringify(v) }, String(v)))); read = () => (input.value ? JSON.parse(input.value) : undefined); }
  else if (ts.length === 1 && ts[0] === "boolean") { input = h("select", {}, h("option", { value: "" }, "(not set)"), h("option", { value: "true" }, "true"), h("option", { value: "false" }, "false")); read = () => (input.value ? input.value === "true" : undefined); }
  else if (ts.length === 1 && (ts[0] === "integer" || ts[0] === "number")) { input = h("input", { type: "number", step: ts[0] === "integer" ? "1" : "any" }); read = () => (input.value === "" ? undefined : Number(input.value)); }
  else if (ts.length === 1 && ts[0] === "string") { input = h("input", { type: "text" }); read = () => (input.value === "" ? undefined : input.value); }
  else { input = h("textarea", { rows: 2, placeholder: "JSON" }); read = () => (input.value.trim() === "" ? undefined : JSON.parse(input.value)); }
  if (ts.includes("null") && ts.length > 1) input.placeholder = "text, or null";
  return { el: h("label", { class: required ? "req" : "" }, name, input), read, name };
}

function opCard(name, o) {
  const props = (o.input && o.input.properties) || {}, required = new Set((o.input && o.input.required) || []);
  const fields = Object.entries(props).map(([k, s]) => field(k, s, required.has(k)));
  const out = h("pre", { hidden: true });
  const run = async (btn) => {
    btn.disabled = true; out.hidden = false; out.className = "";
    try {
      const input = {};
      for (const f of fields) { const v = f.read(); if (v !== undefined) input[f.name] = v; }
      const extra = name === "auth.sign_in" ? { "X-Nuah-Session": "cookie" } : {};
      const body = await call(name, input, extra);
      out.textContent = JSON.stringify(body.output, null, 2) + (body.replayed ? "\n(replayed)" : "");
    } catch (e) { out.className = "error"; out.textContent = (e.category || "error") + (e.reason ? " [" + e.reason + "]" : "") + ": " + (e.message || e); }
    btn.disabled = false;
  };
  const btn = h("button", { class: "primary", onclick: () => run(btn) }, o.kind === "query" ? "Run query" : "Run command");
  const reasons = Object.entries(o.reasons || {});
  return h("details", { "data-allowed": String(o.allowed !== false), "data-name": name },
    h("summary", {}, h("strong", {}, name), h("span", { class: "kind" }, o.kind), o.allowed === false ? h("span", { class: "kind" }, "not for you") : null, h("div", { class: "muted" }, o.description || "")),
    fields.map((f) => f.el), reasons.length ? h("div", { class: "muted" }, "can fail with: " + reasons.map(([c]) => c).join(", ")) : null,
    h("div", { style: "margin-top:8px" }, btn), out);
}

function renderOps() {
  const q = $("find").value.trim().toLowerCase();
  const groups = new Map();
  for (const [name, o] of Object.entries(D.operations).sort()) {
    if (q && !name.includes(q) && !(o.description || "").toLowerCase().includes(q)) continue;
    (groups.get(o.plugin) || groups.set(o.plugin, []).get(o.plugin)).push([name, o]);
  }
  $("ops").replaceChildren(...[...groups].map(([plugin, ops]) => h("div", {}, h("h2", {}, plugin), ops.map(([n, o]) => opCard(n, o)))));
}

function connect() {
  stop();
  es = new EventSource("/pulse");
  es.addEventListener("ready", () => ($("live").dataset.s = "live"));
  es.onerror = () => ($("live").dataset.s = "down");
  es.addEventListener("closed", () => load());
  for (const kind of [...Object.keys(D.events || {}), "reset"]) es.addEventListener(kind, (ev) => {
    const line = new Date().toLocaleTimeString() + "  " + kind + "  " + (ev.data.length > 160 ? ev.data.slice(0, 160) + "…" : ev.data) + "\n";
    $("pulse").textContent = (line + $("pulse").textContent).slice(0, 6000);
  });
}
function stop() { if (es) { es.close(); es = null; } }

$("find").addEventListener("input", () => D && D.who && renderOps());
$("out").addEventListener("click", async () => { try { await call("auth.sign_out", {}); } catch {} load(); });
$("form").addEventListener("submit", async (e) => {
  e.preventDefault(); $("err").textContent = "";
  try { await call("auth.sign_in", { name: e.target.name.value, password: e.target.password.value }, { "X-Nuah-Session": "cookie" }); e.target.password.value = ""; await load(); }
  catch (x) { $("err").textContent = x.message || "Sign-in failed"; }
});
load();


// ---- Accounts: shown when the auth plugin offers account management (auth.account.*) ----
const allowed = (op) => D.operations[op] && D.operations[op].allowed !== false;
async function accounts() {
  const box = $("accounts-box");
  box.hidden = !allowed("auth.account.list");
  if (box.hidden) return;
  let list = [];
  try { list = (await call("auth.account.list", {})).output.accounts; } catch (e) { $("accounts").textContent = e.message || "Cannot list accounts"; return; }
  const act = (fn) => async () => { $("acct-err").textContent = ""; try { await fn(); } catch (e) { $("acct-err").textContent = e.message || String(e); } accounts(); };
  $("accounts").replaceChildren(h("table", {}, h("tbody", {}, list.map((a) => h("tr", { class: a.disabled ? "off" : "" },
    h("td", {}, h("strong", {}, a.name), a.display && a.display !== a.name ? " (" + a.display + ")" : ""),
    h("td", {}, (a.roles || []).join(", ")),
    h("td", {}, a.disabled ? "disabled" : ""),
    h("td", {},
      h("button", { onclick: act(async () => { const n = prompt("New name for " + a.name, a.name); if (n && n !== a.name) await call("auth.account.update", { subject: a.subject, name: n }); }) }, "Rename"),
      h("button", { onclick: act(async () => { const r = prompt("Roles for " + a.name + " (comma separated)", (a.roles || []).join(", ")); if (r != null) await call("auth.account.update", { subject: a.subject, roles: r.split(",").map((x) => x.trim()).filter(Boolean) }); }) }, "Roles"),
      h("button", { onclick: act(async () => { const p = prompt("New password for " + a.name + " (8+ characters)"); if (p) await call("auth.account.set_password", { subject: a.subject, password: p }); }) }, "Password"),
      h("button", { onclick: act(async () => { await call("auth.account.update", { subject: a.subject, disabled: !a.disabled }); }) }, a.disabled ? "Enable" : "Disable")))))));
}
$("newacct").addEventListener("submit", async (e) => {
  e.preventDefault(); $("acct-err").textContent = "";
  const f = e.target;
  try {
    await call("auth.account.create", { name: f.name.value, password: f.password.value, roles: f.roles.value.split(",").map((x) => x.trim()).filter(Boolean) });
    f.name.value = ""; f.password.value = "";
  } catch (x) { $("acct-err").textContent = x.message || String(x); }
  accounts();
});

// ---- Configuration: the identity file, when the operator let the console change it (capability host) ----
let C = null;
async function config() {
  const box = $("config-box");
  box.hidden = !allowed("console.profile.get");
  if (box.hidden) return;
  $("config-msg").textContent = ""; $("config-restart").hidden = true;
  try { C = (await call("console.profile.get", {})).output; } catch (e) { $("config").textContent = e.message || "Cannot read the configuration"; return; }
  $("config-path").textContent = C.path + (C.problems.length ? " - problems: " + C.problems.map((p) => p.message).join(" ") : "");
  const on = new Set(C.profile.enabled || []);
  $("config").replaceChildren(...C.plugins.map((p) => {
    const box = h("input", { type: "checkbox", "data-plugin": p.id, ...(on.has(p.id) ? { checked: true } : {}) });
    const area = h("textarea", { rows: 4, "data-settings": p.id, "aria-label": "Settings of " + p.id }, JSON.stringify((C.profile.settings || {})[p.id] ?? {}, null, 2));
    const grants = (C.profile.grants || {})[p.id];
    return h("details", {}, h("summary", {}, box, " ", h("strong", {}, p.id), " ", h("span", { class: "muted" }, p.version + " " + (p.description || ""))),
      grants && grants.length ? h("div", { class: "muted" }, "granted: " + grants.join(", ")) : null,
      p.settings_schema && p.settings_schema.properties ? h("div", { class: "muted" }, "settings: " + Object.keys(p.settings_schema.properties).join(", ")) : null, area);
  }));
}
function draftProfile() {
  const p = structuredClone(C.profile);
  p.enabled = [...document.querySelectorAll("[data-plugin]")].filter((b) => b.checked).map((b) => b.dataset.plugin);
  p.settings = {}; p.grants = p.grants || {};
  for (const a of document.querySelectorAll("[data-settings]")) {
    const id = a.dataset.settings;
    if (!p.enabled.includes(id)) { delete p.grants[id]; continue; }
    const v = JSON.parse(a.value.trim() || "{}");
    if (v !== null && !(typeof v === "object" && !Array.isArray(v) && Object.keys(v).length === 0)) p.settings[id] = v;
    const need = (C.plugins.find((x) => x.id === id).requires.capabilities) || [];
    if (need.length) p.grants[id] = [...new Set([...(p.grants[id] || []), ...need])];
  }
  return p;
}
$("config-save").addEventListener("click", async () => {
  const msg = $("config-msg"); msg.className = ""; msg.textContent = "";
  try {
    await call("console.profile.save", { profile: draftProfile() });
    msg.textContent = "Saved. The change takes effect when the instance restarts."; $("config-restart").hidden = false;
  } catch (e) { msg.className = "err"; msg.textContent = e.message || String(e); }
});
$("config-restart").addEventListener("click", async () => {
  const msg = $("config-msg");
  try { await call("console.restart", {}); } catch (e) { msg.className = "err"; msg.textContent = e.message || String(e); return; }
  msg.textContent = "Restarting...";
  for (let i = 0; i < 60; i++) {
    await new Promise((r) => setTimeout(r, 500));
    try { if ((await fetch("/healthz", { cache: "no-store" })).ok && i > 1) { location.reload(); return; } } catch {}
  }
  msg.className = "err"; msg.textContent = "The instance did not come back. Check its log; the previous file is kept as a .bak next to it.";
});
