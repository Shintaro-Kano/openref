<script lang="ts">
  /* 書けないまま編集を終えた本文（edit.svelte の ed.stranded）を黙って捨てない。OpenNote を無効にした・ワークスペースのフォルダが
     変わった・conflict のまま畳んだ等。クリップボードへコピーしてから閉じる（閉じる = その本文を捨てる）。
     App の直下に置く（Wiki が無効でも出す）。ed.stranded が空なら App が描かない = Wiki を使っていなければ痕跡なし */
  import { t } from "../i18n.svelte";
  import { ed } from "./edit.svelte";
  import { errText } from "../err";
  import { holdFocus } from "../focusReturn";
  import { trapTab } from "../focusTrap";

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
      copyError = errText(e, 160);
    }
  }

  /** 開いたときの焦点 = 枠（tabindex=-1・焦点の輪は出さない）。裏のエディタ・欄から焦点を外す = 続けて打った字・⌘F（CodeMirror の keymap）が
      裏に入らない。ボタンに置かない: [捨てて閉じる] は本文を捨て、[クリップボードにコピー] も編集中に窓が出た瞬間の Enter / Space で
      利用者のクリップボードを上書きする = 枠なら Enter / Space で何も起きない（取込の窓と同じ）。
      明示の focus()（WKWebView では、焦点が他の欄に残っていると autofocus が無視される）。窓が出ている間に次の本文へ移っても枠は同じ = 1 回だけ。
      閉じたら（枠が DOM から外れたら = 全部の本文を閉じた）開く前の焦点へ戻す（focusReturn.ts の条件 = 消えた・隠れた・別の窓が開いた等は戻さない） */
  function focusFirst(node: HTMLElement) {
    const back = holdFocus(node); // 枠へ移す前に、開く前の焦点を覚える
    queueMicrotask(() => node.focus());
    return { destroy: back };
  }

  function drop() {
    ed.stranded.shift();
    copied = false;
    copyError = null;
  }
</script>

{#if cur}
  <div class="overlay" role="presentation">
    <div class="modal wide wk-rs" role="alertdialog" aria-labelledby="wk-rescue-h" tabindex="-1" use:focusFirst use:trapTab>
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
  .wk-rs:focus { outline: none; } /* 開いたときに枠へ焦点を置く（Enter / Space で何も起きない先）= 輪は出さない（取込の窓・フォルダの窓と同じ） */
  .wk-rs-path { font-size: 13px; overflow-wrap: anywhere; }
  .wk-rs-note { font-size: 11.5px; color: var(--dim); line-height: 1.5; overflow-wrap: anywhere; }
  .wk-rs-bad { font-size: 12px; color: var(--danger); overflow-wrap: anywhere; }
  .wk-rs-pre {
    max-height: 180px; overflow: auto; font-size: 11.5px; padding: 8px; border: 1px solid var(--border); border-radius: 6px;
    background: var(--bg); white-space: pre-wrap; overflow-wrap: anywhere;
  }
</style>
