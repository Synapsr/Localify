import { defineConfig } from "vite";

export default defineConfig({
  root: "desktop-ui",
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
});
