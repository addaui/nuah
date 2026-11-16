import { resolve } from "node:path"
import tailwindcss from "@tailwindcss/vite"
import react from "@vitejs/plugin-react"
import { defineConfig } from "vitest/config"

// The setup server serves the build at /faces/setup/ (faces/nuah/setup-web). `bun run dev` proxies the setup API to a running setup
// server: NUAH_SETUP=http://127.0.0.1:3100.
const setup = process.env.NUAH_SETUP ?? "http://127.0.0.1:3100"

export default defineConfig({
  base: "/faces/setup/",
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": resolve(import.meta.dirname, "./src") } },
  server: { proxy: { "/setup": setup, "/healthz": setup, "/readyz": setup } },
  build: {
    outDir: "../../faces/nuah/setup-web",
    emptyOutDir: true,
    target: "es2023",
    sourcemap: false,
    // the setup server's policy forbids inline script and style
    assetsInlineLimit: 0,
    modulePreload: { polyfill: false },
  },
  test: { environment: "jsdom", globals: true },
})
