<script lang="ts">
  import { i18n, t } from "../i18n.svelte";
  /* ページ（サムネイル）タブ（Acrobat 参照）。
     サイドバー内に独立した IntersectionObserver を張り、可視近傍だけ描画。
     メインの遅延描画とは別の page proxy 経路だが、pdf.js は同一ページの複数 canvas への
     並行 render を許容する（intentState.operatorList を共有）。それでも CPU を抑えるため
     サムネ描画は直列キューで 1 枚ずつ処理する。 */

  let {
    doc,
    pageCount,
    currentPage,
    baseSizes,
    onJump,
  }: {
    doc: any;
    pageCount: number;
    currentPage: number;
    baseSizes: { w: number; h: number }[]; // 1-indexed・intrinsic(scale=1)。親 load() で確定済み
    onJump: (page: number) => void;
  } = $props();

  const TARGET_W = 150; // サムネの目標横幅(px)

  let scroller = $state<HTMLDivElement | null>(null);
  const thumbEls: (HTMLElement[] | null)[] = []; // 1-indexed（[wrap, canvasHolder]）
  const rendered = new Set<number>();
  const queued = new Set<number>();
  const order: number[] = [];
  let running = false;
  let io: IntersectionObserver | null = null;
  let built = false;

  function thumbScale(n: number): number {
    const b = baseSizes[n];
    return b && b.w > 0 ? TARGET_W / b.w : 0.2;
  }

  $effect(() => {
    if (!scroller || !doc || built) return;
    built = true;
    build();
    return () => io?.disconnect();
  });

  // currentPage が変わったら強調 + 視野内へ
  $effect(() => {
    const n = currentPage;
    for (let i = 1; i <= pageCount; i++) {
      const els = thumbEls[i];
      if (els) els[0].classList.toggle("active", i === n);
    }
    const cur = thumbEls[n]?.[0];
    if (cur && scroller) {
      const cr = scroller.getBoundingClientRect();
      const tr = cur.getBoundingClientRect();
      if (tr.top < cr.top || tr.bottom > cr.bottom) {
        cur.scrollIntoView({ block: "nearest" });
      }
    }
  });

  // サムネイルの title（「ページ n」）は DOM に直接置く → 言語の切替で書き直す
  $effect(() => {
    i18n.lang;
    scroller?.querySelectorAll<HTMLElement>(".th").forEach((w) => (w.title = `${t("ページ")} ${w.dataset.page}`));
  });

  function build() {
    if (!scroller) return;
    scroller.replaceChildren();
    io = new IntersectionObserver(onIntersect, {
      root: scroller,
      rootMargin: "400px 0px 400px 0px",
      threshold: 0,
    });
    for (let n = 1; n <= pageCount; n++) {
      const b = baseSizes[n] ?? { w: TARGET_W, h: TARGET_W * 1.3 };
      const s = thumbScale(n);
      const wrap = document.createElement("button");
      wrap.className = "th placeholder";
      wrap.dataset.page = String(n);
      wrap.title = `${t("ページ")} ${n}`;
      wrap.onclick = () => onJump(n);

      const holder = document.createElement("div");
      holder.className = "th-canvas";
      holder.style.width = Math.round(b.w * s) + "px";
      holder.style.height = Math.round(b.h * s) + "px";

      const num = document.createElement("div");
      num.className = "th-num";
      num.textContent = String(n);

      wrap.append(holder, num);
      scroller.appendChild(wrap);
      thumbEls[n] = [wrap, holder];
      io.observe(wrap);
    }
    // 初期状態の currentPage 強調
    thumbEls[currentPage]?.[0].classList.add("active");
  }

  function onIntersect(entries: IntersectionObserverEntry[]) {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      const n = Number((e.target as HTMLElement).dataset.page);
      if (rendered.has(n) || queued.has(n)) continue;
      queued.add(n);
      order.push(n);
    }
    void pump();
  }

  async function pump() {
    if (running) return;
    running = true;
    try {
      while (order.length) {
        const n = order.shift()!;
        queued.delete(n);
        if (rendered.has(n)) continue;
        await renderThumb(n);
      }
    } finally {
      running = false;
    }
  }

  async function renderThumb(n: number) {
    const els = thumbEls[n];
    if (!els) return;
    const [wrap, holder] = els;
    rendered.add(n);
    try {
      const page = await doc.getPage(n);
      const s = thumbScale(n);
      const viewport = page.getViewport({ scale: s });
      const dpr = window.devicePixelRatio || 1;
      const canvas = document.createElement("canvas");
      canvas.width = Math.floor(viewport.width * dpr);
      canvas.height = Math.floor(viewport.height * dpr);
      canvas.style.width = viewport.width + "px";
      canvas.style.height = viewport.height + "px";
      const ctx = canvas.getContext("2d")!;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      await page.render({ canvasContext: ctx, viewport }).promise;
      holder.replaceChildren(canvas);
      wrap.classList.remove("placeholder");
    } catch (e) {
      rendered.delete(n); // 再試行余地を残す
      wrap.classList.remove("placeholder");
      holder.textContent = "×";
      console.error("thumb", n, e);
    }
  }
</script>

<div class="side-scroll thumbs" bind:this={scroller}></div>
