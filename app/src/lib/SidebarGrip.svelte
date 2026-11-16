<script lang="ts">
  /* 左の列と中央の境界のつまみ（LIBRARY / NOTE / SHELF・幅の状態 = sidebarw.svelte.ts）。App が main の最後の子に置く。
     置き場 = main（app.css の main.sbw = position: relative）に対する absolute・top 0 / bottom 0 = main の高さいっぱい
     （top: auto のままでは高さが中身 = 0 になる = CSS 2.1 §10.6.4）。横は列の幅と同じ式（--sb-col を main から受け継ぐ・100% = main の幅）。
     帯（6px）は境界から中央の側へ寄せる（左へ 1px = 木の右の境界線だけに被る = 木のスクロールバーを覆わない）。
     ドラッグの骨格は ChatDock のつまみと共通（dragGuide.ts）: 動かしている間は線（.sbw-guide = position: fixed）だけ動かしてレイアウトを変えない（中央の再描画・隠れたタブの
     ResizeObserver の描き直しを起こさない）・離したときに 1 回だけ幅を確定する（始めたときのモードへ）・Esc = 取り消し（元の幅のまま）。
     範囲 = 180〜（中央の最小幅と右ペインを残せる幅）。ダブルクリック = 既定の幅（230）= dblclick と、押下の間隔の自前の判定の両方
     （押下で preventDefault するので WebKit が dblclick を出さない恐れ）。キーボード = role="separator"・← → で 16px ずつ = 押している間は線だけ・
     離したとき（keyup）に 1 回確定・起点は表示の幅（窓で詰めた幅）・Home = 既定。pointer は自作（Tauri の dragDropEnabled と HTML5 DnD が競合する）。
     集中モードでは App が置かない。class は sbw- 接頭辞 */
  import { t } from "./i18n.svelte";
  import { SIDEBAR_DEFAULT, SIDEBAR_MIN, dragMax, setSidebarW, sidebarW, clampSidebarW, type SidebarMode } from "./sidebarw.svelte";
  import { startGuideDrag } from "./dragGuide";

  let { mode, right }: { mode: SidebarMode; right: boolean } = $props();

  /** 動かしている間の線の位置（窓の x）。null = 動かしていない */
  let guide = $state<number | null>(null);
  let abort: (() => void) | null = null;
  let el: HTMLDivElement | undefined = $state();
  /** main の幅（aria と表示の幅の計算・窓の大きさの変化で読み直す） */
  let mainW = $state(0);

  /** ダブルクリックの自前の判定の間隔（ms）と、ずれの許し（px） */
  const DOUBLE_MS = 400;
  const DOUBLE_PX = 4;
  let lastDown: { t: number; x: number } | null = null;

  function mainBox(): DOMRect | null {
    return el?.parentElement?.getBoundingClientRect() ?? null;
  }
  function measure() {
    mainW = mainBox()?.width ?? 0;
  }
  /** ドラッグとキーの上限（main の幅が分からなければ上限なし） */
  function maxNow(): number | undefined {
    const w = mainBox()?.width;
    return w ? dragMax(w, right) : undefined;
  }
  /** 表示の幅 = 覚えた幅を窓で詰めた物（app.css の --sb-col と同じ式） */
  const shownW = $derived(clampSidebarW(sidebarW[mode], mainW ? dragMax(mainW, right) : undefined));
  const ariaMax = $derived(mainW ? Math.max(SIDEBAR_MIN, dragMax(mainW, right)) : undefined);

  // つまみが出た・右ペインの有無が変わった = main の幅を読み直す
  $effect(() => {
    void right;
    void el;
    measure();
  });

  // 消えるとき（モードの切替・集中モード）は進行中のドラッグ / キーを確定せずに終える
  $effect(() => () => {
    abort?.();
    keyAbort();
  });

  function grip(e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault(); // 入力欄・エディタの focus と選択を動かさない
    // 押下の間隔でダブルクリックを判定する（dblclick が届かなくても既定へ戻す）
    const now = performance.now();
    if (lastDown && now - lastDown.t < DOUBLE_MS && Math.abs(e.clientX - lastDown.x) <= DOUBLE_PX) {
      lastDown = null;
      setSidebarW(mode, SIDEBAR_DEFAULT);
      return;
    }
    lastDown = { t: now, x: e.clientX };
    const box = mainBox();
    if (!box) return;
    const m = mode; // 始めたときのモードへ書く（途中でモードが変わっても新しいモードへ書かない）
    const max = dragMax(box.width, right);
    abort?.();
    abort = startGuideDrag(e, {
      at: (x) => {
        const w = clampSidebarW(x - box.left, max);
        return { value: w, guide: box.left + w };
      },
      guide: (x) => (guide = x),
      commit: (w) => setSidebarW(m, w, max),
      escape: true, // Esc = 取り消し（元の幅のまま・背後へ流さない）
    });
  }

  /** ← → を押している間の幅（離したときに確定）。モードは押し始めたときの物 */
  let keyNext: { mode: SidebarMode; w: number; max: number | undefined } | null = null;
  function keyAbort() {
    if (keyNext) guide = null;
    keyNext = null;
  }
  function keyCommit() {
    const k = keyNext;
    keyAbort();
    if (k) setSidebarW(k.mode, k.w, k.max);
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      e.preventDefault();
      const box = mainBox();
      const max = maxNow();
      // 起点 = 表示の幅（窓で詰めた幅。覚えた幅から数えると、最初の 1 押しは見た目が変わらずに覚えた値だけが変わる）
      if (!keyNext || keyNext.mode !== mode) keyNext = { mode, w: clampSidebarW(sidebarW[mode], max), max };
      keyNext.w = clampSidebarW(keyNext.w + (e.key === "ArrowRight" ? 16 : -16), max);
      guide = (box?.left ?? 0) + keyNext.w;
    } else if (e.key === "Home") {
      e.preventDefault();
      keyAbort();
      setSidebarW(mode, SIDEBAR_DEFAULT, maxNow());
    } else if (e.key === "Escape" && keyNext) {
      e.preventDefault();
      e.stopPropagation();
      keyAbort();
    }
  }

  function onKeyUp(e: KeyboardEvent) {
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") keyCommit();
  }
