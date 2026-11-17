import { useCallback, useEffect, useRef, useState } from "react"
import { Layers, SlidersHorizontal, Zap } from "lucide-react"
import { Card, CardContent } from "@/components/ui/card"
import { cn } from "@/lib/utils"
import { rememberCode, send, setupCode, type Info, type Problem } from "./api"
import { I18nProvider, useI18n, type Key } from "./i18n"
import { LangSwitch } from "./LangSwitch"
import type { W } from "./common"
import { Gate } from "./Gate"
import { STEPS, Shell } from "./Shell"
import { draft, initial, type Choices } from "./wizard"

type Load = { at: "loading" } | { at: "gate"; wrong: boolean } | { at: "error"; message: string } | { at: "ready"; info: Info }
type Start = "choose" | "default" | "custom"

const CHOICES: { id: Exclude<Start, "choose">; name: Key; time: Key; blurb: Key; Icon: typeof Zap }[] = [
  { id: "default", name: "choose.default", time: "choose.defaultTime", blurb: "choose.defaultText", Icon: Zap },
  { id: "custom", name: "choose.custom", time: "choose.customTime", blurb: "choose.customText", Icon: SlidersHorizontal },
]

/** A small mark in the bottom corner, linking to Nuah. The Nuah icon replaces the placeholder when the identity is designed. */
function BuiltOn() {
  const { t } = useI18n()
  return <a href="https://github.com/addaui/nuah" target="_blank" rel="noreferrer" className="fixed bottom-4 start-4 z-40 flex items-center gap-1.5 rounded-full border bg-background/80 px-2.5 py-1 text-xs text-muted-foreground backdrop-blur transition-[color,transform] duration-150 ease-out active:scale-[0.97] focus-visible:ring-2 focus-visible:ring-ring [@media(hover:hover)_and_(pointer:fine)]:hover:text-foreground"><Layers className="size-3" />{t("app.builtOn")}</a>
}

export function App() {
  return <I18nProvider><Setup /></I18nProvider>
}

function Setup() {
  const { t } = useI18n()
  const [load, setLoad] = useState<Load>({ at: "loading" })
  useEffect(() => {
    let live = true
    ;(async () => {
      const { status, body } = await send<Info & { error?: { message: string } }>("/setup/state")
      if (!live) return
      if (status === 401) setLoad({ at: "gate", wrong: !!setupCode() })
      else if (status !== 200) setLoad({ at: "error", message: body.error?.message ?? `The setup server answered ${status}.` })
      else setLoad({ at: "ready", info: body })
    })()
    return () => { live = false }
  }, [])
  const again = (code: string) => { rememberCode(code); setLoad({ at: "loading" }); location.reload() }
  if (load.at === "loading") return <div className="grid min-h-svh place-items-center text-sm text-muted-foreground" role="status">{t("app.loading")}</div>
  if (load.at === "gate") return <Gate wrong={load.wrong} onCode={again} />
  if (load.at === "error") return <div className="grid min-h-svh place-items-center px-4"><p role="alert" className="m-0 max-w-md text-sm text-destructive">{load.message}</p></div>
  return <Wizard info={load.info} />
}

function Wizard({ info }: { info: Info }) {
  const { t } = useI18n()
  const [c, setC] = useState<Choices>(() => initial(info))
  const [start, setStart] = useState<Start>("choose")
  const [index, setIndex] = useState(0)
  const [problems, setProblems] = useState<Problem[]>(info.problems)
  const seq = useRef(0)
  // the Core's own Resolver judges every draft, a moment after the last change; the Face holds no rules of its own
  useEffect(() => {
    const mine = ++seq.current
    const t = setTimeout(async () => {
      const { body } = await send<{ problems?: Problem[]; error?: { message: string } }>("/setup/check", draft(info, c))
      if (mine !== seq.current) return
      setProblems(body.problems ?? (body.error ? [{ code: "invalid_profile", plugins: [], message: body.error.message }] : []))
    }, 250)
    return () => clearTimeout(t)
  }, [info, c])
  const update = useCallback((f: (c: Choices) => Choices) => setC(f), [])
  const w: W = { info, c, update, problems, tried: false }
  return (
    <>
      <BuiltOn />
      {start === "choose" ? (
        <div className="grid min-h-svh place-items-center px-4 py-6 pb-20">
          <div className="grid w-full max-w-xl gap-5">
            <div className="grid gap-1.5">
              <div className="flex items-center justify-between gap-2"><div className="flex items-center gap-2 text-sm font-medium"><span className="grid size-6 place-items-center rounded-md bg-primary text-primary-foreground"><Layers className="size-3.5" /></span>{t("app.setupName", { product: info.distribution[0].toUpperCase() + info.distribution.slice(1) })}</div><LangSwitch /></div>
              <h1 className="m-0 text-2xl font-semibold tracking-tight">{t("choose.heading")}</h1>
            </div>
            {info.load_error && <p role="alert" className="m-0 text-sm text-destructive">{t("choose.loadError", { error: info.load_error })}</p>}
            <div className="grid gap-3">
              {CHOICES.map((m) => (
                <button key={m.id} type="button" onClick={() => { setIndex(m.id === "default" ? STEPS.length - 1 : 0); setStart(m.id) }} className="group text-start outline-none">
                  <Card className={cn("transition-[transform,box-shadow] duration-150 ease-out group-active:scale-[0.99] group-focus-visible:ring-2 group-focus-visible:ring-ring [@media(hover:hover)_and_(pointer:fine)]:group-hover:-translate-y-0.5", m.id === "default" && "ring-1 ring-primary/40")}>
                    <CardContent className="flex items-start gap-4">
                      <span className="grid size-10 shrink-0 place-items-center rounded-lg bg-muted"><m.Icon className="size-5" /></span>
                      <span className="grid min-w-0 flex-1 gap-1">
                        <span className="flex flex-wrap items-baseline justify-between gap-x-3"><span className="text-base font-medium">{t(m.name)}</span><span className="text-xs text-muted-foreground">{t(m.time)}{m.id === "default" && ` · ${t("choose.recommended")}`}</span></span>
                        <span className="text-sm text-muted-foreground">{t(m.blurb)}</span>
                      </span>
                    </CardContent>
                  </Card>
                </button>
              ))}
            </div>
            <p className="m-0 text-xs text-muted-foreground">{t("choose.note")}</p>
          </div>
        </div>
      ) : (
        <Shell w={w} index={index} setIndex={setIndex} />
      )}
    </>
  )
}
