<script lang="ts">
  /* セグメント [MY LIBRARY | MY NOTE]（サイドバー最上段）。OpenNote が無効なら何も描かない。
     押下の規則は wiki.svelte.ts の switchMode */
  import { t } from "../i18n.svelte";
  import { app, frontWiki } from "../state.svelte";
  import { switchMode } from "./wiki.svelte";

  const wiki = $derived(!!frontWiki());
</script>

{#if app.wikiOn}
  <div class="wk-seg" role="group" aria-label={t("表示の切替")}>
    <button class="wk-seg-btn" class:wk-on={!wiki} aria-pressed={!wiki} onclick={() => void switchMode("library")}>MY LIBRARY</button>
    <button class="wk-seg-btn" class:wk-on={wiki} aria-pressed={wiki} onclick={() => void switchMode("wiki")}>MY NOTE</button>
  </div>
{/if}

<style>
  .wk-seg {
    display: flex; gap: 0; margin: 0 2px 8px; padding: 2px; border: 1px solid var(--border); border-radius: 7px; background: var(--bg);
    flex-shrink: 0;
  }
  .wk-seg-btn {
    flex: 1 1 0; justify-content: center; padding: 4px 0; border: 0; border-radius: 5px; background: transparent;
    font-size: 11px; font-weight: 600; letter-spacing: 0.3px; color: var(--dim); cursor: pointer;
  }
  .wk-seg-btn:hover { color: var(--text); background: transparent; }
  .wk-seg-btn.wk-on { background: var(--panel); color: var(--accent); box-shadow: 0 0 0 1px var(--border); }
</style>
