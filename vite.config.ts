import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Tauri 开发服务器固定端口 1420。
export default defineConfig({
  root: "ui",
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { outDir: "../dist", emptyOutDir: true, target: "es2022" },
  test: { root: ".", environment: "jsdom", include: ["ui/**/*.test.ts", "ui/**/*.test.tsx"] },
});
