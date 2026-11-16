import { useLayoutEffect, useRef, useState } from "react"
import { Check } from "lucide-react"
import { cn } from "@/lib/utils"

/**
 * The steps as a floating pill at the bottom of the screen. The step you are on is filled with the primary colour (a highlight that
 * slides to it), the steps you finished are secondary, and the ones still ahead are primary text. Steps ahead cannot be opened yet.
 */
export function StepBar({ steps, index, go }: { steps: readonly { id: string; title: string }[]; index: number; go: (i: number) => void }) {
  const hl = useRef<HTMLSpanElement>(null)
  const items = useRef<(HTMLButtonElement | null)[]>([])
  const [ready, setReady] = useState(false)
  useLayoutEffect(() => {
    const move = () => {
      const el = items.current[index]
      if (!el || !hl.current) return
      hl.current.style.width = el.offsetWidth + "px"
      hl.current.style.transform = `translateX(${el.offsetLeft}px)`
      el.scrollIntoView({ block: "nearest", inline: "center" })
    }
    move()
    const id = requestAnimationFrame(() => setReady(true))
    window.addEventListener("resize", move)
    return () => { cancelAnimationFrame(id); window.removeEventListener("resize", move) }
  }, [index])
  return (
    <nav aria-label="Steps" className="fixed bottom-4 left-1/2 z-50 max-w-[calc(100vw-1rem)] -translate-x-1/2 overflow-x-auto rounded-full bg-popover/90 p-1 shadow-lg ring-1 ring-foreground/10 backdrop-blur [scrollbar-width:none]">
      <ol className="relative m-0 flex list-none items-center gap-0.5 p-0">
        <span ref={hl} aria-hidden className={cn("absolute top-0 left-0 h-8 rounded-full bg-primary", ready && "transition-[transform,width] duration-[250ms] ease-[cubic-bezier(0.23,1,0.32,1)] motion-reduce:transition-none")} />
        {steps.map((st, i) => {
          const done = i < index
          return (
            <li key={st.id}>
              <button ref={(el) => { items.current[i] = el }} type="button" disabled={i > index} onClick={() => go(i)} aria-current={i === index ? "step" : undefined}
                className={cn("relative flex h-8 items-center gap-1.5 rounded-full px-3 text-[13px] font-medium outline-none transition-[color,background-color,transform] duration-150 ease-out focus-visible:ring-2 focus-visible:ring-ring active:scale-[0.97] disabled:cursor-default disabled:active:scale-100", i === index ? "text-primary-foreground" : done ? "bg-secondary text-secondary-foreground" : "text-primary")}>
                {done ? <Check className="size-3.5" /> : <span className="text-[11px] opacity-70">{i + 1}</span>}
                <span className={cn(i !== index && "hidden sm:inline")}>{st.title}</span>
              </button>
            </li>
          )
        })}
      </ol>
    </nav>
  )
}
