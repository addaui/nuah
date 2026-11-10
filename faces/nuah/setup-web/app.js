// SPDX-License-Identifier: Apache-2.0
// The setup wizard. It reads the catalog from the setup server and walks through a few plain questions:
// a name, where the data is kept, how people sign in, what else runs, and a review. Settings are forms drawn
// from each plugin's settings schema. It holds no rules itself: the kernel's own Resolver says whether the
// draft works, and nothing is saved unless it does.

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
const nameOf = (id) => (id.startsWith('@') ? id.slice(id.indexOf('/') + 1) : id);
const scopeOf = (id) => (id.startsWith('@') ? id.slice(1, id.indexOf('/')) : 'other');

// The setup code is typed once and kept only for this tab.
let code = '';
try { code = sessionStorage.getItem('nuah-setup-code') || ''; } catch { /* storage may be blocked */ }

async function send(path, body) {
  const headers = { 'X-Nuah-Setup': code, 'X-Nuah-Face': 'setup' };
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  const r = await fetch(path, { method: body === undefined ? 'GET' : 'POST', headers, cache: 'no-store', body: body === undefined ? undefined : JSON.stringify(body) });
  return { status: r.status, body: await r.json().catch(() => ({})) };
}

const st = { info: null, profile: null, name: '', plugins: {}, bindings: {}, first: { name: '', password: '' }, step: 0, steps: [], problems: [], timer: 0, seq: 0, saved: null, seen: new Set() };

const isOn = (id) => !!st.plugins[id]?.enabled;
const settingsOf = (p) => { const s = st.plugins[p.id]; return s.form ? s.form.get() : s.settings; };
const hasFields = (p) => p.settings_schema?.properties && Object.keys(p.settings_schema.properties).length > 0;
const fills = (port) => st.info.plugins.filter((p) => p.fills === port);
const INFRA = (p) => p.fills && p.fills !== 'db' && p.fills !== 'auth';
const features = () => st.info.plugins.filter((p) => !p.fills);

// An enabled plugin that keeps accounts and has none saved yet asks for the first administrator.
function firstAccountPlugin() {
  return st.info.plugins.find((p) => isOn(p.id) && p.settings_schema?.properties?.accounts?.['x-first-account'] === 'password-hash');
}
function needsFirst() {
  const p = firstAccountPlugin();
  if (!p) return false;
  const a = settingsOf(p)?.accounts;
  return !(Array.isArray(a) ? a.length : a);
}

function draft() {
  const plugins = {};
  for (const p of st.info.plugins) {
    const s = st.plugins[p.id], orig = st.profile.plugins?.[p.id] || {};
    const settings = settingsOf(p);
    const grants = new Set(orig.grants || []);
    // Switching a plugin on is the operator's yes to the capabilities it asks for.
    if (s.enabled) for (const c of p.requires.capabilities || []) grants.add(c);
    const e = { enabled: s.enabled };
    if (grants.size) e.grants = [...grants];
    if (settings && Object.keys(settings).length) e.settings = settings;
    if (s.enabled || e.grants || e.settings) plugins[p.id] = e;
  }
  const bindings = {};
  for (const [port, id] of Object.entries(st.bindings)) if (isOn(id)) bindings[port] = id;
  return { ...st.profile, name: st.name, plugins, bindings, ...(needsFirst() ? { first_account: { ...st.first } } : {}) };
}

function buildSteps() {
  const list = [{ id: 'name', label: 'Name' }];
  if (fills('db').length) list.push({ id: 'storage', label: 'Storage' });
  if (fills('auth').length) list.push({ id: 'signin', label: 'Sign-in' });
  if (features().length || st.info.plugins.some(INFRA)) list.push({ id: 'features', label: 'Features' });
  list.push({ id: 'review', label: 'Review' });
  return list;
}

