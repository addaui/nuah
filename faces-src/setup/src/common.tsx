import type { ReactNode } from "react"
import { Label } from "@/components/ui/label"
import { RadioGroupItem } from "@/components/ui/radio-group"
import { cn } from "@/lib/utils"
import type { Info, Power, Problem } from "./api"
import type { Choices } from "./wizard"

/** What every step is given: what the server said, what the person chose, and how to change it. */
export interface W {
  info: Info
  c: Choices
  update: (f: (c: Choices) => Choices) => void
  problems: Problem[]
  /** The person pressed Continue and something was missing: show it. */
  tried: boolean
}

export const TAG: Record<Power, string> = {
  stores: "bg-sky-500/10 text-sky-700 dark:text-sky-300",
  listens: "bg-violet-500/10 text-violet-700 dark:text-violet-300",
  "reaches out": "bg-amber-500/10 text-amber-700 dark:text-amber-300",
  "changes settings": "bg-rose-500/10 text-rose-700 dark:text-rose-300",
}
export const SHORT: Record<Power, string> = { stores: "keeps its own data", listens: "opens a port", "reaches out": "uses the internet", "changes settings": "can change how this runs" }
export const POWER_WORDS: Record<Power, string> = {
  stores: "Keeps its own data in your database",
  listens: "Opens a port and answers requests",
  "reaches out": "Contacts websites on the internet",
  "changes settings": "Can change how this instance runs",
}
export const Pill = ({ w }: { w: Power }) => <span className={cn("rounded-full px-2 py-0.5 text-[11px] font-medium", TAG[w])}>{w}</span>

/** A labelled field; the hint is the short plain note and the error shows only once the person has tried to go on. */
export function Field({ label, hint, error, optional, children, htmlFor }: { label: string; hint?: ReactNode; error?: string; optional?: boolean; children: ReactNode; htmlFor?: string }) {
  return (
    <div className="grid content-start gap-1.5">
      <div className="flex items-baseline justify-between gap-2"><Label htmlFor={htmlFor}>{label}</Label><span className="text-xs text-muted-foreground">{optional ? "Optional" : "Required"}</span></div>
      {children}
      {error ? <p className="m-0 text-xs text-destructive" role="alert">{error}</p> : hint ? <p className="m-0 text-xs text-muted-foreground">{hint}</p> : null}
    </div>
  )
}

/** One choice in a radio group, as a card; the whole card is the target. */
export function Choice({ value, checked, title, children, aside }: { value: string; checked: boolean; title: string; children?: ReactNode; aside?: ReactNode }) {
  return (
    <label className={cn("flex cursor-pointer items-start gap-3 rounded-lg border bg-card p-3 transition-[border-color,background-color,transform] duration-150 ease-out active:scale-[0.99]", checked ? "border-primary bg-muted/40" : "hover:border-foreground/30")}>
      <RadioGroupItem value={value} className="mt-0.5" aria-label={title} />
      <span className="grid min-w-0 flex-1 gap-0.5">
        <span className="flex flex-wrap items-center gap-2 text-sm font-medium">{title}{aside}</span>
        {children && <span className="text-xs text-muted-foreground">{children}</span>}
      </span>
    </label>
  )
}
