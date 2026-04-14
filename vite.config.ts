import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "path";

export default defineConfig({
  root: "desktop-ui",
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./desktop-ui/src"),
    },
  },

  server: {
    port: 1420,
    strictPort: true,
  },

  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
});
