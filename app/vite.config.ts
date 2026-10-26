import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { execSync } from "node:child_process";

/** ビルド指紋（git short sha·HHmm）。配備したバンドルが最新かを切り分けるため
    ヘッダに小さく表示する（PdfViewer.svelte）。git 不在などで失敗したら "dev"。 */
function buildId(): string {
  try {
    const sha = execSync("git rev-parse --short HEAD", { stdio: ["ignore", "pipe", "ignore"] })
      .toString()
      .trim();
    const now = new Date();
    const hh = String(now.getHours()).padStart(2, "0");
    const mm = String(now.getMinutes()).padStart(2, "0");
    return `${sha}·${hh}${mm}`;
  } catch {
    return "dev";
  }
}

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  define: {
    __BUILD_ID__: JSON.stringify(buildId()),
  },
  server: {
    port: 5173,
    strictPort: true,
    // word/cite-core.js（「DOI / URL を常に含める」の実装を Word アドインと共有。src/lib/cite.ts が import）を dev でも配る。
    // 相対パスは root（app/）基準
    fs: { allow: [".", "../word"] },
  },
  build: {
    target: "safari15",
  },
});
