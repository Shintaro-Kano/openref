<script lang="ts">
  /* OpenNote の検索欄（ヘッダー = vault 名の右）。`/` で入る（App.svelte が #wk-search へ focus）。
     結果は中央の一覧（WikiResults）。↑↓ / Enter は結果の選択・Esc は消して欄から出る。
     欄にいて何も打っていない間は、欄の真下に重ねる小さな一覧に最近開いたページ（recent.svelte.ts・10 件・× で外す）= ↑↓ / Enter もそれを選ぶ。
     本文・エディタは隠さない。一覧の押下は欄のフォーカスを奪わない（mousedown を止める = blur で一覧が消えてクリックが空振りしない）。
     旗は blur・欄が消えた（onDestroy = 欄にいたまま OpenNote 以外のタブへ移ると blur が来ない）・Esc・欄と一覧の外の押下で下ろす
     （WKWebView はボタンの押下でフォーカスを移さない = 木の行や [＋ 新規] を押しても blur が来ない）。IME の変換中は出し入れを変えない。
     重なりはモーダルの覆い（40）より下（35）・覆いの下では出さない（recentShown）。状態と検索の本体（hasQuery / searchSoon）は wiki.svelte.ts・動詞（clearSearch / openHit）は search.svelte.ts */
  import { onDestroy } from "svelte";
  import { t } from "../i18n.svelte";
  import { composing } from "../input";
  import { hasQuery, searchSoon, wk } from "./wiki.svelte";
  import { clearSearch, openHit } from "./search.svelte";
  import { leaveRecent, loadRecent, openRecent, recent, recentList, recentShown, removeRecent } from "./recent.svelte";

  const showRecent = $derived(recentShown());
  const recents = $derived(showRecent ? recentList() : []);
  const recentSel = $derived(Math.min(recent.sel, recents.length - 1));
  onDestroy(leaveRecent);
  let box: HTMLDivElement | undefined = $state();

  // 一覧を出している間: 欄と一覧の外の押下（capture）= 一覧を畳んで欄から出る
  $effect(() => {
    if (!showRecent) return;
    const down = (e: MouseEvent) => {
      if (box?.contains(e.target as Node)) return;
      leaveRecent();
      const el = document.activeElement as HTMLElement | null;
      if (el?.id === "wk-search") el.blur();
    };
    window.addEventListener("mousedown", down, true);
    return () => window.removeEventListener("mousedown", down, true);
  });

  function pickRecent(e: MouseEvent, path: string) {
    if (e.button === 1 || e.metaKey) {
      e.preventDefault();
      void openRecent(path, true);
    } else if (e.button === 0) void openRecent(path);
  }

  function forget(e: MouseEvent, path: string) {
    e.stopPropagation();
    const root = wk.status?.root;
    if (root) void removeRecent(root, path);
  }

  // IME の変換中は検索しない（確定 = compositionend で 1 回）
  function onInput(e: Event) {
    if (composing(e as InputEvent)) return;
    searchSoon();
  }

  function onKey(e: KeyboardEvent) {
    if (composing(e)) return; // IME の変換中の Enter / Esc / 矢印は入力欄のもの
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      leaveRecent();
      clearSearch();
      (e.currentTarget as HTMLInputElement).blur();
    } else if (recentShown() && (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Enter")) {
      const list = recentList();
      if (!list.length) return;
      e.preventDefault();
      if (e.key === "Enter") void openRecent(list[Math.min(recent.sel, list.length - 1)].path, e.metaKey);
      else recent.sel = (Math.min(recent.sel, list.length - 1) + (e.key === "ArrowDown" ? 1 : list.length - 1)) % list.length;
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      if (!wk.showResults || !wk.results.length) return;
      e.preventDefault();
      const n = wk.results.length;
      wk.sel = (wk.sel + (e.key === "ArrowDown" ? 1 : n - 1)) % n;
    } else if (e.key === "Enter") {
      const hit = wk.results[wk.sel];
      if (!wk.showResults || !hit) return;
      e.preventDefault();
      void openHit(hit.path, e.metaKey);
    }
  }

  /** 検索欄に戻った = 条件が残っていれば結果一覧を再び出す */
  function onFocus() {
    recent.focus = true;
    recent.sel = 0;
    void loadRecent();
    if (hasQuery() && !wk.showResults) searchSoon(0);
  }
