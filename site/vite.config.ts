import { defineConfig } from "vite"
import react from "@vitejs/plugin-react"

export default defineConfig({
  plugins: [react()],
  resolve: {
    conditions: ["development", "module", "browser", "default"],
  },
  // three.js lives in the lazily loaded hero brush chunk.
  build: { chunkSizeWarningLimit: 700 },
})
