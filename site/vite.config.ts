import { defineConfig } from "vite"
import react from "@vitejs/plugin-react"

export default defineConfig({
  plugins: [react()],
  // three.js lives in the lazily loaded hero brush chunk.
  build: { chunkSizeWarningLimit: 700 },
  // Previews are shared over Tailscale (studio.*.ts.net).
  server: { allowedHosts: [".ts.net"] },
  preview: { allowedHosts: [".ts.net"] },
})
