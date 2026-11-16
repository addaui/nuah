import { Fragment, useState } from "react"
import { Link2, Lock, Search, Settings2 } from "lucide-react"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Switch } from "@/components/ui/switch"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { cn } from "@/lib/utils"
import type { Plugin, Power, Role } from "@/api"
import { Pill, POWER_WORDS, SHORT, type W } from "@/common"
import { fieldsOf, SchemaForm, settingsError } from "@/schema"
import { brief, byId, label, chosenElsewhere, dependents, locked, toggle } from "@/wizard"

type Filter = "all" | "on" | "off"
const FILTERS: [Filter, string][] = [["all", "All"], ["on", "On"], ["off", "Off"]]
const KINDS: { id: Role; title: string; note: string }[] = [
  { id: "adapter", title: "Adapters", note: "A port is a fixed slot in Nuah; an adapter fills it." },
  { id: "projection", title: "Projections", note: "Carry Nuah's Link over a transport." },
  { id: "face", title: "Faces", note: "What people and programs use." },
  { id: "provider", title: "Providers", note: "Fill a slot another plugin offers." },
  { id: "feature", title: "Features", note: "Add what the service can do." },
  { id: "theme", title: "Themes", note: "Only data: colours and looks." },
]

/** What the plugin is to the rest: the port it fills, what it provides or needs, what it belongs to. */
function relations(w: W, p: Plugin): string[] {
  return [
    p.fills && `fills ${p.fills}`,
    ...p.provides.map((x) => `provides ${x}`),
    ...p.requires.extension_points.map((x) => `needs ${x}`),
    ...p.requires.plugins.map((id) => `for ${byId(w.info, id)?.name ?? id}`),
  ].filter(Boolean) as string[]
}

/** Said once: what the pills and the gear mean, so no plugin has to explain them. */
function Legend() {
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-muted-foreground">
      {(Object.keys(SHORT) as Power[]).map((x) => <span key={x} className="flex items-center gap-1"><Pill w={x} />{SHORT[x]}</span>)}
      <span className="flex items-center gap-1"><Settings2 className="size-3.5" />settings</span>
      <span className="flex items-center gap-1"><span className="rounded border border-amber-500/50 px-1 text-[10px] font-medium text-amber-600">Set up</span>needs a setting</span>
    </div>
  )
}

