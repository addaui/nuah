import { Button } from "@/components/ui/button"
import { LANGS, useI18n } from "./i18n"

/** The language of the whole page; the direction follows it. */
export function LangSwitch() {
  const { lang, setLang, t } = useI18n()
  return (
    <div className="flex items-center gap-1" role="group" aria-label={t("app.language")}>
      {LANGS.map((l) => <Button key={l.id} size="sm" variant={lang === l.id ? "secondary" : "ghost"} aria-pressed={lang === l.id} lang={l.id} onClick={() => setLang(l.id)}>{l.name}</Button>)}
    </div>
  )
}
