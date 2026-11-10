// SPDX-License-Identifier: Apache-2.0
"use strict";
// The generated console. It knows nothing about any product: it reads /link/describe and builds everything
// from it. Plugins, their settings forms (from each plugin's settings schema), accounts, and a form for each
// Operation all appear on their own.
const $ = (id) => document.getElementById(id);
let D = null, es = null;

function h(tag, props, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (k === "class") e.className = v; else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else if (v != null && v !== false) e.setAttribute(k, v === true ? "" : v);
  }
  for (const kid of kids.flat(Infinity)) if (kid != null && kid !== false) e.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
  return e;
}
const uuid = () => (crypto.randomUUID ? crypto.randomUUID() : String(Date.now()) + Math.random());
const allowed = (op) => D.operations[op] && D.operations[op].allowed !== false;
const scopeOf = (id) => (id.startsWith("@") ? id.slice(1, id.indexOf("/")) : "other");
const nameOf = (id) => (id.startsWith("@") ? id.slice(id.indexOf("/") + 1) : id);

async function call(op, input, extra) {
  const command = D && D.operations[op] && D.operations[op].kind === "command";
  const res = await fetch("/link/call/" + op, { method: "POST", credentials: "same-origin",
    headers: { "Content-Type": "application/json", "X-Nuah-Face": "console", ...(D ? { "X-Nuah-Contract": D.contract } : {}), ...(command ? { "Idempotency-Key": uuid() } : {}), ...extra }, body: JSON.stringify(input) });
  const body = await res.json();
  if (!body.ok) throw body.error;
  return body;
}
const say = (e) => (e && (e.message || e.reason)) || String(e);

// ---- a small dialog: ask(title, fields) resolves with the values, or null when cancelled ----
function ask(title, fields, okLabel) {
  return new Promise((resolve) => {
    const dlg = $("dlg"); $("dlg-title").textContent = title; $("dlg-ok").textContent = okLabel || "OK";
    const inputs = fields.map((f) => ({ f, input: h("input", { id: "d-" + f.name, type: f.type || "text", value: f.value || "", autocomplete: f.type === "password" ? "new-password" : "off", placeholder: f.placeholder || "" }) }));
    $("dlg-body").replaceChildren(...inputs.map(({ f, input }) => h("div", { class: "field" }, h("label", { for: input.id }, f.label), input, f.help ? h("div", { class: "help" }, f.help) : null)));
    const done = (v) => { dlg.close(); resolve(v); };
    $("dlg-form").onsubmit = (e) => { e.preventDefault(); done(Object.fromEntries(inputs.map(({ f, input }) => [f.name, input.value]))); };
    $("dlg-cancel").onclick = () => done(null);
    dlg.oncancel = () => resolve(null);
    dlg.showModal(); (inputs[0] && inputs[0].input).focus();
  });
}
function review(title, node, okLabel) {
  return new Promise((resolve) => {
    const dlg = $("dlg"); $("dlg-title").textContent = title; $("dlg-ok").textContent = okLabel; $("dlg-body").replaceChildren(node);
    const done = (v) => { dlg.close(); resolve(v); };
    $("dlg-form").onsubmit = (e) => { e.preventDefault(); done(true); };
    $("dlg-cancel").onclick = () => done(false);
    dlg.oncancel = () => resolve(false);
    dlg.showModal();
  });
}

// ---- tabs ----
let TAB = null;
function tabs() {
  const list = [["overview", "Overview", true], ["config", "Plugins and settings", allowed("console.profile.get")], ["accounts", "Accounts", allowed("auth.account.list")], ["ops", "Operations", true], ["live", "Live", true]].filter((t) => t[2]);
  $("tabs").replaceChildren(...list.map(([id, label]) => h("button", { role: "tab", "aria-selected": String(TAB === id), "data-tab": id, onclick: () => go(id) }, label)));
  for (const [id] of [["overview"], ["config"], ["accounts"], ["ops"], ["live"]]) $("tab-" + id).hidden = id !== TAB;
}
function go(id) {
  if (TAB === "config" && id !== "config" && dirty()) { if (!confirm("You have unsaved changes on Plugins and settings. Leave anyway?")) return; }
  TAB = id; try { history.replaceState(null, "", "#" + id); } catch {}
  tabs();
  if (id === "config") config(); if (id === "accounts") accounts();
}

