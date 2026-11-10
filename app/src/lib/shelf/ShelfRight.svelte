<script lang="ts">
  /* 本棚の右ペイン（グリッドの右・読み手の右）= 本の情報。開閉は app.detailOpen（⌘⌥I）。
     チャットはアプリ右端の柱（ChatDock）= ここには置かない（本棚・本のタブでは文脈なし = viewing） */
  import { t } from "../i18n.svelte";
  import { openExternal, revealBook } from "./shelf.svelte";
  import { fmtSize, type ShelfBookTab } from "./tree";

  let { book }: { book: ShelfBookTab | null } = $props();

  const when = (s: number) => new Date(s * 1000).toLocaleString();
</script>

<aside class="sh-right">
  <div class="sh-info">
    {#if book}
      {@const b = book}
      <h2 class="sh-info-title">{b.title}</h2>
      <dl>
        <dt>{t("パス")}</dt>
        <dd class="sh-info-path">{b.path}</dd>
        <dt>{t("サイズ")}</dt>
        <dd>{fmtSize(b.size)}</dd>
        <dt>{t("更新日")}</dt>
        <dd>{when(b.mtime)}</dd>
      </dl>
      <div class="sh-info-acts">
        <button onclick={() => revealBook(b.root, b.id)}>{t("Finder で表示")}</button>
        <button onclick={() => openExternal(b.root, b.id)}>{t("外部で開く")}</button>
      </div>
    {:else}
      <p class="sh-info-none">{t("開いた本・右クリックした本の情報をここに出します")}</p>
    {/if}
  </div>
</aside>

<style>
  .sh-right { border-left: 1px solid var(--border); background: var(--panel); min-width: 0; min-height: 0; display: flex; flex-direction: column; }
  .sh-info { padding: 14px 16px; overflow-y: auto; min-height: 0; flex: 1; }
  .sh-info-title { font-size: 14px; font-weight: 600; line-height: 1.4; margin-bottom: 12px; overflow-wrap: anywhere; }
  .sh-info dl { display: grid; grid-template-columns: auto 1fr; gap: 6px 12px; font-size: 12px; }
  .sh-info dt { color: var(--dim); white-space: nowrap; }
  .sh-info dd { margin: 0; min-width: 0; }
  .sh-info-path { overflow-wrap: anywhere; }
  .sh-info-acts { display: flex; gap: 8px; margin-top: 14px; flex-wrap: wrap; }
  .sh-info-acts button { padding: 4px 10px; font-size: 12px; border: 1px solid var(--border); border-radius: 6px; background: var(--panel); }
  .sh-info-acts button:hover { border-color: var(--accent); }
  .sh-info-none { color: var(--dim); font-size: 12px; line-height: 1.6; }
</style>
