import { useEffect, useRef, useState } from "react"
import { Check, LoaderCircle, TriangleAlert } from "lucide-react"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import { Progress } from "@/components/ui/progress"
import { cn } from "@/lib/utils"
import { send, type Problem } from "./api"
import type { W } from "./common"
import { useI18n, type Key } from "./i18n"
import { draft } from "./wizard"

const STAGES: Key[] = ["apply.saving", "apply.starting", "apply.checking", "apply.opening"]
const WAIT_MS = 45_000

type Phase = { at: "saving" } | { at: "starting"; since: number } | { at: "rejected"; problems: Problem[] } | { at: "silent"; path: string }

/**
 * Saves the draft, then waits for the service the setup hands over to and goes there when it answers. It says what is going on while
 * it waits, and says plainly when the Core refused the draft or nothing came up, instead of asking the person to reload.
 */
export function Apply({ info, c, onBack }: Pick<W, "info" | "c"> & { onBack: () => void }) {
  const { t } = useI18n()
  const [phase, setPhase] = useState<Phase>({ at: "saving" })
  const [stage, setStage] = useState(0)
  const [run, setRun] = useState(0)
  const started = useRef(-1)
  // the text of the fallback message is read when it is needed, so changing the language while it waits does not restart anything
  const tRef = useRef(t)
  useEffect(() => { tRef.current = t })

  useEffect(() => {
    if (started.current === run) return
    started.current = run
    let stop = false
    ;(async () => {
      const { status, body } = await send<{ ok?: boolean; problems?: Problem[]; saved?: string; error?: { message: string } }>("/setup/save", draft(info, c))
      if (stop) return
      if (status !== 200 || !body.ok) {
        setPhase({ at: "rejected", problems: body.problems ?? [{ code: "invalid_profile", plugins: [], message: body.error?.message ?? tRef.current("apply.couldNot") }] })
        return
      }
      const t0 = Date.now()
      setStage(1)
      setPhase({ at: "starting", since: t0 })
      // the setup server stops and the service starts on the same address: wait until it answers
      while (!stop && Date.now() - t0 < WAIT_MS) {
        await new Promise((r) => setTimeout(r, 700))
        try {
          const r = await fetch("/readyz", { cache: "no-store" })
          if (r.ok) { setStage(2); await new Promise((x) => setTimeout(x, 500)); setStage(3); await new Promise((x) => setTimeout(x, 600)); if (!stop) location.assign("/"); return }
        } catch { /* still starting */ }
      }
      if (!stop) setPhase({ at: "silent", path: body.saved ?? info.path })
    })()
    return () => { stop = true }
  }, [run, info, c])

  if (phase.at === "rejected") {
    return (
      <div className="grid gap-4">
        <Alert variant="destructive"><TriangleAlert /><AlertTitle>{t("apply.rejected")}</AlertTitle><AlertDescription className="grid gap-1">{phase.problems.map((p, i) => <span key={i}>{p.message}</span>)}</AlertDescription></Alert>
        <div><Button onClick={onBack}>{t("apply.change")}</Button></div>
      </div>
    )
  }
  if (phase.at === "silent") {
    return (
      <div className="grid gap-4">
        <Alert><TriangleAlert /><AlertTitle>{t("apply.silent")}</AlertTitle><AlertDescription>{t("apply.silentText", { path: phase.path })}</AlertDescription></Alert>
        <div className="flex gap-2"><Button onClick={() => { started.current = -1; setPhase({ at: "saving" }); setStage(0); setRun((n) => n + 1) }}>{t("apply.again")}</Button><Button variant="outline" onClick={onBack}>{t("apply.change")}</Button></div>
      </div>
    )
  }
  return (
    <div className="grid gap-4" role="status" aria-live="polite">
      <Progress value={Math.round((stage / STAGES.length) * 100)} />
      <ol className="m-0 grid list-none gap-2 p-0 text-sm">
        {STAGES.map((k, i) => (
          <li key={k} className={cn("flex items-center gap-2 transition-opacity duration-200", i > stage && "opacity-40")}>
            {i < stage ? <Check className="size-4 text-emerald-600" /> : i === stage ? <LoaderCircle className="size-4 animate-spin" /> : <span className="size-4 rounded-full border" />}{t(k)}
          </li>
        ))}
      </ol>
      <p className="m-0 text-xs text-muted-foreground">{t("apply.note")}</p>
    </div>
  )
}
