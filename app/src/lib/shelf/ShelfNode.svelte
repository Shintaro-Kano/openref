<script lang="ts">
  /* サイドバーのフォルダの行。クリック = 前面のタブのフォルダ / ⌘クリック・中クリック = 新しいタブ */
  import Self from "./ShelfNode.svelte";
  import { t } from "../i18n.svelte";
  import { frontShelf } from "../state.svelte";
  import { folderDir, gotoDir, sh, type ShelfIndex } from "./shelf.svelte";
  import type { ShelfFolder } from "./tree";

  let { folder, ix, depth = 0 }: { folder: ShelfFolder; ix: ShelfIndex; depth?: number } = $props();

  const dir = $derived(folderDir(ix, folder));
  const kids = $derived(folder.children.length > 0);
  const open = $derived(!!sh.expanded[folder.id]);
  const active = $derived(frontShelf()?.dir === dir && !frontShelf()?.query);

  function go(e: MouseEvent | KeyboardEvent) {
    if (e instanceof MouseEvent && e.button !== 0) return;
    void gotoDir(dir, e.metaKey);
  }
  function goAux(e: MouseEvent) {
    if (e.button !== 1) return;
    e.preventDefault();
    void gotoDir(dir, true);
  }
</script>

<div
  class="fd-row sh-node"
  role="button"
  tabindex="0"
  style="--depth:{depth}"
  class:active
  onclick={go}
  onauxclick={goAux}
  onkeydown={(e) => {
    if (e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) {
      e.preventDefault();
      go(e);
    }
  }}>
  {#if kids}
    <button
      class="fd-chev"
      class:open
      title={open ? t("折り畳む") : t("展開する")}
      aria-label={open ? t("折り畳む") : t("展開する")}
      aria-expanded={open}
      onclick={(e) => {
        e.stopPropagation();
        sh.expanded[folder.id] = !open;
      }}>
      <svg viewBox="0 0 16 16" width="10" height="10" aria-hidden="true"
        ><path d="M6 3.5 10.5 8 6 12.5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" /></svg>
    </button>
  {:else}
    <span class="fd-chev"></span>
  {/if}
  <span class="name" title={folder.name}>{folder.name}</span>
  <span class="n">{folder.totalBooks}</span>
</div>
{#if kids && open}
  {#each folder.children as c (c.id)}
    <Self folder={c} {ix} depth={depth + 1} />
  {/each}
{/if}
