<script lang="ts">
  /* 本棚の左バー（SHELF）の見出し = 今の本棚 📚 <本棚名>（title = root）。クリック / 右クリック = 本棚の選択のメニュー（ShelfMenu =
     履歴の本棚・別のフォルダを本棚として開く…・この本棚の設定）。文献へ戻るのは最上段のセグメント */
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

{#if s}
  <button class="sh-head" title={s.root} aria-haspopup="menu" aria-expanded={!!sh.menu} onclick={open} oncontextmenu={open}>
    <span class="sh-head-name">📚 {shelfName(s.root)}</span><span class="sh-head-caret" aria-hidden="true">▾</span>
  </button>
{/if}

<style>
  :global(.ls-nav) .sh-head {
    display: inline-flex; justify-content: flex-start; width: auto; min-width: 0; max-width: 100%; padding: 0 4px; margin-left: -4px; gap: 4px;
    font: inherit; color: inherit; border-radius: 4px;
  }
  :global(.ls-nav) .sh-head:hover { color: var(--text); background: var(--bg); }
  .sh-head-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
  .sh-head-caret { font-size: 9px; color: var(--dim); }
</style>
