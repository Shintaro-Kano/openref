<script lang="ts">
  /* 保存の競合: 編集を始めた後に外部（Claude Code 等）がこのページを書き換えた。
     黙って負けない・勝たない = 読み直す（外部の内容で置き換え・自分の変更は捨てる）/ 上書き（core が上書き前を wiki-history へ退避）。
     キャンセル = ダイアログだけ閉じる（自動保存は止めたまま・⌘S でまた尋ねる） */
  import { t } from "../i18n.svelte";
  import { composing } from "../input";
  import type { WikiPageData } from "./types";
  import { ed, resolveConflict } from "./edit.svelte";

  let { onReloaded }: { onReloaded?: (p: WikiPageData) => void } = $props();
  let busy = $state(false);

  async function pick(c: "reload" | "overwrite" | "cancel") {
    if (busy) return;
    busy = true;
    try {
      const p = await resolveConflict(c);
      if (p) onReloaded?.(p);
    } finally {
      busy = false;
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape" && !composing(e)) {
      e.preventDefault();
      e.stopPropagation();
      void pick("cancel");
    }
  }

  function focusFirst(node: HTMLButtonElement) {
    queueMicrotask(() => node.focus());
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="overlay" role="presentation">
  <div class="modal wide wk-conflict" role="alertdialog" aria-labelledby="wk-conflict-h">
    <div class="modal-head"><h3 id="wk-conflict-h">{t("このページは外部で書き換えられています")}</h3></div>
    <div class="modal-body">
      <p class="wk-cf-lead">
        {ed.theirs === null
          ? t("編集を始めた後に、ファイルが移動または削除されました。")
          : t("編集を始めた後に、別のアプリ（Claude Code など）がこのファイルを保存しました。")}
      </p>
      <p class="wk-cf-path">{ed.path}</p>
      <ul class="wk-cf-list">
        <li><b>{t("読み直す")}</b>: {t("外部の内容を開き直します（ここでの未保存の変更は捨てます）")}</li>
        <li><b>{t("上書き")}</b>: {t("ここでの内容で保存します（外部の内容は退避してから上書きします）")}</li>
      </ul>
    </div>
    <div class="modal-foot">
      <button disabled={busy} use:focusFirst onclick={() => void pick("cancel")}>{t("キャンセル")}</button>
      <button disabled={busy || ed.theirs === null} onclick={() => void pick("reload")}>{t("読み直す")}</button>
      <button class="primary" disabled={busy} onclick={() => void pick("overwrite")}>{t("上書き")}</button>
    </div>
  </div>
</div>

<style>
  .wk-cf-lead { font-size: 13px; line-height: 1.6; }
  .wk-cf-path { font-size: 12px; color: var(--dim); overflow-wrap: anywhere; }
  .wk-cf-list { margin: 0; padding-left: 18px; font-size: 12.5px; line-height: 1.7; }
</style>
