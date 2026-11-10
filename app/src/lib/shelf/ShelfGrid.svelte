<script lang="ts">
  /* 本棚の中央: パンくず・並び・件数・右ペインの開閉のバー + 最近開いた本・フォルダ・本のグリッド。
     すべての本（dir = null）= 最近開いた本 + 直下のフォルダ + 全部の本。App が {#key app.tab} で作り直す = スクロール位置は mount で戻す */
  import { tick, untrack } from "svelte";
  import { t, tn } from "../i18n.svelte";
  import { app, frontShelf, toggleDetail } from "../state.svelte";
  import ShelfBookCard from "./ShelfBookCard.svelte";
  import ShelfFolderCard from "./ShelfFolderCard.svelte";
  import { crumbs, currentFolder, folderDir, gotoDir, setSort, sh, shelfIndex, switchShelf } from "./shelf.svelte";
  import { filterBooks, searchTerms, sortBooks, type ShelfBook, type ShelfFolder, type ShelfSortKey } from "./tree";

  const s = $derived(frontShelf());
  const root = $derived(s?.root ?? "");
  const ix = $derived(shelfIndex(s?.root));
  const err = $derived.by(() => {
    const e = s ? sh.trees[s.root] : undefined;
    return e && "error" in e ? e.error : null;
  });
  const searching = $derived(!!s && searchTerms(s.query).length > 0);
  const folder = $derived(s && ix ? currentFolder(s, ix) : null);
  const books = $derived.by<ShelfBook[]>(() => {
    if (!s || !ix) return [];
    const list = searching ? filterBooks(ix.allBooks, s.query) : folder ? folder.books : ix.allBooks;
    return sortBooks(list, s.sort);
  });
  const folders = $derived<ShelfFolder[]>(searching || !ix ? [] : (folder ?? ix.tree).children);
  const recents = $derived.by<ShelfBook[]>(() => {
    if (!s || !ix || searching || folder) return [];
    return sh.recent.flatMap((r) => (r.root === s.root ? (ix.byPath.get(r.path) ?? []) : []));
  });
  const trail = $derived(s && ix ? crumbs(s, ix) : []);

  const SORTS: ShelfSortKey[] = ["title", "mtime", "size"];
  const sortLabel = (k: ShelfSortKey) => (k === "mtime" ? t("更新") : k === "size" ? t("サイズ") : t("名前"));
  let sortOpen = $state(false);

  let scroller = $state<HTMLDivElement | null>(null);
  let restored = false;

  // 木が読めたら 1 回だけスクロール位置を戻す（タブの切替・起動の復元）
  $effect(() => {
    if (!ix || !scroller || restored) return;
    restored = true;
    const top = untrack(() => frontShelf()?.scrollTop ?? 0);
    void tick().then(() => {
      if (scroller) scroller.scrollTop = top;
    });
  });

  // 検索語・フォルダ・並びが変わったら先頭へ（操作の側が scrollTop を 0 にしている）
  $effect(() => {
    s?.dir;
    s?.query;
    s?.sort;
    untrack(() => {
      if (scroller && frontShelf()?.scrollTop === 0) scroller.scrollTop = 0;
    });
  });

  function onScroll() {
    const tab = frontShelf();
    if (tab && scroller && restored) tab.scrollTop = scroller.scrollTop;
  }

  function goFolder(e: MouseEvent | KeyboardEvent, f: ShelfFolder) {
    if (ix) void gotoDir(folderDir(ix, f), e.metaKey);
  }
</script>

<svelte:window
  onclickcapture={(e) => {
    if (sortOpen && !(e.target as Element | null)?.closest?.(".lb-wrap")) sortOpen = false;
  }} />

