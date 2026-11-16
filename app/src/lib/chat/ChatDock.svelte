<script lang="ts">
  /* チャットの面 = アプリ右端の柱 1 本。中身は ChatPanel 1 つ = どのタブが前面でも同じ場所・同じ会話。
     App が .tabs-page の外（右）に置き、chatShown() && chat.dock.open のときだけ mount する（開閉 = タブ列右端のボタン / ⌘⌥C）。
     .tabs-page の {#if} の枝の外 = タブの種類を切り替えても作り直されない。
     幅 = 左端のつまみ。動かしている間は線（.cd-guide = position: fixed）だけ動かしてレイアウトを変えない = 読み手（PDF / 本）は
     動かしている間に描き直さない。離したときに 1 回だけ幅を確定する（setDockWidth = 320〜窓幅の半分・ui_chat_dock）。
     表示の幅は窓に合わせて収める（覚えた値は書き換えない）。
     状態の読み直し = 柱を開いた（mount）とき・開いたまま前面のタブが変わったとき（Terminal での claude auth login を拾う）。読み中とは重ねない。
     ドラッグの骨格は左の列のつまみ（SidebarGrip）と共通 = dragGuide.ts（Esc の取り消しは柱には付けない = 今までどおり）。class は cd- 接頭辞 */
  import { untrack } from "svelte";
  import { t } from "../i18n.svelte";
  import { app } from "../state.svelte";
  import ChatPanel from "./ChatPanel.svelte";
  import { startGuideDrag } from "../dragGuide";
  import { clampDockW } from "./chat";
  import { chat, loadChatStatus, setDockWidth, statusInFlight } from "./chat.svelte";

  let vw = $state(window.innerWidth);
  const w = $derived(clampDockW(chat.dock.w, vw));
  /** つまみを動かしている間の線の位置（窓の x）。null = 動かしていない */
  let guide = $state<number | null>(null);
  /** 進行中のドラッグを終える（柱が消えるとき = pointerup が届かない） */
  let abort: (() => void) | null = null;

  // 柱を開いた（mount）・開いたまま前面のタブが変わった = 状態を読み直す（読み中なら重ねない）
  $effect(() => {
    void app.tab;
    untrack(() => {
      if (!statusInFlight()) void loadChatStatus();
    });
  });

  // 消えるとき（⌘⌥C・チャット OFF）は進行中のドラッグを終える（リスナーと body の印を残さない）
  $effect(() => () => abort?.());

  function grip(e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault(); // 入力欄・エディタの focus と選択を動かさない
    const right = ((e.currentTarget as HTMLElement).parentElement as HTMLElement).getBoundingClientRect().right;
    abort?.();
    abort = startGuideDrag(e, {
      at: (x) => {
        const w = clampDockW(right - x, window.innerWidth);
        return { value: w, guide: right - w };
      },
      guide: (x) => (guide = x),
      commit: setDockWidth,
    });
  }
</script>

<svelte:window bind:innerWidth={vw} />

<aside class="cd" style:width="{w}px" aria-label={t("チャット")}>
  <div class="cd-grip" role="presentation" title={t("ドラッグで幅を変える")} onpointerdown={grip}></div>
  <ChatPanel />
</aside>
{#if guide != null}<div class="cd-guide" style:left="{guide}px" aria-hidden="true"></div>{/if}

<style>
  /* 縮むのは .tabs-page の側（app.css の .dock-row）= 柱は常に全幅が窓の中。z-index 6 = PDF / 本の読み手（.viewer-overlay の 5）より上・モーダル（40）より下 */
  .cd {
    flex: none; position: relative; z-index: 6; display: flex; flex-direction: column; min-width: 0; min-height: 0;
    border-left: 1px solid var(--border); background: var(--panel);
  }
  /* つまみは柱の内側（左端 6px）= .tabs-page の右端（PDF / 本の重ねスクロールバー）に乗らない */
  .cd-grip { position: absolute; left: -1px; top: 0; bottom: 0; width: 6px; cursor: col-resize; z-index: 2; touch-action: none; }
  .cd-grip:hover { background: linear-gradient(to right, var(--accent) 2px, transparent 2px); }
  .cd-guide { position: fixed; top: 0; bottom: 0; width: 2px; margin-left: -1px; background: var(--accent); z-index: 30; pointer-events: none; }
</style>
