import { useEffect, useLayoutEffect, useRef, useState } from "react"
import "./picker.css"

/** The prototype picker: chrome, not a contestant. Markup, keys and URL param follow the harness spec. */
export function Picker({ names, current, onSelect, onReplay }: { names: string[]; current: number; onSelect: (i: number) => void; onReplay: () => void }) {
  const nav = useRef<HTMLElement>(null)
  const hl = useRef<HTMLSpanElement>(null)
  const items = useRef<(HTMLButtonElement | null)[]>([])
  const [ready, setReady] = useState(false)

  useLayoutEffect(() => {
    const move = () => {
      const el = items.current[current]
      if (!el || !hl.current) return
      hl.current.style.width = el.offsetWidth + "px"
      hl.current.style.transform = `translateX(${el.offsetLeft}px)`
    }
    move()
    window.addEventListener("resize", move)
    return () => window.removeEventListener("resize", move)
  }, [current, names])

  useEffect(() => { const id = requestAnimationFrame(() => requestAnimationFrame(() => setReady(true))); return () => cancelAnimationFrame(id) }, [])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement
      if (/^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName) || t.isContentEditable) return
      if (e.metaKey || e.ctrlKey || e.altKey) return
      const n = parseInt(e.key, 10)
      if (n >= 1 && n <= names.length) onSelect(n - 1)
      else if (e.key === "ArrowRight") onSelect((current + 1) % names.length)
      else if (e.key === "ArrowLeft") onSelect((current - 1 + names.length) % names.length)
      else if (e.key === "r" || e.key === "R") onReplay()
    }
    document.addEventListener("keydown", onKey)
    return () => document.removeEventListener("keydown", onKey)
  }, [current, names.length, onSelect, onReplay])

  return (
    <nav ref={nav} className="proto-picker" aria-label="Prototype variants" {...(ready ? { "data-ready": "" } : {})}>
      <span ref={hl} className="proto-picker-highlight" aria-hidden="true" />
      {names.map((n, i) => (
        <button key={n} ref={(el) => { items.current[i] = el }} className="proto-picker-item" onClick={() => onSelect(i)} {...(i === current ? { "data-active": "", "aria-current": "true" as const } : {})}>{n}</button>
      ))}
      <span className="proto-picker-divider" aria-hidden="true" />
      <button className="proto-picker-item proto-picker-replay" aria-label="Replay animation (R)" onClick={onReplay}>↻</button>
    </nav>
  )
}
