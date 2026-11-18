import { describe, expect, it } from "vitest"
import { translate } from "./i18n"
import { CODES, problemKey, problemParams, reportText, type ReportState } from "./report"

const state: ReportState = {
  product: "madar",
  problems: [
    { plugin: "@madar/tasks", collection: "tasks.tasks", path: "level", code: "field_missing", params: {} },
    { plugin: "@madar/tasks", collection: "tasks.tasks", path: "due", code: "type_changed", params: { from: "string", to: "integer" } },
  ],
  plugins: { "@madar/tasks": { name: "Tasks", from_release: "0.3.0", to_release: "0.4.0", from_version: 1, to_version: 2 } },
  without: { "@madar/tasks": ["@madar/export"] },
}

describe("the upgrade report", () => {
  it("has a sentence for every problem code, in English and Arabic, naming where", () => {
    for (const code of CODES) {
      const p = { plugin: "@t/p", collection: "t.docs", path: "a.b", code, params: { from: "string", to: "integer", limit: "maxLength", stored: 1, floor: 3, version: 2, to_version: 4 } }
      for (const lang of ["en", "ar"] as const) {
        const text = translate(lang, problemKey(p), problemParams(p))
        expect(text, `${lang} ${code}`).not.toBe(problemKey(p))
        expect(text, `${lang} ${code}`).not.toMatch(/\{\w+\}/)
      }
    }
  })

  it("an unknown code still reads as something", () => {
    expect(problemKey({ plugin: "p", collection: "c", path: "", code: "something_new", params: {} })).toBe("upgrade.problem.other")
  })

  it("the copied report names the plugin, both versions, every path and code, and what to do", () => {
    const text = reportText(state)
    for (const want of ["@madar/tasks", "0.3.0", "0.4.0", "data version 1 → 2", "tasks.tasks › level", "field_missing", "tasks.tasks › due", "type_changed", "string", "integer"]) {
      expect(text).toContain(want)
    }
  })
})

describe("what a plugin will have access to", () => {
  it("groups the fields a plugin reads by the plugin that owns them, named as people see them", async () => {
    const { accessGroups } = await import("./report")
    const reads = [
      { owner: "@madar/pages", collection: "pages.pages", fields: ["title", "body"] },
      { owner: "@madar/pages", collection: "pages.versions", fields: ["at"] },
      { owner: "@madar/files", collection: "files.files", fields: ["name"] },
      { owner: null, collection: "x.y", fields: ["z"] },
    ]
    const names = { "@madar/pages": "Pages", "@madar/files": "Files" }
    expect(accessGroups(reads, names)).toEqual([
      { owner: "Files", items: [{ collection: "files.files", fields: ["name"] }] },
      { owner: "Pages", items: [{ collection: "pages.pages", fields: ["title", "body"] }, { collection: "pages.versions", fields: ["at"] }] },
      { owner: "x.y", items: [{ collection: "x.y", fields: ["z"] }] },
    ])
  })
})

describe("the report in a right-to-left language", () => {
  it("keeps a Latin name (collection, field, plugin id) in its own left-to-right run", () => {
    const p = { plugin: "@t/p", collection: "tasks.tasks", path: "position", code: "field_missing", params: {} }
    const at = String(problemParams(p).at)
    expect(at.startsWith("⁦") && at.endsWith("⁩")).toBe(true)
  })
})

describe("the report's title", () => {
  it("names the releases when they differ, and the data versions when the release is the same", async () => {
    const { titleOf } = await import("./report")
    expect(titleOf(state)).toEqual({ key: "upgrade.title", params: { plugin: "Tasks", id: "⁦@madar/tasks⁩", from: "0.3.0", to: "0.4.0" } })
    const same = { ...state, plugins: { "@madar/tasks": { ...state.plugins["@madar/tasks"], from_release: "0.4.0" } } }
    expect(titleOf(same)).toEqual({ key: "upgrade.titleData", params: { plugin: "Tasks", id: "⁦@madar/tasks⁩", from: 1, to: 2 } })
    expect(titleOf({ ...state, plugins: { ...state.plugins, "@x/y": state.plugins["@madar/tasks"] } }).key).toBe("upgrade.titleMany")
  })
})
