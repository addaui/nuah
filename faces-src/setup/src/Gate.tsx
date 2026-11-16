import { useState } from "react"
import { Layers } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Input } from "@/components/ui/input"

/** The setup server prints a one-time code when it starts; it is typed here once, so only whoever can read the log can change the setup. */
export function Gate({ wrong, onCode }: { wrong: boolean; onCode: (code: string) => void }) {
  const [code, setCode] = useState("")
  return (
    <div className="grid min-h-svh place-items-center px-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <div className="flex items-center gap-2 text-sm font-medium"><span className="grid size-6 place-items-center rounded-md bg-primary text-primary-foreground"><Layers className="size-3.5" /></span>Setup</div>
          <CardTitle>Enter the setup code</CardTitle>
          <CardDescription>The server printed it when it started: look for “setup code” in its log. It changes every start.</CardDescription>
        </CardHeader>
        <CardContent>
          <form className="grid gap-3" onSubmit={(e) => { e.preventDefault(); onCode(code.trim()) }}>
            <Input autoFocus value={code} onChange={(e) => setCode(e.target.value)} aria-label="Setup code" aria-invalid={wrong} autoComplete="off" spellCheck={false} className="font-mono" />
            {wrong && <p role="alert" className="m-0 text-xs text-destructive">That code is not right.</p>}
            <Button type="submit" disabled={!code.trim()}>Continue</Button>
          </form>
        </CardContent>
      </Card>
    </div>
  )
}
