import { useState } from "react"
import { ChevronDown } from "lucide-react"
import { Badge } from "@/components/ui/badge"
import { Checkbox } from "@/components/ui/checkbox"
import { Input } from "@/components/ui/input"
import { InputGroup, InputGroupAddon, InputGroupInput, InputGroupText } from "@/components/ui/input-group"
import { RadioGroup } from "@/components/ui/radio-group"
import { Switch } from "@/components/ui/switch"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { TriangleAlert } from "lucide-react"
import { cn } from "@/lib/utils"
import { SchemaForm } from "@/schema"
import { Choice, Field, type W } from "@/common"
import { useI18n } from "@/i18n"
import { authPlugins, label, chooseAuth, fills, isAccounts, LOCAL, MIN_PASSWORD, needsFirst, OPEN, risky, settingsPasswordSlot, signKind, toggle, USERNAME } from "@/wizard"
import type { Reach } from "@/api"

export function Instance({ info, c, update, tried }: W) {
  const { t } = useI18n()
  return <Field label={t("instance.label")} error={tried && !c.name.trim() ? t("instance.missing") : undefined} hint={t("instance.hint")} htmlFor="name"><Input id="name" value={c.name} onChange={(e) => update((x) => ({ ...x, name: e.target.value }))} placeholder={info.distribution} /></Field>
}

function More({ children }: { children: React.ReactNode }) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  return (
    <div className="grid gap-3">
      <button type="button" onClick={() => setOpen((o) => !o)} aria-expanded={open} className="flex items-center gap-1 justify-self-start text-xs text-muted-foreground transition-colors hover:text-foreground"><ChevronDown className={cn("size-3.5 transition-transform duration-200", open && "rotate-180")} />{t("signin.more")}</button>
      {open && children}
    </div>
  )
}

/** Where the data is kept: the adapters that fill the db port, with the chosen one's own settings. */
export function Data({ info, c, update }: W) {
  const { t } = useI18n()
  const dbs = [...fills(info, "db")].sort((a, b) => Number(!!info.profile.plugins?.[b.id]?.enabled) - Number(!!info.profile.plugins?.[a.id]?.enabled))
  const chosen = dbs.find((p) => c.enabled[p.id])
  return (
    <div className="grid gap-4">
      <RadioGroup value={chosen?.id ?? ""} onValueChange={(id) => update((x) => { const p = dbs.find((d) => d.id === id)!; return toggle(info, x, p, true) })}>
        {dbs.map((p) => <Choice key={p.id} value={p.id} checked={chosen?.id === p.id} title={label(p)} aside={info.profile.plugins?.[p.id]?.enabled ? <Badge>{t("data.default")}</Badge> : undefined}>{p.description}</Choice>)}
      </RadioGroup>
      {chosen && <SchemaForm schema={chosen.settings_schema} value={c.settings[chosen.id]} idPrefix={chosen.id} onChange={(v) => update((x) => ({ ...x, settings: { ...x.settings, [chosen.id]: v } }))} />}
      <p className="m-0 text-xs text-muted-foreground">{t("data.more")}</p>
    </div>
  )
}

/** What has no default: the first administrator's name and password, or the settings password when there is no sign-in. Asked once, where it is needed. */
export function CredentialFields({ info, c, update, tried }: W) {
  const { t } = useI18n()
  const slot = settingsPasswordSlot(info, c)
  const first = needsFirst(info, c)
  const userErr = tried && first && !USERNAME.test(c.first.username) ? t("signin.badUsername") : undefined
  const passErr = tried && first && c.first.password.length < MIN_PASSWORD ? t("signin.atLeast8") : undefined
  if (!first && !slot) return null
  return (
        <div className="grid items-start gap-3 rounded-lg bg-muted/40 p-3 sm:grid-cols-2">
          {first && (
            <Field label={t("signin.username")} error={userErr} hint={t("signin.usernameHint")} htmlFor="username">
              <InputGroup dir="ltr"><InputGroupAddon><InputGroupText>@</InputGroupText></InputGroupAddon>
                <InputGroupInput id="username" dir="ltr" autoComplete="username" autoCapitalize="none" spellCheck={false} value={c.first.username} placeholder={t("signin.usernamePh")} onChange={(e) => update((x) => ({ ...x, first: { ...x.first, username: e.target.value.replace(/^@+/, "").replace(/\s/g, "") } }))} />
              </InputGroup>
            </Field>
          )}
          <Field label={first ? t("signin.password") : t("signin.settingsPassword")} error={passErr ?? (tried && slot && !first && c.settingsPassword.length < MIN_PASSWORD ? t("signin.atLeast8") : undefined)} hint={first ? t("signin.passwordHint") : t("signin.settingsPasswordHint")} htmlFor="pw">
            <Input id="pw" dir="ltr" type="password" autoComplete="new-password" value={first ? c.first.password : c.settingsPassword} onChange={(e) => update((x) => (first ? { ...x, first: { ...x.first, password: e.target.value } } : { ...x, settingsPassword: e.target.value }))} />
          </Field>
        </div>
      
  )
}

