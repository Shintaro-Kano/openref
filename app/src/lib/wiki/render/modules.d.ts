// 型を同梱しない markdown-it のプラグイン（OpenNote の描画）
declare module "markdown-it-footnote" {
  import type { MarkdownIt } from "markdown-it";
  const plugin: (md: MarkdownIt) => void;
  export default plugin;
}
declare module "markdown-it-task-lists" {
  import type { MarkdownIt } from "markdown-it";
  const plugin: (md: MarkdownIt, opts?: { enabled?: boolean; label?: boolean; labelAfter?: boolean }) => void;
  export default plugin;
}
