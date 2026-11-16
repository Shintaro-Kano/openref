<script lang="ts">
  /* OpenNote のヘッダー（App の <header> の中・アプリ名「OpenNote」と spinner の間）: 作り直しの進捗・
     戻る / 進む（このタブの中の履歴 = ⌘[ / ⌘]）・検索欄（WikiSearch）・監視の失敗・[＋ 新規 ▾]（メニュー = 新規ページ… / 新規フォルダ… を今のフォルダに）・右ペインの開閉（⌘⌥I と同じ）。
     索引を作り直すのはワークスペース名のメニュー。作り直しの進捗と失敗の印はここに残す。
     */
  import { t } from "../i18n.svelte";
  import WikiSearch from "./WikiSearch.svelte";
  import PaneToggle from "../PaneToggle.svelte";
  import { canGo, goHistory, toggleInfo, wk } from "./wiki.svelte";
  import { openNewMenu } from "./files.svelte";

  /** [＋ 新規] の下にメニューを出す（開いていれば閉じる。外側の mousedown / Esc で閉じるのは WikiDialogs） */
  function openNew(e: MouseEvent) {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    openNewMenu(Math.max(8, Math.min(r.left, window.innerWidth - 190)), r.bottom + 4);
  }
</script>

<div class="wk-head">
  <!-- 戻る / 進む = ブラウザの位置（アプリ名と検索欄の間）。押せないときは淡く -->
  <div class="wk-head-hist" role="group" aria-label={t("ページの履歴")}>
    <button class="wk-head-histbtn" disabled={!canGo(-1)} title={t("戻る（⌘[）")} aria-label={t("戻る")} onclick={() => void goHistory(-1)}>
      <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="M7.5 2 L3.5 6 L7.5 10" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
    </button>
    <button class="wk-head-histbtn" disabled={!canGo(1)} title={t("進む（⌘]）")} aria-label={t("進む")} onclick={() => void goHistory(1)}>
      <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="M4.5 2 L8.5 6 L4.5 10" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
    </button>
  </div>
  <!-- ワークスペース名はヘッダーに出さない（左の木の最上段の行にある）= 検索欄がアプリ名の隣 -->
  <WikiSearch />
  {#if wk.status?.rebuilding}
    <span class="wk-head-note">{t("索引を作成中…")} {wk.status.rebuilding.percent}%</span>
  {:else if wk.status?.statusError}
    <span class="wk-head-bad" title={wk.status.statusError}>{t("⚠ OpenNote の状態を読めません")}</span>
  {:else if wk.status?.frontError}
    <!-- 設定をこのワークスペースへ揃えられない（未接続のディスク等 = core の文「…見つかりません」は title と toast）。揃え直せたら消える -->
    <span class="wk-head-bad" title={wk.status.frontError}>{t("⚠ ワークスペースを切り替えられません")}</span>
  {:else if wk.error}
    <span class="wk-head-bad" title={wk.error}>{t("⚠ 変更の自動反映が止まっています")}</span>
  {/if}
  <button
    class="wk-head-btn wk-head-new"
    title={t("今のフォルダに新しいページかフォルダを作る")}
    aria-haspopup="menu"
    aria-expanded={wk.menu?.kind === "new"}
    onclick={openNew}>
    {t("＋ 新規")}<span class="wk-head-caret" aria-hidden="true">▾</span>
  </button>
  <!-- 右ペインの開閉（▣ = 一覧・本棚と同じ部品・同じ場所 = アプリのヘッダーの右端） -->
  <PaneToggle open={wk.infoOpen} ontoggle={toggleInfo} />
</div>

<style>
  .wk-head { flex: 1; display: flex; align-items: center; gap: 10px; min-width: 0; }
  .wk-head-hist { display: inline-flex; gap: 2px; flex-shrink: 0; }
  .wk-head-histbtn {
    display: inline-flex; align-items: center; justify-content: center; width: 26px; height: 26px; border: 1px solid transparent;
    border-radius: 6px; background: transparent; color: var(--text); cursor: pointer;
  }
  .wk-head-histbtn:hover:not(:disabled) { border-color: var(--border); background: var(--bg); }
  .wk-head-histbtn:disabled { color: var(--dim); opacity: 0.45; cursor: default; }
  .wk-head-note { color: var(--dim); font-size: 12px; white-space: nowrap; }
  .wk-head-bad { color: var(--danger); font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .wk-head-btn {
    margin-left: auto; border: 1px solid var(--border); border-radius: 6px; padding: 4px 12px; background: var(--bg); cursor: pointer;
    color: var(--text); font-size: 12.5px;
  }
  .wk-head-btn:hover { border-color: var(--accent); }
  .wk-head-caret { margin-left: 5px; font-size: 10px; color: var(--dim); }
</style>
