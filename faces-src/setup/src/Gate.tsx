import { useState } from "react"
import { Layers } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { useI18n } from "./i18n"
import { LangSwitch } from "./LangSwitch"

/** The setup server prints a one-time code when it starts; it is typed here once, so only whoever can read the log can change the setup. */
export function Gate({ wrong, onCode }: { wrong: boolean; onCode: (code: string) => void }) {
  const { t } = useI18n()
  const [code, setCode] = useState("")
  return (
    <div className="grid min-h-svh place-items-center px-4">
      <div className="fixed end-4 top-4"><LangSwitch /></div>
      <Card className="w-full max-w-sm">
        <CardHeader>
          <div className="flex items-center gap-2 text-sm font-medium"><span className="grid size-6 place-items-center rounded-md bg-primary text-primary-foreground"><Layers className="size-3.5" /></span>{t("gate.heading")}</div>
          <CardTitle>{t("gate.title")}</CardTitle>
          <CardDescription>{t("gate.text")}</CardDescription>
        </CardHeader>
        <CardContent>
          <form className="grid gap-3" onSubmit={(e) => { e.preventDefault(); onCode(code.trim()) }}>
            <Input autoFocus dir="ltr" value={code} onChange={(e) => setCode(e.target.value)} aria-label={t("gate.label")} aria-invalid={wrong} autoComplete="off" spellCheck={false} className="font-mono" />
            {wrong && <p role="alert" className="m-0 text-xs text-destructive">{t("gate.wrong")}</p>}
            <Button type="submit" disabled={!code.trim()}>{t("gate.go")}</Button>
          </form>
        </CardContent>
      </Card>
    </div>
  )
}