<section class="list sh-main">
  <div class="listbar">
    <nav class="sh-crumbs" aria-label={t("パンくず")}>
      {#if searching}
        <span class="sh-crumb-cur">{t("検索結果::shelf")}</span>
      {:else if folder}
        {#each trail as f, i (f.id)}
          {#if i > 0}<span class="sh-crumb-sep">›</span>{/if}
          {#if i === trail.length - 1}
            <span class="sh-crumb-cur">{f.name}</span>
          {:else}
            <button class="sh-crumb" onclick={(e) => goFolder(e, f)}>{f.name}</button>
          {/if}
        {/each}
      {:else}
        <span class="sh-crumb-cur">{t("すべての本")}</span>
      {/if}
    </nav>
    <div class="lb-tail">
      <div class="lb-wrap">
        <button class="lb-btn lb-sort" title={t("並び順")} aria-haspopup="menu" aria-expanded={sortOpen} onclick={() => (sortOpen = !sortOpen)}>
          {sortLabel(s?.sort ?? "title")} <span class="caret">▾</span>
        </button>
        {#if sortOpen}
          <div class="lb-pop lb-menu lb-sort-pop" role="menu">
            {#each SORTS as k (k)}
              <button
                class:lb-sort-on={s?.sort === k}
                role="menuitemradio"
                aria-checked={s?.sort === k}
                onclick={() => {
                  sortOpen = false;
                  setSort(k);
                }}><span>{sortLabel(k)}</span></button>
            {/each}
          </div>
        {/if}
      </div>
      <div class="lb-count">{tn(books.length, "冊")}</div>
      <button
        class="lb-pane"
        class:lb-pane-off={!app.detailOpen}
        title={app.detailOpen ? t("右のペインを隠す（⌘⌥I）") : t("右のペインを表示（⌘⌥I）")}
        aria-label={app.detailOpen ? t("右のペインを隠す（⌘⌥I）") : t("右のペインを表示（⌘⌥I）")}
        aria-pressed={app.detailOpen}
        onclick={toggleDetail}>
        <svg width="16" height="14" viewBox="0 0 16 14" aria-hidden="true">
          <rect x="0.75" y="0.75" width="14.5" height="12.5" rx="2" fill="none" stroke="currentColor" stroke-width="1.5" />
          <rect class="lb-pane-fill" x="10" y="1.5" width="4.5" height="11" fill="currentColor" />
          <line x1="10" y1="1" x2="10" y2="13" stroke="currentColor" stroke-width="1.5" />
        </svg>
      </button>
    </div>
  </div>

  <div class="sh-scroll" bind:this={scroller} onscroll={onScroll}>
    {#if err}
      <div class="sh-state">
        <p class="sh-err">{t("本棚を読めません")}</p>
        <p>{err}</p>
        <button class="sh-retry" onclick={() => void switchShelf(root)}>{t("読み直す")}</button>
      </div>
    {:else if !ix}
      <div class="sh-state"><p>{t("読込中…")}</p></div>
    {:else}
      {#if recents.length}
        <section>
          <h2 class="sh-h2">{t("最近開いた本")}</h2>
          <div class="sh-recents">
            {#each recents as b (b.id)}
              <div class="sh-recent-item"><ShelfBookCard {root} book={b} dir={ix.thumbsDir} small /></div>
            {/each}
          </div>
        </section>
      {/if}
      {#if folders.length}
        <section class="sh-folders">
          {#each folders as f (f.id)}
            <ShelfFolderCard {root} folder={f} dir={ix.thumbsDir} onopen={(e) => goFolder(e, f)} />
          {/each}
        </section>
      {/if}
      {#if books.length}
        <section class="sh-grid">
          {#each books as b (b.id)}
            <ShelfBookCard {root} book={b} dir={ix.thumbsDir} showFolder={searching || !folder} />
          {/each}
        </section>
      {:else if searching}
        <div class="sh-state"><p>{t("「::no-book")}{s?.query.trim()}{t("」に一致する本はありません")}</p></div>
      {:else if !folders.length}
        <div class="sh-state"><p>{t("このフォルダに PDF はありません")}</p></div>
      {/if}
    {/if}
  </div>
</section>

<style>
  .sh-main { min-width: 0; }
  .sh-crumbs { display: flex; align-items: center; gap: 4px; min-width: 0; flex: 1 1 auto; overflow: hidden; white-space: nowrap; padding-left: 4px; }
  .sh-crumb { padding: 2px 4px; border-radius: 4px; color: var(--dim); overflow: hidden; text-overflow: ellipsis; max-width: 200px; }
  .sh-crumb:hover { color: var(--text); background: var(--bg); }
  .sh-crumb-cur { font-weight: 600; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
  .sh-crumb-sep { color: var(--dim); }
  .sh-scroll { flex: 1; min-height: 0; overflow-y: auto; padding: 16px 20px 28px; }
  .sh-h2 { font-size: 11.5px; font-weight: 600; color: var(--dim); margin-bottom: 8px; }
  .sh-recents { display: flex; gap: 16px; overflow-x: auto; padding: 8px 2px 12px; margin-bottom: 14px; }
  .sh-recent-item { flex: none; width: 112px; }
  .sh-folders { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: 12px; margin-bottom: 22px; }
  .sh-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: 24px 20px; }
  .sh-state { padding: 48px 20px; text-align: center; color: var(--dim); display: flex; flex-direction: column; gap: 8px; align-items: center; }
  .sh-err { color: var(--danger); }
  .sh-retry { padding: 4px 12px; border: 1px solid var(--border); border-radius: 6px; background: var(--panel); }
  .sh-retry:hover { border-color: var(--accent); }
</style>
