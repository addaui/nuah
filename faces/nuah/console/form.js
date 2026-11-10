// SPDX-License-Identifier: Apache-2.0
"use strict";
// Draws one plugin's settings as a form from its settings schema (ADR 0042) and reads the value back.
// The console and the setup wizard each carry this same file (a test keeps the two identical).
//   const f = SettingsForm.build(schema, currentValue, onchange);   // f.el, f.get(), f.error()
// A setting is a text field, number field, checkbox, select or list; a secret is a password field that
// keeps its stored value unless something is typed; a text setting may point at an environment variable or a
// file instead ({"$env": "NAME"}, {"$file": "/path"}) unless its schema says "x-ref": false.
// Whatever the schema does not describe stays editable as JSON.
(function (root) {
  const isObj = (v) => v !== null && typeof v === "object" && !Array.isArray(v);
  const isMarker = (v) => isObj(v) && v.$secret === true;
  const isRef = (v) => isObj(v) && Object.keys(v).length === 1 && (typeof v.$env === "string" || typeof v.$file === "string");
  const typesOf = (s) => (Array.isArray(s.type) ? s.type : s.type ? [s.type] : []);
  const nice = (k) => k.replace(/_/g, " ").replace(/^./, (c) => c.toUpperCase());

  function el(tag, props, ...kids) {
    const e = document.createElement(tag);
    for (const [k, v] of Object.entries(props || {})) {
      if (k === "class") e.className = v;
      else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
      else if (v === true) e.setAttribute(k, "");
      else if (v !== false && v != null) e.setAttribute(k, v);
    }
    for (const kid of kids.flat(Infinity)) if (kid != null && kid !== false) e.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
    return e;
  }

  // One setting. Returns { el, get(): value | undefined, error(): text | null }.
  function field(key, s, value, required, onchange) {
    const ts = typesOf(s).filter((t) => t !== "null");
    const label = s.title || nice(key);
    const help = s.description ? el("div", { class: "help" }, s.description) : null;
    const secret = s["x-secret"] === true;
    const canRef = s["x-ref"] !== false && ts.length === 1 && ts[0] === "string" && !s.enum;
    const id = "f-" + Math.random().toString(36).slice(2, 9);
    let get, error = () => null, input, extra = null;
    const fire = () => onchange && onchange();
    const wrap = (control) => el("div", { class: "field" + (required ? " req" : "") }, el("label", { for: id }, label), control, help, extra);

    if (ts.length === 1 && ts[0] === "boolean" && !s.enum) {
      input = el("input", { type: "checkbox", id, ...(value === true || (value === undefined && s.default === true) ? { checked: true } : {}) });
      let touched = false;
      input.addEventListener("change", () => { touched = true; fire(); });
      get = () => (touched || value !== undefined ? input.checked : undefined);
      return { el: el("div", { class: "field check" }, input, el("label", { for: id }, label), help), get, error };
    }
    if (s.enum) {
      input = el("select", { id }, el("option", { value: "" }, s.default !== undefined ? `Default (${s.default})` : "Not set"), s.enum.map((v) => el("option", { value: JSON.stringify(v), ...(JSON.stringify(v) === JSON.stringify(value) ? { selected: true } : {}) }, String(v))));
      input.addEventListener("change", fire);
      get = () => (input.value === "" ? undefined : JSON.parse(input.value));
      return { el: wrap(input), get, error: () => (required && get() === undefined ? "Required" : null) };
    }
    if (ts.length === 1 && (ts[0] === "integer" || ts[0] === "number")) {
      input = el("input", { type: "number", id, step: ts[0] === "integer" ? "1" : "any", min: s.minimum, max: s.maximum, placeholder: s.default !== undefined ? String(s.default) : "" });
      if (typeof value === "number") input.value = String(value);
      input.addEventListener("input", fire);
      get = () => (input.value === "" ? undefined : Number(input.value));
      error = () => {
        const v = get();
        if (v === undefined) return required ? "Required" : null;
        if (!Number.isFinite(v) || (ts[0] === "integer" && !Number.isInteger(v))) return "Enter a whole number";
        if (s.minimum !== undefined && v < s.minimum) return `At least ${s.minimum}`;
        if (s.maximum !== undefined && v > s.maximum) return `At most ${s.maximum}`;
        return null;
      };
      return { el: wrap(input), get, error };
    }
    if (ts.length === 1 && ts[0] === "array" && s.items && typesOf(s.items).join() === "string") {
      input = el("textarea", { id, rows: 3, spellcheck: "false", placeholder: "One per line" }, Array.isArray(value) ? value.join("\n") : "");
      input.addEventListener("input", fire);
      get = () => { const l = input.value.split("\n").map((x) => x.trim()).filter(Boolean); return l.length || Array.isArray(value) ? l : undefined; };
      return { el: wrap(input), get, error: () => (required && get() === undefined ? "Required" : null) };
    }
    if (ts.length === 1 && ts[0] === "string") {
      const hidden = isMarker(value);
      let side = null;
      const mode = isRef(value) ? ("$env" in value ? "env" : "file") : "value";
      let current = mode;
      input = el("input", { type: secret ? "password" : "text", id, autocomplete: secret ? "new-password" : "off", spellcheck: "false", maxlength: s.maxLength, placeholder: hidden ? "Set. Leave empty to keep it." : (s.examples && s.examples[0]) || (s.default !== undefined ? String(s.default) : "") });
      input.value = mode === "value" ? (typeof value === "string" ? value : "") : isRef(value) ? value.$env ?? value.$file : "";
      input.addEventListener("input", fire);
      if (canRef) {
        const pick = el("select", { class: "refmode", "aria-label": `Where ${label} comes from` },
          el("option", { value: "value", ...(mode === "value" ? { selected: true } : {}) }, secret ? "Type it here" : "Value"),
          el("option", { value: "env", ...(mode === "env" ? { selected: true } : {}) }, "From an environment variable"),
          el("option", { value: "file", ...(mode === "file" ? { selected: true } : {}) }, "From a file"));
        pick.addEventListener("change", () => {
          current = pick.value;
          input.type = current === "value" && secret ? "password" : "text";
          input.placeholder = current === "env" ? "NAME_OF_THE_VARIABLE" : current === "file" ? "/path/to/the/file" : hidden ? "Set. Leave empty to keep it." : "";
          fire();
        });
        if (!secret && mode === "value") {
          // Most settings are just typed; the other sources are one click away.
          pick.hidden = true;
          const more = el("button", { type: "button", class: "link small" }, "Use an environment variable or file instead");
          more.addEventListener("click", () => { pick.hidden = false; more.hidden = true; });
          side = el("div", {}, pick, more);
        } else side = pick;
      }
      get = () => {
        const t = input.value;
        if (current === "env") return t.trim() ? { $env: t.trim() } : undefined;
        if (current === "file") return t.trim() ? { $file: t.trim() } : undefined;
        if (t === "") return hidden && mode === "value" ? value : undefined;
        return t;
      };
      error = () => {
        const v = get();
        if (v === undefined) return required ? "Required" : null;
        if (typeof v === "string" && s.minLength && v.length < s.minLength) return `At least ${s.minLength} characters`;
        return null;
      };
      return { el: wrap(side ? el("div", { class: "withmode" }, input, side) : input), get, error };
    }
    // Anything else (objects, lists of objects, mixed types): JSON, checked as it is typed.
    input = el("textarea", { id, rows: 4, spellcheck: "false", placeholder: "JSON" }, value === undefined ? "" : JSON.stringify(value, null, 2));
    let bad = null;
    input.addEventListener("input", () => { try { if (input.value.trim()) JSON.parse(input.value); bad = null; } catch (e) { bad = "Not valid JSON"; } fire(); });
    get = () => { try { return input.value.trim() ? JSON.parse(input.value) : undefined; } catch (e) { return value; } };
    return { el: wrap(input), get, error: () => bad || (required && get() === undefined ? "Required" : null) };
  }

  // The whole settings object of one plugin. With no properties in the schema it is just JSON.
  function build(schema, value, onchange) {
    schema = schema || {};
    value = isObj(value) ? value : {};
    const props = isObj(schema.properties) ? schema.properties : {};
    const required = new Set(schema.required || []);
    const root = el("div", { class: "settings" });
    let fields = [], jsonArea = null, mode = Object.keys(props).length ? "form" : "json";

    // A setting the product asks for through its own screen (the first administrator) is kept, not drawn.
    const hidden = (s) => s && s["x-first-account"] !== undefined;
    const formGet = () => {
      const out = {};
      for (const k of Object.keys(value)) if (!(k in props) || hidden(props[k])) out[k] = value[k];
      for (const f of fields) { const v = f.field.get(); if (v !== undefined) out[f.key] = v; }
      return out;
    };
    const get = () => {
      if (mode === "json") { try { return jsonArea.value.trim() ? JSON.parse(jsonArea.value) : {}; } catch (e) { return value; } }
      return formGet();
    };
    const error = () => {
      if (mode === "json") { try { jsonArea.value.trim() && JSON.parse(jsonArea.value); return null; } catch (e) { return "The JSON is not valid"; } }
      for (const f of fields) { const m = f.field.error(); if (m) return `${f.field.label}: ${m}`; }
      return null;
    };
    const show = () => {
      root.replaceChildren();
      if (mode === "json") {
        jsonArea = el("textarea", { rows: 8, spellcheck: "false", "aria-label": "Settings as JSON" }, JSON.stringify(value, null, 2));
        jsonArea.addEventListener("input", () => onchange && onchange());
        root.append(jsonArea);
      } else {
        fields = Object.entries(props).filter(([, s]) => !hidden(s)).sort(([a], [b]) => (required.has(b) ? 1 : 0) - (required.has(a) ? 1 : 0)).map(([k, s]) => { const f = field(k, s, value[k], required.has(k), onchange); f.label = s.title || nice(k); return { key: k, field: f }; });
        root.append(...fields.map((f) => f.field.el));
      }
      if (Object.keys(props).length) {
        const sw = el("button", { type: "button", class: "link" }, mode === "form" ? "Edit as JSON" : "Back to the form");
        sw.addEventListener("click", () => {
          if (mode === "form") { value = formGet(); mode = "json"; } else { if (error()) return; value = get(); mode = "form"; }
          show(); onchange && onchange();
        });
        root.append(sw);
      }
    };
    show();
    return { el: root, get, error };
  }

  root.SettingsForm = { build, field, isRef, isMarker };
})(typeof window !== "undefined" ? window : globalThis);