async function load() {
  D = await (await fetch("/link/describe", { credentials: "same-origin" })).json();
  const signedIn = !!D.who;
  $("signin").hidden = signedIn; $("app").hidden = !signedIn; $("out").hidden = !signedIn;
  $("who").textContent = signedIn ? `${D.who.display} · ${D.profile}` : "";
  if (!signedIn) { $("err").textContent = D.operations["auth.sign_in"] ? "" : "This core has no sign-in."; stop(); return; }
  overview(); renderOps(); connect();
  const want = location.hash.slice(1);
  TAB = ["overview", "config", "accounts", "ops", "live"].includes(want) ? want : "overview";
  tabs();
  if (TAB === "config") config(); if (TAB === "accounts") accounts();
  notice();
}

// ---- overview ----
function overview() {
  const active = D.plugins.filter((p) => p.state === "active").length;
  $("summary").replaceChildren(
    ...[[D.profile, "instance"], [active + " of " + D.plugins.length, "plugins running"], [Object.keys(D.operations).length, "operations"], [D.contract, "contract"]].map(([v, l]) => h("div", { class: "stat" }, h("b", {}, v), h("span", {}, l))));
  const groups = new Map();
  for (const p of D.plugins) { const s = p.scope || scopeOf(p.id); (groups.get(s) || groups.set(s, []).get(s)).push(p); }
  $("plugins").replaceChildren(...[...groups].map(([scope, list]) => h("div", { class: "scope" }, h("h2", {}, "@" + scope, " ", h("span", { class: "chip" }, list.length)),
    list.map((p) => h("div", { class: "plugin" + (p.state === "active" ? " on" : "") }, h("div", { class: "head" }, h("span", { class: "name" }, nameOf(p.id)), p.state === "active" ? h("span", { class: "chip" }, p.kind + " " + p.version) : h("span", { class: "chip warn" }, "inactive: " + p.reason)),
      p.description ? h("div", { class: "desc" }, p.description) : null)))));
  const pts = Object.entries(D.extension_points || {});
  $("points").replaceChildren(...(pts.length ? pts.map(([n, p]) => h("div", {}, h("strong", {}, n), ` [${p.policy}] `, h("span", { class: "muted" }, "filled by: " + (p.providers.join(", ") || "nobody")))) : [h("span", { class: "muted" }, "none")]));
  $("events").textContent = Object.keys(D.events || {}).join(", ") || "none";
}
async function notice() {
  if (!allowed("console.profile.get")) return;
  try { const n = (await call("console.profile.get", {})).output.notice; $("notice").hidden = !n; $("notice").textContent = n || ""; } catch {}
}

// ---- operations explorer ----
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
      const body = await call(name, input, name === "auth.sign_in" ? { "X-Nuah-Session": "cookie" } : {});
      out.textContent = JSON.stringify(body.output, null, 2) + (body.replayed ? "\n(replayed)" : "");
    } catch (e) { out.className = "error"; out.textContent = (e.category || "error") + (e.reason ? " [" + e.reason + "]" : "") + ": " + (e.message || e); }
    btn.disabled = false;
  };
  const btn = h("button", { class: "primary", onclick: () => run(btn) }, o.kind === "query" ? "Run query" : "Run command");
  const reasons = Object.entries(o.reasons || {});
  return h("details", { class: "op", "data-allowed": String(o.allowed !== false), "data-name": name },
    h("summary", {}, h("strong", {}, name), h("span", { class: "kind" }, o.kind), o.allowed === false ? h("span", { class: "kind" }, "not for you") : null, h("div", { class: "muted" }, o.description || "")),
    fields.map((f) => f.el), reasons.length ? h("div", { class: "muted" }, "can fail with: " + reasons.map(([c]) => c).join(", ")) : null,
    h("div", { class: "opbar" }, btn), out);
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

// ---- live ----
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