// Why the current step cannot go on yet, or null.
function stepError(id) {
  const formErr = (ps) => { for (const p of ps) if (isOn(p.id) && st.plugins[p.id].form?.error()) return `${nameOf(p.id)}: ${st.plugins[p.id].form.error()}`; return null; };
  if (id === 'name') return st.name.trim() ? null : 'Give this instance a name.';
  if (id === 'storage') return !fills('db').some((p) => isOn(p.id)) ? 'Choose where the data is kept.' : formErr(fills('db'));
  if (id === 'signin') {
    if (!fills('auth').some((p) => isOn(p.id))) return 'Choose how people sign in.';
    const e = formErr(fills('auth')); if (e) return e;
    if (needsFirst()) { if (!st.first.name.trim()) return 'Choose a name for the first administrator.'; if (st.first.password.length < 8) return 'The password needs at least 8 characters.'; }
    return null;
  }
  if (id === 'features') return formErr(st.info.plugins.filter((p) => !fills('db').includes(p) && !fills('auth').includes(p)));
  if (id === 'review') return st.problems.length ? 'Fix the problems above first.' : null;
  return null;
}

function schedule() {
  clearTimeout(st.timer);
  st.timer = setTimeout(verify, 200);
  chrome();
}
async function verify() {
  const mine = ++st.seq;
  const { body } = await send('/setup/check', draft());
  if (mine !== st.seq) return;
  st.problems = body.problems || (body.error ? [{ code: 'invalid_profile', plugins: [], message: body.error.message }] : []);
  chrome();
  if (st.steps[st.step]?.id === 'review') renderProblems();
}

// The step list, the buttons and the message under them: cheap, and never rebuilds the form being typed in.
function chrome() {
  const cur = st.steps[st.step]?.id;
  $('steps').replaceChildren(...st.steps.map((s, i) => h('li', {}, h('button', { type: 'button', 'aria-current': i === st.step ? 'step' : null, class: (i < st.step && !stepError(s.id) ? 'done' : '') + (st.seen.has(s.id) && i !== st.step && stepError(s.id) ? ' bad' : ''), onclick: () => go(i) }, `${i + 1}. ${s.label}`))));
  const err = st.saved ? null : stepError(cur);
  $('stepmsg').textContent = err || '';
  $('back').hidden = st.step === 0 || !!st.saved;
  $('next').hidden = !!st.saved;
  $('next').textContent = cur === 'review' ? 'Save and finish' : 'Next';
  $('next').disabled = !!err;
  $('path').textContent = st.info.path + (st.info.file_exists ? '' : ' (new file)');
}

function go(i) {
  if (st.saved) return;
  // Moving forward is only allowed over steps that are complete.
  for (let k = st.step; k < i; k++) { st.seen.add(st.steps[k].id); if (stepError(st.steps[k].id)) { st.step = k; chrome(); render(); return; } }
  st.seen.add(st.steps[st.step].id);
  st.step = i; render();
}

// ---- pages ----
function lead(title, text) { return [h('h1', {}, title), h('p', { class: 'lead' }, text)]; }

function pickOne(port, ps) {
  return ps.map((p) => {
    const on = isOn(p.id);
    const radio = h('input', { type: 'radio', name: 'pick-' + port, id: 'pick-' + p.id, checked: on });
    radio.addEventListener('change', () => { for (const q of ps) st.plugins[q.id].enabled = q.id === p.id; scheduleRender(); });
    const s = st.plugins[p.id];
    if (on && hasFields(p) && !s.form) s.form = SettingsForm.build(p.settings_schema, s.settings, schedule);
    return h('div', { class: 'choice' + (on ? ' on' : '') },
      h('label', { class: 'head', for: 'pick-' + p.id }, radio, h('span', { class: 'name' }, p.name || nameOf(p.id)), h('span', { class: 'chip' }, `${p.id} v${p.version}`)),
      p.description ? h('div', { class: 'desc' }, p.description) : null,
      on && s.form ? h('div', { class: 'form' }, s.form.el) : null);
  });
}
let again = 0;
function scheduleRender() { cancelAnimationFrame(again); again = requestAnimationFrame(() => { render(); schedule(); }); }

