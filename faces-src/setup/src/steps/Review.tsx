import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { TriangleAlert } from "lucide-react"
import { cn } from "@/lib/utils"
import type { Plugin } from "@/api"
import { Pill, POWER_WORDS, type W } from "@/common"
import { reachWords } from "@/Graph"
import { authPlugins, label, fills, needsFirst, OPEN, signKind } from "@/wizard"

const on = (w: W, ps: Plugin[]) => ps.filter((p) => w.c.enabled[p.id])

/** What this service will be allowed to do, in plain words, and anything the Core objects to. */
export function Review(w: W) {
  const { info, c, problems } = w
  const data = on(w, fills(info, "db"))[0]
  const auth = on(w, authPlugins(info))[0]
  const kind = signKind(info, c)
  const signText = kind === "accounts" ? `Accounts${needsFirst(info, c) || c.first.username ? ` · first administrator @${c.first.username}` : ""}` : kind === "none" ? (c.enabled[OPEN] ? "No sign-in, settings password" : "No sign-in") : auth?.name ?? "None"
  const rows: [string, string][] = [["Instance name", c.name], ["Data", data?.name ?? "None"], ["Sign-in", signText], ["Who can open it", reachWords(c.reach)]]
  const enabled = info.plugins.filter((p) => c.enabled[p.id])
  // a plugin with a power worth knowing about gets a row; the rest only keep their own data
  const loud = enabled.filter((p) => p.powers.some((x) => x !== "stores"))
  const quiet = enabled.length - loud.length - (data ? 1 : 0) - (auth && !loud.includes(auth) ? 1 : 0)
  return (
    <div className="grid gap-4">
      {problems.length > 0 && (
        <Alert variant="destructive"><TriangleAlert /><AlertTitle>The Core cannot start this yet</AlertTitle><AlertDescription className="grid gap-1">{problems.map((p, i) => <span key={i}>{p.message}</span>)}</AlertDescription></Alert>
      )}
      <dl className="m-0 grid gap-1.5 text-sm">{rows.map(([k, v]) => <div key={k} className="flex items-center justify-between gap-3"><dt className="text-muted-foreground">{k}</dt><dd className="m-0 font-medium">{v}</dd></div>)}</dl>
      <div className="border-t pt-2">
        <h3 className="m-0 mb-1 text-sm font-medium">What you are allowing</h3>
        <ul className="m-0 grid list-none gap-0 p-0">
          {data && <Row name={label(data)} says={data.description} powers={[]} />}
          {auth && !loud.includes(auth) && <Row name={label(auth)} says={auth.description} powers={[]} />}
          {loud.map((p) => <Row key={p.id} name={label(p)} says={p.powers.filter((x) => x !== "stores").map((x) => POWER_WORDS[x]).join(". ") + "."} powers={p.powers.filter((x) => x !== "stores")} />)}
          {quiet > 0 && <Row name={`${quiet} other plugins`} says="Only keep their own data, and use no more than what you chose above." powers={["stores"]} />}
        </ul>
      </div>
      {info.notice && <p className="m-0 text-xs text-muted-foreground">{info.notice}</p>}
    </div>
  )
}

function Row({ name, says, powers }: { name: string; says: string; powers: ("stores" | "listens" | "reaches out" | "changes settings")[] }) {
  return (
    <li className={cn("grid gap-1 border-t py-2.5 first:border-t-0 sm:grid-cols-[11rem_1fr_auto] sm:items-baseline sm:gap-3")}>
      <span className="text-sm font-medium">{name}</span>
      <span className="text-sm text-muted-foreground">{says}</span>
      <span className="flex flex-wrap gap-1">{powers.map((p) => <Pill key={p} w={p} />)}</span>
    </li>
  )
}
