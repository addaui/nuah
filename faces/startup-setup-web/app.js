// SPDX-License-Identifier: Apache-2.0
// The setup wizard. It reads the catalog from the setup server, lets you choose plugins, bindings and
// settings, and asks the kernel's own resolver whether the draft works. It holds no rules itself.

const $ = (id) => document.getElementById(id);
function h(tag, props, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (k === 'class') el.className = v;
    else if (k.startsWith('on')) el.addEventListener(k.slice(2).toLowerCase(), v);
    else if (v === true) el.setAttribute(k, '');
    else if (v !== false && v != null) el.setAttribute(k, v);
  }
  for (const kid of kids.flat(Infinity)) if (kid != null && kid !== false) el.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
  return el;
}
let toastTimer;
function toast(msg, err) { const t = $('toast'); t.textContent = msg; t.className = 'toast' + (err ? ' err' : ''); t.hidden = false; clearTimeout(toastTimer); toastTimer = setTimeout(() => (t.hidden = true), err ? 6000 : 2500); }

// The setup code is typed once and kept only for this tab.
let code = '';
try { code = sessionStorage.getItem('nuah-setup-code') || ''; } catch { /* storage may be blocked */ }

async function send(path, body) {
  const headers = { 'X-Nuah-Setup': code, 'X-Nuah-Face': 'setup' };
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  const r = await fetch(path, { method: body === undefined ? 'GET' : 'POST', headers, cache: 'no-store', body: body === undefined ? undefined : JSON.stringify(body) });
  return { status: r.status, body: await r.json().catch(() => ({})) };
}
const post = send;

const st = { first: { name: '', password: '' }, info: null, name: '', enabled: [], bindings: {}, settings: {}, grants: {}, permissions: {}, parked: {}, bad: {}, raw: {}, problems: [], timer: 0, seq: 0 };

// Grants and permissions are not edited here, but they belong to the Profile and must survive a save.
const draft = () => ({ name: st.name, enabled: st.enabled, bindings: st.bindings, settings: st.settings, grants: st.grants, permissions: st.permissions, ...(needsFirstAccount() ? { first_account: { ...st.first } } : {}) });

// An enabled plugin that keeps accounts and has none saved yet asks for the first administrator.
function needsFirstAccount() {
  return st.info.plugins.some((p) => isOn(p.id) && p.settings_schema?.properties?.accounts?.['x-first-account'] === 'password-hash'
    && !(Array.isArray(st.settings[p.id]?.accounts) ? st.settings[p.id].accounts.length : st.settings[p.id]?.accounts));
}
const plugin = (id) => st.info.plugins.find((p) => p.id === id);
const isOn = (id) => st.enabled.includes(id);

// Which enabled plugins could fill each port that has to be chosen between.
function choices() {
  const out = {};
  const ports = new Set(st.info.core_ports);
  for (const p of st.info.plugins) for (const d of p.defines) if (d.policy === 'single') ports.add(d.name);
  for (const port of ports) {
    const who = st.info.plugins.filter((p) => isOn(p.id) && (p.fills === port || p.provides.includes(port))).map((p) => p.id);
    if (who.length > 1) out[port] = who;
  }
  return out;
}

function schedule() {
  clearTimeout(st.timer);
  st.timer = setTimeout(verify, 200);
  render();
}

async function verify() {
  const mine = ++st.seq;
  const { body } = await post('/setup/check', draft());
  if (mine !== st.seq) return;
  st.problems = body.problems || (body.error ? [{ code: 'invalid_profile', plugins: [], message: body.error.message }] : []);
  render();
}

function render() {
  $('name').value = st.name;
  const byPlugin = {};
  for (const p of st.problems) for (const id of p.plugins) (byPlugin[id] ||= []).push(p);
  const anyBad = st.enabled.some((id) => st.bad[id]);
  $('save').disabled = st.problems.length > 0 || anyBad;
  $('first').hidden = !needsFirstAccount();
  $('path').textContent = st.info.path + (st.info.file_exists ? '' : ' (new file)');

  $('problems').replaceChildren(h('h2', {}, 'Does it work?'), ...(st.problems.length
    ? st.problems.map((p) => h('div', { class: 'problem' }, h('code', {}, p.code), p.message))
    : [h('div', { class: 'ok' }, 'Everything resolves. You can save this profile.')]));

  // Never rebuild the cards under someone who is typing settings: that would drop focus and text.
  if (document.activeElement?.tagName === 'TEXTAREA') return;
  const ch = choices();
  const ports = Object.keys(ch);
  $('bindings').replaceChildren(...(ports.length ? [h('h2', {}, 'Where several plugins could do the job, pick one')] : []),
    ...ports.map((port) => {
      const sel = h('select', { 'aria-label': `Plugin for ${port}` }, h('option', { value: '' }, '(choose)'),
        ch[port].map((id) => h('option', { value: id, selected: st.bindings[port] === id }, id)));
      sel.addEventListener('change', () => { if (sel.value) st.bindings[port] = sel.value; else delete st.bindings[port]; schedule(); });
      return h('div', { class: 'bind' }, h('code', {}, port), sel);
    }));

  const groups = {};
  for (const p of st.info.plugins) (groups[p.kind || 'other'] ||= []).push(p);
  $('plugins').replaceChildren(...Object.entries(groups).flatMap(([kind, list]) => [
    h('h2', {}, kind),
    h('div', { class: 'group' }, list.map((p) => card(p, byPlugin[p.id] || []))),
  ]));
}