// ---- accounts: shown when the auth plugin offers account management (auth.account.*) ----
async function accounts() {
  if (!allowed("auth.account.list")) return;
  let list = [];
  try { list = (await call("auth.account.list", {})).output.accounts; } catch (e) { $("accounts").textContent = say(e); return; }
  const act = (fn) => async () => { $("acct-err").textContent = ""; try { await fn(); } catch (e) { $("acct-err").textContent = say(e); } accounts(); };
  $("accounts").replaceChildren(h("table", {}, h("tbody", {}, list.map((a) => h("tr", { class: a.disabled ? "off" : "" },
    h("td", {}, h("strong", {}, a.name), a.display && a.display !== a.name ? " (" + a.display + ")" : ""),
    h("td", {}, (a.roles || []).map((r) => h("span", { class: "chip" }, r)), " "),
    h("td", {}, a.disabled ? h("span", { class: "chip warn" }, "disabled") : ""),
    h("td", { class: "actions" },
      h("button", { onclick: act(async () => { const v = await ask("Rename " + a.name, [{ name: "name", label: "New name", value: a.name }], "Rename"); if (v && v.name && v.name !== a.name) await call("auth.account.update", { subject: a.subject, name: v.name }); }) }, "Rename"), " ",
      h("button", { onclick: act(async () => { const v = await ask("Roles of " + a.name, [{ name: "roles", label: "Roles, separated by commas", value: (a.roles || []).join(", "), help: "admin may manage accounts and the configuration; member is an ordinary person." }], "Save"); if (v) await call("auth.account.update", { subject: a.subject, roles: v.roles.split(",").map((x) => x.trim()).filter(Boolean) }); }) }, "Roles"), " ",
      h("button", { onclick: act(async () => { const v = await ask("New password for " + a.name, [{ name: "password", label: "Password", type: "password", help: "At least 8 characters." }], "Set password"); if (v && v.password) await call("auth.account.set_password", { subject: a.subject, password: v.password }); }) }, "Password"), " ",
      h("button", { onclick: act(async () => { await call("auth.account.update", { subject: a.subject, disabled: !a.disabled }); }) }, a.disabled ? "Enable" : "Disable")))))));
}
$("newacct").addEventListener("click", async () => {
  $("acct-err").textContent = "";
  const v = await ask("New account", [{ name: "name", label: "Name" }, { name: "password", label: "Password", type: "password", help: "At least 8 characters." }, { name: "roles", label: "Roles, separated by commas", value: "member" }], "Create");
  if (!v) return;
  try { await call("auth.account.create", { name: v.name, password: v.password, roles: v.roles.split(",").map((x) => x.trim()).filter(Boolean) }); } catch (e) { $("acct-err").textContent = say(e); }
  accounts();
});

// ---- plugins and settings: the identity file, when the operator let the console change it (capability shell) ----
let C = null;          // what the server sent: the catalog and the stored profile
let P = {};            // per plugin: { enabled, form }
let checkTimer = 0, checkSeq = 0, problems = [];
const entryOf = (id) => (C.profile.plugins || {})[id];

async function config() {
  if (!allowed("console.profile.get")) return;
  $("restartbar").hidden = true;
  try { C = (await call("console.profile.get", {})).output; } catch (e) { $("config").textContent = say(e); return; }
  $("config-notice").hidden = !C.notice; $("config-notice").textContent = C.notice || "";
  $("config-path").textContent = "Saved in " + C.path + (C.load_error ? " (it could not be read: " + C.load_error + ")" : "") + ". Changes take effect after a restart.";
  P = {};
  for (const p of C.plugins) { const e = entryOf(p.id); P[p.id] = { enabled: !!(e && e.enabled), form: null, settings: e && e.settings }; }
  problems = C.problems || [];
  drawConfig();
}

function schemaHasFields(p) { return p.settings_schema && p.settings_schema.properties && Object.keys(p.settings_schema.properties).length; }

