<script lang="ts">
  /* 本棚のサイドバー: セグメント・見出し（ShelfHead）・すべての本 + フォルダの木・温めの進捗の 1 行。LABELS・ゴミ箱・＋・… は描かない（本棚に無い物を見せない）。
     本棚を離れる = ここが消える = 温めを止める（読み手のタブも本棚を離れた扱い） */
  import { onDestroy } from "svelte";
  import { t, tn } from "../i18n.svelte";
  import { frontShelf } from "../state.svelte";
  import WikiSegment from "../wiki/WikiSegment.svelte";
  import ShelfHead from "./ShelfHead.svelte";
  import ShelfMenu from "./ShelfMenu.svelte";
  import ShelfNode from "./ShelfNode.svelte";
  import { dropCovers, gotoDir, loadTree, sh, shelfIndex, startWarm, stopWarm } from "./shelf.svelte";

  const s = $derived(frontShelf());
  const ix = $derived(shelfIndex(s?.root));

  // 開いた本棚の木を読む（表があれば走査なし）
  $effect(() => {
    const root = s?.root;
    if (root && !sh.trees[root]) void loadTree(root);
  });

  // 開いた本棚の背景の温め。木が読み直されたら（collapse・読み直し）やり直す・離れたら止める
  $effect(() => {
    const root = s?.root;
    if (!root || !ix) return;
    startWarm(root);
    return stopWarm;
  });

  onDestroy(dropCovers);

  function all(e: MouseEvent) {
    if (e.button === 0) void gotoDir(null, e.metaKey);
  }
  function allAux(e: MouseEvent) {
    if (e.button !== 1) return;
    e.preventDefault();
    void gotoDir(null, true);
  }
</script>

<nav class="sidebar">
  <WikiSegment />
  <div class="sep sec-head"><ShelfHead /></div>
  <button class:active={s?.dir === null && !s?.query} onclick={all} onauxclick={allAux}>
    <span>{t("すべての本")}</span><span class="n">{ix?.tree.totalBooks ?? ""}</span>
  </button>
  {#if ix}
    {#each ix.tree.children as f (f.id)}
      <ShelfNode folder={f} {ix} />
    {/each}
    {#if ix.unreadable > 0}
      <p class="sh-side-note" title={t("権限がない等で開けなかったフォルダは飛ばしています")}>{t("読めなかったフォルダ")} {ix.unreadable}</p>
    {/if}
  {/if}
  {#if sh.warm?.slow}
    <p class="sh-side-note" role="status">{t("表紙を作成中")} {sh.warm.done} / {tn(sh.warm.total, "冊")}</p>
  {/if}
</nav>
<ShelfMenu />

<style>
  .sh-side-note { margin: 8px 10px 2px; font-size: 11px; color: var(--dim); }
</style>
