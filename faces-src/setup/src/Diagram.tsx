import { ArrowDown, Maximize2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { cn } from "@/lib/utils"
import type { Role } from "./api"
import type { W } from "./common"
import { Graph, reachKey } from "./Graph"
import { useI18n } from "./i18n"
import { fills, label, risky } from "./wizard"

/** The whole picture in a window. */
export function GraphDialog({ info, c, open, onOpenChange }: Pick<W, "info" | "c"> & { open: boolean; onOpenChange: (o: boolean) => void }) {
  const { t } = useI18n()
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[calc(100svh-2rem)] max-w-[min(68rem,calc(100vw-2rem))] overflow-y-auto sm:max-w-[min(68rem,calc(100vw-2rem))]">
        <DialogHeader><DialogTitle>{t("pic.title")}</DialogTitle><DialogDescription>{t("pic.text")}</DialogDescription></DialogHeader>
        <Graph info={info} c={c} />
      </DialogContent>
    </Dialog>
  )
}

const Layer = ({ title, hot, danger, children }: { title: string; hot?: boolean; danger?: boolean; children: React.ReactNode }) => (
  <div className={cn("grid gap-1.5 rounded-lg border bg-card px-3 py-2 transition-[border-color,box-shadow] duration-200", hot && "border-foreground/40 shadow-sm", danger && "border-destructive/60")}>
    <span className="text-[11px] uppercase tracking-wide text-muted-foreground">{title}</span>
    <div className="flex flex-wrap gap-1">{children}</div>
  </div>
)
const Chip = ({ children, danger }: { children: React.ReactNode; danger?: boolean }) => <span className={cn("rounded-full px-2 py-0.5 text-[11px]", danger ? "bg-destructive/10 text-destructive" : "bg-muted")}>{children}</span>

/** The picture in a small shape: the same layers as the full graph, following the choices. Expand it for every plugin and the lines between them. */
export function Diagram({ info, c, step, onExpand }: Pick<W, "info" | "c"> & { step: string; onExpand: () => void }) {
  const { t } = useI18n()
  const names = (r: Role) => info.plugins.filter((p) => p.role === r && c.enabled[p.id]).map((p) => label(p))
  const plugs = [...names("feature"), ...names("provider")]
  const port = (id: string) => fills(info, id).filter((p) => c.enabled[p.id]).map((p) => label(p))
  const danger = risky(info, c).length > 0
  return (
    <div className="grid gap-1.5" role="group" aria-label="How your choices fit together">
      <div className={cn("rounded-lg border border-dashed bg-background/60 px-3 py-2 text-center text-xs", danger && "border-destructive/60 text-destructive")}>{t("pic.canOpen", { who: t(reachKey(c.reach)) })}</div>
      <ArrowDown className="mx-auto size-3.5 text-muted-foreground" />
      <Layer title={t("pic.faces")}>{names("face").map((n) => <Chip key={n}>{n}</Chip>)}</Layer>
      <ArrowDown className="mx-auto size-3.5 text-muted-foreground" />
      <Layer title={t("pic.projection")}>{names("projection").map((n) => <Chip key={n}>{n}</Chip>)}</Layer>
      <ArrowDown className="mx-auto size-3.5 text-muted-foreground" />
      <div className={cn("grid gap-2 rounded-xl border bg-muted/40 p-2.5 transition-shadow duration-200", step === "plugins" && "ring-2 ring-primary/30")}>
        <div className="flex items-baseline justify-between"><b className="text-xs">{t("pic.core")}</b><span className="text-[11px] text-muted-foreground">{t("pic.pluginsOn", { n: plugs.length })}</span></div>
        <div className="flex flex-wrap gap-1">{plugs.slice(0, 6).map((n) => <Chip key={n}>{n}</Chip>)}{plugs.length > 6 && <Chip>+{plugs.length - 6}</Chip>}</div>
        <div className="grid grid-cols-2 gap-2">
          <div className={cn("rounded-lg transition-shadow duration-200", step === "data" && "ring-2 ring-primary/40")}><Layer title="Port · db" hot={step === "data"}>{port("db").map((n) => <Chip key={n}>{n}</Chip>)}</Layer></div>
          <div className={cn("rounded-lg transition-shadow duration-200", step === "signin" && "ring-2 ring-primary/40")}><Layer title="Port · auth" hot={step === "signin"} danger={danger}>{port("auth").map((n) => <Chip key={n} danger={danger}>{n}</Chip>)}</Layer></div>
        </div>
      </div>
      <Button variant="outline" size="sm" onClick={onExpand}><Maximize2 />{t("pic.expand")}</Button>
    </div>
  )
}