function draft() {
  const out = structuredClone(C.profile); out.plugins = {};
  for (const p of C.plugins) {
    const st = P[p.id], orig = entryOf(p.id) || {};
    const settings = st.form ? st.form.get() : st.settings;
    const grants = new Set(orig.grants || []);
    if (st.enabled) for (const c of (p.requires.capabilities || [])) grants.add(c);
    const e = { enabled: st.enabled };
    if (grants.size) e.grants = [...grants];
    if (settings && Object.keys(settings).length) e.settings = settings;
    if (st.enabled || e.grants || e.settings) out.plugins[p.id] = e;
  }
  for (const [port, id] of Object.entries(out.bindings || {})) if (!P[id] || !P[id].enabled) delete out.bindings[port];
  return out;
}
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
function changes() {
  if (!C) return [];
  const d = draft(), list = [];
  for (const p of C.plugins) {
    const was = entryOf(p.id), now = d.plugins[p.id];
    const wasOn = !!(was && was.enabled), nowOn = !!(now && now.enabled);
    if (wasOn !== nowOn) list.push({ id: p.id, what: nowOn ? "switched on" : "switched off", before: was, after: now });
    else if (!same(was && was.settings, now && now.settings)) list.push({ id: p.id, what: "settings changed", before: was && was.settings, after: now && now.settings });
  }
  if (!same(C.profile.bindings || {}, d.bindings || {})) list.push({ id: "(choices)", what: "which plugin fills a Port changed", before: C.profile.bindings, after: d.bindings });
  return list;
}
const dirty = () => changes().length > 0;
const formErrors = () => C.plugins.filter((p) => P[p.id].enabled && P[p.id].form && P[p.id].form.error()).map((p) => p.id + ": " + P[p.id].form.error());

function choices() {
  const out = {}, ports = new Set(C.core_ports || []);
  for (const p of C.plugins) for (const d of p.defines || []) if (d.policy === "single") ports.add(d.name);
  for (const port of ports) {
    const who = C.plugins.filter((p) => P[p.id].enabled && (p.fills === port || (p.provides || []).includes(port))).map((p) => p.id);
    if (who.length > 1) out[port] = who;
  }
  return out;
}

