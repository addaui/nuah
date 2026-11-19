import { describe, expect, it } from "vitest"
import { translate } from "./i18n"
import { startingLines, type Progress } from "./starting"

const p: Progress = { product: "madar", phase: "upgrading", plugin: "@madar/tasks", name: "Tasks", collection: "tasks.tasks", done: 1500, total: 2000, collection_number: 1, collections: 2 }

describe("the starting view", () => {
  it("says which plugin, which part of it, and how far, in numbers of the person's language", () => {
    const en = startingLines("en", p)
    expect(en.percent).toBe(75)
    expect(translate("en", en.part.key, en.part.params).replace(/[\u2066\u2069]/g, "")).toBe("Tasks: part 1 of 2")
    expect(translate("en", en.count!.key, en.count!.params)).toBe("1,500 of 2,000 items (75%)")
    const ar = startingLines("ar", p)
    expect(translate("ar", ar.count!.key, ar.count!.params)).toContain("١٬٥٠٠")
  })

  it("an empty collection or no progress yet reads as just started", () => {
    expect(startingLines("en", { ...p, done: 0, total: 0 }).percent).toBe(0)
    expect(startingLines("en", { product: "madar" }).part.key).toBe("starting.preparing")
  })

  it("names the step: checking the data first, then saving a copy, then upgrading", () => {
    const check = startingLines("en", { ...p, phase: "checking" })
    expect(translate("en", check.part.key, check.part.params).replace(/[\u2066\u2069]/g, "")).toBe("Checking Tasks before changing anything: part 1 of 2")
    const copy = startingLines("en", { product: "madar", phase: "copying", plugin: "", done: 0, total: 0 })
    expect(translate("en", copy.part.key, copy.part.params)).toBe("Saving a copy of the data first…")
    expect(copy.count).toBeNull()
  })
})
