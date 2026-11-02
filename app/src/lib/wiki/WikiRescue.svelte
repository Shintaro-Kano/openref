<script lang="ts">
  /* 書けないまま編集を終えた本文（edit.svelte の ed.stranded）を黙って捨てない。OpenNote を無効にした・ワークスペースのフォルダが
     変わった・conflict のまま畳んだ等。クリップボードへコピーしてから閉じる（閉じる = その本文を捨てる）。
     App の直下に置く（Wiki が無効でも出す）。ed.stranded が空なら App が描かない = Wiki を使っていなければ痕跡なし */
  import { t } from "../i18n.svelte";
  import { ed } from "./edit.svelte";

  const cur = $derived(ed.stranded[0] ?? null);
  let copied = $state(false);
  let copyError = $state<string | null>(null);

  async function copy() {
    if (!cur) return;
    try {
      await navigator.clipboard.writeText(cur.content);
      copied = true;
      copyError = null;
    } catch (e) {
      copied = false;
      copyError = String(e).slice(0, 160);
    }
  }

  function drop() {
    ed.stranded.shift();
    copied = false;
    copyError = null;
  }
</script>

{#if cur}
  <div class="overlay" role="presentation">
    <div class="modal wide" role="alertdialog" aria-labelledby="wk-rescue-h">
      <div class="modal-head"><h3 id="wk-rescue-h">{t("保存できていない変更があります")}</h3></div>
      <div class="modal-body">
        <p class="wk-rs-path">{cur.path}</p>
        <p class="wk-rs-note">{cur.reason}</p>
        <p class="wk-rs-note">{t("この内容はファイルに保存されていません。コピーしてから閉じてください（閉じると捨てます）")}</p>
        <pre class="wk-rs-pre">{cur.content.slice(0, 2000)}</pre>
        {#if copyError}<p class="wk-rs-bad">{t("コピーできませんでした")}: {copyError}</p>{/if}
        {#if ed.stranded.length > 1}<p class="wk-rs-note">{t("ほかに")} {ed.stranded.length - 1}</p>{/if}
      </div>
      <div class="modal-foot">
        <button class="primary" onclick={() => void copy()}>{copied ? t("コピーしました") : t("クリップボードにコピー")}</button>
        <button class:danger={!copied} onclick={drop}>{copied ? t("閉じる") : t("捨てて閉じる")}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .wk-rs-path { font-size: 13px; overflow-wrap: anywhere; }
  .wk-rs-note { font-size: 11.5px; color: var(--dim); line-height: 1.5; overflow-wrap: anywhere; }
  .wk-rs-bad { font-size: 12px; color: var(--danger); overflow-wrap: anywhere; }
  .wk-rs-pre {
    max-height: 180px; overflow: auto; font-size: 11.5px; padding: 8px; border: 1px solid var(--border); border-radius: 6px;
    background: var(--bg); white-space: pre-wrap; overflow-wrap: anywhere;
  }
</style>
