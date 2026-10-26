<script lang="ts">
  import { t, tn } from "./i18n.svelte";
  /* 完全削除の確認（Paperpile の Delete permanently に相当。ゴミ箱の中からのみ到達）。
     Enter では実行しない（誤爆防止）。Esc / 背景クリック / キャンセルで閉じる。 */
  import { app, cancelPurge, confirmPurge } from "./state.svelte";

  const MAX_SHOWN = 6;

  const open = $derived(app.modal === "purge");
  const keys = $derived(app.purgeKeys);
  const titles = $derived.by(() => {
    const byKey = new Map(app.items.map((i) => [i.key, i.title]));
    return keys.map((k) => {
      if (byKey.has(k)) return byKey.get(k) ?? k;
      if (app.detail?.key === k) return (app.detail.csl?.title as string | undefined) ?? k;
      return k;
    });
  });
  const shown = $derived(titles.slice(0, MAX_SHOWN));
  const rest = $derived(Math.max(0, titles.length - MAX_SHOWN));
</script>

<svelte:window
  onkeydown={(e) => {
    if (open && e.key === "Escape") {
      e.stopPropagation();
      cancelPurge();
    }
  }} />

{#if open}
  <div class="overlay" onclick={cancelPurge} role="presentation">
    <div class="modal" onclick={(e) => e.stopPropagation()} role="dialog" aria-label={t("完全に削除")}>
      <div class="modal-head">
        <h3>{t("完全に削除")}</h3>
        <button class="x" onclick={cancelPurge}>×</button>
      </div>
      <div class="modal-body">
        <p class="pg-lead">
          {tn(keys.length, "件をライブラリから完全に削除します。")}<b>{t("この操作は取り消せません。")}</b>
        </p>
        <ul class="pg-list">
          {#each shown as t, i (keys[i])}
            <li title={t}>{t}</li>
          {/each}
          {#if rest > 0}<li class="more">{t("ほか")} {tn(rest, "件")}</li>{/if}
        </ul>
        <p class="pg-note">
          {t("注釈・メモ・フォルダ/ラベルの所属も一緒に消えます。Google Drive 等にリンクした PDF ファイルは残します （OpenRef 管理フォルダ内の PDF だけを削除）。")}
        </p>
      </div>
      <div class="modal-foot">
        <button onclick={cancelPurge}>{t("キャンセル")}</button>
        <button class="danger" disabled={app.busy} onclick={confirmPurge}>{t("完全に削除")}</button>
      </div>
    </div>
  </div>
{/if}
