<script lang="ts">
  /* 本のカード。クリック = 読み手を新しいタブで・右クリック = 右ペインの本の情報へ・hover = [Finder で表示] [外部で開く] */
  import { t } from "../i18n.svelte";
  import ShelfCover from "./ShelfCover.svelte";
  import { bookTab, openBook, openExternal, revealBook, sh } from "./shelf.svelte";
  import { fmtSize, type ShelfBook } from "./tree";

  let { root, book, dir, small = false, showFolder = false }: { root: string; book: ShelfBook; dir: string | null; small?: boolean; showFolder?: boolean } = $props();
</script>

<div
  class="sh-book"
  class:sh-small={small}
  role="button"
  tabindex="0"
  title={book.title}
  onclick={() => void openBook(root, book)}
  onkeydown={(e) => e.key === "Enter" && e.target === e.currentTarget && void openBook(root, book)}
  oncontextmenu={(e) => {
    e.preventDefault();
    sh.info = bookTab(root, book);
  }}>
  <div class="sh-book-cover">
    <ShelfCover {root} {book} {dir} />
    <div class="sh-book-acts">
      <button
        title={t("Finder で表示")}
        aria-label={t("Finder で表示")}
        onclick={(e) => {
          e.stopPropagation();
          revealBook(root, book.id);
        }}>
        <svg width="13" height="13" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"
          ><path d="M1.5 3A1.5 1.5 0 0 1 3 1.5h3.1c.4 0 .78.16 1.06.44L8.6 3.4c.1.1.22.15.35.15H13A1.5 1.5 0 0 1 14.5 5v7A1.5 1.5 0 0 1 13 13.5H3A1.5 1.5 0 0 1 1.5 12V3Z" /></svg>
      </button>
      <button
        title={t("外部で開く")}
        aria-label={t("外部で開く")}
        onclick={(e) => {
          e.stopPropagation();
          openExternal(root, book.id);
        }}>
        <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"
          ><path d="M6.5 3.5H3.5a1 1 0 0 0-1 1v8a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1V9.5" /><path d="M9.5 2.5h4v4" /><path d="M13.5 2.5 7.5 8.5" /></svg>
      </button>
    </div>
  </div>
  <div class="sh-book-title">{book.title}</div>
  <div class="sh-book-meta">
    {#if showFolder}<span class="sh-chip">{book.folder}</span>{/if}
    <span>{fmtSize(book.size)}</span>
  </div>
</div>

<style>
  .sh-book { display: flex; flex-direction: column; gap: 3px; cursor: pointer; outline: none; min-width: 0; }
  /* 棚に立っている表現: 下端揃え + 床の影 */
  .sh-book-cover { position: relative; height: 198px; margin-bottom: 6px; display: flex; align-items: flex-end; justify-content: center; }
  .sh-small .sh-book-cover { height: 150px; }
  .sh-book-cover :global(.sh-cover), .sh-book-cover :global(.sh-fallback) {
    box-shadow: 0 6px 14px rgba(0, 0, 0, 0.28), 0 14px 22px -12px rgba(0, 0, 0, 0.4); transition: transform 0.16s ease, box-shadow 0.16s ease;
  }
  .sh-book:hover .sh-book-cover :global(.sh-cover), .sh-book:hover .sh-book-cover :global(.sh-fallback),
  .sh-book:focus-visible .sh-book-cover :global(.sh-cover), .sh-book:focus-visible .sh-book-cover :global(.sh-fallback) {
    transform: translateY(-5px); box-shadow: 0 10px 20px rgba(0, 0, 0, 0.32), 0 22px 30px -14px rgba(0, 0, 0, 0.45);
  }
  .sh-book-acts { position: absolute; top: 4px; right: 4px; display: flex; gap: 4px; opacity: 0; transition: opacity 0.14s ease; }
  .sh-book:hover .sh-book-acts, .sh-book:focus-within .sh-book-acts { opacity: 1; }
  .sh-book-acts button {
    width: 26px; height: 26px; display: grid; place-items: center; border-radius: 6px; color: #d5dbe3;
    background: rgba(16, 19, 24, 0.82); border: 1px solid rgba(255, 255, 255, 0.14);
  }
  .sh-book-acts button:hover { color: #fff; background: rgba(28, 33, 42, 0.95); }
  .sh-book-title {
    font-size: 12.5px; line-height: 1.45; color: var(--text); overflow: hidden;
    display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 2; line-clamp: 2;
  }
  .sh-small .sh-book-title { font-size: 11.5px; }
  .sh-book-meta { font-size: 10.5px; color: var(--dim); display: flex; align-items: center; gap: 6px; min-width: 0; }
  .sh-chip {
    background: var(--panel); border: 1px solid var(--border); padding: 1px 6px; border-radius: 99px;
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 130px;
  }
</style>
