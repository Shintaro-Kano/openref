<script lang="ts">
  /* Settings › OpenNote の [チャット] を ON にする前の説明。
     開くのは 2 つの道: Settings › OpenNote › チャット（ネイティブのメニュー = Rust が event "wiki-chat-consent" を出す・types.ts の契約）と、
     チャットが無効と分かっているときのタブ列のチャットのボタン（openChatEntry）。開いている間に有効と分かったら閉じる（chat.svelte.ts）。
     初期の focus は [キャンセル]（Enter の押し流しで有効にしない）。
     [有効にする] = wiki_chat_set_enabled {enabled: true}。ログインは利用者の Claude Code のもの（チャットの [ログイン] はターミナルに claude auth login を開くだけ） */
  import { t } from "../i18n.svelte";
  import { composing } from "../input";
  import Modal from "../Modal.svelte";
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

<!-- 覆いと枠は骨格（Modal = .overlay > .modal）。覆いで押して覆いで離したときだけ閉じる = 同意しない（本文を選んで外へ出ても
     同意しない扱いにならない）。Esc（IME の変換中は除く）は上の onKey のまま（骨格では扱わない）。開いているかの数え方は chat.consent -->
<Modal open={chat.consent} onbackdrop={() => void answerConsent(false)} labelledby="wk-cc-h">
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
</Modal>

<style>
  .wk-cc-list { margin: 0; padding-left: 18px; font-size: 12.5px; line-height: 1.6; }
  .wk-cc-list li { margin-bottom: 4px; }
</style>
