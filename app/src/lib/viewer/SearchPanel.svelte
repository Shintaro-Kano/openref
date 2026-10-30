<script lang="ts">
  import { t, tn } from "../i18n.svelte";
  /* サイドバー「検索」タブ（Paperpile の Search document 相当・表示専用）。
     検索ロジック・ヒット巡回・ページ上のヒット帯は親 PdfViewer が持ち、ここは入力と結果一覧だけを描く。
     結果はページごとに見出しを挟み、各ヒットは前後文脈つきスニペット（語は <mark>）。クリックでそのヒットへ。
     class は sr- 接頭辞（app.css は単一グローバル）。 */
  import type { SearchHit } from "./util";
  import { composing } from "../input";

  let {
    query,
    hits,
    matchIdx,
    pending,
    onInput,
    onNext,
    onPrev,
    onPick,
    onClear,
  }: {
    query: string;
    hits: SearchHit[];
    matchIdx: number;
    pending: boolean; // 入力中（デバウンス待ち / 全ページ prefetch 待ち）
    onInput: (v: string) => void;
    onNext: () => void;
    onPrev: () => void;
    onPick: (i: number) => void;
    onClear: () => void;
  } = $props();

  let inputEl = $state<HTMLInputElement | null>(null);
  let listEl = $state<HTMLDivElement | null>(null);

  export function focusInput() {
    inputEl?.focus();
    inputEl?.select();
  }

  // 現在ヒットが変わったら一覧内で見える位置へ
  $effect(() => {
    if (matchIdx < 0 || !listEl) return;
    const el = listEl.querySelector<HTMLElement>(`[data-i="${matchIdx}"]`);
    el?.scrollIntoView({ block: "nearest" });
  });
</script>

<div class="sr-panel">
  <div class="sr-box">
    <input
      bind:this={inputEl}
      class="sr-input"
      placeholder={t("文書内を検索")}
      value={query}
      oninput={(e) => onInput(e.currentTarget.value)}
      onkeydown={(e) => {
        if (composing(e)) return; // IME の確定 Enter・取消 Esc は巡回 / クリアにしない
        if (e.key === "Enter") {
          e.preventDefault();
          if (e.shiftKey) onPrev();
          else onNext();
        } else if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation(); // window の Esc ハンドラ（ビューア閉）まで伝播させない
          if (query) onClear();
          else e.currentTarget.blur();
        }
      }}
    />
    {#if query}
      <button class="sr-clear" title={t("クリア")} aria-label={t("検索をクリア")} onclick={onClear}>×</button>
    {/if}
  </div>
  <div class="sr-head">
    <span class="sr-count">
      {#if !query}
        &nbsp;
      {:else if pending}
        {t("検索中…")}
      {:else if hits.length === 0}
        {t("一致なし")}
      {:else}
        {tn(hits.length, "件::hits")}{#if matchIdx >= 0}{t("（")}{matchIdx + 1} / {hits.length}{t("）")}{/if}
      {/if}
    </span>
    <span class="sr-nav">
      <button class="sr-btn" title={t("前へ (⇧Enter)")} disabled={hits.length === 0} onclick={onPrev}>‹</button>
      <button class="sr-btn" title={t("次へ (Enter)")} disabled={hits.length === 0} onclick={onNext}>›</button>
    </span>
  </div>
  <div class="sr-list" bind:this={listEl}>
    {#each hits as h, i (i)}
      {#if i === 0 || hits[i - 1].page !== h.page}
        <div class="sr-page">{t("ページ")} {h.page}</div>
      {/if}
      <button class="sr-hit" class:current={i === matchIdx} data-i={i} onclick={() => onPick(i)}>
        {h.before}<mark>{h.hit}</mark>{h.after}
      </button>
    {/each}
  </div>
</div>
