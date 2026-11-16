import { useEffect, useRef, useState } from "react"
import { ChevronsUpDown } from "lucide-react"
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover"
import { cn } from "@/lib/utils"
import "./wheel.css"

export interface WheelOption<T extends string> {
  id: T
  /** Shown in the wheel. */
  label: string
  /** Shown on the trigger when this is the chosen one; defaults to the label. */
  short?: string
  /** A small note at the end of the row. */
  hint?: string
}

const ROW = 36
const CHIP = 28 // the chip is 28px tall
const PAD = 4 // popup padding

/**
 * A reusable wheel select for a short, ordered list of three choices (the wheel shows the chosen one in the middle with its
 * neighbours above and below). The chosen value is a small chip; hover or press it and the chip grows into the wheel.
 * The wheel moves with a transition (interruptible); keyboard moves are instant, because keys are repeated and should not wait.
 */
export function WheelSelect<T extends string>({ value, options, onChange, label }: { value: T; options: [WheelOption<T>, WheelOption<T>, WheelOption<T>]; onChange: (v: T) => void; label: string }) {
  const ORDER = options
  const current = Math.max(0, ORDER.findIndex((m) => m.id === value))
  const [open, setOpen] = useState(false)
  const [chipW, setChipW] = useState(90)
  const trig = useRef<HTMLButtonElement>(null)
  const [sel, setSel] = useState(current)
  const [byKey, setByKey] = useState(false)
  const lastWheel = useRef(0)
  const list = useRef<HTMLDivElement>(null)

  useEffect(() => { if (open) list.current?.focus() }, [open])

  const move = (to: number, key: boolean) => { setByKey(key); setSel(Math.min(ORDER.length - 1, Math.max(0, to))) }
  const commit = (i: number) => { setOpen(false); if (ORDER[i].id !== value) onChange(ORDER[i].id) }

  return (
    <Popover open={open} onOpenChange={(o) => { if (o) { setSel(current); setByKey(false); setChipW(trig.current?.offsetWidth ?? 90) } setOpen(o) }}>
      <PopoverTrigger ref={trig} openOnHover delay={120} closeDelay={250} className="inline-flex h-7 items-center gap-1.5 rounded-full bg-secondary px-3 text-xs font-medium text-secondary-foreground outline-none transition-[background-color,transform] duration-150 ease-out focus-visible:ring-2 focus-visible:ring-ring active:scale-[0.97] data-popup-open:bg-accent" aria-label={`${label}: ${ORDER[current].short ?? ORDER[current].label}. Change`}>
        {ORDER[current].short ?? ORDER[current].label}<ChevronsUpDown className="size-3 text-muted-foreground" />
      </PopoverTrigger>
      <PopoverContent
        align="start"
        alignOffset={-4}
        sideOffset={-(CHIP + PAD + ROW * 1.5 - CHIP / 2)}
        style={{ animation: "none", "--chip-w": `${chipW}px` } as React.CSSProperties}
        className="mode-wheel w-52 gap-1 bg-popover p-1"
      >
        <div
          ref={list}
          role="listbox"
          tabIndex={0}
          aria-label={label}
          aria-activedescendant={`wheel-${ORDER[sel].id}`}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") { e.preventDefault(); move(sel + 1, true) }
            else if (e.key === "ArrowUp") { e.preventDefault(); move(sel - 1, true) }
            else if (e.key === "Enter" || e.key === " ") { e.preventDefault(); commit(sel) }
          }}
          onWheel={(e) => { const now = performance.now(); if (now - lastWheel.current < 140 || Math.abs(e.deltaY) < 4) return; lastWheel.current = now; move(sel + (e.deltaY > 0 ? 1 : -1), false) }}
          className="relative h-[108px] overflow-hidden rounded-md outline-none [mask-image:linear-gradient(to_bottom,transparent,black_30%,black_70%,transparent)] focus-visible:ring-2 focus-visible:ring-ring"
        >
          <div aria-hidden className="wheel-bar absolute inset-x-0 top-[36px] h-9 rounded-md bg-accent" />
          <div className={cn("absolute inset-x-0 top-0", byKey ? "transition-none" : "transition-transform duration-[220ms] ease-[cubic-bezier(0.23,1,0.32,1)] motion-reduce:transition-none")} style={{ transform: `translateY(${(1 - sel) * ROW}px)` }}>
            {ORDER.map((m, i) => (
              <button
                key={m.id}
                id={`wheel-${m.id}`}
                role="option"
                aria-selected={i === sel}
                type="button"
                tabIndex={-1}
                onClick={() => (i === sel ? commit(i) : (move(i, false), setTimeout(() => commit(i), 180)))}
                style={{ height: ROW, opacity: i === sel ? 1 : 0.55, transform: `scale(${i === sel ? 1 : 0.94})` }}
                className="flex w-full items-center justify-between px-3 text-xs transition-[opacity,transform] duration-[220ms] ease-[cubic-bezier(0.23,1,0.32,1)] motion-reduce:transition-none"
              >
                <span className={cn(i === sel && "font-medium")}>{m.label}</span>
                <span className="text-xs text-muted-foreground">{m.hint}</span>
              </button>
            ))}
          </div>
        </div>
        
      </PopoverContent>
    </Popover>
  )
}
