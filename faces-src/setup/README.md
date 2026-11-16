# Nuah setup Face

The setup wizard: a React 19 and Tailwind v4 app built with shadcn/ui on Base UI ([ADR 0086](https://docs.addaui.com/nuah/adr/0086-every-interface-is-built-with-shadcn-ui-on-the-shadcn-default-theme-and-follows-the-emil-kowalski-skills)).
It talks to the setup server (`/setup/state`, `/setup/check`, `/setup/save`) and holds no rules: the Core's own Resolver judges every draft.

```
bun install
bun run dev        # NUAH_SETUP=http://127.0.0.1:3100 proxies the setup API to a running setup server
bun run test       # the wizard's logic
bun run build      # writes ../../faces/nuah/setup-web, which is committed and served at /faces/setup/
```

`src/lib/WheelSelect.tsx` and `src/lib/Picker.tsx` are kept for reuse and are not used by the wizard now.
