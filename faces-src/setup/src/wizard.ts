import type { Entry, Info, Plugin, Reach } from "./api"
import type { Key } from "./i18n"
import { settingsError } from "./schema"

/** Everything the person chose, kept apart from what the server sent. The draft sent to the Core is built from both. */
export interface Choices {
  name: string
  enabled: Record<string, boolean>
  settings: Record<string, Record<string, unknown>>
  bindings: Record<string, string>
  first: { username: string; password: string }
  settingsPassword: string
  reach: Reach
  accepted: boolean
}

export const USERNAME = /^[A-Za-z0-9._-]{2,32}$/
export const MIN_PASSWORD = 8
/** The no-sign-in plugins: with a settings password, and without. */
export const OPEN = "@nuah/auth-open"
export const LOCAL = "@nuah/auth-local"
export const REACHES: Reach[] = ["this-computer", "network", "internet"]
const MANY = new Set(["log", "trace", "metric", "heartbeat"])
const ALWAYS = new Set(["@nuah/console", "@nuah/setup-web"])

export const rank = (r: Reach) => REACHES.indexOf(r)
export const byId = (info: Info, id: string) => info.plugins.find((p) => p.id === id)
export const fills = (info: Info, port: string) => info.plugins.filter((p) => p.fills === port)
/** The name a person reads: the manifest's, or the id made readable when the manifest has none (`@nuah/clock-system` is "Clock system"). */
export const label = (p: Plugin) => (p.name && p.name !== p.id ? p.name : capital(p.id.slice(p.id.indexOf("/") + 1).replace(/[-_]+/g, " ")))
/** The first sentence of a description: enough for a table row; the whole of it is in the panel. */
export const brief = (d: string) => { const first = d.split(/(?<=[.!?])\s/)[0] ?? d; return first.length > 130 ? first.slice(0, 127) + "…" : first }
const capital = (s: string) => (s ? s[0].toUpperCase() + s.slice(1) : s)

export function initial(info: Info): Choices {
  const prof = info.profile
  const enabled: Record<string, boolean> = {}
  const settings: Record<string, Record<string, unknown>> = {}
  for (const p of info.plugins) {
    const e: Entry | undefined = prof.plugins?.[p.id]
    enabled[p.id] = !!e?.enabled
    if (e?.settings) settings[p.id] = { ...e.settings }
  }
  return {
    // a first run starts from the product's own name, not the file's
    name: info.file_exists ? prof.name : capital(info.distribution),
    enabled,
    settings,
    bindings: { ...(prof.bindings ?? {}) },
    first: { username: "", password: "" },
    settingsPassword: "",
    reach: prof.reach ?? info.reach_default ?? "this-computer",
    accepted: (prof.accepted_risks ?? []).length > 0,
  }
}

// ---- sign-in: the two kinds that exist, plus any other the product offers -------------------------

export const isAccounts = (p: Plugin) => !!p.settings_schema?.properties?.accounts?.["x-first-account"]
export const authPlugins = (info: Info) => fills(info, "auth")
export type SignKind = "accounts" | "none" | "other"

export function signKind(info: Info, c: Choices): SignKind | null {
  const on = authPlugins(info).find((p) => c.enabled[p.id])
  if (!on) return null
  return isAccounts(on) ? "accounts" : on.id === OPEN || on.id === LOCAL ? "none" : "other"
}

export function chooseAuth(info: Info, c: Choices, id: string): Choices {
  const enabled = { ...c.enabled }
  for (const p of authPlugins(info)) enabled[p.id] = p.id === id
  return { ...c, enabled, accepted: false }
}

// ---- what is on ------------------------------------------------------------------------------------

/** A plugin the person cannot switch: it carries the Link, it is the wizard or the console, or it is the only choice for its port. */
export function locked(info: Info, p: Plugin): boolean {
  if (p.role === "projection" || ALWAYS.has(p.id)) return true
  return p.role === "adapter" && !!p.fills && !MANY.has(p.fills) && fills(info, p.fills).length === 1
}

/** An adapter chosen on an earlier step (data, sign-in): shown, changed there. */
export const chosenElsewhere = (p: Plugin) => p.role === "adapter" && (p.fills === "db" || p.fills === "auth")

export function toggle(info: Info, c: Choices, p: Plugin, on: boolean): Choices {
  const enabled = { ...c.enabled }
  const turnOn = (q: Plugin) => {
    if (enabled[q.id]) return
    enabled[q.id] = true
    if (q.fills && !MANY.has(q.fills)) for (const o of fills(info, q.fills)) if (o.id !== q.id) enabled[o.id] = false
    for (const need of q.requires.plugins) { const n = byId(info, need); if (n) turnOn(n) }
  }
  const turnOff = (q: Plugin) => {
    if (!enabled[q.id]) return
    enabled[q.id] = false
    for (const o of info.plugins) if (enabled[o.id] && o.requires.plugins.includes(q.id)) turnOff(o)
  }
  if (on) turnOn(p)
  else if (!(p.fills && !MANY.has(p.fills))) turnOff(p)
  return { ...c, enabled }
}

