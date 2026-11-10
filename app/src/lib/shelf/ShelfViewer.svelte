<script lang="ts">
  /* 本棚の読み手。PDF のタブと同じく新しいタブ・全面・⌘W。
     - 読み方 = asset:// + PDFDataRangeTransport（rangeTransport.ts）。全量を読まない・素の GET を出さない
     - 描くのは可視の帯だけ・離れたページは canvas の bitmap を返して page.cleanup()（無いと 4.2 GB → 有ると約 2 GB）
     - 開いている間は生かす（タブを切り替えても位置・倍率を失わない）。隠れている間は描かず・キーを取らない
     - 落とし穴: onDestroy の例外は {#if} の effect ツリーを壊して次が開けなくなる = 後始末は全部 try の中（実際に起きる）。
       props は親の state を読む getter = 親が null にした後の await の続き・onDestroy で読むと TypeError = 最初の値を控える（実際に起きる）。
       page.render に canvas を渡すと canvasContext が無視される = canvasContext を渡す */
  import { onDestroy, onMount, tick, untrack } from "svelte";
  import * as pdfjs from "pdfjs-dist";
  import workerUrl from "pdfjs-dist/build/pdf.worker.mjs?url";
  import { convertFileSrc } from "@tauri-apps/api/core";
  import { t } from "../i18n.svelte";
  import { composing } from "../input";
  import { app, modalOpen, toggleDetail } from "../state.svelte";
  import { AssetRangeTransport, CHUNK, probeLength } from "./rangeTransport";
  import ShelfRight from "./ShelfRight.svelte";
  import { bookPos, openExternal, revealBook, saveBookPos } from "./shelf.svelte";
  import type { ShelfBookTab } from "./tree";

  pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;
  // PdfViewer と同じ（useWorkerFetch: false = tauri:// は pdf.js が worker から読めない URL と見なす）
  const pdfAsset = (dir: string) => new URL(`/pdfjs/${dir}/`, location.href).href;

  let { book, front, onClose }: { book: ShelfBookTab; front: boolean; onClose: () => void } = $props();
  const b = untrack(() => $state.snapshot(book)) as ShelfBookTab;

  let scroller = $state<HTMLDivElement | null>(null);
  let doc: pdfjs.PDFDocumentProxy | null = null;
  let loadingTask: ReturnType<typeof pdfjs.getDocument> | null = null;
  const ac = new AbortController();
  let disposed = false;

  let pageCount = $state(0);
  let currentPage = $state(1);
  let pageInput = $state("1");
  let status = $state<"loading" | "ready" | "error">("loading");
  let errorMsg = $state("");
  /** fit = 幅合わせ。数値は指定の倍率（CSS px / PDF pt） */
  let zoomMode = $state<"fit" | number>("fit");
  let effectiveScale = $state(1);
  /** 基準の寸法（pt）。1 ページ目を全ページに仮定し、描くときに実寸へ直す */
  let baseW = 595;
  let baseH = 842;
  const placeW = $derived(Math.floor(baseW * effectiveScale));
  const placeH = $derived(Math.floor(baseH * effectiveScale));

  const wrappers: (HTMLDivElement | null)[] = [];
  let observer: IntersectionObserver | null = null;
  const rendered = new Map<number, { canvas: HTMLCanvasElement; scale: number }>();
  /** 描いている途中の印（呼び出しごと）。帯を出ると外す = 入り直したページは古い呼び出しの終わりを待たずに頼める（同じ callback に [出た, 入った] が積まれて来る） */
  const rendering = new Map<number, symbol>();
  /** 描いている途中のページ。worker は頼まれた順に処理する = 帯を出たページを打ち切らないと、止まったページは通り過ぎた全部の後に回る */
  const tasks = new Map<number, ReturnType<pdfjs.PDFPageProxy["render"]>>();
  const visible = new Set<number>();

  function pageEl(el: HTMLDivElement, n: number) {
    wrappers[n - 1] = el;
    observer?.observe(el);
    return {
      destroy() {
        observer?.unobserve(el);
        wrappers[n - 1] = null;
      },
    };
  }

  function computeScale(): number {
    if (typeof zoomMode === "number") return zoomMode;
    const w = scroller ? scroller.clientWidth : 900;
    return Math.min((w - 96) / baseW, 1.6); // 左右の余白を除いた幅に合わせる（上限 1.6 倍）
  }

  function fail(e: unknown) {
    if (disposed) return;
    status = "error";
    errorMsg = e instanceof Error ? e.message : String(e);
  }

  async function load() {
    try {
      const url = convertFileSrc(b.path);
      const length = await probeLength(url, ac.signal);
      if (disposed) return;
      const range = new AssetRangeTransport(url, length, fail);
      loadingTask = pdfjs.getDocument({
        range,
        rangeChunkSize: CHUNK,
        disableAutoFetch: true,
        disableStream: true,
        enableXfa: false,
        cMapUrl: pdfAsset("cmaps"),
        standardFontDataUrl: pdfAsset("standard_fonts"),
        wasmUrl: pdfAsset("wasm"),
        iccUrl: pdfAsset("iccs"),
        useWorkerFetch: false,
      });
      const d = await loadingTask.promise;
      if (disposed) return;
      doc = d;
      const p1 = await d.getPage(1);
      if (disposed) return;
      const vp = p1.getViewport({ scale: 1 });
      baseW = vp.width;
      baseH = vp.height;
      pageCount = d.numPages;
      status = "ready";
      await tick(); // 枠を DOM に生やす
      effectiveScale = computeScale();
      await tick();
      setupObserver();
      const page = bookPos(b.id);
      if (page > 1 && page <= pageCount) scrollToPage(page);
      if (untrack(() => front)) scroller?.focus({ preventScroll: true });
    } catch (e) {
      fail(e);
    }
  }

  function setupObserver() {
    observer?.disconnect();
    observer = new IntersectionObserver(
      (entries) => {
        for (const en of entries) {
          const n = Number((en.target as HTMLElement).dataset.page);
          if (en.isIntersecting) {
            visible.add(n);
            void renderPage(n);
          } else {
            visible.delete(n);
            cancelRender(n);
            dropPage(n);
          }
        }
      },
      { root: scroller, rootMargin: "120% 0px 120% 0px" },
    );
    for (const w of wrappers) if (w) observer.observe(w);
  }

  async function renderPage(n: number) {
    if (!doc || status !== "ready" || !untrack(() => front)) return;
    const existing = rendered.get(n);
    if (existing && existing.scale === effectiveScale) return;
    if (rendering.has(n)) return;
    const mine = Symbol();
    rendering.set(n, mine);
    const live = () => rendering.get(n) === mine;
    const scale = effectiveScale;
    let task: ReturnType<pdfjs.PDFPageProxy["render"]> | null = null;
    /** まだ枠に入っていない canvas（打ち切り・失敗のときに bitmap を返す） */
    let loose: HTMLCanvasElement | null = null;
    try {
      const page = await doc.getPage(n);
      if (!live()) return; // 待つ間に帯を出た・倍率が変わった = 描かない
      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      const vp = page.getViewport({ scale: scale * dpr });
      const canvas = (loose = document.createElement("canvas"));
      canvas.width = Math.floor(vp.width);
      canvas.height = Math.floor(vp.height);
      canvas.style.width = `${Math.floor(vp.width / dpr)}px`;
      canvas.style.height = `${Math.floor(vp.height / dpr)}px`;
      const ctx = canvas.getContext("2d", { alpha: false });
      if (!ctx) throw new Error("no 2d context");
      task = page.render({ canvasContext: ctx, viewport: vp } as Parameters<typeof page.render>[0]);
      tasks.set(n, task);
      await task.promise;
      const wrapper = wrappers[n - 1];
      if (!live() || !wrapper) return;
      // 実寸へ（基準と違う判型のページ）
      wrapper.style.width = `${Math.floor(vp.width / dpr)}px`;
      wrapper.style.height = `${Math.floor(vp.height / dpr)}px`;
      wrapper.replaceChildren(canvas);
      loose = null;
      wrapper.classList.remove("sh-v-failed");
      rendered.set(n, { canvas, scale });
      if (!visible.has(n)) dropPage(n); // 安全網: 帯の外の canvas を残さない
    } catch (e) {
      if (e instanceof Error && e.name === "RenderingCancelledException") {
        cleanupPage(n); // 途中まで読んだ画像を worker に残さない
        return;
      }
      if (disposed) return;
      const wrapper = wrappers[n - 1];
      if (wrapper && !rendered.has(n)) wrapper.classList.add("sh-v-failed");
    } finally {
      if (loose) releaseCanvas(loose);
      if (task && tasks.get(n) === task) tasks.delete(n);
      if (live()) rendering.delete(n);
    }
  }

  function releaseCanvas(c: HTMLCanvasElement) {
    // 消す前に canvas の裏の bitmap を返す（WebKit は canvas の GC を待たない）
    c.width = 0;
    c.height = 0;
  }

  function cleanupPage(n: number) {
    void doc?.getPage(n).then(
      (p) => p.cleanup(),
      () => {},
    );
  }

  function dropPage(n: number) {
    const r = rendered.get(n);
    if (!r) return;
    rendered.delete(n);
    releaseCanvas(r.canvas);
    wrappers[n - 1]?.replaceChildren();
    cleanupPage(n);
  }

  function cancelRender(n: number) {
    rendering.delete(n);
    const task = tasks.get(n);
    if (!task) return;
    tasks.delete(n);
    try {
      task.cancel();
    } catch (e) {
      console.warn("shelf render cancel failed", n, e);
    }
  }

  function rerenderAll() {
    for (const n of [...tasks.keys()]) cancelRender(n);
    for (const r of rendered.values()) releaseCanvas(r.canvas);
    rendered.clear();
    rendering.clear();
    for (const w of wrappers) {
      if (!w) continue;
      w.replaceChildren();
      // 描いたときの実寸を基準へ戻す。倍率が変わらないとき（幅合わせの再押下）テンプレートの style は書き直されない = 空にすると枠が潰れる
      w.style.width = `${placeW}px`;
      w.style.height = `${placeH}px`;
    }
  }

  async function setZoom(mode: "fit" | number) {
    const keep = currentPage; // 今のページを保ったまま
    zoomMode = mode;
    effectiveScale = computeScale();
    rerenderAll();
    await tick();
    scrollToPage(keep);
    for (const n of [...visible]) void renderPage(n);
  }

  const zoomBy = (f: number) => void setZoom(Math.min(4, Math.max(0.3, effectiveScale * f)));

  function scrollToPage(n: number) {
    const w = wrappers[n - 1];
    if (w && scroller) scroller.scrollTo({ top: w.offsetTop - 12, behavior: "auto" });
  }

  /** 読んでいるページ = 枠の上端が見える範囲の 4 割の位置より上にある最後のページ（offsetTop は増える一方 = 2 分探索） */
  function pageAt(mid: number): number {
    let lo = 0;
    let hi = pageCount - 1;
    while (lo < hi) {
      const m = (lo + hi + 1) >> 1;
      if ((wrappers[m]?.offsetTop ?? 0) <= mid) lo = m;
      else hi = m - 1;
    }
    return lo + 1;
  }

  let saveTimer: ReturnType<typeof setTimeout> | undefined;
  function onScroll() {
    if (!scroller || !pageCount) return;
    currentPage = pageAt(scroller.scrollTop + scroller.clientHeight * 0.4);
    pageInput = String(currentPage);
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => saveBookPos(b.id, currentPage), 400);
  }

  function jump() {
    const n = Math.min(pageCount, Math.max(1, parseInt(pageInput, 10) || 1));
    pageInput = String(n);
    scrollToPage(n);
  }

  function editable(el: EventTarget | null) {
    const e = el as HTMLElement | null;
    return !!e && (e.tagName === "INPUT" || e.tagName === "TEXTAREA" || e.isContentEditable);
  }

  /* 上にエディタ / モーダル / 取込があったか（capture 段で控える = PdfViewer の coveredAtKey と同じ型）。モーダルの window keydown
     （例: Settings から開いた MigrationModal の Esc）は onKey より先に走って閉じるので、onKey の時点の状態を見ると同じ Esc で本のタブまで閉じる */
  let coveredAtKey = false;
  function onKeyCapture() {
    coveredAtKey = modalOpen() || app.editorOpen || !!app.importJob;
  }

  function onKey(e: KeyboardEvent) {
    if (!untrack(() => front) || coveredAtKey || modalOpen() || app.editorOpen || app.importJob || composing(e)) return;
    const cmd = e.metaKey || e.ctrlKey;
    if (cmd && (e.key === "=" || e.key === "+")) {
      e.preventDefault();
      zoomBy(1.15);
    } else if (cmd && e.key === "-") {
      e.preventDefault();
      zoomBy(1 / 1.15);
    } else if (cmd && e.key === "0") {
      e.preventDefault();
      void setZoom("fit");
    } else if (cmd || e.altKey) {
      return;
    } else if (e.key === "Escape") {
      if (editable(e.target)) (e.target as HTMLElement).blur(); // 入力欄から抜ける・チャットの入力欄でタブを閉じない
      else if (!(e.target as Element | null)?.closest?.(".wk-ch")) {
        e.preventDefault();
        onClose();
      }
    } else if (editable(e.target)) {
      return;
    } else if (e.key === "ArrowRight" || e.key === "PageDown") {
      e.preventDefault();
      scrollToPage(Math.min(pageCount, currentPage + 1));
    } else if (e.key === "ArrowLeft" || e.key === "PageUp") {
      e.preventDefault();
      scrollToPage(Math.max(1, currentPage - 1));
    }
  }

  // 前面に戻った = 隠れている間に描かなかった帯を描く・ページにフォーカス。隠れている間に幅が変わっていたら 1 回だけ合わせ直す
  // （setZoom が帯を描く = 帯の描き直しを二重に頼まない）
  $effect(() => {
    if (!front) return;
    untrack(() => {
      if (fitStale) {
        fitStale = false;
        void setZoom("fit");
      } else for (const n of visible) void renderPage(n);
      if (status === "ready") scroller?.focus({ preventScroll: true });
    });
  });

  // 幅合わせのときだけ、幅が変わったら描き直す（窓のリサイズ・右ペインの開閉・チャットの柱の開閉）。
  // 隠れている（前面でない）間は合わせ直さない = 描き直さない。印（fitStale）だけ付け、前面に戻ったときに 1 回合わせる
  // （PdfViewer の fitStale と同じ形。読み手は全部 .tabs-page の幅に追従する = 柱の開閉 1 回で開いている本が全部描き直さない）
  let resizeTimer: ReturnType<typeof setTimeout> | undefined;
  let lastW = 0;
  let fitStale = false;
  function watchSize(el: HTMLDivElement) {
    const ro = new ResizeObserver(() => {
      if (el.clientWidth === lastW) return;
      lastW = el.clientWidth;
      if (zoomMode !== "fit" || status !== "ready" || !el.clientWidth) return;
      clearTimeout(resizeTimer);
      if (!untrack(() => front)) {
        fitStale = true;
        return;
      }
      // 待つ間に隠れた（タブを切り替えた）= 合わせ直さずに印だけ
      resizeTimer = setTimeout(() => {
        if (untrack(() => front)) void setZoom("fit");
        else fitStale = true;
      }, 150);
    });
    ro.observe(el);
    return { destroy: () => ro.disconnect() };
  }

  onMount(() => {
    void load();
    window.addEventListener("keydown", onKeyCapture, true);
    window.addEventListener("keydown", onKey);
  });

  onDestroy(() => {
    try {
      disposed = true;
      window.removeEventListener("keydown", onKeyCapture, true);
      window.removeEventListener("keydown", onKey);
      clearTimeout(saveTimer);
      clearTimeout(resizeTimer);
      if (pageCount) saveBookPos(b.id, currentPage);
      observer?.disconnect();
      rendering.clear();
      for (const n of [...tasks.keys()]) cancelRender(n);
      for (const r of rendered.values()) releaseCanvas(r.canvas);
      rendered.clear();
      ac.abort();
      const task = loadingTask;
      doc = null;
      // pdf.js v6: 後始末は loadingTask.destroy()（通信の中断 + worker の破棄）。閉じたものの失敗は握る
      void task?.destroy().catch(() => {});
    } catch (e) {
      console.error("shelf viewer cleanup failed:", e);
    }
  });
