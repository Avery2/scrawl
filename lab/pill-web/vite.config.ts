import { execFile, execFileSync } from "node:child_process"
import { existsSync, readFileSync, watch } from "node:fs"
import { homedir } from "node:os"
import { join, resolve } from "node:path"
import { promisify } from "node:util"
import { defineConfig, type Plugin } from "vite"

const run = promisify(execFile)
const MODEL = resolve(import.meta.dirname, "../../src/native/pill/model.rs")
const TUNE = process.env.SCRAWL_TUNE ?? join(homedir(), ".config/scrawl/tune.json")

/** The `scrawl` CLI that edits tune.json: `$SCRAWL_BIN`, the installed app, or PATH. */
function scrawlBin(): string | null {
  const candidates = [
    process.env.SCRAWL_BIN,
    join(homedir(), "Applications/Scrawl.app/Contents/MacOS/scrawl"),
    join(homedir(), "bin/scrawl"),
  ]
  return candidates.find((p) => p && existsSync(p)) ?? null
}

function body(req: import("node:http").IncomingMessage): Promise<string> {
  return new Promise((ok, fail) => {
    let text = ""
    req.on("data", (c) => (text += c))
    req.on("end", () => ok(text))
    req.on("error", fail)
  })
}

/** `GET /api/tune` reads Kit's tune.json; `POST /api/apply` writes it through `scrawl tune set`. */
function tuneApi(): Plugin {
  return {
    name: "scrawl-tune-api",
    configureServer(server) {
      server.middlewares.use(async (req, res, next) => {
        if (!req.url?.startsWith("/api/")) return next()
        const send = (code: number, value: unknown) => {
          res.statusCode = code
          res.setHeader("content-type", "application/json")
          res.end(JSON.stringify(value))
        }
        try {
          if (req.url === "/api/tune" && req.method === "GET") {
            const text = existsSync(TUNE) ? readFileSync(TUNE, "utf8") : "{}"
            return send(200, { path: TUNE, values: JSON.parse(text) })
          }
          if (req.url === "/api/apply" && req.method === "POST") {
            const { values } = JSON.parse(await body(req)) as { values: Record<string, number> }
            const bin = scrawlBin()
            if (!bin) return send(500, { error: "no scrawl CLI found (set SCRAWL_BIN)" })
            const lines: string[] = []
            // One at a time: each `tune set` reads, edits and rewrites the file.
            for (const [key, value] of Object.entries(values)) {
              if (typeof value !== "number" || !Number.isFinite(value)) continue
              const { stdout } = await run(bin, ["tune", "set", key, String(value)])
              lines.push(stdout.trim())
            }
            return send(200, { path: TUNE, bin, lines })
          }
          send(404, { error: "unknown endpoint" })
        } catch (err) {
          send(500, { error: String(err instanceof Error ? err.message : err) })
        }
      })
    },
  }
}

/** Rebuilds public/pill.wasm when the Rust model changes and reloads the page. */
function modelWatch(): Plugin {
  return {
    name: "scrawl-model-watch",
    configureServer(server) {
      let timer: ReturnType<typeof setTimeout> | undefined
      const rebuild = () => {
        clearTimeout(timer)
        timer = setTimeout(() => {
          try {
            execFileSync("sh", [resolve(import.meta.dirname, "build-wasm.sh")], { stdio: "inherit" })
            server.ws.send({ type: "full-reload" })
          } catch (err) {
            server.config.logger.error(`wasm build failed: ${err}`)
          }
        }, 150)
      }
      for (const file of [MODEL, resolve(import.meta.dirname, "wasm/src/lib.rs")]) {
        if (existsSync(file)) watch(file, rebuild)
      }
    },
  }
}

export default defineConfig({
  plugins: [tuneApi(), modelWatch()],
  optimizeDeps: { exclude: ["knobs"] },
  server: {
    host: "127.0.0.1",
    port: Number(process.env.PORT ?? 4466),
    strictPort: true,
    allowedHosts: [".ts.net"],
    hmr: { clientPort: process.env.HMR_CLIENT_PORT ? Number(process.env.HMR_CLIENT_PORT) : undefined },
  },
})