function toggles(list) {
  return list.map((p) => {
    const on = isOn(p.id), s = st.plugins[p.id];
    const box = h('input', { type: 'checkbox', id: 'en-' + p.id, checked: on });
    box.addEventListener('change', () => { s.enabled = box.checked; if (s.enabled && hasFields(p) && !s.form) s.form = SettingsForm.build(p.settings_schema, s.settings, schedule); scheduleRender(); });
    if (on && hasFields(p) && !s.form) s.form = SettingsForm.build(p.settings_schema, s.settings, schedule);
    const needs = [...(p.requires.plugins || []).map((x) => 'needs ' + x), ...(p.requires.capabilities || []).map((x) => 'may ' + x), ...(p.provides || []).map((x) => 'provides ' + x)];
    return h('div', { class: 'choice' + (on ? ' on' : '') },
      h('label', { class: 'head', for: 'en-' + p.id }, box, h('span', { class: 'name' }, p.name || nameOf(p.id)), h('span', { class: 'chip' }, `${nameOf(p.id)} v${p.version}`)),
      p.description ? h('div', { class: 'desc' }, p.description) : null, needs.length ? h('div', { class: 'needs' }, needs.join(' · ')) : null,
      on && s.form ? h('div', { class: 'form' }, s.form.el) : null);
  });
}

function bindingsBlock() {
  const ports = new Set(st.info.core_ports);
  for (const p of st.info.plugins) for (const d of p.defines) if (d.policy === 'single') ports.add(d.name);
  const out = [];
  for (const port of ports) {
    const who = st.info.plugins.filter((p) => isOn(p.id) && (p.fills === port || p.provides.includes(port))).map((p) => p.id);
    if (who.length < 2) continue;
    const sel = h('select', { 'aria-label': 'Plugin for ' + port }, h('option', { value: '' }, '(choose)'), who.map((id) => h('option', { value: id, selected: st.bindings[port] === id }, id)));
    sel.addEventListener('change', () => { if (sel.value) st.bindings[port] = sel.value; else delete st.bindings[port]; schedule(); });
    out.push(h('div', { class: 'field' }, h('label', {}, `Which plugin handles "${port}"?`), sel));
  }
  return out.length ? [h('h2', {}, 'Several plugins could do the same job'), ...out] : [];
}

function renderProblems() {
  const box = $('problems');
  if (!box) return;
  box.replaceChildren(...(st.problems.length ? st.problems.map((p) => h('div', { class: 'problem' }, h('code', {}, p.code), p.message)) : [h('div', { class: 'ok' }, 'Everything fits together. You can save.')]));
}

function render() {
  const cur = st.steps[st.step].id, page = $('page');
  if (cur === 'name') {
    const input = h('input', { type: 'text', id: 'name', autocomplete: 'off', maxlength: 60 });
    input.value = st.name;
    input.addEventListener('input', () => { st.name = input.value; schedule(); });
    page.replaceChildren(...lead('Name this instance', 'It appears in the console and in logs. You can change it later.'), h('div', { class: 'field req' }, h('label', { for: 'name' }, 'Name'), input));
    input.focus();
  } else if (cur === 'storage') {
    page.replaceChildren(...lead('Where is the data kept?', 'Pick one. You can move to another later from the console.'), ...pickOne('db', fills('db')));
  } else if (cur === 'signin') {
    const first = needsFirst() ? [h('h2', {}, 'First administrator'), h('p', { class: 'lead' }, 'The person who signs in first and can add the others. The password is stored hashed; you can change it later.'),
      (() => { const i = h('input', { type: 'text', id: 'first-name', autocomplete: 'username', maxlength: 200 }); i.value = st.first.name; i.addEventListener('input', () => { st.first.name = i.value; schedule(); }); return h('div', { class: 'field req' }, h('label', { for: 'first-name' }, 'Name'), i); })(),
      (() => { const i = h('input', { type: 'password', id: 'first-pass', autocomplete: 'new-password', maxlength: 1000 }); i.value = st.first.password; i.addEventListener('input', () => { st.first.password = i.value; schedule(); }); return h('div', { class: 'field req' }, h('label', { for: 'first-pass' }, 'Password'), i, h('div', { class: 'help' }, 'At least 8 characters.')); })()] : [];
    page.replaceChildren(...lead('How do people sign in?', 'Pick one.'), ...pickOne('auth', fills('auth')), ...first);
  } else if (cur === 'features') {
    const groups = {};
    for (const p of features()) (groups[scopeOf(p.id)] ||= []).push(p);
    const infra = st.info.plugins.filter(INFRA);
    page.replaceChildren(...lead('What else should run?', 'Switch on what you want. Each one can ask for settings.'),
      ...Object.entries(groups).flatMap(([scope, list]) => [h('h2', {}, '@' + scope), ...toggles(list)]),
      ...bindingsBlock(),
      infra.length ? h('details', { class: 'more' }, h('summary', {}, `Infrastructure (${infra.length}): sensible defaults, usually left as they are`), ...toggles(infra)) : null);
  } else {
    const on = st.info.plugins.filter((p) => isOn(p.id));
    const row = (k, v) => h('div', {}, h('b', {}, k), h('span', {}, v));
    const pick = (port) => on.find((p) => p.fills === port);
    page.replaceChildren(...lead('Review', 'Nothing is written until you save, and only a setup that works can be saved.'),
      h('div', { class: 'sum' }, row('Name', st.name), pick('db') ? row('Data', pick('db').name || pick('db').id) : null, pick('auth') ? row('Sign-in', (pick('auth').name || pick('auth').id) + (needsFirst() ? ` (first administrator: ${st.first.name})` : '')) : null,
        row('Running', on.filter((p) => !p.fills).map((p) => nameOf(p.id)).join(', ') || 'only the basics'), row('Saved to', st.info.path)),
      h('h2', {}, 'Does it work?'), h('div', { id: 'problems' }));
    renderProblems();
  }
  chrome();
}

