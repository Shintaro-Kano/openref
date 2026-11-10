<script lang="ts">
  import { t } from "../i18n.svelte";
  /* ビューアのツールバー（表示専用）。状態は親 PdfViewer が保持し、操作はコールバックで通知する。
     グローバル CSS（app.css の .v-toolbar 群）でスタイルする流儀に合わせる。 */
  import type { ZoomMode } from "./util";

  let {
    zoomPct,
    mode,
    page,
    pageCount,
    searchOpen,
    sideOpen,
    onZoomIn,
    onZoomOut,
    onSetMode,
    onRotate,
    onGoto,
    onToggleSearch,
    onToggleSide,
  }: {
    zoomPct: number;
    mode: ZoomMode;
    page: number;
    pageCount: number;
    searchOpen: boolean;
    /** 右バー（検索 / 注釈 / 目次 / ページ）が出ているか */
    sideOpen: boolean;
    onZoomIn: () => void;
    onZoomOut: () => void;
    onSetMode: (m: ZoomMode) => void;
    onRotate: (dir: 1 | -1) => void;
    onGoto: (n: number) => void;
    onToggleSearch: () => void;
    onToggleSide: () => void;
  } = $props();

  // ページ番号入力: 表示はカレントページに追従、編集中は上書きしない
  let buf = $state("");
  let editing = $state(false);
  $effect(() => {
    if (!editing) buf = String(page);
  });
  function commit() {
    const n = parseInt(buf, 10);
    editing = false;
    if (Number.isFinite(n)) onGoto(n);
    else buf = String(page);
  }
</script>

<div class="v-toolbar">
  <div class="tb-group">
    <button class="tb-btn" title={t("縮小 (⌘−)")} onclick={onZoomOut}>−</button>
    <span class="tb-pct">{zoomPct}%</span>
    <button class="tb-btn" title={t("拡大 (⌘+)")} onclick={onZoomIn}>+</button>
  </div>

  <select
    class="tb-mode"
    value={mode}
    onchange={(e) => onSetMode(e.currentTarget.value as ZoomMode)}
  >
    <option value="auto">{t("自動")}</option>
    <option value="width">{t("幅に合わせる")}</option>
    <option value="page">{t("ページ全体")}</option>
    {#if mode === "custom"}<option value="custom">{t("カスタム")}</option>{/if}
  </select>

  <div class="tb-sep"></div>

  <div class="tb-group">
    <button class="tb-btn" title={t("左に回転")} onclick={() => onRotate(-1)}>⟲</button>
    <button class="tb-btn" title={t("右に回転")} onclick={() => onRotate(1)}>⟳</button>
  </div>

  <div class="tb-sep"></div>

  <div class="tb-group tb-page">
    <input
      class="tb-pageinput"
      bind:value={buf}
      onfocus={() => (editing = true)}
      onblur={commit}
      onkeydown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          commit();
          e.currentTarget.blur();
        } else if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation(); // window の Esc ハンドラ（ビューア閉）まで伝播させない（SearchBar.svelte と同様）
          editing = false;
          buf = String(page);
          e.currentTarget.blur();
        }
      }}
    />
    <span class="tb-pagesep">/ {pageCount}</span>
  </div>

  <div class="tb-spacer"></div>

  <button
    class="tb-btn tb-search"
    class:active={searchOpen}
    title={t("文書内検索 (⌘F)")}
    onclick={onToggleSearch}>⌕</button
  >
  <!-- 右バーの開閉（一覧の右ペインの開閉 .lb-pane と同じ絵・⌘⌥I） -->
  <button
    class="tb-btn tb-pane"
    class:tb-pane-off={!sideOpen}
    title={sideOpen ? t("右のペインを隠す（⌘⌥I）") : t("右のペインを表示（⌘⌥I）")}
    aria-label={sideOpen ? t("右のペインを隠す（⌘⌥I）") : t("右のペインを表示（⌘⌥I）")}
    aria-pressed={sideOpen}
    onclick={onToggleSide}>
    <svg width="16" height="14" viewBox="0 0 16 14" aria-hidden="true">
      <rect x="0.75" y="0.75" width="14.5" height="12.5" rx="2" fill="none" stroke="currentColor" stroke-width="1.5" />
      <rect class="tb-pane-fill" x="10" y="1.5" width="4.5" height="11" fill="currentColor" />
      <line x1="10" y1="1" x2="10" y2="13" stroke="currentColor" stroke-width="1.5" />
    </svg>
  </button>
</div>