</script>

<div class="wk-search" bind:this={box}>
  <input
    id="wk-search"
    type="search"
    autocomplete="off"
    spellcheck="false"
    placeholder={t("ページを検索（/）")}
    aria-label={t("ページを検索")}
    bind:value={wk.q}
    oninput={onInput}
    oncompositionstart={() => (recent.frozen = recentShown())}
    oncompositionend={() => {
      recent.frozen = null;
      searchSoon();
    }}
    onkeydown={onKey}
    onfocus={onFocus}
    onblur={leaveRecent}
  />
  {#if hasQuery()}
    <button class="wk-search-x" title={t("検索を消す（Esc）")} aria-label={t("検索を消す（Esc）")} onclick={clearSearch}>×</button>
  {/if}
  {#if showRecent}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="wk-recent-pop" onmousedown={(e) => e.preventDefault()}>
      <div class="wk-recent-head">{t("最近開いたページ")}</div>
      {#if !recents.length}
        <div class="wk-recent-empty">{recent.readError ? t("最近開いたページを読めませんでした") : t("最近開いたページはありません")}</div>
      {/if}
      <div role="listbox" aria-label={t("最近開いたページ")}>
        {#each recents as r, i (r.path)}
          <div
            class="wk-recent"
            class:wk-recent-sel={i === recentSel}
            role="option"
            aria-selected={i === recentSel}
            tabindex="-1"
            title={r.path}
            onclick={(e) => pickRecent(e, r.path)}
            onauxclick={(e) => pickRecent(e, r.path)}
            onkeydown={(e) => e.key === "Enter" && void openRecent(r.path, e.metaKey)}
            onmouseenter={() => (recent.sel = i)}>
            <span class="wk-recent-main">
              <span class="wk-recent-title">{r.title}</span>
              <span class="wk-recent-path">{r.path}</span>
            </span>
            <button class="wk-recent-x" title={t("最近開いたページから外す")} aria-label={t("最近開いたページから外す")} onclick={(e) => forget(e, r.path)}>×</button>
          </div>
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .wk-search { position: relative; flex: 0 1 320px; min-width: 120px; }
  .wk-search input {
    width: 100%; box-sizing: border-box; padding: 4px 24px 4px 8px; border: 1px solid var(--border); border-radius: 6px;
    background: var(--bg); color: var(--text); font-size: 12.5px;
  }
  .wk-search input:focus { outline: none; border-color: var(--accent); }
  .wk-search input::-webkit-search-cancel-button { display: none; }
  .wk-search-x {
    position: absolute; right: 4px; top: 50%; transform: translateY(-50%); border: 0; background: transparent; color: var(--dim);
    font-size: 14px; line-height: 1; padding: 2px 4px; cursor: pointer;
  }
  .wk-search-x:hover { color: var(--text); }
  .wk-recent-pop {
    position: absolute; left: 0; top: calc(100% + 4px); z-index: 35; width: max(100%, 320px); max-width: min(480px, 90vw); padding: 4px;
    border: 1px solid var(--border); border-radius: 8px; background: var(--panel); box-shadow: 0 8px 24px rgba(0, 0, 0, 0.18);
  }
  .wk-recent-head { padding: 3px 8px 5px; font-size: 11px; color: var(--dim); }
  .wk-recent-empty { padding: 4px 8px 6px; font-size: 12px; color: var(--dim); }
  .wk-recent { display: flex; align-items: center; gap: 6px; padding: 4px 8px; border-radius: 5px; cursor: pointer; min-width: 0; }
  .wk-recent-sel { background: var(--accent-soft); }
  .wk-recent-main { flex: 1; min-width: 0; display: flex; flex-direction: column; }
  .wk-recent-title { font-size: 12.5px; color: var(--text); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wk-recent-path { font-size: 10.5px; color: var(--dim); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wk-recent-x {
    flex-shrink: 0; border: 0; background: transparent; color: var(--dim); font-size: 14px; line-height: 1; padding: 0 4px; cursor: pointer;
    visibility: hidden;
  }
  .wk-recent:hover .wk-recent-x, .wk-recent-sel .wk-recent-x { visibility: visible; }
  .wk-recent-x:hover { color: var(--text); }
</style>
