/** The "starting" view (ADR 0091, point 6): what the server sends while it upgrades data (`/upgrade/progress`), and how it reads. */
import type { Key, Lang } from "./i18n"
import { ltr } from "./report"

export interface Progress {
  product: string
  /** `checking` the stored data before changing anything, `copying` the database, then `upgrading`. */
  phase?: "checking" | "copying" | "upgrading"
  plugin?: string
  /** What people call the plugin. */
  name?: string
  collection?: string
  done?: number
  total?: number
  collection_number?: number
  collections?: number
}

type Line = { key: Key; params: Record<string, string | number> }

/** The lines the view shows, with numbers written the way the person's language writes them. */
export function startingLines(lang: Lang, p: Progress): { percent: number; part: Line; count: Line | null } {
  const n = (x: number) => new Intl.NumberFormat(lang === "ar" ? "ar-EG" : "en-US").format(x)
  const done = p.done ?? 0, total = p.total ?? 0
  const percent = total > 0 ? Math.min(100, Math.floor((done / total) * 100)) : 0
  if (p.phase === "copying") return { percent: 0, part: { key: "starting.copying", params: {} }, count: null }
  if (!p.plugin) return { percent, part: { key: "starting.preparing", params: {} }, count: null }
  return {
    percent,
    part: { key: p.phase === "checking" ? "starting.checking" : "starting.part", params: { name: ltr(p.name ?? p.plugin), n: n(p.collection_number ?? 1), m: n(p.collections ?? 1) } },
    count: { key: "starting.count", params: { done: n(done), total: n(total), percent: n(percent) } },
  }
}
