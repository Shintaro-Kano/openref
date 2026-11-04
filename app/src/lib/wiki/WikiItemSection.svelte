<script lang="ts">
  /* ItemDetail の末尾の Wiki 節: この item を参照するページ（wiki_backlinks {key}）。
     Wiki が無効なら何も描かない。クリック = そのページを新しい Wiki タブで開く。索引の変更（wiki-changed）は touchedAny のときだけ読み直す */
  import { untrack } from "svelte";
  import { t } from "../i18n.svelte";
  import { app } from "../state.svelte";
  import { backlinks, openWikiPage, touchedAny, wk } from "./wiki.svelte";
  import type { WikiBacklink } from "./types";

  let { itemKey }: { itemKey: string } = $props();

  /** 画面のワークスペース（値が変わったときだけ読み直す = 同じ root の状態の読み直しでは読まない） */
  const wsRoot = $derived(wk.status?.root ?? null);
  let pages = $state<WikiBacklink[]>([]);
  let err = $state<string | null>(null);
  let loaded = $state(false);
  let seq = 0;

  function load(key: string) {
    const my = ++seq;
    void backlinks({ key }).then(
      (r) => {
        if (my !== seq) return;
        pages = r.pages;
        err = null;
        loaded = true;
      },
      (e) => {
        if (my !== seq) return;
        pages = [];
        err = String(e);
        loaded = true;
      }
    );
  }

  // 対象の item が変わった（か有効になった・画面のワークスペースが変わった） = 前の結果を消してから読む
  $effect(() => {
    const key = itemKey;
    const on = app.wikiOn;
    void wsRoot; // 読む先 = 画面の root（最後に前面だった OpenNote タブ）。クリックで開くタブも同じ root を継ぐ
    pages = [];
    err = null;
    loaded = false;
    seq++;
    if (on) load(key);
  });
  let seenStamp = untrack(() => wk.stamp);
  $effect(() => {
    const s = wk.stamp;
    if (s === seenStamp) return;
    seenStamp = s;
    untrack(() => {
      if (app.wikiOn && touchedAny(pages.map((p) => p.path))) load(itemKey);
    });
  });
</script>

{#if app.wikiOn}
  <div class="section wk-item">
    <h3>OpenNote</h3>
    {#if err}
      <p class="wk-item-bad">{err.slice(0, 200)}</p>
    {:else if loaded && !pages.length}
      <p class="dim">{t("この文献を参照するページはありません")}</p>
    {/if}
    {#each pages as p (p.path)}
      <button class="wk-item-link" title={p.path} onclick={() => void openWikiPage(p.path, true)}>{p.title}</button>
    {/each}
  </div>
{/if}

<style>
  .wk-item-bad { color: var(--danger); font-size: 12px; overflow-wrap: anywhere; }
  .wk-item-link {
    display: block; width: 100%; border: 0; background: transparent; text-align: left; padding: 3px 0; cursor: pointer;
    color: var(--accent); font-size: 12.5px; overflow-wrap: anywhere;
  }
  .wk-item-link:hover { text-decoration: underline; }
</style>
