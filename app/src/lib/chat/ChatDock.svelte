<script lang="ts">
  /* チャットの面 = アプリ右端の柱 1 本。中身は WikiChat 1 つ = どのタブが前面でも同じ場所・同じ会話。
     App が .tabs-page の外（右）に置き、chatShown() && chat.dock.open のときだけ mount する（開閉 = タブ列右端のボタン / ⌘⌥C）。
     .tabs-page の {#if} の枝の外 = タブの種類を切り替えても作り直されない。
     幅 = 左端のつまみ。動かしている間は線（.cd-guide = position: fixed）だけ動かしてレイアウトを変えない = 読み手（PDF / 本）は
     動かしている間に描き直さない。離したときに 1 回だけ幅を確定する（setDockWidth = 320〜窓幅の半分・ui_chat_dock）。
     表示の幅は窓に合わせて収める（覚えた値は書き換えない）。
     状態の読み直し = 柱を開いた（mount）とき・開いたまま前面のタブが変わったとき（Terminal での claude auth login を拾う）。読み中とは重ねない。
     pointer は自作（Tauri の dragDropEnabled と HTML5 DnD が競合する = 行ドラッグと同じ）。class は cd- 接頭辞 */
  import { untrack } from "svelte";
  import { t } from "../i18n.svelte";
  import { app } from "../state.svelte";
  import WikiChat from "./ChatPanel.svelte";
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
    const el = e.currentTarget as HTMLElement;
    const right = (el.parentElement as HTMLElement).getBoundingClientRect().right;
    let next: number | null = null;
    const onMove = (ev: PointerEvent) => {
      if (!(ev.buttons & 1)) return finish();
      next = clampDockW(right - ev.clientX, window.innerWidth);
      guide = right - next;
    };
    /** commit = 離した（pointerup・取りこぼし）。柱が消えた（abort）ときは確定しない */
    const end = (commit: boolean) => {
      abort = null;
      el.removeEventListener("pointermove", onMove);
      el.removeEventListener("pointerup", finish);
      el.removeEventListener("pointercancel", finish);
      el.removeEventListener("lostpointercapture", finish);
      window.removeEventListener("blur", finish);
      try {
        el.releasePointerCapture(e.pointerId);
      } catch {
        /* 既に離れている */
      }
      document.body.classList.remove("dragging");
      guide = null;
      if (commit && next != null) setDockWidth(next); // 幅が変わるのはここ 1 回だけ
    };
    const finish = () => end(true);
    abort?.();
    abort = () => end(false);
    try {
      el.setPointerCapture(e.pointerId);
    } catch {
      /* jsdom 等 */
    }
    document.body.classList.add("dragging");
    el.addEventListener("pointermove", onMove);
    el.addEventListener("pointerup", finish);
    el.addEventListener("pointercancel", finish);
    el.addEventListener("lostpointercapture", finish);
    window.addEventListener("blur", finish);
  }
</script>

<svelte:window bind:innerWidth={vw} />

<aside class="cd" style:width="{w}px" aria-label={t("チャット")}>
  <div class="cd-grip" role="presentation" title={t("ドラッグで幅を変える")} onpointerdown={grip}></div>
  <WikiChat />
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
