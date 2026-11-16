import { useState } from "react"
import { Layers, Maximize2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card"
import { cn } from "@/lib/utils"
import { Apply } from "./Apply"
import type { W } from "./common"
import { Diagram, GraphDialog } from "./Diagram"
import { StepBar } from "./StepBar"
import { Access, CredentialFields, Data, Instance, SignIn } from "./steps/Basics"
import { Plugins } from "./steps/Plugins"
import { Review } from "./steps/Review"
import { stepError, type StepId } from "./wizard"

export const STEPS: { id: StepId; title: string; why: string }[] = [
  { id: "instance", title: "Instance", why: "Only a label. It changes nothing else." },
  { id: "data", title: "Data", why: "Everything you make is kept here. Moving later is possible, but easier to get right now." },
  { id: "signin", title: "Sign-in", why: "Decides who can read what. It is your main protection." },
  { id: "access", title: "Access", why: "The network matters as much as sign-in. Together they decide who gets in." },
  { id: "plugins", title: "Plugins", why: "Madar is built from plugins. Choose which are on, add more, or bring your own." },
  { id: "review", title: "Review", why: "Every plugin asks for what it needs. Here is the list in plain words." },
]

function body(id: StepId, w: W, seen: Set<string>) {
  switch (id) {
    case "instance": return <Instance {...w} />
    case "data": return <Data {...w} />
    case "signin": return <SignIn {...w} />
    case "access": return <Access {...w} />
    case "plugins": return <Plugins {...w} />
    case "review": return <div className="grid gap-4">{!seen.has("signin") && <CredentialFields {...w} />}<Review {...w} /></div>
  }
}

function Brand({ name }: { name: string }) {
  return <div className="flex items-center gap-2 text-sm font-medium"><span className="grid size-6 place-items-center rounded-md bg-primary text-primary-foreground"><Layers className="size-3.5" /></span>Set up {name}</div>
}

/** One wizard: the card for the step in the middle, the picture of what is being built beside it, and the steps in a pill at the bottom. */
export function Shell({ w, index, setIndex }: { w: W; index: number; setIndex: (i: number) => void }) {
  const [tried, setTried] = useState(false)
  const [applying, setApplying] = useState(false)
  const [wide, setWide] = useState(false)
  // the steps the person has been on: what has no default is asked on the review only when its own step was skipped (the default installation)
  const [seen, setSeen] = useState<Set<string>>(() => new Set([STEPS[index].id]))
  const step = STEPS[index]
  const last = index === STEPS.length - 1
  const plugins = step.id === "plugins"
  const product = w.info.distribution[0].toUpperCase() + w.info.distribution.slice(1)
  const wt = { ...w, tried }
  const go = (i: number) => { setSeen((x) => new Set(x).add(STEPS[i].id)); setIndex(i) }
  const to = (i: number) => { setTried(false); go(i) }
  const next = () => {
    if (stepError(w.info, w.c, step.id, w.problems.length)) return setTried(true)
    setTried(false)
    if (last) setApplying(true)
    else go(index + 1)
  }
  return (
    <div className="min-h-svh px-4 pt-5 pb-24">
      <div className="mx-auto grid w-full max-w-6xl gap-4">
        <header className="flex items-center justify-between gap-3"><Brand name={product} />{!applying && <Button variant="outline" size="sm" className="xl:hidden" onClick={() => setWide(true)}><Maximize2 />Picture</Button>}</header>
        <div className={cn("grid h-[min(42rem,calc(100svh-9.5rem))] gap-4", !plugins && !applying && "xl:grid-cols-[minmax(0,1fr)_18rem]")}>
          <Card className="flex min-h-0 flex-col gap-0 overflow-hidden py-0">
            {!applying ? (
              <>
                <CardHeader className="border-b py-4"><CardTitle className="flex items-center justify-between gap-3">{step.title}{plugins && <Button variant="outline" size="sm" onClick={() => setWide(true)}><Maximize2 />How it fits together</Button>}</CardTitle><CardDescription>{step.why.replace("Madar", product)}</CardDescription></CardHeader>
                <CardContent key={step.id} className="min-h-0 flex-1 overflow-y-auto py-4 animate-in fade-in-0 duration-200">{plugins ? body(step.id, wt, seen) : <div className="max-w-2xl">{body(step.id, wt, seen)}</div>}</CardContent>
                <CardFooter className="justify-between border-t py-3">
                  {index > 0 ? <Button variant="ghost" onClick={() => to(index - 1)}>Back</Button> : <span />}
                  <Button onClick={next}>{last ? `Set up ${product}` : "Continue"}</Button>
                </CardFooter>
              </>
            ) : (
              <><CardHeader className="border-b py-4"><CardTitle>Setting up {w.c.name}</CardTitle></CardHeader><CardContent className="py-4"><Apply info={w.info} c={w.c} onBack={() => setApplying(false)} /></CardContent></>
            )}
          </Card>
          {!plugins && !applying && (
            <aside className="hidden min-h-0 content-start gap-3 overflow-y-auto rounded-xl border bg-muted/30 p-3 xl:grid">
              <h2 className="m-0 text-xs font-medium uppercase tracking-wide text-muted-foreground">What you are building</h2>
              <Diagram info={w.info} c={w.c} step={step.id} onExpand={() => setWide(true)} />
            </aside>
          )}
        </div>
      </div>
      {!applying && <StepBar steps={STEPS} index={index} go={to} />}
      <GraphDialog info={w.info} c={w.c} open={wide} onOpenChange={setWide} />
    </div>
  )
}
