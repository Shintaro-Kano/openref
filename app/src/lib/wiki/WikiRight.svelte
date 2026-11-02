<script lang="ts">
  /* OpenNote の右ペイン。チャットが有効なときだけ [ページ情報 | チャット] のタブにする。
     無効なら WikiInfo をそのまま出す（タブの痕跡を残さない = 原則 1）。開閉は今どおり ⌘⌥I（wk.infoOpen）。
     タブの選択は ui_wiki_right_tab。チャットの会話は chat.svelte.ts が持つ = タブを切り替えても消えない。
     同じ WikiChat を内蔵の PDF viewer の右バー（PdfViewer）も出す = 同じ会話・文脈は前面のタブで決まる */
  import { untrack } from "svelte";
  import { t } from "../i18n.svelte";
  import { app } from "../state.svelte";
  import WikiInfo from "./WikiInfo.svelte";
  import WikiChat from "./WikiChat.svelte";
  import { chat, chatShown, loadChatStatus, setFloat, setTab, statusInFlight } from "./chat.svelte";

  // 右ペインが出た（Wiki のタブへ切替・⌘⌥I）= 状態を読み直す（Terminal での claude auth login / brew upgrade を拾う）。
  // ただし読み中（起動時の initChat の読み = MY LIBRARY の PDF のタブの [チャット] が右ペインの mount を待たないための読み）と
  // 重ねない。文脈の元（ページ / 文献）の変化は initChat が見る
  $effect(() => {
    if (app.wikiOn && !statusInFlight()) untrack(() => void loadChatStatus());
  });

  const tabs = $derived(chatShown());
  const tab = $derived(tabs ? chat.tab : "info");
</script>

{#if !tabs}
  <WikiInfo />
{:else}
  <aside class="wk-right">
    <div class="wk-rtabs" role="tablist" aria-label={t("右ペイン")}>
      <button role="tab" class="wk-rtab" class:wk-rtab-on={tab === "info"} aria-selected={tab === "info"} onclick={() => setTab("info")}>{t("ページ情報")}</button>
      <button role="tab" class="wk-rtab" class:wk-rtab-on={tab === "chat"} aria-selected={tab === "chat"} onclick={() => setTab("chat")}>
        {t("チャット")}{#if chat.status?.state === "busy"}<span class="wk-rtab-dot" aria-hidden="true"></span>{/if}
      </button>
    </div>
    {#if tab === "info"}
      <WikiInfo embedded />
    {:else if chat.float}
      <div class="wk-floated">
        <p>{t("チャットは浮かせたパネルに表示中です")}</p>
        <button class="wk-floated-btn" onclick={() => setFloat(null)}>{t("ここに戻す")}</button>
      </div>
    {:else}
      <WikiChat />
    {/if}
  </aside>
{/if}

<style>
  .wk-right { border-left: 1px solid var(--border); background: var(--panel); min-width: 0; min-height: 0; display: flex; flex-direction: column; }
  .wk-rtabs { display: flex; gap: 2px; padding: 6px 10px 0; border-bottom: 1px solid var(--border); flex-shrink: 0; }
  .wk-rtab {
    border: 0; border-bottom: 2px solid transparent; background: transparent; padding: 4px 10px 5px; font-size: 12px; font-weight: 600;
    color: var(--dim); cursor: pointer; display: inline-flex; align-items: center; gap: 5px; border-radius: 0;
  }
  .wk-rtab:hover { color: var(--text); background: transparent; }
  .wk-rtab.wk-rtab-on { color: var(--accent); border-bottom-color: var(--accent); }
  .wk-rtab-dot { width: 6px; height: 6px; border-radius: 50%; background: var(--accent); }
</style>