</script>

<div class="viewer-overlay sh-v" class:v-hidden={!front}>
  <div class="sh-v-bar">
    <button class="sh-v-btn" title={t("タブを閉じる（⌘W）")} aria-label={t("タブを閉じる（⌘W）")} onclick={onClose}>×</button>
    <div class="sh-v-title" title={b.path}>{b.title}</div>
    {#if status === "ready"}
      <div class="sh-v-pager">
        <input aria-label={t("ページ番号")} bind:value={pageInput} onchange={jump} onkeydown={(e) => e.key === "Enter" && !composing(e) && jump()} />
        <span>/ {pageCount}</span>
      </div>
      <div class="sh-v-zoom">
        <button class="sh-v-btn" title={t("縮小（⌘-）")} aria-label={t("縮小（⌘-）")} onclick={() => zoomBy(1 / 1.15)}>−</button>
        <span class="sh-v-pct">{Math.round(effectiveScale * 100)}%</span>
        <button class="sh-v-btn" title={t("拡大（⌘+）")} aria-label={t("拡大（⌘+）")} onclick={() => zoomBy(1.15)}>{t("＋")}</button>
        <button class="sh-v-btn" class:sh-v-on={zoomMode === "fit"} title={t("幅に合わせる（⌘0）")} onclick={() => void setZoom("fit")}>{t("幅")}</button>
      </div>
    {/if}
    <button class="sh-v-btn" onclick={() => revealBook(b.root, b.id)}>{t("Finder で表示")}</button>
    <button class="sh-v-btn" onclick={() => openExternal(b.root, b.id)}>{t("外部で開く")}</button>
    <button
      class="lb-pane"
      class:lb-pane-off={!app.detailOpen}
      title={app.detailOpen ? t("右のペインを隠す（⌘⌥I）") : t("右のペインを表示（⌘⌥I）")}
      aria-label={app.detailOpen ? t("右のペインを隠す（⌘⌥I）") : t("右のペインを表示（⌘⌥I）")}
      aria-pressed={app.detailOpen}
      onclick={toggleDetail}>
      <svg width="16" height="14" viewBox="0 0 16 14" aria-hidden="true">
        <rect x="0.75" y="0.75" width="14.5" height="12.5" rx="2" fill="none" stroke="currentColor" stroke-width="1.5" />
        <rect class="lb-pane-fill" x="10" y="1.5" width="4.5" height="11" fill="currentColor" />
        <line x1="10" y1="1" x2="10" y2="13" stroke="currentColor" stroke-width="1.5" />
      </svg>
    </button>
  </div>
  <div class="sh-v-main" class:sh-v-noinfo={!app.detailOpen}>
    <div class="sh-v-body" tabindex="-1" bind:this={scroller} onscroll={onScroll} use:watchSize>
      {#if status === "loading"}
        <div class="sh-v-center"><div class="spinner"></div><p>{t("開いています…")}</p></div>
      {:else if status === "error"}
        <div class="sh-v-center">
          <p class="sh-v-err">{t("読み込みに失敗しました")}</p>
          <p>{errorMsg}</p>
          <button class="sh-v-btn" onclick={() => openExternal(b.root, b.id)}>{t("外部で開く")}</button>
        </div>
      {:else}
        {#each { length: pageCount } as _, i (i)}
          <div class="sh-v-page" data-page={i + 1} style="width:{placeW}px;height:{placeH}px" use:pageEl={i + 1}></div>
        {/each}
      {/if}
    </div>
    <!-- 隠れている読み手は右ペインを作らない（本の情報は前面の 1 つだけ。チャットはアプリ右端の柱 = ここには無い） -->
    {#if front && app.detailOpen}<ShelfRight book={b} />{/if}
  </div>
</div>

<style>
  .sh-v { display: flex; flex-direction: column; background: var(--bg); }
  .sh-v-bar { flex: none; height: 42px; display: flex; align-items: center; gap: 8px; padding: 0 10px; background: var(--panel); border-bottom: 1px solid var(--border); }
  .sh-v-title { flex: 1 1 0; min-width: 0; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .sh-v-btn { flex: none; height: 26px; padding: 0 9px; font-size: 12px; white-space: nowrap; border: 1px solid var(--border); border-radius: 6px; background: var(--panel); }
  .sh-v-btn:hover { border-color: var(--accent); }
  .sh-v-btn.sh-v-on { color: var(--accent); border-color: var(--accent); }
  .sh-v-pager { flex: none; display: flex; align-items: center; gap: 5px; color: var(--dim); font-size: 12px; }
  .sh-v-pager input { width: 52px; height: 26px; padding: 0 4px; text-align: center; border: 1px solid var(--border); border-radius: 6px; background: var(--bg); }
  .sh-v-zoom { flex: none; display: flex; align-items: center; gap: 4px; }
  .sh-v-pct { width: 42px; text-align: center; color: var(--dim); font-size: 11.5px; }
  .sh-v-main { flex: 1; min-height: 0; display: grid; grid-template-columns: minmax(0, 1fr) 380px; }
  .sh-v-main.sh-v-noinfo { grid-template-columns: minmax(0, 1fr); }
  .sh-v-body { position: relative; min-height: 0; overflow: auto; padding: 16px 0 60px; outline: none; background: #3c4047; }
  .sh-v-page { position: relative; margin: 0 auto 12px; background: #fff; box-shadow: 0 2px 10px rgba(0, 0, 0, 0.35); }
  .sh-v-page:global(.sh-v-failed)::after { content: "✕"; position: absolute; inset: 0; display: grid; place-items: center; color: #999; font-size: 20px; }
  .sh-v-center { height: 100%; display: flex; flex-direction: column; gap: 12px; align-items: center; justify-content: center; color: #d5dbe3; font-size: 12px; text-align: center; padding: 0 24px; }
  .sh-v-err { color: #ff9a8c; font-size: 13px; }
</style>
