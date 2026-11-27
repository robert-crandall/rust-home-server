import solid from "@solidjs/vite-plugin";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vitest/config";

const apiTarget = process.env.VITE_API_ORIGIN ?? "http://127.0.0.1:8080";

export default defineConfig({
  plugins: [tailwindcss(), solid()],
  build: {
    outDir: "build",
    emptyOutDir: true,
    target: "es2022",
  },
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      "/api": {
        target: apiTarget,
        changeOrigin: false,
      },
      "/healthz": {
        target: apiTarget,
        changeOrigin: false,
      },
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
  },
});
