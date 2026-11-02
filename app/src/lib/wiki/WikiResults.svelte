<script lang="ts">
  /* OpenNote の中央の検索結果。ヘッダーの検索欄（WikiSearch・語）と Tags 箱（tag の AND）の結果 = wiki_search。
     行 = 題名・パス・抜粋（一致箇所を <mark>）・tags。クリック = そのページを開いて結果を畳む（条件は残る）・⌘クリック = 新しいタブ。
     行の tag のクリック = その tag を絞込に足す。Esc（検索欄で）/ × = 検索を消して木（ページ）に戻る */
  import { tick } from "svelte";
  import { t } from "../i18n.svelte";
  import { wk } from "./wiki.svelte";
  import { clearSearch, openHit, toggleTag } from "./search.svelte";

  let list = $state<HTMLDivElement | null>(null);

  /** 抜粋の印（U+E000 … U+E001）を分ける（蔵書の一覧の fulltext_snippet と同じ） */
  function parts(s: string): { text: string; hit: boolean }[] {
    const out: { text: string; hit: boolean }[] = [];
    const re = /([^]*)/g;
    let last = 0;
    for (let m = re.exec(s); m; m = re.exec(s)) {
      if (m.index > last) out.push({ text: s.slice(last, m.index), hit: false });
      out.push({ text: m[1], hit: true });
      last = m.index + m[0].length;
    }
    if (last < s.length) out.push({ text: s.slice(last).replace(/[]/g, ""), hit: false });
    return out;
  }

  function pick(e: MouseEvent, path: string) {
    if (e.button === 1 || e.metaKey) {
      e.preventDefault();
      void openHit(path, true);
    } else if (e.button === 0) void openHit(path);
  }

  // hover で選んだ行（$state にしない = effect の依存にしない）。hover の選択ではスクロールしない（半分見えている行に乗るとリストが動く）
  let hoverSel = -1;
  function hover(i: number) {
    if (wk.sel === i) return;
    hoverSel = i;
    wk.sel = i;
  }

  // 検索欄の ↑↓ で選んだ行を見える所へ（hover で選んだときは動かさない）
  $effect(() => {
    const i = wk.sel;
    if (i === hoverSel) {
      hoverSel = -1;
      return;
    }
    void tick().then(() => list?.querySelector<HTMLElement>(`[data-i="${i}"]`)?.scrollIntoView({ block: "nearest" }));
  });

  const kindLabel = (k: string) => (k === "media" ? t("メディア") : k === "other" ? t("その他") : k);
</script>

