/** The Upgrade report (ADR 0091): what the server sends (`/upgrade/state`), how each problem is worded, and the report a person copies for the plugin's maker. */
import type { Key } from "./i18n"

export interface UpgradeProblem { plugin: string; collection: string; path: string; code: string; params: Record<string, unknown> }
export interface PluginVersions { name: string; from_release: string | null; to_release: string; from_version: number; to_version: number }
export interface ReportState {
  product: string
  problems: UpgradeProblem[]
  plugins: Record<string, PluginVersions>
  /** For each plugin named: the other plugins that "start without it" also switches off. */
  without: Record<string, string[]>
}

/** Every code the Core's Schema check can give. */
export const CODES = [
  "field_missing", "collection_missing", "type_changed", "limit_tightened", "required_without_default", "name_reused", "earlier_name_twice",
  "earlier_name_dropped", "below_floor", "data_newer", "schema_changed_without_version", "data_without_schema", "interrupted_upgrade",
] as const

export const problemKey = (p: UpgradeProblem): Key => ((CODES as readonly string[]).includes(p.code) ? `upgrade.problem.${p.code}` : "upgrade.problem.other") as Key

/** Where the problem is, as a person reads it: `tasks.tasks › "due"`, or the collection alone. */
export const where = (p: UpgradeProblem) => (p.path ? `${p.collection} › "${p.path}"` : p.collection)

export function problemParams(p: UpgradeProblem): Record<string, string | number> {
  const s = (k: string) => { const v = p.params[k]; return typeof v === "number" || typeof v === "string" ? v : v == null ? "" : JSON.stringify(v) }
  return { at: where(p), code: p.code, from: s("from"), to: s("to"), limit: s("limit"), stored: s("stored"), floor: s("floor"), version: s("version") }
}

/** The copied report: plain English, with everything a plugin maker needs to find the cause. */
export function reportText(s: ReportState): string {
  const lines = [`# Upgrade refused (${s.product})`, ""]
  for (const [id, v] of Object.entries(s.plugins)) {
    lines.push(`## ${v.name} (${id}) ${v.from_release ?? "?"} → ${v.to_release}, data version ${v.from_version} → ${v.to_version}`, "")
    for (const p of s.problems.filter((x) => x.plugin === id)) {
      const params = Object.keys(p.params).length ? ` ${JSON.stringify(p.params)}` : ""
      lines.push(`- ${p.path ? `${p.collection} › ${p.path}` : p.collection || "(plugin)"} — ${p.code}${params}`)
    }
    lines.push("")
  }
  return lines.join("\n")
}

/** One Read declaration of a plugin, as the catalog gives it. */
export interface Read { owner: string | null; collection: string; fields: string[] }

/** The reads of a plugin grouped by the plugin that owns the data (by its name), sorted by that name. */
export function accessGroups(reads: Read[], names: Record<string, string>): { owner: string; items: { collection: string; fields: string[] }[] }[] {
  const groups = new Map<string, { collection: string; fields: string[] }[]>()
  for (const r of reads) {
    const owner = r.owner ? names[r.owner] ?? r.owner : r.collection
    groups.set(owner, [...(groups.get(owner) ?? []), { collection: r.collection, fields: r.fields }])
  }
  return [...groups.entries()].sort(([a], [b]) => a.localeCompare(b)).map(([owner, items]) => ({ owner, items }))
}