/** The two kinds that exist, plus any other the product offers; the first administrator or the settings password is asked here. */
export function SignIn({ info, c, update, tried }: W) {
  const { t } = useI18n()
  const auth = authPlugins(info)
  const accounts = auth.find(isAccounts)
  const open = auth.find((p) => p.id === OPEN), local = auth.find((p) => p.id === LOCAL)
  const others = auth.filter((p) => p !== accounts && p !== open && p !== local)
  const kind = signKind(info, c)
  const chosen = auth.find((p) => c.enabled[p.id])
  const pick = (v: string) => update((x) => chooseAuth(info, x, v === "accounts" ? accounts!.id : v === "none" ? (open ?? local)!.id : v))
  const value = kind === "accounts" ? "accounts" : kind === "none" ? "none" : chosen?.id ?? ""
  return (
    <div className="grid gap-4">
      <RadioGroup value={value} onValueChange={pick}>
        {accounts && <Choice value="accounts" checked={kind === "accounts"} title={t("signin.accounts")} aside={<Badge>{t("choose.recommended")}</Badge>}>{t("signin.accountsText")}</Choice>}
        {(open || local) && <Choice value="none" checked={kind === "none"} title={t("signin.none")}>{t("signin.noneText")}</Choice>}
        {others.map((p) => <Choice key={p.id} value={p.id} checked={chosen?.id === p.id} title={label(p)}>{p.description}</Choice>)}
      </RadioGroup>
      {kind === "none" && open && local && (
        <label className="flex cursor-pointer items-start gap-3 rounded-lg border p-3">
          <Switch checked={!!c.enabled[OPEN]} onCheckedChange={(v) => update((x) => chooseAuth(info, x, v ? OPEN : LOCAL))} className="mt-0.5" aria-label={t("signin.protect")} />
          <span className="grid gap-0.5"><span className="text-sm font-medium">{t("signin.protect")}</span><span className="text-xs text-muted-foreground">{c.enabled[OPEN] ? t("signin.protectOn") : t("signin.protectOff")}</span></span>
        </label>
      )}
      <CredentialFields info={info} c={c} update={update} problems={[]} tried={tried} />
      {chosen && <More><SchemaForm schema={chosen.settings_schema} value={c.settings[chosen.id]} idPrefix={chosen.id} onChange={(v) => update((x) => ({ ...x, settings: { ...x.settings, [chosen.id]: v } }))} /></More>}
      <p className="m-0 text-xs text-muted-foreground">{kind === "accounts" ? `${t("signin.firstAdmin")} ` : ""}{t("signin.later")}</p>
    </div>
  )
}

/** Who can open it, and the red callout when a plugin is safe only for less than that. Accepting is allowed. */
export function Access({ info, c, update, tried }: W) {
  const { t } = useI18n()
  const risk = risky(info, c)
  const who = c.reach === "internet" ? t("reach.noteInternet") : t("reach.noteNetwork")
  return (
    <div className="grid gap-4">
      <RadioGroup value={c.reach} onValueChange={(v) => update((x) => ({ ...x, reach: v as Reach, accepted: false }))}>
        {(["this-computer", "network", "internet"] as Reach[]).map((r) => <Choice key={r} value={r} checked={c.reach === r} title={t(`reach.${r}` as "reach.network")} aside={info.reach_default === r ? <Badge variant="outline">{t("reach.suggested")}</Badge> : undefined}>{t(`reach.${r}Text` as "reach.networkText")}</Choice>)}
      </RadioGroup>
      {risk.length > 0 && (
        <Alert variant="destructive" className="border-destructive/50 bg-destructive/5">
          <TriangleAlert />
          <AlertTitle>{t("risk.title", { who })}</AlertTitle>
          <AlertDescription className="grid gap-3">
            <span>{t("risk.text", { names: risk.map((p) => label(p)).join(", "), verb: risk.length > 1 ? t("risk.are") : t("risk.is") })}</span>
            <label className="flex cursor-pointer items-center gap-2 text-foreground"><Checkbox checked={c.accepted} onCheckedChange={(v) => update((x) => ({ ...x, accepted: v === true }))} /><span className="text-sm">{t("risk.accept")}</span></label>
            {tried && !c.accepted && <span role="alert" className="text-xs">{t("risk.tick")}</span>}
          </AlertDescription>
        </Alert>
      )}
    </div>
  )
}