function card(p, problems) {
  const on = isOn(p.id);
  const box = h('input', { type: 'checkbox', checked: on, id: `en-${p.id}` });
  box.addEventListener('change', () => {
    if (box.checked) {
      if (!st.enabled.includes(p.id)) st.enabled.push(p.id);
      // Enabling a plugin is the operator's yes to the capabilities it asks for (listed on its card).
      if (p.requires.capabilities?.length) st.grants[p.id] = [...new Set([...(st.grants[p.id] || []), ...p.requires.capabilities])];
      // Bring back what was set aside when it was switched off.
      const kept = st.parked[p.id];
      if (kept) { if (kept.settings !== undefined) st.settings[p.id] = kept.settings; if (kept.grants !== undefined) st.grants[p.id] = kept.grants; delete st.parked[p.id]; }
    }
    else {
      st.enabled = st.enabled.filter((x) => x !== p.id);
      // A choice that points at a plugin that is now off would be an error with no control to fix it.
      for (const [port, who] of Object.entries(st.bindings)) if (who === p.id) delete st.bindings[port];
      // The Resolver rejects settings and grants for a plugin that is off, so set them aside, not lose them.
      st.parked[p.id] = { settings: st.settings[p.id], grants: st.grants[p.id] };
      delete st.settings[p.id]; delete st.grants[p.id];
      delete st.bad[p.id]; delete st.raw[p.id];
    }
    schedule();
  });
  const needs = [
    ...(p.fills ? [`fills ${p.fills}`] : []), ...p.provides.map((x) => `provides ${x}`),
    ...p.defines.map((d) => `defines ${d.name}`),
    ...p.requires.plugins.map((x) => `needs ${x}`), ...(p.requires.capabilities || []).map((x) => `may ${x}`), ...p.requires.ports.map((x) => `uses ${x}`), ...(p.requires.extension_points || []).map((x) => `uses ${x}`),
  ];
  const hasSettings = p.settings_schema && Object.keys(p.settings_schema).length > 0 || st.settings[p.id] !== undefined;
  let box2 = null;
  if (on && hasSettings) {
    const ta = h('textarea', { rows: 4, 'aria-label': `Settings of ${p.id}`, spellcheck: 'false' }, st.raw[p.id] ?? JSON.stringify(st.settings[p.id] ?? {}, null, 2));
    ta.addEventListener('input', () => {
      st.raw[p.id] = ta.value;
      try { st.settings[p.id] = JSON.parse(ta.value); delete st.bad[p.id]; ta.removeAttribute('aria-invalid'); schedule(); }
      catch { st.bad[p.id] = true; ta.setAttribute('aria-invalid', 'true'); $('save').disabled = true; }
    });
    box2 = [h('div', { class: 'needs' }, 'Settings (JSON)', JSON.stringify(st.settings[p.id] ?? {}).includes('$secret') ? ' — a hidden secret stays as it is unless you replace it' : '', p.settings_schema?.required ? ` — required: ${p.settings_schema.required.join(', ')}` : ''), ta];
  }
  return h('div', { class: 'card' + (on ? ' on' : '') + (problems.length ? ' bad' : '') },
    h('header', {}, h('label', { class: 'toggle', for: `en-${p.id}` }, box, p.id), h('span', { class: 'chip' }, `v${p.version}`),
      p.quality ? h('span', { class: 'chip' }, p.quality) : null),
    p.description ? h('p', {}, p.description) : null,
    needs.length ? h('div', { class: 'needs' }, needs.join(' · ')) : null,
    problems.map((x) => h('div', { class: 'msg' }, x.message)), box2);
}

async function save() {
  const { status, body } = await post('/setup/save', draft());
  if (status === 200 && body.ok) {
    toast(`Saved ${body.saved}. Start it with: serve`);
    st.info.file_exists = true; render();
  } else {
    st.problems = body.problems || [{ code: 'invalid_profile', plugins: [], message: body.error?.message || 'Could not save' }];
    render();
    toast('Not saved: the profile has problems', true);
  }
}

function ask() {
  $('gate').hidden = false;
  $('code').focus();
}

async function load() {
  const { status, body } = await send('/setup/state');
  if (status === 401) { $('gate-msg').textContent = code ? 'That code is not right.' : ''; ask(); return; }
  if (status !== 200) { $('problems').replaceChildren(h('div', { class: 'problem' }, body.error?.message || `HTTP ${status}`)); return; }
  $('gate').hidden = true;
  st.info = body;
  st.name = body.profile.name; st.enabled = [...body.profile.enabled];
  st.bindings = { ...body.profile.bindings }; st.settings = structuredClone(body.profile.settings);
  st.grants = structuredClone(body.profile.grants || {}); st.permissions = structuredClone(body.profile.permissions || {});
  st.problems = body.problems;
  if (body.load_error) st.problems = [{ code: 'invalid_profile', plugins: [], message: `The existing file could not be read (${body.load_error}). Starting from the default; saving replaces it.` }, ...st.problems];
  render();
}

$('first-name').addEventListener('input', (e) => { st.first.name = e.target.value; schedule(); });
$('first-pass').addEventListener('input', (e) => { st.first.password = e.target.value; schedule(); });
$('name').addEventListener('input', (e) => { st.name = e.target.value; schedule(); });
$('save').addEventListener('click', save);
$('gate-form').addEventListener('submit', (e) => {
  e.preventDefault();
  code = $('code').value.trim();
  try { sessionStorage.setItem('nuah-setup-code', code); } catch { /* keep it in memory only */ }
  load();
});
load();
