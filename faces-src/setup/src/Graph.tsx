import type { Plugin } from "./api"
import type { W } from "./common"
import { label, risky } from "./wizard"

interface Node { id: string; label: string; sub?: string; x: number; y: number; w: number; h: number; dim?: boolean; danger?: boolean; port?: boolean }

const REACH_WORDS = { "this-computer": "Only this computer", network: "My home or office network", internet: "Anyone on the internet" }
export const reachWords = (r: keyof typeof REACH_WORDS) => REACH_WORDS[r]

/**
 * How a Nuah service is put together, whatever the service is: people use Faces, a Projection carries the Link between a Face and the
 * Core, the Core runs the plugins (features, providers, themes), and every plugin reaches the world through Ports, each filled by an Adapter.
 */
export function Graph({ info, c }: Pick<W, "info" | "c">) {
  const on = (p: Plugin) => !!c.enabled[p.id]
  const of = (role: Plugin["role"]) => info.plugins.filter((p) => p.role === role && on(p))
  const faces = of("face"), projections = of("projection"), providers = of("provider"), themes = of("theme")
  // features: parents with their children right after, then the ones that need a provider last, so those lines stay short
  const all = of("feature")
  const needs = (p: Plugin) => p.requires.extension_points.length > 0
  const plain = all.filter((p) => !needs(p) && p.requires.plugins.length === 0).flatMap((p) => [p, ...all.filter((x) => x.requires.plugins.includes(p.id) && !needs(x))])
  const feats = [...plain, ...all.filter((p) => !plain.includes(p))]
  const PER = 6, COL = 196, NW = 176, NH = 34, STEP = NH + 8
  const featCols = Math.max(1, Math.ceil(feats.length / PER))
  const W = Math.max(900, (featCols + 2) * COL + 40)

  const peopleY = 0
  const facesY = 66, facesH = 64
  const projY = facesY + facesH + 46, projH = 64
  const coreY = projY + projH + 46
  const rows = Math.max(1, Math.min(PER, feats.length), providers.length, themes.length)
  const pluginsY = coreY + 50
  const portsLabelY = pluginsY + rows * STEP + 22
  const portsY = portsLabelY + 14
  const perRow = Math.max(1, Math.floor((W - 56) / 150))
  const ports = info.core_ports
  const portRows = Math.ceil(ports.length / perRow)
  const coreH = portsY + portRows * 54 + 10 - coreY
  const H = coreY + coreH + 10

  const nodes: Node[] = []
  const byId = new Map<string, Node>()
  const reg = (p: Plugin, n: Node) => { nodes.push(n); byId.set(p.id, n) }
  feats.forEach((p, i) => reg(p, { id: p.id, label: label(p), x: 28 + Math.floor(i / PER) * COL, y: pluginsY + (i % PER) * STEP, w: NW, h: NH }))
  providers.forEach((p, i) => reg(p, { id: p.id, label: label(p), sub: p.provides.length ? `provides ${p.provides[0]}` : undefined, x: 28 + featCols * COL, y: pluginsY + i * STEP, w: NW, h: NH + 6 }))
  themes.forEach((p, i) => reg(p, { id: p.id, label: label(p), x: 28 + (featCols + 1) * COL, y: pluginsY + i * STEP, w: NW, h: NH }))
  const fx = (W - (faces.length * 170 + Math.max(0, faces.length - 1) * 12)) / 2
  faces.forEach((p, i) => reg(p, { id: p.id, label: label(p), x: fx + i * 182, y: facesY + 22, w: 170, h: NH }))
  projections.forEach((p) => reg(p, { id: p.id, label: label(p), sub: "carries the Link", x: W / 2 - 100, y: projY + 16, w: 200, h: 40 }))
  ports.forEach((port, i) => {
    const filling = info.plugins.filter((p) => p.fills === port && on(p))
    nodes.push({ id: `port-${port}`, port: true, label: filling.length ? filling[0].name + (filling.length > 1 ? ` +${filling.length - 1}` : "") : "no adapter", sub: port, x: 28 + (i % perRow) * 150, y: portsY + Math.floor(i / perRow) * 54, w: 140, h: 44, dim: filling.length === 0, danger: port === "auth" && risky(info, c).length > 0 })
  })

  const mid = W / 2
  const edges: { d: string; label?: string; lx?: number; ly?: number; dashed?: boolean }[] = []
  edges.push({ d: `M ${mid} ${peopleY + 34} V ${facesY}`, label: "see and use", lx: mid + 8, ly: facesY - 14 })
  edges.push({ d: `M ${mid} ${facesY + facesH} V ${projY}`, label: "the Face is served by it and calls through the Link", lx: mid + 8, ly: projY - 16 })
  edges.push({ d: `M ${mid} ${projY + projH} V ${coreY}`, label: "operations", lx: mid + 8, ly: coreY - 16 })
  edges.push({ d: `M ${mid} ${portsLabelY - 18} V ${portsY - 6}`, label: "plugins reach the world through ports", lx: mid + 8, ly: portsLabelY + 4 })
  for (const f of feats) {
    const n = byId.get(f.id)!
    for (const ep of f.requires.extension_points) for (const p of providers.filter((x) => x.provides.includes(ep))) { const to = byId.get(p.id)!; edges.push({ d: `M ${n.x + n.w} ${n.y + n.h / 2} H ${n.x + n.w + 10} V ${to.y + to.h / 2} H ${to.x}`, dashed: true }) }
    for (const parentId of f.requires.plugins) { const parent = byId.get(parentId); if (parent && Math.abs(parent.x - n.x) < 1) edges.push({ d: `M ${n.x + n.w} ${n.y + n.h / 2} H ${n.x + n.w + 6} V ${parent.y + parent.h / 2} H ${parent.x + parent.w}` }) }
  }
  const danger = risky(info, c).length > 0

  return (
    <svg viewBox={`0 0 ${W} ${H}`} role="img" aria-label="How the plugins that are on fit together" className="h-auto w-full text-foreground">
      <defs><marker id="arr" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto"><path d="M0 0 L8 4 L0 8 z" className="fill-muted-foreground" /></marker></defs>
      <g><rect x={mid - 150} y={peopleY} width={300} height={34} rx={17} className={danger ? "fill-destructive/10 stroke-destructive" : "fill-card stroke-border"} /><text x={mid} y={21} textAnchor="middle" className="fill-current text-[12px]">People and programs · {reachWords(c.reach)}</text></g>
      <g><rect x={8} y={facesY} width={W - 16} height={facesH} rx={14} className="fill-muted/30 stroke-border" /><text x={24} y={facesY + 18} className="fill-current text-[12px] font-semibold">Faces</text><text x={64} y={facesY + 18} className="fill-muted-foreground text-[11px]">what people and programs use</text></g>
      <g><rect x={8} y={projY} width={W - 16} height={projH} rx={14} className="fill-muted/30 stroke-border" /><text x={24} y={projY + 18} className="fill-current text-[12px] font-semibold">Projection</text><text x={96} y={projY + 18} className="fill-muted-foreground text-[11px]">carries the Link</text></g>
      <g><rect x={8} y={coreY} width={W - 16} height={coreH} rx={14} className="fill-muted/60 stroke-border" strokeDasharray="4 3" /><text x={24} y={coreY + 24} className="fill-current text-[13px] font-semibold">Nuah Core</text><text x={100} y={coreY + 24} className="fill-muted-foreground text-[11px]">runs every plugin</text>
        <text x={28} y={pluginsY - 8} className="fill-muted-foreground text-[10px] uppercase tracking-wide">Features</text>
        {providers.length > 0 && <text x={28 + featCols * COL} y={pluginsY - 8} className="fill-muted-foreground text-[10px] uppercase tracking-wide">Providers</text>}
        {themes.length > 0 && <text x={28 + (featCols + 1) * COL} y={pluginsY - 8} className="fill-muted-foreground text-[10px] uppercase tracking-wide">Themes</text>}
        <text x={28} y={portsLabelY} className="fill-muted-foreground text-[10px] uppercase tracking-wide">Ports: fixed slots, each filled by an adapter</text></g>
      {edges.map((e, i) => <g key={i}><path d={e.d} fill="none" className="stroke-muted-foreground" strokeDasharray={e.dashed ? "4 3" : undefined} markerEnd="url(#arr)" />{e.label && <text x={e.lx} y={e.ly} className="fill-muted-foreground text-[10px]">{e.label}</text>}</g>)}
      {nodes.map((n) => (
        <g key={n.id} opacity={n.dim ? 0.5 : 1}>
          <rect x={n.x} y={n.y} width={n.w} height={n.h} rx={8} strokeDasharray={n.dim ? "3 3" : undefined} className={n.danger ? "fill-card stroke-destructive" : "fill-card stroke-border"} />
          {n.port ? (
            <><text x={n.x + 10} y={n.y + 15} className="fill-muted-foreground text-[10px] uppercase tracking-wide">{n.sub}</text><text x={n.x + 10} y={n.y + 33} className="fill-current text-[12px] font-medium">{n.label.length > 17 ? n.label.slice(0, 16) + "…" : n.label}</text></>
          ) : (
            <><text x={n.x + 10} y={n.y + (n.sub ? 16 : 22)} className="fill-current text-[12px] font-medium">{n.label.length > 24 ? n.label.slice(0, 23) + "…" : n.label}</text>{n.sub && <text x={n.x + 10} y={n.y + 31} className="fill-muted-foreground text-[10px]">{n.sub}</text>}</>
          )}
        </g>
      ))}
    </svg>
  )
}