function Inspector({ w, p, onPick }: { w: W; p: Plugin | null; onPick: (id: string) => void }) {
  const { info, c, update } = w
  const [link, setLink] = useState("")
  if (!p) {
    return (
      <div className="grid content-start gap-4">
        <section className="grid gap-2">
          <h3 className="m-0 text-xs font-medium uppercase tracking-wide text-muted-foreground">Add your own</h3>
          <div className="flex gap-2"><Input value={link} onChange={(e) => setLink(e.target.value)} placeholder="Link to a plugin or its repository" aria-label="Link to a plugin" disabled /><Button variant="outline" disabled><Link2 />Add</Button></div>
          <p className="m-0 text-xs text-muted-foreground">Plugins are built into this program for now, so the list is what it was built with. Adding one from a link or a file will come later.</p>
        </section>
        <p className="m-0 text-xs text-muted-foreground">Select a row to see a plugin's details and settings.</p>
      </div>
    )
  }
  const own = w.problems.filter((x) => x.plugins.includes(p.id))
  const providers = info.plugins.filter((x) => p.requires.extension_points.some((e) => x.provides.includes(e)))
  const users = info.plugins.filter((x) => x.requires.extension_points.some((e) => p.provides.includes(e)))
  return (
    <div className="grid content-start gap-4">
      <div><h3 className="m-0 text-base font-semibold">{label(p)}</h3><p className="m-0 font-mono text-[11px] text-muted-foreground">{p.id}</p><p className="mt-1 mb-0 text-sm text-muted-foreground">{p.description}</p></div>
      {locked(info, p) && <p className="m-0 flex items-center gap-1.5 text-xs text-muted-foreground"><Lock className="size-3" />Included with Nuah. It keeps the service running, so it stays on.</p>}
      {chosenElsewhere(p) && <p className="m-0 text-xs text-muted-foreground">Chosen in the {p.fills === "db" ? "Data" : "Sign-in"} step.</p>}
      {p.fills === "net" && <p className="m-0 rounded-md border border-amber-500/40 bg-amber-500/5 p-2 text-xs text-amber-700 dark:text-amber-300">This protects only the plugins that use it. It cannot stop a plugin from reaching the internet some other way.</p>}
      {own.length > 0 && <div role="alert" className="grid gap-1 rounded-md border border-destructive/40 bg-destructive/5 p-2 text-xs text-destructive">{own.map((x, i) => <span key={i}>{x.message}</span>)}</div>}
      <section className="grid gap-1.5">
        <h4 className="m-0 text-xs font-medium uppercase tracking-wide text-muted-foreground">What it may do</h4>
        {p.powers.length === 0 ? <p className="m-0 text-sm text-muted-foreground">Nothing beyond what it is for.</p> : p.powers.map((x) => <p key={x} className="m-0 flex items-start gap-2 text-sm"><span className="mt-0.5 shrink-0"><Pill w={x} /></span>{POWER_WORDS[x]}</p>)}
      </section>
      {p.requires.extension_points.length > 0 && (
        <section className="grid gap-1.5">
          <h4 className="m-0 text-xs font-medium uppercase tracking-wide text-muted-foreground">Needs {p.requires.extension_points.join(", ")}</h4>
          {providers.length === 0 ? <p className="m-0 text-xs text-destructive">No plugin in this program provides it.</p> : providers.map((x) => <button key={x.id} type="button" onClick={() => onPick(x.id)} className="flex items-center justify-between rounded-md border px-2 py-1.5 text-start text-sm outline-none hover:bg-muted/50 focus-visible:ring-2 focus-visible:ring-ring">{label(x)}<span className={cn("text-xs", c.enabled[x.id] ? "text-emerald-600" : "text-muted-foreground")}>{c.enabled[x.id] ? "On" : "Off"}</span></button>)}
        </section>
      )}
      {users.length > 0 && <p className="m-0 text-xs text-muted-foreground">Provides {p.provides.join(", ")} to {users.map((x) => label(x)).join(", ")}.</p>}
      {fieldsOf(p.settings_schema).length > 0 && (
        <section className="grid gap-3">
          <h4 className="m-0 text-xs font-medium uppercase tracking-wide text-muted-foreground">Settings</h4>
          {!c.enabled[p.id] && <p className="m-0 text-xs text-muted-foreground">Switch it on to use these.</p>}
          <SchemaForm schema={p.settings_schema} value={c.settings[p.id]} idPrefix={p.id} onChange={(v) => update((x) => ({ ...x, settings: { ...x.settings, [p.id]: v } }))} />
        </section>
      )}
      <Button variant="ghost" className="justify-self-start" onClick={() => onPick("")}>Back to the list</Button>
    </div>
  )
}

