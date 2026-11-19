import { useEffect, useState } from "react"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Progress } from "@/components/ui/progress"
import { send } from "./api"
import { useI18n } from "./i18n"
import { LangSwitch } from "./LangSwitch"
import { startingLines, type Progress as Now } from "./starting"

/** Shown while the server upgrades data at start (ADR 0091): how far it has come. When the page's server stops answering
 *  this route, the app has started (or the report took over), so the page opens the address again. */
export function Starting({ first }: { first: Now }) {
  const { t, lang } = useI18n()
  const [now, setNow] = useState(first)
  useEffect(() => {
    let live = true
    const tick = async () => {
      const r = await send<Now>("/upgrade/progress").catch(() => null)
      if (!live) return
      if (r?.status === 200 && r.body.product) {
        setNow(r.body)
        setTimeout(tick, 1000)
      } else {
        // the Core (or the report) answers now: start from the top
        setTimeout(() => location.assign("/"), r ? 0 : 1000)
      }
    }
    const first = setTimeout(tick, 1000)
    return () => { live = false; clearTimeout(first) }
  }, [])
  const l = startingLines(lang, now)
  return (
    <div className="grid min-h-svh place-items-center px-4 py-10">
      <div className="fixed top-4 end-4"><LangSwitch /></div>
      <Card className="w-full max-w-md">
        <CardHeader>
          <CardTitle className="text-balance">{t("starting.title", { product: now.product })}</CardTitle>
          <CardDescription className="text-pretty">{t("starting.text")}</CardDescription>
        </CardHeader>
        <CardContent className="grid gap-2" aria-live="polite">
          <p className="m-0 text-sm font-medium">{t(l.part.key, l.part.params)}</p>
          <Progress value={l.percent} aria-label={t("starting.label")} />
          {l.count && <p className="m-0 text-sm text-muted-foreground tabular-nums">{t(l.count.key, l.count.params)}</p>}
        </CardContent>
      </Card>
    </div>
  )
}
