import { Checkbox } from "@/components/ui/checkbox"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { cn } from "@/lib/utils"
import { useI18n, type Key } from "./i18n"

/** The part of a JSON Schema a settings form draws. */
export interface Prop {
  type?: string | string[]
  enum?: unknown[]
  items?: { enum?: string[]; type?: string }
  title?: string
  description?: string
  minimum?: number
  maximum?: number
  minLength?: number
  maxLength?: number
  default?: unknown
  "x-secret"?: boolean
  "x-first-account"?: string
  "x-wizard-password"?: boolean
}
export interface Schema { properties?: Record<string, Prop>; required?: string[] }

const isMarker = (v: unknown) => !!v && typeof v === "object" && (v as { $secret?: boolean }).$secret === true
const typesOf = (p: Prop) => (Array.isArray(p.type) ? p.type : p.type ? [p.type] : [])
/** A setting the wizard asks in its own way, or one a form cannot draw (objects, lists of objects). */
const own = (p: Prop) => !!p["x-first-account"] || !!p["x-wizard-password"]
const drawable = (p: Prop) => !own(p) && (typesOf(p).every((t) => ["string", "integer", "number", "boolean", "array", "null"].includes(t)) || !!p.enum)

export function fieldsOf(schema: Schema | null | undefined): [string, Prop, boolean][] {
  const required = new Set(schema?.required ?? [])
  return Object.entries(schema?.properties ?? {}).filter(([, p]) => drawable(p)).map(([k, p]) => [k, p, required.has(k)])
}

export interface FormError { key: Key; name: string }

/** The first thing missing or out of bounds, or null: what stops the step. */
export function settingsError(schema: Schema | null | undefined, value: Record<string, unknown> | undefined): FormError | null {
  for (const [k, p, req] of fieldsOf(schema)) {
    const v = value?.[k]
    const empty = v === undefined || v === "" || v === null
    const name = p.title ?? k
    if (req && empty && p.default === undefined) return { key: "field.needed", name }
    if (typeof v === "number" && ((p.minimum !== undefined && v < p.minimum) || (p.maximum !== undefined && v > p.maximum))) return { key: "field.range", name }
    if (typeof v === "string" && v !== "" && p.minLength !== undefined && v.length < p.minLength) return { key: "field.short", name }
  }
  return null
}

/** A settings form drawn from a plugin's schema. Required fields are marked, defaults are shown as placeholders, secrets are never shown again. */
export function SchemaForm({ schema, value, onChange, idPrefix }: { schema: Schema | null | undefined; value: Record<string, unknown> | undefined; onChange: (v: Record<string, unknown>) => void; idPrefix: string }) {
  const { t } = useI18n()
  const fields = fieldsOf(schema)
  const set = (k: string, v: unknown) => {
    const next = { ...(value ?? {}) }
    if (v === undefined || v === "") delete next[k]
    else next[k] = v
    onChange(next)
  }
  if (fields.length === 0) return null
  return (
    <div className="grid gap-3">
      {fields.map(([k, p, req]) => {
        const id = `${idPrefix}-${k}`
        const label = p.title ?? k
        const v = value?.[k]
        const types = typesOf(p)
        const head = (
          <div className="flex items-baseline justify-between gap-2"><Label htmlFor={id}>{label}</Label><span className="text-xs text-muted-foreground">{req ? t("field.required") : t("field.optional")}</span></div>
        )
        const hint = p.description ? <p className="m-0 text-xs text-muted-foreground">{p.description}</p> : null
        if (types.includes("array") && p.items?.enum) {
          const chosen = Array.isArray(v) ? (v as string[]) : []
          return (
            <fieldset key={k} className="m-0 grid content-start gap-1.5 border-0 p-0">
              <legend className="mb-1 text-sm font-medium">{label}</legend>{hint}
              {p.items.enum.map((e) => <label key={e} className="flex cursor-pointer items-center gap-2 text-sm"><Checkbox checked={chosen.includes(e)} onCheckedChange={(on) => set(k, on ? [...chosen, e] : chosen.filter((x) => x !== e))} />{e}</label>)}
            </fieldset>
          )
        }
        if (types.includes("array")) {
          const text = Array.isArray(v) ? (v as unknown[]).join(", ") : ""
          return <div key={k} className="grid content-start gap-1.5">{head}<Input id={id} value={text} onChange={(e) => set(k, e.target.value.split(",").map((x) => x.trim()).filter(Boolean))} placeholder="one, two, three" />{hint}</div>
        }
        if (p.enum) {
          return (
            <div key={k} className="grid content-start gap-1.5">{head}
              <select id={id} value={String(v ?? p.default ?? "")} onChange={(e) => set(k, e.target.value)} className={cn("h-8 rounded-lg border border-input bg-transparent px-2 text-sm outline-none focus-visible:ring-3 focus-visible:ring-ring/50")}>{p.enum.map((e) => <option key={String(e)} value={String(e)}>{String(e)}</option>)}</select>{hint}
            </div>
          )
        }
        if (types.includes("boolean")) {
          return <label key={k} className="flex cursor-pointer items-start gap-2 text-sm"><Checkbox checked={!!(v ?? p.default)} onCheckedChange={(on) => set(k, on === true)} className="mt-0.5" /><span className="grid gap-0.5"><span>{label}</span>{hint}</span></label>
        }
        if (types.includes("integer") || types.includes("number")) {
          return <div key={k} className="grid content-start gap-1.5">{head}<Input id={id} type="number" className="w-32" min={p.minimum} max={p.maximum} value={v === undefined ? "" : String(v)} placeholder={p.default === undefined ? "" : String(p.default)} onChange={(e) => set(k, e.target.value === "" ? undefined : Number(e.target.value))} />{hint}</div>
        }
        const secret = !!p["x-secret"]
        return (
          <div key={k} className="grid content-start gap-1.5">{head}
            <Input id={id} type={secret ? "password" : "text"} autoComplete={secret ? "new-password" : "off"} value={isMarker(v) ? "" : String(v ?? "")} placeholder={isMarker(v) ? t("field.secretSet") : p.default === undefined ? "" : String(p.default)} onChange={(e) => (secret && e.target.value === "" && isMarker(v) ? undefined : set(k, e.target.value))} />{hint}
          </div>
        )
      })}
    </div>
  )
}
