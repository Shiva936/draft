import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

/** Flat, content-stable asset names keep the embedded `console/dist` reproducible. */
export default defineConfig({
  plugins: [react()],
  base: "/",
  build: {
    outDir: "../dist",
    emptyOutDir: true,
    sourcemap: false,
    chunkSizeWarningLimit: 900,
    rollupOptions: {
      output: {
        entryFileNames: "assets/[name].js",
        chunkFileNames: "assets/[name].js",
        assetFileNames: "assets/[name][extname]",
        // Editor language modes load on demand; name their chunks after the
        // language so the embedded asset list stays readable.
        manualChunks(id) {
          const match = /node_modules\/@codemirror\/lang-([a-z+]+)\//.exec(id);
          return match ? `lang-${match[1]}` : undefined;
        },
      },
    },
  },
});
