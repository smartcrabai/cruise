import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    server: {
      deps: {
        // @asamuzakjp/css-color@5.1.5 uses top-level await in its ESM build,
        // which Node >= 22 cannot load via require(). Inline it through Vite's
        // pipeline so the TLA is handled correctly.
        inline: ["@asamuzakjp/css-color"],
      },
    },
  },
});