function drawConfig() {
  const q = $("config-find").value.trim().toLowerCase(), onlyOn = $("config-active").checked;
  // Never rebuild the cards under someone who is typing in one of them.
  const keepFocus = document.activeElement && $("config").contains(document.activeElement) && /^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement.tagName);
  const ch = choices(), d = draft();
  $("bindings").replaceChildren(...(Object.keys(ch).length ? [h("h2", {}, "Where several plugins could do the job, pick one")] : []), ...Object.entries(ch).map(([port, who]) => {
    const sel = h("select", { "aria-label": "Plugin for " + port }, h("option", { value: "" }, "(choose)"), who.map((id) => h("option", { value: id, ...((d.bindings || {})[port] === id ? { selected: true } : {}) }, id)));
    sel.addEventListener("change", () => { C.profile.bindings = { ...(C.profile.bindings || {}) }; if (sel.value) C.profile.bindings[port] = sel.value; else delete C.profile.bindings[port]; touch(); });
    return h("div", { class: "toolbar" }, h("code", {}, port), sel);
  }));
  if (!keepFocus) {
    const groups = new Map();
    for (const p of C.plugins) {
      if (q && !(p.id + " " + (p.name || "") + " " + (p.description || "")).toLowerCase().includes(q)) continue;
      if (onlyOn && !P[p.id].enabled) continue;
      const s = p.scope || scopeOf(p.id); (groups.get(s) || groups.set(s, []).get(s)).push(p);
    }
    $("config").replaceChildren(...[...groups].map(([scope, list]) => h("div", { class: "scope" }, h("h2", {}, "@" + scope), list.map(card))));
  } else for (const el of $("config").querySelectorAll(".plugin")) markCard(el);
  bar();
}
function markCard(el) {
  const id = el.dataset.id, p = C.plugins.find((x) => x.id === id), mine = problems.filter((x) => x.plugins.includes(id));
  const ch = changes().some((c) => c.id === id);
  el.classList.toggle("on", P[id].enabled); el.classList.toggle("changed", ch); el.classList.toggle("bad", mine.length > 0);
  el.querySelector(".msgs").replaceChildren(...mine.map((x) => h("div", { class: "msg" }, x.message)));
}
function card(p) {
  const st = P[p.id], needs = [
    ...(p.fills ? ["fills " + p.fills] : []), ...(p.provides || []).map((x) => "provides " + x), ...(p.defines || []).map((x) => "defines " + x.name),
    ...(p.requires.plugins || []).map((x) => "needs " + x), ...(p.requires.capabilities || []).map((x) => "may " + x), ...(p.requires.ports || []).map((x) => "uses " + x)];
  const sw = h("input", { type: "checkbox", id: "sw-" + p.id, "aria-label": "Switch " + p.id + " on" });
  sw.checked = st.enabled;
  sw.addEventListener("change", () => { st.enabled = sw.checked; if (st.enabled && schemaHasFields(p) && !st.form) st.form = SettingsForm.build(p.settings_schema, st.settings, touch); draw(); touch(true); });
  let body = null;
  const draw = () => { body.replaceChildren(); if (st.enabled && (schemaHasFields(p) || st.settings)) { if (!st.form) st.form = SettingsForm.build(p.settings_schema || {}, st.settings, touch); body.append(st.form.el); } };
  body = h("div", {});
  const el = h("div", { class: "plugin", "data-id": p.id },
    h("div", { class: "head" }, h("label", { class: "switch" }, sw, h("i", {})), h("span", { class: "name" }, nameOf(p.id)), h("span", { class: "chip" }, p.kind + " " + p.version), p.quality && p.quality !== "reference" ? h("span", { class: "chip" }, p.quality) : null),
    p.description ? h("div", { class: "desc" }, p.description) : null, needs.length ? h("div", { class: "needs" }, needs.join(" · ")) : null,
    h("div", { class: "msgs" }), body);
  draw(); markCard(el);
  return el;
}
function bar() {
  const list = changes(), errs = formErrors(), bad = problems.length || errs.length;
  $("savebar").hidden = !list.length;
  $("save-count").textContent = list.length + (list.length === 1 ? " unsaved change" : " unsaved changes") + ": " + list.map((c) => c.id).join(", ");
  $("save-problems").textContent = bad ? "  " + (errs[0] || problems[0].message) : "";
  $("config-save").disabled = !!bad;
}
function touch(redraw) {
  clearTimeout(checkTimer);
  checkTimer = setTimeout(verify, 250);
  if (redraw) drawConfig(); else { bar(); for (const el of $("config").querySelectorAll(".plugin")) markCard(el); }
}
async function verify() {
  const mine = ++checkSeq;
  try {
    const r = (await call("console.profile.check", { profile: draft() })).output;
    if (mine !== checkSeq) return;
    problems = r.problems || [];
  } catch (e) { problems = [{ code: "invalid", plugins: [], message: say(e) }]; }
  drawConfig();
}
$("config-find").addEventListener("input", () => C && drawConfig());
$("config-active").addEventListener("change", () => C && drawConfig());
$("config-discard").addEventListener("click", () => { if (!dirty() || confirm("Throw away the changes you have not saved?")) config(); });
$("config-save").addEventListener("click", async () => {
  const list = changes();
  const node = h("div", {}, h("p", {}, "These changes will be written to the identity file. The old file is kept next to it as a backup."),
    list.map((c) => h("div", {}, h("strong", {}, c.id), " ", h("span", { class: "muted" }, c.what), h("div", { class: "diff" }, "before: " + JSON.stringify(c.before ?? null, null, 1) + "\nafter:  " + JSON.stringify(c.after ?? null, null, 1)))));
  if (!(await review("Save these changes?", node, "Save"))) return;
  try {
    await call("console.profile.save", { profile: draft() });
    $("restartbar").hidden = false; $("restart-msg").textContent = "Saved. The change takes effect when the instance restarts."; $("restartbar").className = "banner ok";
    await config(); $("restartbar").hidden = false;
  } catch (e) { $("restartbar").hidden = false; $("restartbar").className = "banner bad"; $("restart-msg").textContent = "Not saved: " + say(e); }
});
$("config-restart").addEventListener("click", async () => {
  const msg = $("restart-msg"), btn = $("config-restart");
  btn.disabled = true;
  try { await call("console.restart", {}); } catch (e) { msg.textContent = say(e); btn.disabled = false; return; }
  msg.textContent = "Restarting...";
  for (let i = 0; i < 60; i++) {
    await new Promise((r) => setTimeout(r, 500));
    try { if ((await fetch("/healthz", { cache: "no-store" })).ok && i > 1) { location.reload(); return; } } catch {}
  }
  $("restartbar").className = "banner bad";
  msg.textContent = "The instance did not come back. Check its log; the previous file is kept as nuah.json.bak next to it.";
  btn.disabled = false;
});
window.addEventListener("beforeunload", (e) => { if (C && TAB === "config" && dirty()) { e.preventDefault(); e.returnValue = ""; } });
load();
