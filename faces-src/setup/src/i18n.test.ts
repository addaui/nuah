import { readdirSync, readFileSync, statSync } from "node:fs"
import { join } from "node:path"
import { describe, expect, it } from "vitest"
import { ar } from "./locales/ar"
import { en } from "./locales/en"
import { translate } from "./i18n"

const files = (dir: string): string[] => readdirSync(dir).flatMap((f) => (statSync(join(dir, f)).isDirectory() ? (f === "locales" || f === "components" ? [] : files(join(dir, f))) : /\.(tsx?)$/.test(f) && !f.includes(".test.") ? [join(dir, f)] : []))

describe("translations", () => {
  it("has every key in both languages, none empty", () => {
    expect(Object.keys(ar).sort()).toEqual(Object.keys(en).sort())
    for (const [k, v] of Object.entries({ ...ar })) expect(v.trim(), k).not.toBe("")
  })

  it("keeps the {placeholders} of the English text in the Arabic text", () => {
    const holes = (s: string) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort()
    for (const k of Object.keys(en) as (keyof typeof en)[]) expect(holes(ar[k]), k).toEqual(holes(en[k]))
  })

  it("fills the placeholders, and falls back to English for a key a language lacks", () => {
    expect(translate("en", "nav.setUp", { product: "Madar" })).toBe("Set up Madar")
    expect(translate("ar", "nav.setUp", { product: "Madar" })).toContain("Madar")
    expect(translate("ar", "no.such.key")).toBe("no.such.key")
  })

  it("uses only keys that exist: every t(\"key\") in the source is in the English file", () => {
    const used = new Set<string>()
    for (const f of files("src")) for (const m of readFileSync(f, "utf8").matchAll(/\bt\("([a-zA-Z.-]+)"/g)) used.add(m[1])
    const missing = [...used].filter((k) => !(k in en))
    expect(missing).toEqual([])
  })

  it("uses logical properties only, so the page turns with the language", () => {
    const physical = /(?:^|[\s"'`:])(?:-?(?:ml|mr|pl|pr)-\d|text-(?:left|right)|(?:left|right)-\d|rounded-(?:l|r)-|border-(?:l|r)(?:-|\s|")|float-(?:left|right))/
    for (const f of files("src").filter((x) => !x.includes("lib/") && !x.includes("StepBar"))) {
      const bad = readFileSync(f, "utf8").split("\n").filter((l) => physical.test(l) && !l.includes("//"))
      expect(bad, f).toEqual([])
    }
  })
})
