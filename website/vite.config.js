import { defineConfig } from "vite";

export default defineConfig({
  appType: "spa",
  base: "./",
  build: {
    emptyOutDir: true
  }
});
