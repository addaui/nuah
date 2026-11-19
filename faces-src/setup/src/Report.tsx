import { useState } from "react"
import { Check, Copy, Layers, TriangleAlert } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { send } from "./api"
import { useI18n } from "./i18n"
import { LangSwitch } from "./LangSwitch"
import { ltr, problemKey, problemParams, reportText, titleOf, type ReportState } from "./report"

/** Shown instead of the app when a plugin's data cannot be upgraded (ADR 0091): nothing was changed, here is why, and what to do. */
export function Report({ state }: { state: ReportState }) {
  const { t, lang } = useI18n()
  const [copied, setCopied] = useState(false)
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [started, setStarted] = useState(false)
  const ids = Object.keys(state.plugins)
  const name = (id: string) => state.plugins[id]?.name ?? id
  const heading = titleOf(state)
  const title = t(heading.key, heading.params)
  const copy = async () => {
    try { await navigator.clipboard.writeText(reportText(state)); setCopied(true); setTimeout(() => setCopied(false), 1600) } catch { setError(t("upgrade.failed", { error: "clipboard" })) }
  }
  const without = async (plugin: string) => {
    setBusy(plugin)
    setError(null)
    const { status, body } = await send<{ ok?: boolean; error?: { message: string } }>("/upgrade/without", { plugin })
    setBusy(null)
    if (status === 200 && body.ok) {
      setStarted(true)
      setTimeout(() => location.assign("/"), 4000)
    } else setError(t("upgrade.failed", { error: body.error?.message ?? String(status) }))
  }
  return (
    <div className="grid min-h-svh place-items-center px-4 py-8">
      <div className="fixed end-4 top-4"><LangSwitch /></div>
      <Card className="w-full max-w-2xl">
        <CardHeader>
          <div className="flex items-center gap-2 text-sm font-medium"><span className="grid size-6 place-items-center rounded-md bg-primary text-primary-foreground"><Layers className="size-3.5" /></span>{state.product}</div>
          <CardTitle className="flex items-start gap-2 text-xl"><TriangleAlert className="mt-1 size-5 shrink-0 text-amber-600" aria-hidden />{title}</CardTitle>
          <CardDescription>{t("upgrade.intro")}</CardDescription>
        </CardHeader>
        <CardContent className="grid gap-6">
          <section className="grid gap-2">
            <h2 className="m-0 text-sm font-semibold">{t("upgrade.wrong")}</h2>
            {ids.map((id) => (
              <div key={id} className="grid gap-1.5">
                {ids.length > 1 && <h3 className="m-0 text-sm font-medium">{name(id)} <bdi dir="ltr" className="font-mono text-xs text-muted-foreground">{id}</bdi></h3>}
                <ol className="m-0 grid list-decimal gap-1.5 ps-5 text-sm">
                  {state.problems.filter((p) => p.plugin === id).map((p, i) => <li key={i}>{t(problemKey(p), problemParams(p, lang))}</li>)}
                </ol>
              </div>
            ))}
          </section>
          <section className="grid gap-3">
            <h2 className="m-0 text-sm font-semibold">{t("upgrade.can")}</h2>
            <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border p-3">
              <p className="m-0 min-w-0 flex-1 text-sm">{t("upgrade.copyText")}</p>
              <Button variant="outline" onClick={copy}>{copied ? <Check /> : <Copy />}{copied ? t("upgrade.copied") : t("upgrade.copy")}</Button>
            </div>
            {ids.map((id) => (
              <div key={id} className="flex flex-wrap items-center justify-between gap-3 rounded-lg border p-3">
                <div className="grid min-w-0 flex-1 gap-1 text-sm">
                  <p className="m-0">{t("upgrade.withoutText", { plugin: name(id) })}</p>
                  {(state.without[id] ?? []).length > 0 && <p className="m-0 text-muted-foreground">{t("upgrade.withoutAlso", { list: state.without[id].map((x) => state.names?.[x] ?? ltr(x)).join(", ") })}</p>}
                </div>
                <Button variant="secondary" disabled={busy !== null || started} onClick={() => without(id)}>{busy === id ? t("upgrade.starting") : t("upgrade.without")}</Button>
              </div>
            ))}
            <p className="m-0 rounded-lg border p-3 text-sm text-muted-foreground">{t("upgrade.goBack")}</p>
            {started && <p role="status" className="m-0 text-sm text-emerald-700 dark:text-emerald-400">{t("upgrade.started")}</p>}
            {error && <p role="alert" className="m-0 text-sm text-destructive">{error}</p>}
          </section>
        </CardContent>
      </Card>
    </div>
  )
}
