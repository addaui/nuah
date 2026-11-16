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
import { authPlugins, label, chooseAuth, fills, isAccounts, LOCAL, MIN_PASSWORD, needsFirst, OPEN, risky, settingsPasswordSlot, signKind, toggle, USERNAME } from "@/wizard"
import type { Reach } from "@/api"

export function Instance({ info, c, update, tried }: W) {
  return <Field label="Instance name" error={tried && !c.name.trim() ? "Give this instance a name." : undefined} hint="Shown in the console and in logs, like “Home” or “Studio”." htmlFor="name"><Input id="name" value={c.name} onChange={(e) => update((x) => ({ ...x, name: e.target.value }))} placeholder={info.distribution} /></Field>
}

function More({ children }: { children: React.ReactNode }) {
  const [open, setOpen] = useState(false)
  return (
    <div className="grid gap-3">
      <button type="button" onClick={() => setOpen((o) => !o)} aria-expanded={open} className="flex items-center gap-1 justify-self-start text-xs text-muted-foreground transition-colors hover:text-foreground"><ChevronDown className={cn("size-3.5 transition-transform duration-200", open && "rotate-180")} />More settings</button>
      {open && children}
    </div>
  )
}

/** Where the data is kept: the adapters that fill the db port, with the chosen one's own settings. */
export function Data({ info, c, update }: W) {
  const dbs = [...fills(info, "db")].sort((a, b) => Number(!!info.profile.plugins?.[b.id]?.enabled) - Number(!!info.profile.plugins?.[a.id]?.enabled))
  const chosen = dbs.find((p) => c.enabled[p.id])
  return (
    <div className="grid gap-4">
      <RadioGroup value={chosen?.id ?? ""} onValueChange={(id) => update((x) => { const p = dbs.find((d) => d.id === id)!; return toggle(info, x, p, true) })}>
        {dbs.map((p) => <Choice key={p.id} value={p.id} checked={chosen?.id === p.id} title={label(p)} aside={info.profile.plugins?.[p.id]?.enabled ? <Badge>Default</Badge> : undefined}>{p.description}</Choice>)}
      </RadioGroup>
      {chosen && <SchemaForm schema={chosen.settings_schema} value={c.settings[chosen.id]} idPrefix={chosen.id} onChange={(v) => update((x) => ({ ...x, settings: { ...x.settings, [chosen.id]: v } }))} />}
      <p className="m-0 text-xs text-muted-foreground">More data adapters can be added later as plugins.</p>
    </div>
  )
}

/** What has no default: the first administrator's name and password, or the settings password when there is no sign-in. Asked once, where it is needed. */
export function CredentialFields({ info, c, update, tried }: W) {
  const slot = settingsPasswordSlot(info, c)
  const first = needsFirst(info, c)
  const userErr = tried && first && !USERNAME.test(c.first.username) ? "Choose a username of 2 to 32 letters, numbers, dots, dashes or underscores." : undefined
  const passErr = tried && first && c.first.password.length < MIN_PASSWORD ? "At least 8 characters." : undefined
  if (!first && !slot) return null
  return (
        <div className="grid items-start gap-3 rounded-lg bg-muted/40 p-3 sm:grid-cols-2">
          {first && (
            <Field label="Username" error={userErr} hint="What you type, with your password, to sign in. Letters, numbers, dots, dashes." htmlFor="username">
              <InputGroup><InputGroupAddon><InputGroupText>@</InputGroupText></InputGroupAddon>
                <InputGroupInput id="username" autoComplete="username" autoCapitalize="none" spellCheck={false} value={c.first.username} placeholder="Your Username" onChange={(e) => update((x) => ({ ...x, first: { ...x.first, username: e.target.value.replace(/^@+/, "").replace(/\s/g, "") } }))} />
              </InputGroup>
            </Field>
          )}
          <Field label={first ? "Password" : "Settings password"} error={passErr ?? (tried && slot && !first && c.settingsPassword.length < MIN_PASSWORD ? "At least 8 characters." : undefined)} hint={first ? "At least 8 characters." : "Asked only when opening the settings area. At least 8 characters."} htmlFor="pw">
            <Input id="pw" type="password" autoComplete="new-password" value={first ? c.first.password : c.settingsPassword} onChange={(e) => update((x) => (first ? { ...x, first: { ...x.first, password: e.target.value } } : { ...x, settingsPassword: e.target.value }))} />
          </Field>
        </div>
      
  )
}