</script>

<svelte:window onresize={measure} />

<!-- 動かせる区切り（aria-valuenow つきの separator = フォーカスできる部品）。lint は separator を非対話と見なすので抑える -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
  class="sbw-grip"
  bind:this={el}
  role="separator"
  aria-orientation="vertical"
  aria-label={t("左の列の幅")}
  aria-valuenow={shownW}
  aria-valuemin={SIDEBAR_MIN}
  aria-valuemax={ariaMax}
  tabindex="0"
  title={t("ドラッグで幅を変える（ダブルクリックで元の幅）")}
  onpointerdown={grip}
  ondblclick={() => setSidebarW(mode, SIDEBAR_DEFAULT)}
  onkeydown={onKey}
  onkeyup={onKeyUp}
  onblur={keyCommit}
></div>
{#if guide != null}<div class="sbw-guide" style:left="{guide}px" aria-hidden="true"></div>{/if}

<style>
  /* --sb-col の既定 230 = sidebarw.svelte.ts の SIDEBAR_DEFAULT と対（つまみは main.sbw の中にだけ出る = 実際は常に --sb-col がある） */
  .sbw-grip {
    position: absolute; top: 0; bottom: 0; left: var(--sb-col, 230px); width: 6px; margin-left: -1px; z-index: 4;
    cursor: col-resize; touch-action: none;
  }
  .sbw-grip:hover, .sbw-grip:focus-visible { background: linear-gradient(to right, var(--accent) 2px, transparent 2px); outline: none; }
  .sbw-guide { position: fixed; top: 0; bottom: 0; width: 2px; margin-left: -1px; background: var(--accent); z-index: 30; pointer-events: none; }
</style>
