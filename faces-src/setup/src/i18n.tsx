import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react"
import { ar } from "./locales/ar"
import { en } from "./locales/en"

export type Lang = "en" | "ar"
export type Key = keyof typeof en
const tables: Record<Lang, Record<string, string>> = { en, ar }
export const LANGS: { id: Lang; name: string; dir: "ltr" | "rtl" }[] = [{ id: "en", name: "English", dir: "ltr" }, { id: "ar", name: "العربية", dir: "rtl" }]

const stored = (): Lang | null => { try { const v = localStorage.getItem("nuah-lang"); return v === "en" || v === "ar" ? v : null } catch { return null } }
export const startLang = (): Lang => stored() ?? (typeof navigator !== "undefined" && navigator.language.toLowerCase().startsWith("ar") ? "ar" : "en")

/** Texts are keys, never literals in a component. `{name}` in a text is filled from the params. A text missing in a language falls back to English. */
export function translate(lang: Lang, key: string, params?: Record<string, string | number>): string {
  const text = tables[lang][key] ?? tables.en[key] ?? key
  return params ? text.replace(/\{(\w+)\}/g, (_, k: string) => String(params[k] ?? "")) : text
}

interface Ctx { lang: Lang; setLang: (l: Lang) => void; t: (key: Key, params?: Record<string, string | number>) => string }
const I18n = createContext<Ctx>({ lang: "en", setLang: () => undefined, t: (k, p) => translate("en", k, p) })
export const useI18n = () => useContext(I18n)

export function I18nProvider({ children, initial }: { children: ReactNode; initial?: Lang }) {
  const [lang, setLangState] = useState<Lang>(initial ?? startLang())
  // the page's language and direction follow the language, so every logical property (start, end) turns with it
  useEffect(() => {
    const l = LANGS.find((x) => x.id === lang)!
    document.documentElement.lang = l.id
    document.documentElement.dir = l.dir
  }, [lang])
  const value = useMemo<Ctx>(() => ({ lang, setLang: (l) => { setLangState(l); try { localStorage.setItem("nuah-lang", l) } catch { /* not kept */ } }, t: (key, params) => translate(lang, key, params) }), [lang])
  return <I18n.Provider value={value}>{children}</I18n.Provider>
}