/** The two kinds that exist, plus any other the product offers; the first administrator or the settings password is asked here. */
export function SignIn({ info, c, update, tried }: W) {
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
        {accounts && <Choice value="accounts" checked={kind === "accounts"} title="Accounts" aside={<Badge>Recommended</Badge>}>You add the people who can use it, each with a username and a password. The right choice if anyone else will use it.</Choice>}
        {(open || local) && <Choice value="none" checked={kind === "none"} title="No sign-in">Nobody signs in. Only for one person, on a computer or network you trust.</Choice>}
        {others.map((p) => <Choice key={p.id} value={p.id} checked={chosen?.id === p.id} title={label(p)}>{p.description}</Choice>)}
      </RadioGroup>
      {kind === "none" && open && local && (
        <label className="flex cursor-pointer items-start gap-3 rounded-lg border p-3">
          <Switch checked={!!c.enabled[OPEN]} onCheckedChange={(v) => update((x) => chooseAuth(info, x, v ? OPEN : LOCAL))} className="mt-0.5" aria-label="Protect the settings area with a password" />
          <span className="grid gap-0.5"><span className="text-sm font-medium">Protect the settings area with a password</span><span className="text-xs text-muted-foreground">{c.enabled[OPEN] ? "Anyone can use it, but changing settings asks for the password." : "You are always the administrator, and nothing asks for a password."}</span></span>
        </label>
      )}
      <CredentialFields info={info} c={c} update={update} problems={[]} tried={tried} />
      {chosen && <More><SchemaForm schema={chosen.settings_schema} value={c.settings[chosen.id]} idPrefix={chosen.id} onChange={(v) => update((x) => ({ ...x, settings: { ...x.settings, [chosen.id]: v } }))} /></More>}
      <p className="m-0 text-xs text-muted-foreground">{kind === "accounts" ? "You are the first administrator. Add everyone else after setup, under Accounts in the console. " : ""}More ways to sign in, such as email or single sign-on, can be added later as plugins.</p>
    </div>
  )
}

const REACH_TEXT: Record<Reach, [string, string]> = {
  "this-computer": ["Only this computer", "Nobody else can open it."],
  network: ["My home or office network", "Any phone or computer on the same Wi-Fi or router, but not the internet."],
  internet: ["Anyone on the internet", "Through a domain name, a proxy or an open port."],
}

/** Who can open it, and the red callout when a plugin is safe only for less than that. Accepting is allowed. */
export function Access({ info, c, update, tried }: W) {
  const risk = risky(info, c)
  const who = c.reach === "internet" ? "anyone on the internet" : "anyone on your home or office network"
  return (
    <div className="grid gap-4">
      <RadioGroup value={c.reach} onValueChange={(v) => update((x) => ({ ...x, reach: v as Reach, accepted: false }))}>
        {(Object.keys(REACH_TEXT) as Reach[]).map((r) => <Choice key={r} value={r} checked={c.reach === r} title={REACH_TEXT[r][0]} aside={info.reach_default === r ? <Badge variant="outline">Suggested for this setup</Badge> : undefined}>{REACH_TEXT[r][1]}</Choice>)}
      </RadioGroup>
      {risk.length > 0 && (
        <Alert variant="destructive" className="border-destructive/50 bg-destructive/5">
          <TriangleAlert />
          <AlertTitle>Your data and settings could be seen by {who}</AlertTitle>
          <AlertDescription className="grid gap-3">
            <span>{risk.map((p) => label(p)).join(" and ")} {risk.length > 1 ? "are" : "is"} safe only for this computer. Without a sign-in, whoever can open this address can read and change everything in it. Be sure you know what you are doing, for example a sign-in portal or a firewall in front of it.</span>
            <label className="flex cursor-pointer items-center gap-2 text-foreground"><Checkbox checked={c.accepted} onCheckedChange={(v) => update((x) => ({ ...x, accepted: v === true }))} /><span className="text-sm">I understand and want to go on</span></label>
            {tried && !c.accepted && <span role="alert" className="text-xs">Tick this to go on, or choose accounts with passwords.</span>}
          </AlertDescription>
        </Alert>
      )}
    </div>
  )
}
