<script lang="ts">
  /* MY LIBRARY の見出し。文字は現在を示す: 文献 = MY LIBRARY・本棚 = 📚 <本棚名>（title = root）。クリック / 右クリック = メニュー（ShelfMenu） */
  import { t } from "../i18n.svelte";
  import { app, frontShelf } from "../state.svelte";
  import { loadShelves, shelfName, sh } from "./shelf.svelte";

  const s = $derived(frontShelf());

  function open(e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    if (e.type === "click" && sh.menu) {
      sh.menu = null;
      return;
    }
    app.listMenu = null; // 他のメニューと重ねない
    app.labelMenu = false;
    app.folderMenu = null;
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    sh.menu = { x: Math.max(8, Math.min(r.left, window.innerWidth - 340)), y: r.bottom + 4 };
    loadShelves().catch(() => {}); // 履歴の exists を今の状態に（MCP の改名も）。読めなければ持っている物を出す
  }
</script>

<button class="sh-head" title={s ? s.root : t("文献 / 本棚を切り替える")} aria-haspopup="menu" aria-expanded={!!sh.menu} onclick={open} oncontextmenu={open}>
  <span class="sh-head-name">{s ? `📚 ${shelfName(s.root)}` : "MY LIBRARY"}</span><span class="sh-head-caret" aria-hidden="true">▾</span>
</button>

<style>
  :global(.sidebar) .sh-head {
    display: inline-flex; justify-content: flex-start; width: auto; min-width: 0; max-width: 100%; padding: 0 4px; margin-left: -4px; gap: 4px;
    font: inherit; color: inherit; border-radius: 4px;
  }
  :global(.sidebar) .sh-head:hover { color: var(--text); background: var(--bg); }
  .sh-head-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
  .sh-head-caret { font-size: 9px; color: var(--dim); }
</style>
