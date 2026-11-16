<script lang="ts">
  /* OpenNote: 画像を大きく見る覆い（状態 = lightbox.svelte.ts）。App に 1 つ（ShadowPage の contain の外・全画面）。
     黒い背景・幅合わせ（画面に収める）/ 等倍の切替・ホイールで拡大（ポインタの位置を中心に）・ドラッグで移動・ダブルクリックで幅合わせ ⇄ 等倍・
     ← → でページの中の前後の画像・Esc（と背景のクリック）で閉じる。覆いは本文のスクロールに触らない = 閉じれば元の位置のまま（フォーカスも戻す）。
     下に alt と「n / 全数」・[新しいタブで開く]（その画像のファイルを MediaView のタブで）。
     キーは開いている間だけ capture で取り、修飾キーの無い物は背後へ流さない（`/` で検索欄へ等を起こさない・背後を変える ⌘E / ⌘⇧N / ⌘F と ⌘⌥K（引用の窓）も止める・ほかの ⌘ / ⌃ の操作 = ⌘W・⌃Tab・⌘⇧[ ]・⌘⌥C 等は素通り）。
     Tab / ⇧Tab のフォーカスは覆いの中で巡る（背後へ抜けない）。前面のタブが変わった・背後のページ（前面の OpenNote タブの path）が変わったら閉じる */
  import { tick, untrack } from "svelte";
  import { t } from "../i18n.svelte";
  import { composing } from "../input";
  import { app, frontWiki } from "../state.svelte";
  import { closeLightbox, lightbox, stepLightbox } from "./lightbox.svelte";
  import { moveTab } from "../focusTrap";

  let stage: HTMLDivElement | undefined = $state();
  let root: HTMLDivElement | undefined = $state();
  /** 表示の状態: fit = 画面に収める・one = 等倍・free = ホイール / ドラッグで動かした後 */
  let mode = $state<"fit" | "one" | "free">("fit");
  let scale = $state(1);
  let tx = $state(0);
  let ty = $state(0);
  let nat = $state<{ w: number; h: number } | null>(null);
  let failed = $state(false);

  const cur = $derived(lightbox.cur ? lightbox.cur.images[lightbox.cur.index] ?? null : null);
  const count = $derived(lightbox.cur?.images.length ?? 0);
  const index = $derived(lightbox.cur?.index ?? 0);

  /** 拡大の範囲 */
  const MIN = 0.05;
  const MAX = 20;

  function stageSize(): { w: number; h: number } {
    const r = stage?.getBoundingClientRect();
    return { w: r?.width || window.innerWidth, h: r?.height || window.innerHeight };
  }

  /** fit / one の置き方（中央） */
  function layout(m: "fit" | "one") {
    mode = m;
    if (!nat) return;
    const s = stageSize();
    scale = m === "one" ? 1 : Math.min(s.w / nat.w, s.h / nat.h);
    if (!Number.isFinite(scale) || scale <= 0) scale = 1;
    tx = (s.w - nat.w * scale) / 2;
    ty = (s.h - nat.h * scale) / 2;
  }

  // 画像が替わった = 大きさを読み直して幅合わせから
  $effect(() => {
    void cur?.src;
    untrack(() => {
      nat = null;
      failed = false;
      mode = "fit";
    });
  });

  function onLoad(e: Event) {
    const img = e.currentTarget as HTMLImageElement;
    nat = { w: img.naturalWidth || 1, h: img.naturalHeight || 1 };
    layout("fit");
  }

  // ホイール = ポインタの位置を中心に拡大 / 縮小（passive: false = 背後の箱を動かさない）
  function onWheel(e: WheelEvent) {
    e.preventDefault();
    if (!nat || !stage) return;
    const r = stage.getBoundingClientRect();
    const px = e.clientX - r.left;
    const py = e.clientY - r.top;
    const next = Math.min(MAX, Math.max(MIN, scale * Math.exp(-e.deltaY * 0.0015)));
    const f = next / scale;
    tx = px - (px - tx) * f;
    ty = py - (py - ty) * f;
    scale = next;
    mode = "free";
  }
  $effect(() => {
    const el = stage;
    if (!el) return;
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  });

  // ドラッグ = 移動。動かした後の click では閉じない
  let drag: { x: number; y: number; tx: number; ty: number; moved: boolean } | null = null;
  let dragged = false;
  function onDown(e: PointerEvent) {
    if (e.button !== 0) return;
    drag = { x: e.clientX, y: e.clientY, tx, ty, moved: false };
    dragged = false;
    try {
      (e.currentTarget as Element).setPointerCapture?.(e.pointerId);
    } catch {
      /* 取れなくても動かせる（窓の外へ出ると止まるだけ） */
    }
  }
  function onMove(e: PointerEvent) {
    if (!drag) return;
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    if (!drag.moved && Math.abs(dx) + Math.abs(dy) < 4) return;
    drag.moved = true;
    tx = drag.tx + dx;
    ty = drag.ty + dy;
    mode = "free";
  }
  function onUp() {
    dragged = !!drag?.moved;
    drag = null;
  }

  /** 背景（画像の外）のクリック = 閉じる。ドラッグの後は閉じない */
  function onStageClick(e: MouseEvent) {
    if (dragged) {
      dragged = false;
      return;
    }
    if ((e.target as Element | null)?.closest?.(".wk-lb-img")) return;
    closeLightbox();
  }

  function openTab() {
    const c = lightbox.cur;
    const nav = cur?.nav;
    if (!c || !nav) return;
    closeLightbox();
    void c.onNavigate(nav, true);
  }

  function go(d: 1 | -1) {
    stepLightbox(d);
  }

  // 開いている間: キー（capture）・フォーカスを覆いへ・閉じたら元のフォーカスへ
  $effect(() => {
    if (!lightbox.cur) return;
    const before = document.activeElement as HTMLElement | null;
    void tick().then(() => root?.focus({ preventScroll: true }));
    const key = (e: KeyboardEvent) => {
      if (composing(e)) return;
      if (e.metaKey || e.ctrlKey) {
        // 背後のページを変える ⌘ の操作（⌘E = 表示 ⇄ 編集・⌘⇧N = 新しい文献・⌘F = ページ内を検索）は止める。
        // ほかの ⌘ / ⌃（⌘W・⌃Tab・⌘⇧[ ]・⌘⌥C 等 = タブ・柱・メニュー）は素通り（タブが変われば覆いは閉じる）
        // ⌘⌥K（引用の窓 = 覆いの裏に開いてフォーカスを奪う）も止める。⌥K は配列で字が変わるので e.code で見る
        const k = e.key.toLowerCase();
        const cite = e.metaKey && e.altKey && !e.ctrlKey && !e.shiftKey && e.code === "KeyK";
        if (cite || (e.metaKey && !e.altKey && !e.ctrlKey && (((k === "e" || k === "f") && !e.shiftKey) || (k === "n" && e.shiftKey)))) {
          e.preventDefault();
          e.stopPropagation();
        }
        return;
      }
      e.stopPropagation();
      if (e.key === "Tab") {
        // 覆いの中のボタンを巡る（覆いそのものに焦点があれば端から）= 窓の骨格と同じ moveTab。
        // この受け手は捕獲の段で止める = 骨格の受け手には届かない
        if (root) moveTab(e, root);
      } else if (e.key === "Escape") {
        e.preventDefault();
        closeLightbox();
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        go(-1);
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        go(1);
      }
    };
    const resize = () => {
      if (mode !== "free") layout(mode);
    };
    window.addEventListener("keydown", key, true);
    window.addEventListener("resize", resize);
    return () => {
      window.removeEventListener("keydown", key, true);
      window.removeEventListener("resize", resize);
      if (before?.isConnected) before.focus({ preventScroll: true });
    };
  });

  // 前面のタブが変わった（切替・閉じる）・背後のページが変わった（⌘[ ⌘] 等）= 閉じる（覆いの下の画像はもう無い）
  const behind = $derived(`${app.tab}\u0000${frontWiki()?.path ?? ""}`);
  let seenBehind = untrack(() => behind);
  $effect(() => {
    const b = behind;
    if (b === seenBehind) return;
    seenBehind = b;
    untrack(closeLightbox);
  });

  /** 下の行の名前 = alt（無ければ src の末尾。壊れた % の並び = data: の SVG 等は decode せずそのまま） */
  function nameOf(src: string): string {
    const tail = src.startsWith("data:") ? src.slice(0, Math.min(40, src.indexOf(",") >>> 0)) : src.replace(/[?#].*$/, "").replace(/^.*\//, "");
    try {
      return decodeURIComponent(tail);
    } catch {
      return tail;
    }
  }
  const label = $derived(cur ? cur.alt || nameOf(cur.src) : "");
</script>

{#if lightbox.cur && cur}
  <div class="wk-lb" role="dialog" aria-modal="true" aria-label={t("画像を大きく見る")} tabindex="-1" bind:this={root}>
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div
      class="wk-lb-stage"
      class:wk-lb-grab={mode !== "fit"}
      bind:this={stage}
      onpointerdown={onDown}
      onpointermove={onMove}
      onpointerup={onUp}
      onpointercancel={onUp}
      onclick={onStageClick}
      ondblclick={() => layout(mode === "fit" ? "one" : "fit")}
    >
      {#key cur.src}
        <img
          class="wk-lb-img"
          class:wk-lb-ready={!!nat}
          src={cur.src}
          alt={cur.alt}
          draggable="false"
          onload={onLoad}
          onerror={() => (failed = true)}
          style:width={nat ? `${nat.w}px` : null}
          style:height={nat ? `${nat.h}px` : null}
          style:transform={nat ? `translate(${tx}px, ${ty}px) scale(${scale})` : null}
        />
      {/key}
      {#if failed}<p class="wk-lb-fail">{t("画像を読めません")}</p>{/if}
    </div>
    {#if count > 1}
      <button class="wk-lb-nav wk-lb-prev" title={t("前の画像（←）")} aria-label={t("前の画像（←）")} disabled={index === 0} onclick={() => go(-1)}>‹</button>
      <button class="wk-lb-nav wk-lb-next" title={t("次の画像（→）")} aria-label={t("次の画像（→）")} disabled={index === count - 1} onclick={() => go(1)}>›</button>
    {/if}
    <div class="wk-lb-bar">
      <span class="wk-lb-alt" title={label}>{label}</span>
      <span class="wk-lb-n">{index + 1} / {count}</span>
      <span class="wk-lb-gap"></span>
      <button class:wk-lb-on={mode === "fit"} aria-pressed={mode === "fit"} onclick={() => layout("fit")}>{t("幅合わせ")}</button>
      <button class:wk-lb-on={mode === "one"} aria-pressed={mode === "one"} onclick={() => layout("one")}>{t("等倍")}</button>
      {#if cur.nav}<button onclick={openTab}>{t("新しいタブで開く")}</button>{/if}
      <button class="wk-lb-x" title={t("閉じる（Esc）")} aria-label={t("閉じる（Esc）")} onclick={closeLightbox}>×</button>
    </div>
  </div>
{/if}

<style>
  .wk-lb { position: fixed; inset: 0; z-index: 70; background: #000; display: flex; flex-direction: column; outline: none; }
  .wk-lb-stage { position: relative; flex: 1; min-height: 0; overflow: hidden; cursor: zoom-out; touch-action: none; }
  .wk-lb-stage.wk-lb-grab { cursor: grab; }
  .wk-lb-img { position: absolute; left: 0; top: 0; transform-origin: 0 0; max-width: none; user-select: none; visibility: hidden; cursor: default; }
  .wk-lb-img.wk-lb-ready { visibility: visible; }
  .wk-lb-grab .wk-lb-img { cursor: grab; }
  .wk-lb-fail { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; margin: 0; color: #bbb; font-size: 13px; }
  .wk-lb-nav {
    position: absolute; top: 50%; transform: translateY(-50%); width: 44px; height: 64px; border: 0; border-radius: 8px;
    background: rgba(255, 255, 255, 0.08); color: #eee; font-size: 30px; line-height: 1; cursor: pointer;
  }
  .wk-lb-nav:hover:not(:disabled) { background: rgba(255, 255, 255, 0.18); }
  .wk-lb-nav:disabled { opacity: 0.25; cursor: default; }
  .wk-lb-prev { left: 12px; }
  .wk-lb-next { right: 12px; }
  .wk-lb-bar {
    flex-shrink: 0; display: flex; align-items: center; gap: 8px; padding: 8px 14px; background: rgba(20, 20, 20, 0.95); color: #ddd;
    font-size: 12.5px;
  }
  .wk-lb-alt { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wk-lb-n { flex-shrink: 0; color: #999; font-variant-numeric: tabular-nums; }
  .wk-lb-gap { flex: 1; }
  .wk-lb-bar button {
    flex-shrink: 0; border: 1px solid #444; border-radius: 6px; padding: 3px 10px; background: #222; color: #ddd; font-size: 12px; cursor: pointer;
  }
  .wk-lb-bar button:hover { border-color: #888; }
  .wk-lb-bar button.wk-lb-on { background: #3a3a3a; color: #fff; border-color: #888; }
  .wk-lb-bar .wk-lb-x { font-size: 15px; line-height: 1; padding: 2px 9px; }
</style>
