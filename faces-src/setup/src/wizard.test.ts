import { describe, expect, it } from "vitest"
import type { Info, Plugin } from "./api"
import { chooseAuth, draft, initial, risky, settingsPasswordSlot, stepError, toggle } from "./wizard"

const plugin = (p: Partial<Plugin> & { id: string }): Plugin => ({ scope: "nuah", version: "1", name: p.id, description: "", role: "feature", powers: [], face: null, safe_reach: null, class: { class: "feature", reasons: [] }, fills: null, provides: [], requires: { plugins: [], ports: [], extension_points: [], capabilities: [] }, settings_schema: null, ...p })
const info = (plugins: Plugin[], enabled: string[] = []): Info => ({ distribution: "madar", profile: { name: "server", plugins: Object.fromEntries(enabled.map((id) => [id, { enabled: true }])) }, core_ports: [], plugins, problems: [], path: "nuah.json", file_exists: false })
const accounts = plugin({ id: "@nuah/auth-password", role: "adapter", fills: "auth", settings_schema: { properties: { accounts: { "x-first-account": "password-hash" } } } })
const open = plugin({ id: "@nuah/auth-open", role: "adapter", fills: "auth", safe_reach: "this-computer", settings_schema: { properties: { admin_password_hash: { "x-wizard-password": true } } } })
const local = plugin({ id: "@nuah/auth-local", role: "adapter", fills: "auth", safe_reach: "this-computer" })

describe("the draft the Core judges", () => {
  it("starts a first run with the product's name and what the starting profile turns on", () => {
    const i = info([accounts, open], ["@nuah/auth-password"])
    const c = initial(i)
    expect(c.name).toBe("Madar")
    expect(c.enabled).toEqual({ "@nuah/auth-password": true, "@nuah/auth-open": false })
  })

  it("asks for the first administrator only while no account is saved, and sends it with the draft", () => {
    const i = info([accounts, open], ["@nuah/auth-password"])
    const c = { ...initial(i), first: { username: "ana", password: "long-enough-1" } }
    expect(draft(i, c).first_account).toEqual({ username: "ana", password: "long-enough-1" })
    const saved = { ...c, settings: { "@nuah/auth-password": { accounts: { $secret: true } } } }
    expect(draft(i, saved).first_account).toBeUndefined()
  })

  it("asks for a settings password for the plugin that has one to pick, and not for the others", () => {
    const i = info([accounts, open, local], ["@nuah/auth-open"])
    const c = { ...initial(i), settingsPassword: "long enough pw" }
    expect(settingsPasswordSlot(i, c)?.key).toBe("admin_password_hash")
    expect(draft(i, c).settings_password).toBe("long enough pw")
    expect(settingsPasswordSlot(i, chooseAuth(i, c, "@nuah/auth-local"))).toBeNull()
  })

  it("sends who can reach it and the risks the person accepted, only for plugins that are safe for less", () => {
    const i = info([accounts, open], ["@nuah/auth-open"])
    let c = { ...initial(i), reach: "network" as const }
    expect(risky(i, c).map((p) => p.id)).toEqual(["@nuah/auth-open"])
    expect(draft(i, c).accepted_risks).toEqual([])
    expect(stepError(i, c, "access", 0)).toMatch(/Tick the box/)
    c = { ...c, accepted: true }
    expect(draft(i, c).accepted_risks).toEqual(["@nuah/auth-open"])
    expect(stepError(i, c, "access", 0)).toBeNull()
    // accounts are safe on any network: nothing to accept
    expect(risky(i, chooseAuth(i, c, "@nuah/auth-password"))).toEqual([])
  })

  it("keeps one adapter per single port, and turns off what needs a plugin that goes off", () => {
    const sqlite = plugin({ id: "@nuah/db-sqlite", role: "adapter", fills: "db" })
    const memory = plugin({ id: "@nuah/db-memory", role: "adapter", fills: "db" })
    const pages = plugin({ id: "@madar/pages" })
    const editor = plugin({ id: "@madar/editor", requires: { plugins: ["@madar/pages"], ports: [], extension_points: [], capabilities: [] } })
    const i = info([sqlite, memory, pages, editor], ["@nuah/db-sqlite", "@madar/pages", "@madar/editor"])
    let c = toggle(i, initial(i), memory, true)
    expect(c.enabled["@nuah/db-sqlite"]).toBe(false)
    expect(c.enabled["@nuah/db-memory"]).toBe(true)
    c = toggle(i, c, memory, false)
    expect(c.enabled["@nuah/db-memory"]).toBe(true)
    c = toggle(i, c, pages, false)
    expect(c.enabled["@madar/editor"]).toBe(false)
    c = toggle(i, c, editor, true)
    expect(c.enabled["@madar/pages"]).toBe(true)
  })

  it("grants what an enabled plugin asks for and nothing for one that is off", () => {
    const net = plugin({ id: "@x/fetchy", requires: { plugins: [], ports: [], extension_points: [], capabilities: ["net"] } })
    const i = info([net], ["@x/fetchy"])
    expect(((draft(i, initial(i)).plugins as Record<string, { grants?: string[] }>)["@x/fetchy"]).grants).toEqual(["net"])
    const off = info([net], [])
    expect((draft(off, initial(off)).plugins as Record<string, unknown>)["@x/fetchy"]).toBeUndefined()
  })
})