async function save() {
  $('next').disabled = true;
  const { status, body } = await send('/setup/save', draft());
  if (status === 200 && body.ok) {
    st.saved = body.saved; st.info.file_exists = true;
    const who = needsFirst() || st.first.name ? st.first.name : '';
    $('page').replaceChildren(h('div', { class: 'done-box' }, h('h1', {}, 'Saved'), h('p', {}, 'The configuration is in ', h('code', {}, body.saved), '.'),
      h('p', {}, 'If the server was started for the first time it now starts on its own. Reload this page in a few seconds' + (who ? ' and sign in as ' + who : '') + '.'),
      h('p', { class: 'muted' }, 'If you ran setup by hand, start the product with its serve command.')));
    chrome();
  } else {
    st.problems = body.problems || [{ code: 'invalid_profile', plugins: [], message: body.error?.message || 'Could not save' }];
    renderProblems(); chrome(); toast('Not saved: the setup has problems', true);
  }
}

function ask() { $('gate').hidden = false; $('code').focus(); }

async function load() {
  const { status, body } = await send('/setup/state');
  if (status === 401) { $('gate-msg').textContent = code ? 'That code is not right.' : ''; ask(); return; }
  if (status !== 200) { $('page').replaceChildren(h('div', { class: 'problem' }, body.error?.message || `HTTP ${status}`)); return; }
  $('gate').hidden = true;
  st.info = body; st.profile = structuredClone(body.profile);
  st.name = body.profile.name;
  st.bindings = { ...(body.profile.bindings || {}) };
  for (const p of body.plugins) { const e = body.profile.plugins?.[p.id]; st.plugins[p.id] = { enabled: !!(e && e.enabled), settings: e?.settings, form: null }; }
  st.problems = body.problems;
  if (body.load_error) st.problems = [{ code: 'invalid_profile', plugins: [], message: `The existing file could not be read (${body.load_error}). Starting from the default; saving replaces it.` }, ...st.problems];
  st.steps = buildSteps();
  render();
}

$('back').addEventListener('click', () => go(Math.max(0, st.step - 1)));
$('next').addEventListener('click', () => { if (st.steps[st.step].id === 'review') save(); else go(st.step + 1); });
$('gate-form').addEventListener('submit', (e) => {
  e.preventDefault();
  code = $('code').value.trim();
  try { sessionStorage.setItem('nuah-setup-code', code); } catch { /* keep it in memory only */ }
  load();
});
load();
