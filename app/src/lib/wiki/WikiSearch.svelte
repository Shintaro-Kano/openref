<script lang="ts">
  /* OpenNote の検索欄（ヘッダー = vault 名の右）。`/` で入る（App.svelte が #wk-search へ focus）。
     結果は中央の一覧（WikiResults）。↑↓ / Enter は結果の選択・Esc は消して欄から出る。状態と検索の本体（hasQuery / searchSoon）は wiki.svelte.ts・動詞（clearSearch / openHit）は search.svelte.ts */
  import { t } from "../i18n.svelte";
  import { composing } from "../input";
  import { hasQuery, searchSoon, wk } from "./wiki.svelte";
  import { clearSearch, openHit } from "./search.svelte";

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
      clearSearch();
      (e.currentTarget as HTMLInputElement).blur();
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
    if (hasQuery() && !wk.showResults) searchSoon(0);
  }
</script>

<div class="wk-search">
  <input
    id="wk-search"
    type="search"
    autocomplete="off"
    spellcheck="false"
    placeholder={t("ページを検索（/）")}
    aria-label={t("ページを検索")}
    bind:value={wk.q}
    oninput={onInput}
    oncompositionend={() => searchSoon()}
    onkeydown={onKey}
    onfocus={onFocus}
  />
  {#if hasQuery()}
    <button class="wk-search-x" title={t("検索を消す（Esc）")} aria-label={t("検索を消す（Esc）")} onclick={clearSearch}>×</button>
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
</style>