/** Plugins are Nuah's settings for what is on: a table by kind of plugin, and a panel for the one you picked. */
export function Plugins(w: W) {
  const { info, c, update, tried } = w
  const [picked, setPicked] = useState("")
  const [q, setQ] = useState("")
  const [filter, setFilter] = useState<Filter>("all")
  const [notice, setNotice] = useState("")
  const sel = picked ? byId(info, picked) ?? null : null
  const visible = (p: Plugin) => (!q || `${label(p)} ${p.id} ${p.description}`.toLowerCase().includes(q.toLowerCase())) && (filter === "all" || (filter === "on") === !!c.enabled[p.id])
  const shown = info.plugins.filter(visible)
  const needs = (p: Plugin) => !!c.enabled[p.id] && !!settingsError(p.settings_schema, c.settings[p.id])
  const flip = (p: Plugin, on: boolean) => {
    const gone = on ? [] : dependents(info, c, p)
    update((x) => toggle(info, x, p, on))
    setNotice(gone.length ? `Also switched off: ${gone.map((g) => label(g)).join(", ")}, because they need ${label(p)}.` : "")
    if (on && settingsError(p.settings_schema, c.settings[p.id])) setPicked(p.id)
  }
  return (
    <div className="grid h-full gap-4 lg:grid-cols-[minmax(0,1fr)_19rem]">
      <div className="grid min-h-0 grid-rows-[auto_auto_minmax(0,1fr)_auto] gap-3">
        <div className="flex flex-wrap items-center gap-2">
          <div className="relative min-w-40 flex-1"><Search className="pointer-events-none absolute start-2.5 top-2 size-3.5 text-muted-foreground" /><Input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search plugins" aria-label="Search plugins" className="ps-8" /></div>
          <div className="flex gap-1" role="group" aria-label="Filter">{FILTERS.map(([id, label]) => <Button key={id} size="sm" variant={filter === id ? "secondary" : "ghost"} aria-pressed={filter === id} onClick={() => setFilter(id)}>{label}</Button>)}</div>
        </div>
        <Legend />
        <ScrollArea className="min-h-0 rounded-lg border">
          <Table className="table-fixed [&_td]:whitespace-normal [&_th]:whitespace-normal">
            <TableHeader><TableRow><TableHead className="w-14" /><TableHead>Plugin</TableHead><TableHead className="hidden w-[28%] md:table-cell">What it is to the rest</TableHead><TableHead className="hidden w-[18%] lg:table-cell">May</TableHead><TableHead className="w-24 text-end">Settings</TableHead></TableRow></TableHeader>
            <TableBody>
              {KINDS.map((k) => {
                const all = info.plugins.filter((p) => p.role === k.id)
                const rows = shown.filter((p) => p.role === k.id)
                if (rows.length === 0) return null
                return (
                  <Fragment key={k.id}>
                    <TableRow className="bg-muted/40 hover:bg-muted/40"><TableCell colSpan={5} className="py-1.5"><div className="flex items-baseline justify-between gap-3"><span><b className="text-xs font-semibold">{k.title}</b> <span className="text-xs text-muted-foreground">{k.note}</span></span><span className="shrink-0 text-xs text-muted-foreground">{all.filter((p) => c.enabled[p.id]).length} of {all.length} on</span></div></TableCell></TableRow>
                    {rows.map((p) => {
                      const on = !!c.enabled[p.id]
                      const rel = relations(w, p)
                      const hasForm = fieldsOf(p.settings_schema).length > 0
                      return (
                        <TableRow key={p.id} data-state={picked === p.id ? "selected" : undefined} className="cursor-pointer" onClick={() => setPicked(p.id)}>
                          <TableCell onClick={(e) => e.stopPropagation()}>
                            {locked(info, p) ? <Lock className="size-4 text-muted-foreground" aria-label="Included" /> : chosenElsewhere(p) ? <Switch checked={on} disabled aria-label={`${label(p)} (chosen in an earlier step)`} /> : <Switch checked={on} onCheckedChange={(v) => flip(p, v)} aria-label={label(p)} />}
                          </TableCell>
                          <TableCell className="whitespace-normal"><button type="button" onClick={() => setPicked(p.id)} className={cn("grid text-start outline-none focus-visible:underline", !on && !locked(info, p) && "text-muted-foreground")}><span className="text-sm font-medium">{label(p)}</span><span className="text-xs text-muted-foreground">{brief(p.description)}</span></button></TableCell>
                          <TableCell className="hidden whitespace-normal md:table-cell"><span className="flex flex-wrap gap-1">{rel.length ? rel.map((r) => <Badge key={r} variant="outline" className="h-4 px-1.5 text-[10px]">{r}</Badge>) : <span className="text-muted-foreground/50">—</span>}</span></TableCell>
                          <TableCell className="hidden lg:table-cell"><span className="flex flex-wrap gap-1">{p.powers.length ? p.powers.map((x) => <Pill key={x} w={x} />) : <span className="text-muted-foreground/50">—</span>}</span></TableCell>
                          <TableCell className="text-end" onClick={(e) => e.stopPropagation()}>
                            {!hasForm ? <span className="text-muted-foreground/50">—</span> : needs(p) ? <Button size="xs" variant="outline" className="border-amber-500/50 text-amber-600" onClick={() => setPicked(p.id)}>Set up</Button> : <Button size="icon-sm" variant="ghost" aria-label={`Settings of ${label(p)}`} onClick={() => setPicked(p.id)}><Settings2 /></Button>}
                          </TableCell>
                        </TableRow>
                      )
                    })}
                  </Fragment>
                )
              })}
            </TableBody>
          </Table>
          {shown.length === 0 && <p className="m-0 p-4 text-sm text-muted-foreground">No plugin matches.</p>}
        </ScrollArea>
        <div className="min-h-4 text-sm">{notice && <p role="status" className="m-0 text-xs text-muted-foreground">{notice}</p>}{tried && info.plugins.some(needs) && <p role="alert" className="m-0 text-sm text-destructive">{info.plugins.filter(needs).map((p) => label(p)).join(" and ")} still need a setting.</p>}</div>
      </div>
      <aside className="min-h-0 overflow-y-auto rounded-lg border bg-muted/20 p-3"><Inspector w={w} p={sel} onPick={setPicked} /></aside>
    </div>
  )
}