/** Plugins that go off with this one. */
export const dependents = (info: Info, c: Choices, p: Plugin) => info.plugins.filter((o) => c.enabled[o.id] && o.requires.plugins.includes(p.id))

// ---- who can reach it ------------------------------------------------------------------------------

/** Enabled plugins that are safe only for less than the chosen reach. */
export function risky(info: Info, c: Choices): Plugin[] {
  return info.plugins.filter((p) => c.enabled[p.id] && p.safe_reach && rank(c.reach) > rank(p.safe_reach))
}

// ---- passwords and the first account ---------------------------------------------------------------

export function firstAccountPlugin(info: Info, c: Choices): Plugin | undefined {
  return info.plugins.find((p) => c.enabled[p.id] && isAccounts(p))
}
const hasStored = (v: unknown) => (typeof v === "string" && v !== "") || (!!v && typeof v === "object")
/** An accounts plugin with no account saved yet asks for the first administrator. */
export function needsFirst(info: Info, c: Choices): boolean {
  const p = firstAccountPlugin(info, c)
  if (!p) return false
  const a = c.settings[p.id]?.accounts
  return !(Array.isArray(a) ? a.length > 0 : hasStored(a))
}
/** The plugin and setting that hold a password the person picks for the settings area, when it has none yet. */
export function settingsPasswordSlot(info: Info, c: Choices): { plugin: Plugin; key: string } | null {
  for (const p of info.plugins) {
    if (!c.enabled[p.id]) continue
    const key = Object.entries(p.settings_schema?.properties ?? {}).find(([, v]) => v["x-wizard-password"])?.[0]
    if (key && !hasStored(c.settings[p.id]?.[key])) return { plugin: p, key }
  }
  return null
}

// ---- the draft the Core judges ---------------------------------------------------------------------

export function draft(info: Info, c: Choices): Record<string, unknown> {
  const plugins: Record<string, Entry> = {}
  for (const p of info.plugins) {
    const orig = info.profile.plugins?.[p.id]
    const on = !!c.enabled[p.id]
    const settings = c.settings[p.id] ?? orig?.settings
    // switching a plugin on is the operator's yes to the capabilities it asks for
    const grants = new Set(orig?.grants ?? [])
    if (on) for (const cap of p.requires.capabilities) grants.add(cap)
    const e: Entry = { enabled: on }
    if (grants.size) e.grants = [...grants]
    if (settings && Object.keys(settings).length) e.settings = settings
    if (on || e.grants || e.settings) plugins[p.id] = e
  }
  const bindings: Record<string, string> = {}
  for (const [port, id] of Object.entries(c.bindings)) if (c.enabled[id]) bindings[port] = id
  const body: Record<string, unknown> = { ...info.profile, name: c.name, plugins, bindings, reach: c.reach, accepted_risks: c.accepted ? risky(info, c).map((p) => p.id) : [] }
  if (needsFirst(info, c)) body.first_account = { ...c.first }
  if (settingsPasswordSlot(info, c)) body.settings_password = c.settingsPassword
  return body
}

// ---- why a step cannot go on yet -------------------------------------------------------------------

export type StepId = "instance" | "data" | "signin" | "access" | "plugins" | "review"

export interface StepMessage { key: Key; params?: Record<string, string | number> }

export function stepError(info: Info, c: Choices, id: StepId, serverProblems: number): StepMessage | null {
  const form = (ps: Plugin[]): StepMessage | null => { for (const p of ps) if (c.enabled[p.id]) { const e = settingsError(p.settings_schema, c.settings[p.id]); if (e) return { key: e.key, params: { name: `${label(p)}: ${e.name}` } } } return null }
  switch (id) {
    case "instance": return c.name.trim() ? null : { key: "instance.missing" }
    case "data": return !fills(info, "db").some((p) => c.enabled[p.id]) ? { key: "data.choose" } : form(fills(info, "db"))
    case "signin": {
      if (!authPlugins(info).some((p) => c.enabled[p.id])) return { key: "signin.choose" }
      const e = form(authPlugins(info)); if (e) return e
      if (needsFirst(info, c)) { if (!USERNAME.test(c.first.username)) return { key: "signin.badUsername" }; if (c.first.password.length < MIN_PASSWORD) return { key: "signin.shortPassword" } }
      if (settingsPasswordSlot(info, c) && c.settingsPassword.length < MIN_PASSWORD) return { key: "signin.shortSettingsPassword" }
      return null
    }
    case "access": return risky(info, c).length > 0 && !c.accepted ? { key: "risk.stop" } : null
    case "plugins": return form(info.plugins.filter((p) => !fills(info, "db").includes(p) && !fills(info, "auth").includes(p)))
    case "review": return serverProblems > 0 ? { key: "review.fixFirst" } : null
  }
}
