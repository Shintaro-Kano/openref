<script lang="ts">
  /* 表紙: <thumbs_dir>/<id>-<mtime>.png を asset:// で読む。404 = まだ作っていない → shelf_thumb で作って差し替える。
     作れない本は代わりの表紙（題名から色を決める）。fallback = false のカード（フォルダの束）は何も出さない */
  import { coverUrl, needCover } from "./shelf.svelte";
  import { hueOf, type ShelfBook } from "./tree";

  let { root, book, dir, fallback = true }: { root: string; book: ShelfBook; dir: string | null; fallback?: boolean } = $props();

  let phase = $state<"img" | "wait" | "none">("img");
  /** 作り直した回数（同じ URL の <img> を作り直す。asset_reply は query を見ない = 失敗の写しを避ける） */
  let retry = $state(0);
  const src = $derived(dir ? coverUrl(dir, book) + (retry ? `?r=${retry}` : "") : null);

  async function missing() {
    if (retry || !dir) {
      phase = "none";
      return;
    }
    phase = "wait";
    const ok = await needCover(root, book.id);
    retry++;
    phase = ok ? "img" : "none";
  }
</script>

{#if phase === "img" && src}
  {#key src}
    <img class="sh-cover" {src} alt="" loading="lazy" draggable="false" onerror={missing} />
  {/key}
{:else if fallback}
  <div class="sh-fallback" style="--h:{hueOf(book.title)}"><span>{book.title}</span></div>
{/if}

<style>
  .sh-cover { max-width: 100%; max-height: 100%; border-radius: 3px; background: #fff; }
  .sh-fallback {
    width: 76%; height: 96%; border-radius: 3px; padding: 12px 10px; overflow: hidden; display: flex; align-items: flex-start;
    background: linear-gradient(160deg, hsl(var(--h), 26%, 34%), hsl(var(--h), 30%, 20%));
  }
  .sh-fallback span {
    font-size: 11px; line-height: 1.5; color: rgba(255, 255, 255, 0.9);
    display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 6; line-clamp: 6; overflow: hidden;
  }
</style>
