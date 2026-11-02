<script lang="ts">
  /* Settings › OpenNote の [チャット] を ON にする前の説明。
     Settings はネイティブのメニュー（Rust）なので、メニューが event "wiki-chat-consent" を出したときだけ開く（types.ts の契約）。
     初期の focus は [キャンセル]（Enter の押し流しで有効にしない）。
     [有効にする] = wiki_chat_set_enabled {enabled: true}。ログインは利用者の Claude Code のもの（チャットの [ログイン] はターミナルに claude auth login を開くだけ） */
  import { t } from "../i18n.svelte";
  import { composing } from "../input";
  import { answerConsent, chat } from "./chat.svelte";

  function focusBtn(node: HTMLButtonElement) {
    queueMicrotask(() => node.focus());
  }

  function onKey(e: KeyboardEvent) {
    if (composing(e)) return;
    if (e.key === "Escape" && chat.consent) {
      e.preventDefault();
      e.stopPropagation();
      void answerConsent(false);
    }
  }
</script>

<svelte:window onkeydown={onKey} />

{#if chat.consent}
  <div class="overlay" onclick={(e) => e.target === e.currentTarget && void answerConsent(false)} role="presentation">
    <div class="modal" role="dialog" aria-labelledby="wk-cc-h">
      <div class="modal-head"><h3 id="wk-cc-h">{t("OpenNote のチャットを有効にする")}</h3></div>
      <div class="modal-body">
        <ul class="wk-cc-list">
          <li>{t("チャットは、あなたが自分で入れた Claude Code CLI を OpenRef から呼び出します（ログインはあなたの Claude Code のもの。利用量はあなたのプランに計上されます）")}</li>
          <li>{t("Claude は Claude Code を普通に使うときと同じく、確認なしでワークスペースの読み書き・コマンドの実行ができます。ふるまいはワークスペースの CLAUDE.md で決めます（無ければ OpenRef が既定を置きます）")}</li>
          <li>{t("発言と Claude が読んだ内容は Anthropic に送られ、会話記録は Claude Code の ~/.claude/projects/ に残ります。wiki_write / wiki_move で書いたページは上書き前の内容が退避され、チャットから戻せます")}</li>
        </ul>
      </div>
      <div class="modal-foot">
        <button use:focusBtn onclick={() => void answerConsent(false)}>{t("キャンセル")}</button>
        <button class="primary" onclick={() => void answerConsent(true)}>{t("有効にする")}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .wk-cc-list { margin: 0; padding-left: 18px; font-size: 12.5px; line-height: 1.6; }
  .wk-cc-list li { margin-bottom: 4px; }
</style>
