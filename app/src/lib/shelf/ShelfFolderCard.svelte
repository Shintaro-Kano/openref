<script lang="ts">
  /* フォルダのカード。代表の表紙を 3 冊まで束ねる */
  import { tn } from "../i18n.svelte";
  import ShelfCover from "./ShelfCover.svelte";
  import { repBooks, type ShelfFolder } from "./tree";

  let { root, folder, dir, onopen }: { root: string; folder: ShelfFolder; dir: string | null; onopen: (e: MouseEvent | KeyboardEvent) => void } = $props();
  const covers = $derived(repBooks(folder));
</script>

<div class="sh-folder" role="button" tabindex="0" title={folder.name} onclick={onopen} onkeydown={(e) => e.key === "Enter" && onopen(e)}>
  <div class="sh-stack" class:sh-single={covers.length <= 1}>
    {#each covers as b, i (b.id)}
      <div class="sh-stack-item" style="--i:{i}"><ShelfCover {root} book={b} {dir} fallback={false} /></div>
    {/each}
  </div>
  <div class="sh-folder-name">{folder.name}</div>
  <div class="sh-folder-count">{tn(folder.totalBooks, "冊")}</div>
</div>

<style>
  .sh-folder {
    display: flex; flex-direction: column; align-items: center; gap: 4px; padding: 14px 10px 12px; min-width: 0; cursor: pointer; outline: none;
    border: 1px solid var(--border); border-radius: 8px; background: var(--panel); transition: border-color 0.14s ease, transform 0.14s ease;
  }
  .sh-folder:hover, .sh-folder:focus-visible { border-color: var(--accent); transform: translateY(-2px); }
  .sh-stack { position: relative; width: 110px; height: 118px; margin-bottom: 6px; }
  .sh-stack-item {
    position: absolute; bottom: 0; left: 50%; width: 78px; height: 104px; display: flex; align-items: flex-end; justify-content: center;
    transform-origin: bottom center; transform: translateX(-50%) rotate(calc((var(--i) - 1) * 8deg)) translateX(calc((var(--i) - 1) * 26px)); z-index: calc(3 - var(--i));
  }
  .sh-stack-item :global(.sh-cover) { box-shadow: 0 5px 12px rgba(0, 0, 0, 0.32); }
  .sh-stack.sh-single .sh-stack-item { transform: translateX(-50%); }
  /* 特集名入りの長い号名を読めるよう 3 行まで */
  .sh-folder-name {
    font-size: 12.5px; line-height: 1.45; text-align: center; overflow-wrap: anywhere; max-width: 100%; overflow: hidden;
    display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 3; line-clamp: 3;
  }
  .sh-folder-count { font-size: 10.5px; color: var(--dim); }
</style>