<div class="wk-res">
  <div class="wk-res-bar">
    <span class="wk-res-count">
      {#if wk.searching && !wk.results.length}
        {t("検索中…")}
      {:else}
        {`${wk.resultsTotal} ${t("件")}${wk.resultsTotal > wk.results.length ? ` · ${t("先頭")} ${wk.results.length}` : ""}`}
      {/if}
    </span>
    {#each wk.qTags as tg (tg)}
      <button class="wk-res-chip" title={t("この tag の絞込を外す")} onclick={() => toggleTag(tg)}>#{tg} ×</button>
    {/each}
    <span class="wk-res-gap"></span>
    <button class="wk-res-close" title={t("検索を消す（Esc）")} onclick={clearSearch}>{t("検索を閉じる")}</button>
  </div>
  {#if wk.searchError}
    <p class="wk-res-err">{wk.searchError}</p>
  {:else if !wk.searching && !wk.results.length}
    <p class="wk-res-empty">{t("一致するページはありません")}</p>
  {/if}
  <div class="wk-res-list" bind:this={list} role="listbox" aria-label={t("ページの検索結果")}>
    {#each wk.results as h, i (h.path)}
      <div
        class="wk-hit"
        class:wk-hit-sel={i === wk.sel}
        data-i={i}
        role="option"
        aria-selected={i === wk.sel}
        tabindex="-1"
        onclick={(e) => pick(e, h.path)}
        onauxclick={(e) => pick(e, h.path)}
        onkeydown={(e) => e.key === "Enter" && void openHit(h.path, e.metaKey)}
        onmouseenter={() => hover(i)}>
        <div class="wk-hit-head">
          <span class="wk-hit-title">{h.title}</span>
          {#if h.kind !== "md"}<span class="wk-hit-kind">{kindLabel(h.kind)}</span>{/if}
        </div>
        <div class="wk-hit-path">{h.path}</div>
        {#if h.snippet}
          <div class="wk-hit-snip">
            {#each parts(h.snippet) as p, j (j)}{#if p.hit}<mark>{p.text}</mark>{:else}{p.text}{/if}{/each}
          </div>
        {/if}
        {#if h.tags.length}
          <div class="wk-hit-tags">
            {#each h.tags.slice(0, 8) as tg (tg)}
              <button
                class="wk-hit-tag"
                class:wk-hit-tag-on={wk.qTags.some((x) => x.toLowerCase() === tg.toLowerCase())}
                title={`#${tg}`}
                onclick={(e) => {
                  e.stopPropagation();
                  toggleTag(tg);
                }}>#{tg}</button>
            {/each}
            {#if h.tags.length > 8}<span class="wk-hit-more">+{h.tags.length - 8}</span>{/if}
          </div>
        {/if}
      </div>
    {/each}
  </div>
</div>

<style>
  .wk-res { display: flex; flex-direction: column; min-height: 0; flex: 1; }
  .wk-res-bar {
    display: flex; align-items: center; flex-wrap: wrap; gap: 6px; padding: 6px 14px; border-bottom: 1px solid var(--border); flex-shrink: 0;
  }
  .wk-res-count { font-size: 12px; color: var(--dim); }
  .wk-res-chip {
    border: 1px solid var(--accent); border-radius: 10px; padding: 1px 8px; background: var(--accent-soft); color: var(--accent);
    font-size: 11.5px; cursor: pointer;
  }
  .wk-res-gap { flex: 1; }
  .wk-res-close {
    border: 1px solid var(--border); border-radius: 6px; padding: 3px 10px; background: var(--bg); font-size: 12px; cursor: pointer; color: var(--text);
  }
  .wk-res-close:hover { border-color: var(--accent); }
  .wk-res-err { margin: 16px 20px; color: var(--danger); white-space: pre-wrap; overflow-wrap: anywhere; }
  .wk-res-empty { margin: 16px 20px; color: var(--dim); font-size: 13px; }
  .wk-res-list { flex: 1; min-height: 0; overflow-y: auto; padding: 6px 0 20px; }
  .wk-hit {
    padding: 8px 18px; border-bottom: 1px solid var(--border); cursor: pointer; display: flex; flex-direction: column; gap: 2px; min-width: 0;
  }
  .wk-hit-sel { background: var(--bg); }
  .wk-hit-head { display: flex; align-items: baseline; gap: 8px; min-width: 0; }
  .wk-hit-title { font-weight: 600; color: var(--accent); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wk-hit-kind {
    flex-shrink: 0; font-size: 10.5px; color: var(--dim); border: 1px solid var(--border); border-radius: 4px; padding: 0 4px;
  }
  .wk-hit-path { font-size: 11px; color: var(--dim); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wk-hit-snip {
    font-size: 12.5px; color: var(--text); line-height: 1.5; overflow-wrap: anywhere;
    display: -webkit-box; -webkit-line-clamp: 3; line-clamp: 3; -webkit-box-orient: vertical; overflow: hidden;
  }
  /* 蔵書の一覧の本文抜粋（.row .ft-hl）と同じ印 */
  .wk-hit-snip mark { background: color-mix(in srgb, var(--accent) 18%, transparent); color: var(--text); border-radius: 2px; padding: 0 1px; }
  .wk-hit-tags { display: flex; flex-wrap: wrap; gap: 4px; margin-top: 2px; }
  .wk-hit-tag {
    border: 0; border-radius: 8px; padding: 0 6px; background: var(--bg); color: var(--dim); font-size: 11px; cursor: pointer;
    max-width: 22em; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .wk-hit-sel .wk-hit-tag { background: var(--panel); }
  .wk-hit-tag:hover { color: var(--accent); }
  .wk-hit-tag.wk-hit-tag-on { color: var(--accent); font-weight: 600; }
  .wk-hit-more { font-size: 11px; color: var(--dim); }
</style>
