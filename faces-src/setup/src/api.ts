/** What the setup server says and takes (`/setup/state`, `/setup/check`, `/setup/save`). The Face holds no rules: the Core's own Resolver judges the draft. */

export type Power = "stores" | "listens" | "reaches out" | "changes settings"
export type Role = "adapter" | "projection" | "face" | "provider" | "theme" | "feature"
export type Reach = "this-computer" | "network" | "internet"

export interface Problem { code: string; plugins: string[]; message: string }

export interface Plugin {
  id: string
  scope: string
  version: string
  name: string
  description: string
  role: Role
  powers: Power[]
  face: string | null
  safe_reach: Reach | null
  class: { class: "platform" | "feature"; reasons: string[] }
  fills: string | null
  provides: string[]
  requires: { plugins: string[]; ports: string[]; extension_points: string[]; capabilities: string[] }
  settings_schema: { properties?: Record<string, import("./schema").Prop>; required?: string[] } | null
  /** The fields of other plugins' data it reads (ADR 0091). */
  reads?: import("./report").Read[]
}

export interface Entry { enabled: boolean; grants?: string[]; settings?: Record<string, unknown> }

export interface ProfileFile {
  name: string
  contract?: string
  plugins?: Record<string, Entry>
  bindings?: Record<string, string>
  reach?: Reach
  accepted_risks?: string[]
  [key: string]: unknown
}

export interface Info {
  distribution: string
  profile: ProfileFile
  core_ports: string[]
  plugins: Plugin[]
  problems: Problem[]
  path: string
  file_exists: boolean
  load_error?: string | null
  notice?: string | null
  reach_default?: Reach | null
}

// The setup code is typed once and kept only for this tab.
let code = ""
try { code = sessionStorage.getItem("nuah-setup-code") ?? "" } catch { /* storage may be blocked */ }
export const setupCode = () => code
export function rememberCode(c: string) {
  code = c
  try { sessionStorage.setItem("nuah-setup-code", c) } catch { /* keep it in memory only */ }
}

export interface Reply<T> { status: number; body: T }

export async function send<T = unknown>(path: string, body?: unknown): Promise<Reply<T>> {
  const headers: Record<string, string> = { "X-Nuah-Setup": code, "X-Nuah-Face": "setup" }
  if (body !== undefined) headers["Content-Type"] = "application/json"
  const r = await fetch(path, { method: body === undefined ? "GET" : "POST", headers, cache: "no-store", body: body === undefined ? undefined : JSON.stringify(body) })
  return { status: r.status, body: (await r.json().catch(() => ({}))) as T }
}
