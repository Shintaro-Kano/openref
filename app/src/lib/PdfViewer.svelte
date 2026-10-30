<script module lang="ts">
  // 実グリフ幅測定用のオフスクリーン canvas ctx。全 viewer インスタンスで 1 個を遅延生成し再利用する
  // （矩形ごとに canvas を作らない）。fragmentRects/advanceOffset に MeasureFn として渡す。
  // 使うのは相対比 measure(prefix)/measure(whole) のみなので、測定フォントと埋込フォントの絶対字送り差・
  // font-load タイミングの揺れを構造的に受けない（字形プロポーションは概ねフォント間で保存される）。
  // styles 不在（font 無し）や fh<=0 のときは 0 を返し、advanceOffset 側で等分近似へフォールバックさせる。
  let measureCtx: CanvasRenderingContext2D | null = null;
  function glyphMeasure(it: { font?: string; fh: number }, str: string): number {
    if (!it.font || it.fh <= 0) return 0;
    if (measureCtx === null) measureCtx = document.createElement("canvas").getContext("2d");
    if (!measureCtx) return 0;
    measureCtx.font = `${it.fh}px ${it.font}`;
    return measureCtx.measureText(str).width;
  }
</script>

<script lang="ts">
  import { i18n, t, tn } from "./i18n.svelte";
  /* pdf.js viewer。
     - 遅延描画: 全ページの正確な寸法のプレースホルダを即配置 → IntersectionObserver で可視近傍のみ
       canvas + テキスト層を実描画。遠のいたページは解放してプレースホルダへ戻す（メモリ）。
     - ズーム（自動/ページ全体/幅・25% 刻み 50〜300%）・90° 回転・scroll spy・文書内検索（⌘F）。
     - 注釈 anchor は TextQuoteSelector（quote + page）。表示時に flat テキストで quote を検索し、
       一致した文字区間を PDF item 幾何（item.transform + item.width）へ写して矩形化する。
       行頭/行末（item 境界）は幾何で厳密。item 内の文字境界だけは実グリフ幅比（measureText の
       measure(prefix)/measure(whole)）で按分し、長い行 item 内で接頭辞ぶん累積する横ずれを断つ（advanceOffset）。
       相対比のみ使うので DOM Range/getClientRects・scaleX・font-load・フォント不一致に構造的に依存しない
       （旧 getClientRects の接頭辞累積ずれ根治 + 等分近似の残差横ずれ根治）。
     ※ readTextContent ラッパーと manualTextLayer は温存（WKWebView/JSC 対応の要）。 */
  import { tick, untrack } from "svelte";
  import * as pdfjs from "pdfjs-dist";
  import workerUrl from "pdfjs-dist/build/pdf.worker.mjs?url";
  import noteIconUrl from "pdfjs-dist/web/images/annotation-note.svg?url";
  import "pdfjs-dist/web/pdf_viewer.css"; // v6: textLayer は公式 CSS + --total-scale-factor が前提
  import { api, type AnnotationRow, type AnnotationPatch } from "./api";
  import { app, toast, type TabViewer } from "./state.svelte";
  import { composing } from "./input";
  import Toolbar from "./viewer/Toolbar.svelte";
  import SearchPanel from "./viewer/SearchPanel.svelte";
  import HighlightPopover from "./viewer/HighlightPopover.svelte";
  import SelectionPopover from "./viewer/SelectionPopover.svelte";
  import OutlineTab from "./viewer/OutlineTab.svelte";
  import ThumbnailsTab from "./viewer/ThumbnailsTab.svelte";
  import AnnotationCard from "./viewer/AnnotationCard.svelte";
  import { embeddedComments, inertMarkup, type EmbeddedComment } from "./viewer/embedded";
  import {
    normText,
    matchParts,
    absRotation,
    normRotation,
    fitScale,
    stepZoomPct,
    allMatchIndexes,
    buildSnippet,
    buildGeom,
    squashFlatSpan,
    fragmentRects,
    makePageText,
    applyReadingOrder,
    itemIndexAt,
    splitBlocks,
    bodyFontPx,
    runningTable,
    runningIndexes,
    bodyTextOf,
    colorForAnnotation,
    isClaudeAuthor,
    normalizedColor,
    popoverPosition,
    type ZoomMode,
    type Match,
    type PopoverPos,
    type GeomIndex,
    type TextBlock,
    type PageText,
    type Rect,
    type SearchHit,
  } from "./viewer/util";

  pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

  /** PDF タブ: viewer = このタブの PDF・front = 前面のタブ（隠れている間はキーを取らず、描画もしない。
      名前は描画帯の Set `active` と分ける）・onClose = タブを閉じる（× / Esc。最後の 1 枚ならビューアだけ閉じる） */
  let { viewer, front, onClose }: { viewer: TabViewer; front: boolean; onClose: () => void } = $props();
  // App が添付 id ごとに作り直す（{#key}）= 生きている間は同じ PDF。タブを閉じた（viewer = null）後に届いた await の続きでも
  // 読めるよう最初の値を控える
  const itemKey = untrack(() => viewer.itemKey);
  const attachmentId = untrack(() => viewer.attachmentId);

  // ---- 反応状態（UI 駆動） ----
  let rootEl = $state<HTMLDivElement | null>(null); // .viewer-overlay（隠したときにフォーカスを外す範囲）
  let container = $state<HTMLDivElement | null>(null); // .pages スクローラ
  let pageCount = $state(0);
  let loading = $state(true);
  let error = $state("");
  let anns = $state<AnnotationRow[]>([]);
  let embedded = $state<EmbeddedComment[]>([]);
  let embeddedLoading = $state(false);
  let embeddedError = $state("");
  // 本文と著者は別々に照合する（繋いで照合すると本文末尾と著者名を跨いで一致しうる）。照合は自前注釈と同じ matchParts
  const filteredEmbedded = $derived(embedded.filter((a) => matchParts(a.text, annQuery) || matchParts(a.author, annQuery)));
  let selInfo = $state<{ text: string; page: number } | null>(null);
  let popPos = $state<PopoverPos | null>(null);
  let selColor = $state("yellow");
  let commentDraft = $state("");

  // 注釈カード編集（サイドバー）。editingKey が単一の draft を束ねる旗のため、
  // 別カードで編集を始めると editingKey の付け替えだけで前の編集は自動的に排他される。
  let editingKey = $state<string | null>(null);
  let editColor = $state("yellow");
  let editComment = $state("");

  let sideTab = $state<"search" | "annotations" | "outline" | "pages">("annotations");
  let sideDoc = $state<any>(null); // load 後の doc をタブ（目次/ページ）へ橋渡し（反応駆動用）
  let pagesWrap = $state<HTMLDivElement | null>(null); // ポップオーバー配置の基準

  const POP_W = 264; // ポップオーバー概算寸法（位置クランプ用。CSS の width と揃える）
  const POP_H = 82;

  let scale = $state(1); // 実効 scale ratio
  let zoomMode = $state<ZoomMode>("auto");
  let userRotation = $state(0); // 0/90/180/270
  let currentPage = $state(1);

  let searchQuery = $state("");
  let matchIdx = $state(-1);
  let searchHits = $state<SearchHit[]>([]); // サイドバー一覧（matches と同順）
  let searchPending = $state(false); // 入力中（デバウンス / prefetch 待ち）
  let searchPanel = $state<{ focusInput: () => void } | null>(null); // SearchPanel インスタンス（⌘F でフォーカス）
  let annQuery = $state(""); // 注釈タブの検索語（quote / コメントに部分一致・大小無視）
  // 絞り込みとカードの強調は同じ matchParts（util.ts）= 一致の有無と <mark> の断片が食い違わない
  const filteredAnns = $derived(anns.filter((a) => matchParts(a.quote ?? "", annQuery) || matchParts(a.comment ?? "", annQuery)));
  // ページ上のハイライト編集ポップオーバー（クリックで開く）
  let hlEdit = $state<{ key: string; pos: PopoverPos } | null>(null);
  let hlComment = $state("");

  // テキスト層診断（選択不可トラブルの可視化・維持）
  let tlItems = $state(0);
  let tlFallbackPages = $state(0);
  /** テキスト層の診断（ヘッダに出す）。表示のたびに組む関数（日本語の文を含むので言語の切替で描き直る） */
  let tlErr = $state<(() => string) | null>(null);

  const zoomPct = $derived(Math.round(scale * 100));
  const buildId = __BUILD_ID__; // vite define（git short sha·HHmm）。表示中のビルドの版を切り分けるためヘッダへ表示

  // ---- 非反応の内部状態（DOM/描画管理） ----
  let doc: any = null;
  // pdf.js の読込タスク。後始末はこれを destroy する（worker 側の文書も捨てる。pdfjs-dist 6 の PDFDocumentProxy は destroy を持たない）
  let loadingTask: any = null;
  let loaded = false;
  const pageEls: (HTMLElement | null)[] = []; // 1-indexed
  const baseSizes: { w: number; h: number }[] = []; // 1-indexed・intrinsic(scale=1)
  const pageRotates: number[] = []; // 1-indexed・page 固有 rotate
  const rendered = new Set<number>();
  const active = new Set<number>(); // 描画帯（RENDER_MARGIN 内）: これに入ったら描画する
  const keep = new Set<number>(); // 保持帯（KEEP_MARGIN 内・active を包含）: これを出るまで解放しない＝ヒステリシス
  const failed = new Set<number>(); // 描画失敗ページ（invalidate まで再試行しない）
  // IO 由来の描画キュー（onIntersect 発の ensureRendered 起動を同時 PUMP_CONCURRENCY 件に制限）。
  // 慣性スクロール中に IO が多数ページを同時発火させても main thread を逼迫させないための最小実装。
  // gotoPage/goToMatch/jumpTo 等の手動ジャンプは直接 ensureRendered を呼び、このキューを経由しない。
  const pending = new Set<number>(); // 描画待ちページ番号（Set なので重複投入は自動排除）
  let running = 0; // 現在 in-flight の ensureRendered 数
  const PUMP_CONCURRENCY = 2;
  // geom = そのページの描画ハイライト幾何（GeomIndex = util.ts）。full / flat / posMap は pageTexts の索引と同じ参照
  type Layer = {
    wrap: HTMLElement;
    textLayer: HTMLElement;
    hlLayer: HTMLElement;
    searchLayer: HTMLElement;
    selLayer: HTMLElement;
    geom: GeomIndex | null;
    endDiv: HTMLElement | null; // 選択ガード（pdf.js endOfContent 相当）。テキスト層描画後に生成
    spanBoxes: { el: HTMLElement; l: number; t: number; r: number; b: number }[] | null; // 隙間 pointerdown の最寄り span 探索用（textLayer 相対・遅延生成）
    domAligned: boolean; // textLayer.textContent === geom.full（選択帯を DOM Range の offset から直算してよい）
    blocks: TextBlock[] | null; // テキストブロック分割（遅延生成・柱表の構築で破棄）
    itemBlock: number[] | null; // geom item index → blocks index
    selKey: string; // 直近に描いた選択帯の矩形キー（同一なら DOM に触らない）
  };
  const layers = new Map<number, Layer>();
  // ページのテキスト索引（PageText = util.ts）。全ページ分を検索用に保持し、描画中のページの geom もこれを参照する
  const pageTexts = new Map<number, PageText>();
  const fallbackSet = new Set<number>();
  const renderTasks = new Map<number, any>(); // ページ毎の現行 RenderTask（invalidateAll / 解放時に cancel）
  let matches: Match[] = [];
  let renderGen = 0;
  let io: IntersectionObserver | null = null; // 描画帯（active）監視
  let ioKeep: IntersectionObserver | null = null; // 保持帯（keep）監視。解放ヒステリシスの外側境界
  let ro: ResizeObserver | null = null;
  let prefetchPromise: Promise<void> | null = null;
  let scrollRaf = 0;
  let searchDebounceTimer = 0; // runSearch 入力デバウンス（150ms）
  let disposed = false; // unmount 後の非同期継続（prefetch/render の reject）を診断に流さないための旗
  let fitStale = false; // 隠れている間に .pages の大きさが変わった（前面に戻ったときに合わせ直す）

  // ---------- ロード ----------
  async function load() {
    try {
      const [a] = await api.listAnnotations(itemKey);
      if (disposed) return; // 開いてすぐ閉じた（PDF は読まない）
      anns = a;
      const buf = await api.attachmentData(attachmentId); // ArrayBuffer（base64 / atob を経由しない）
      if (disposed) return; // 読込中にタブを閉じた（unmount 済み）
      loadingTask = pdfjs.getDocument({ data: new Uint8Array(buf), enableXfa:false }); // pdf.js は data を worker へ transfer する（この後は使わない）
      const d = await loadingTask.promise;
      if (disposed) return; // 開く間に閉じた（unmount の後始末が loadingTask を destroy 済み）
      doc = d;
      pageCount = doc.numPages;
      // 全ページの基準寸法（プレースホルダを正確に配置するため先に取得）
      await Promise.all(
        Array.from({ length: pageCount }, (_, i) => i + 1).map(async (n) => {
          const page = await doc.getPage(n);
          const vp = page.getViewport({ scale: 1 });
          baseSizes[n] = { w: vp.width, h: vp.height };
          pageRotates[n] = page.rotate;
        })
      );
      if (disposed) return; // 寸法を読む間に閉じた（doc は unmount の後始末が destroy 済み）
      loading = false;
      sideDoc = doc; // 目次/ページタブへ橋渡し（baseSizes/pageRotates 確定後）
      setupObservers();
      buildPlaceholders();
      recomputeFit(true); // 初期 scale（自動）を確定しプレースホルダ寸法を更新
      prefetchPromise = prefetchTexts().then(buildRunningTable); // 背景でテキスト全ページ prefetch → 柱・ノンブル表
      void readEmbeddedComments();
    } catch (e) {
      error = String(e);
      loading = false;
    }
  }

  async function readEmbeddedComments() {
    embeddedLoading=true;
    const comments:EmbeddedComment[]=[];
    try {
      for(let n=1;n<=pageCount;n++) {
        if(disposed)return;
        const page=await doc.getPage(n);
        const annotations=await page.getAnnotations({intent:"display"});
        if(disposed)return;
        comments.push(...embeddedComments(annotations,n));
        // Batch state updates; yield so a long PDF remains responsive.
        if(n%10===0 || n===pageCount){embedded=[...comments];await new Promise(resolve=>setTimeout(resolve,0));}
      }
    } catch(e) {if(!disposed)embeddedError=String(e);}
    finally {if(!disposed)embeddedLoading=false;}
  }

  /** L0 = 呼び手の ensureRendered が立てた Layer。layers.get(n) が L0 でなくなったら（解放・描き直し）古い継続なので何もしない */
  async function renderEmbeddedLayer(page:any,L0:Layer,viewport:any,n:number) {
    const wrap=L0.wrap;
    try {
      const annotations=inertMarkup(await page.getAnnotations({intent:"display"}));
      if(disposed || layers.get(n)!==L0 || !annotations.length)return;
      const div=document.createElement("div");div.className="annotationLayer embedded-annotation-layer";
      div.setAttribute("aria-hidden","true");div.inert=true;
      const layer=new pdfjs.AnnotationLayer({div,page,viewport:viewport.clone({dontFlip:true}),accessibilityManager:null,annotationCanvasMap:null,annotationEditorUIManager:null,structTreeLayer:null,commentManager:null,linkService:null,annotationStorage:null});
      await layer.render({div,page,viewport:viewport.clone({dontFlip:true}),annotations,linkService:null as any,renderForms:false,enableScripting:false,hasJSActions:false});
      // Bundle a local icon rather than resolving PDF-controlled icon names.
      for(const img of div.querySelectorAll("img"))img.src=noteIconUrl;
      if(disposed || layers.get(n)!==L0){layer.destroy();return;}
      // Canvas paints PDF appearances once. This inert layer supplies markup
      // geometry only; comment text is rendered as plain text in the sidebar.
      wrap.appendChild(div);
    } catch(e){if(!disposed && layers.get(n)===L0)embeddedError=String(e);}
  }

  // 描画帯（active）と保持帯（keep）を別マージンで二段化＝解放ヒステリシス。
  // active ⊂ keep（RENDER_MARGIN < KEEP_MARGIN）なので「描画帯内なのに解放」は原理上起きない
  // （旧・単一 rootMargin が守っていた不変条件はそのまま保つ）。加えて描画帯を出ても保持帯に
  // 残る間は解放しない → 境界での 描画↔解放 スラッシング（＝スクロール中の再描画バースト）を断つ。
  const RENDER_MARGIN = "1500px 0px 1500px 0px"; // これに入ったら描画（先読み距離は従来どおり）
  const KEEP_MARGIN = "2600px 0px 2600px 0px"; // これを出るまで解放しない（描画帯より外側）

  function setupObservers() {
    if (!container) return;
    io = new IntersectionObserver(onIntersect, { root: container, rootMargin: RENDER_MARGIN, threshold: 0 });
    ioKeep = new IntersectionObserver(onKeepIntersect, { root: container, rootMargin: KEEP_MARGIN, threshold: 0 });
    // 隠れている（前面でない）間は合わせ直さない = 描き直さない。前面に戻ったときにまとめて合わせる
    ro = new ResizeObserver(() => {
      if (front) recomputeFit(false);
      else fitStale = true;
    });
    ro.observe(container);
    container.addEventListener("scroll", onScroll, { passive: true });
  }

  function buildPlaceholders() {
    if (!container) return;
    container.replaceChildren();
    rendered.clear();
    active.clear();
    keep.clear();
    layers.clear();
    pending.clear(); // 世代跨ぎの詰まり防止
    for (let n = 1; n <= pageCount; n++) {
      const wrap = document.createElement("div");
      wrap.className = "pdfpage placeholder";
      wrap.dataset.page = String(n);
      pageEls[n] = wrap;
      applyPlaceholderSize(n);
      container.appendChild(wrap);
      io!.observe(wrap);
      ioKeep!.observe(wrap);
    }
  }

  function rotatedBase(n: number): { w: number; h: number } {
    const b = baseSizes[n];
    if (!b) return { w: 0, h: 0 };
    return userRotation % 180 === 0 ? { w: b.w, h: b.h } : { w: b.h, h: b.w };
  }

  function applyPlaceholderSize(n: number) {
    const el = pageEls[n];
    if (!el) return;
    const rb = rotatedBase(n);
    el.style.width = rb.w * scale + "px";
    el.style.height = rb.h * scale + "px";
    el.style.setProperty("--total-scale-factor", String(scale));
    el.style.setProperty("--scale-factor", String(scale));
  }

  // ---------- 遅延描画パイプライン ----------
  function onIntersect(entries: IntersectionObserverEntry[]) {
    for (const e of entries) {
      const n = Number((e.target as HTMLElement).dataset.page);
      if (e.isIntersecting) active.add(n);
      else active.delete(n);
    }
    reconcile(); // 隠れている間は帯の記録だけ（reconcile が前面になるまで描画・解放しない）
  }

  /** 保持帯（keep）の更新。描画帯より外側の境界で、これを出て初めて解放が許可される。 */
  function onKeepIntersect(entries: IntersectionObserverEntry[]) {
    for (const e of entries) {
      const n = Number((e.target as HTMLElement).dataset.page);
      if (e.isIntersecting) keep.add(n);
      else keep.delete(n);
    }
    reconcile();
  }

  /** 描画帯の中で未描画のページを描き、保持帯の外の描画済みページを解放する。隠れている（前面でない）間は何もしない
      （IO の帯の記録だけが進み、前面に戻ったときに追いつく = 読込の完了・ズームの invalidateAll からも描かない） */
  function reconcile() {
    if (!doc || !front) return;
    for (const n of active) if (!rendered.has(n)) enqueueRender(n);
    // 解放は「描画帯にも保持帯にも居ない」ページのみ（ヒステリシス）。gotoPage 等が手動で
    // active.add したページは active 条件で保護される（従来どおり）。描画帯を出ても保持帯に
    // 残る間は解放しないので、境界を跨ぐ度に 描画↔解放 を繰り返すスラッシングが消える。
    for (const n of [...rendered]) if (!active.has(n) && !keep.has(n)) releasePage(n);
  }

  /** IO 由来の描画要求を pending に積みポンプを起動する（reconcile 専用の入口）。
      Set なので同一ページの重複投入は自動的に 1 件へ畳まれる。 */
  function enqueueRender(n: number) {
    pending.add(n);
    pump();
  }

  /** pending から現在のスクロール位置に最も近い 1 件を取り出す（同時に pending から削除）。
      毎 dequeue で距離を評価し直すため、スクロール継続中も優先順が自然に追従する。 */
  function dequeueClosest(): number | undefined {
    const mid = container ? container.scrollTop + container.clientHeight / 2 : 0;
    let best: number | undefined;
    let bestDist = Infinity;
    for (const n of pending) {
      const el = pageEls[n];
      const d = el ? Math.abs(el.offsetTop + el.offsetHeight / 2 - mid) : 0;
      if (best === undefined || d < bestDist) {
        best = n;
        bestDist = d;
      }
    }
    if (best !== undefined) pending.delete(best);
    return best;
  }

  /** running < PUMP_CONCURRENCY の間 pending を汲み出し ensureRendered を起動するポンプ。
      取り出した時点で「active にも keep にも居ない」「rendered/failed 済み」「disposed」の
      いずれかに該当するページは、もう描く意味がないためスキップして捨てる（実行枠を消費しない）。
      ensureRendered は async 関数なので内部の同期 throw も自動的に rejected Promise へ畳まれる
      （async 関数は必ず Promise を返す）→ .finally は例外時も含め必ず走り、running は漏れない。 */
  function pump() {
    while (running < PUMP_CONCURRENCY && pending.size > 0) {
      const n = dequeueClosest();
      if (n === undefined) break;
      if (disposed || (!active.has(n) && !keep.has(n)) || rendered.has(n) || failed.has(n)) continue;
      running++;
      ensureRendered(n).finally(() => {
        running--;
        pump();
      });
    }
  }

  /** invalidateAll / releasePage / unmount による意図的キャンセルかどうか（pdfjs-dist 実物で export/name を確認済み）。 */
  function isCancelledRender(e: unknown): boolean {
    return e instanceof pdfjs.RenderingCancelledException || (e as any)?.name === "RenderingCancelledException";
  }

  /** ページ n の in-flight RenderTask があれば cancel して破棄する（CPU 浪費防止）。 */
  function cancelRenderTask(n: number) {
    const task = renderTasks.get(n);
    if (!task) return;
    renderTasks.delete(n);
    try {
      task.cancel();
    } catch (e) {
      console.warn("renderTask cancel failed", n, e);
    }
  }

  async function ensureRendered(n: number) {
    if (rendered.has(n) || failed.has(n)) return;
    const wrap = pageEls[n];
    if (!wrap) return;
    rendered.add(n);
    const gen = renderGen;
    // この呼び出しが立てる Layer。以降の継続は layers.get(n) === L0 の間だけ進む（解放 = delete・描き直し = 別の Layer）。
    // releasePage は renderGen を上げないので、gen と rendered だけでは「同じ世代の解放 → 再描画」の古い継続を止められない
    let L0: Layer | undefined;
    try {
      const page = await doc.getPage(n);
      // layers.has(n): 解放 → 再描画で重なった同じページの別の呼び出しが先に Layer を立てた（そちらが描く）
      if (gen !== renderGen || !rendered.has(n) || layers.has(n)) return;
      const rot = absRotation(pageRotates[n], userRotation);
      const viewport = page.getViewport({ scale, rotation: rot });
      const dpr = window.devicePixelRatio || 1;

      const canvas = document.createElement("canvas");
      canvas.width = Math.floor(viewport.width * dpr);
      canvas.height = Math.floor(viewport.height * dpr);
      canvas.style.width = viewport.width + "px";
      canvas.style.height = viewport.height + "px";
      const ctx = canvas.getContext("2d")!;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

      const textLayer = document.createElement("div");
      textLayer.className = "textLayer";
      const hlLayer = document.createElement("div");
      hlLayer.className = "hl-layer";
      const searchLayer = document.createElement("div");
      searchLayer.className = "search-layer";
      const selLayer = document.createElement("div");
      selLayer.className = "sel-layer";

      wrap.classList.remove("placeholder");
      wrap.replaceChildren(canvas, textLayer, hlLayer, searchLayer, selLayer);
      wrap.style.width = viewport.width + "px";
      wrap.style.height = viewport.height + "px";
      wrap.style.setProperty("--total-scale-factor", String(scale));
      wrap.style.setProperty("--scale-factor", String(scale));
      wrap.style.setProperty("--scale-round-x", "1px");
      wrap.style.setProperty("--scale-round-y", "1px");
      L0 = {
        wrap,
        textLayer,
        hlLayer,
        searchLayer,
        selLayer,
        geom: null,
        endDiv: null,
        spanBoxes: null,
        domAligned: false,
        blocks: null,
        itemBlock: null,
        selKey: "",
      };
      layers.set(n, L0);

      // canvas は render に渡さない（渡すと canvasContext が無視され DPR 変換が失われる v6 挙動）
      const task = page.render({ canvasContext: ctx, viewport });
      renderTasks.set(n, task);
      try {
        await task.promise;
      } finally {
        if (renderTasks.get(n) === task) renderTasks.delete(n);
      }
      if (layers.get(n) !== L0) return;

      await renderTextLayer(n, page, L0, viewport);
      if (layers.get(n) !== L0) return;
      await renderEmbeddedLayer(page,L0,viewport,n);
      if (layers.get(n) !== L0) return;

      // ハイライト幾何は現在の viewport（scale + rotation）から確定させる。
      const L = layers.get(n);
      if (L) {
        L.geom = buildGeom(pageTexts.get(n), viewport);
        // 索引と幾何の前提（makePageText の後に content.items を書き換えない = item.str 連結が pt.raw）を長さで検査する
        // （同じ長さの並べ替えは見逃す）。崩れていれば握りつぶさずヘッダの診断（tlErr）へ
        const last = L.geom.items[L.geom.items.length - 1];
        const end = last ? last.start + last.str.length : 0;
        if (end !== L.geom.full.length) {
          console.error("buildGeom: 索引と幾何の長さが合わない", n, end, L.geom.full.length);
          if (!tlErr) {
            const len = L.geom.full.length;
            tlErr = () => `p.${n} ${t("geom: 索引の長さ不一致")} ${end} ≠ ${len}`;
          }
        }
        // 選択帯の DOM 直算（selectionOrigSpans）が許されるか: テキスト層の文字連結が geom.full
        // （item.str 連結）と一致することを 1 回だけ実検査する。pdf.js TextLayer は str!=="" の item ごとに
        // 1 span・textContent=str（manualTextLayer も同じ）なので通常一致する。不一致なら squash 照合へ戻る
        L.domAligned = textLayer.textContent === L.geom.full;
        calibrateTextLayer(textLayer, L.geom);
      }
      redrawOverlays(n);
    } catch (e) {
      if (disposed || isCancelledRender(e)) return; // invalidateAll/解放/unmount による意図的キャンセルは診断に流さない
      if (L0 && layers.get(n) !== L0) return; // 古い継続の失敗で、今描いている（または解放済みの）ページを壊さない
      // ページ単位で例外を隔離し、該当ページの位置にエラーを可視化する（握りつぶさない）
      console.error("page", n, e);
      rendered.delete(n);
      layers.delete(n);
      failed.add(n);
      wrap.classList.remove("placeholder");
      const err = document.createElement("div");
      err.className = "pageerr";
      err.dataset.page = String(n);
      err.dataset.err = String(e);
      pageErrText(err);
      wrap.replaceChildren(err);
    }
  }

  /** 描画に失敗したページの文（DOM に直接置くので、言語の切替は下の $effect が書き直す） */
  function pageErrText(el: HTMLElement) {
    el.textContent = `${t("ページ")} ${el.dataset.page} ${t("の描画に失敗")}: ${el.dataset.err}`;
  }
  $effect(() => {
    i18n.lang;
    container?.querySelectorAll<HTMLElement>(".pageerr").forEach(pageErrText);
  });

  function releasePage(n: number) {
    if (!rendered.has(n)) return;
    rendered.delete(n);
    layers.delete(n);
    cancelRenderTask(n);
    const el = pageEls[n];
    if (el) {
      el.replaceChildren();
      el.classList.add("placeholder");
    }
  }

  /** 描画済みページを全部解放し（in-flight の RenderTask も打ち切る）、描画待ちも空にする（描き直すページは reconcile が帯から積み直す） */
  function releaseAllRendered() {
    for (const n of [...rendered]) releasePage(n);
    pending.clear();
  }

  /** ズーム/回転変更: 全プレースホルダ寸法を更新し、描画済みを破棄して可視ページのみ再描画 */
  function invalidateAll() {
    renderGen++;
    failed.clear(); // ズーム/回転で条件が変わるので失敗ページも再試行対象へ
    releaseAllRendered(); // 描画待ちも空にする = 世代跨ぎの詰まり防止（reconcile が現行 active から積み直す）
    for (let n = 1; n <= pageCount; n++) applyPlaceholderSize(n);
    reconcile();
  }

  /** 現在ページの上端が視野内で動かないようにスクロール位置を補正しつつ fn を実行 */
  function withAnchor(fn: () => void) {
    if (!container) {
      fn();
      return;
    }
    const n = currentPage;
    const crect = container.getBoundingClientRect();
    const el = pageEls[n];
    const before = el ? el.getBoundingClientRect().top - crect.top : 0;
    fn();
    const el2 = pageEls[n];
    if (el2) {
      const after = el2.getBoundingClientRect().top - crect.top;
      container.scrollTop += after - before;
    }
  }

  // ---------- テキスト層 ----------
  /** WKWebView(JSC) は ReadableStream の async iteration（for await...of）を実装しない。
     そのため pdf.js v6 の page.getTextContent() は内部の for await で全ページ throw する。
     streamTextContent() の getReader() を手動 pump し getTextContent と同一構造を組み立てて回避する。 */
  async function readTextContent(page: any): Promise<any> {
    const stream = page.streamTextContent({
      includeMarkedContent: false,
      disableNormalization: false,
    });
    const reader = stream.getReader();
    const textContent: any = { items: [], styles: Object.create(null), lang: null };
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      textContent.lang ??= value.lang;
      Object.assign(textContent.styles, value.styles);
      if (value.items) textContent.items.push(...value.items);
    }
    applyReadingOrder(page, textContent);
    return textContent;
  }

  async function prefetchTexts() {
    for (let n = 1; n <= pageCount; n++) {
      if (disposed) return; // unmount 済み（doc 破棄済み）: 継続しない
      if (pageTexts.has(n)) continue;
      try {
        const page = await doc.getPage(n);
        if (disposed) return;
        const content = await readTextContent(page);
        if (disposed) return;
        if (!pageTexts.has(n)) {
          pageTexts.set(n, makePageText(content));
          tlItems += content.items.length;
        }
      } catch (e) {
        if (!disposed && !tlErr) tlErr = () => `p.${n} prefetch: ${String(e)}`; // disposed 中の reject は診断を汚さない
      }
      await new Promise((r) => setTimeout(r, 0)); // 描画を妨げないよう都度 yield
      if (disposed) return;
    }
  }

  /** L0 = 呼び手の ensureRendered が立てた Layer（テキスト層は L0.textLayer）。待つ間に解放・描き直しされたら（layers.get(n) !== L0）
      選択ガードを付けない = 新しい Layer の endDiv を古いテキスト層で上書きしない */
  async function renderTextLayer(n: number, page: any, L0: Layer, viewport: any) {
    const textLayer = L0.textLayer;
    let entry = pageTexts.get(n);
    if (!entry) {
      try {
        const content = await readTextContent(page);
        entry = makePageText(content);
        pageTexts.set(n, entry);
        tlItems += content.items.length;
      } catch (e) {
        if (!tlErr) tlErr = () => `p.${n} getTextContent: ${String(e)}`;
        return;
      }
    }
    const content = entry.content;
    let count = 0;
    try {
      const tl = new pdfjs.TextLayer({ textContentSource: content, container: textLayer, viewport });
      await Promise.race([
        tl.render(),
        new Promise((_, rej) => setTimeout(() => rej(new Error("textLayer timeout")), 8000)),
      ]);
      count = textLayer.querySelectorAll("span").length;
    } catch (e) {
      if (!tlErr) tlErr = () => `p.${n} TextLayer: ${String(e)}`;
      count = 0;
    }
    if (layers.get(n) !== L0) return;
    if (count === 0) {
      textLayer.innerHTML = "";
      count = manualTextLayer(content, viewport, textLayer);
      if (count > 0 && !fallbackSet.has(n)) {
        fallbackSet.add(n);
        tlFallbackPages = fallbackSet.size;
      }
    }
    if (count > 0) {
      attachSelectionGuard(L0);
      ensureSelectionListeners();
    }
  }

  /** テキスト層 span の実レイアウト幅を item の実字送り（fa）へ一致させる。
     真因（実測）: pdf.js は span 幅を「canvas measureText で測った幅 × --scale-x」で合わせるが、
     WebKit では DOM の総称 family（"serif" 等 = pdf.js の標準フォント代替）と canvas が解決する
     フォントが別物になる（実測: DOM 698.9px vs canvas 591.8px = +18%。canvas "serif" は Times 相当）。
     結果 span 箱が実描画より 18% 広がり、ブラウザの文字当たり判定＝手動選択の起点/終点が
     行内で最大 8 文字ずれる（長い行ほど大きい）。埋込フォントが載る PDF では起きず、
     標準フォント代替の PDF でだけ出るため「特定の論文だけ」に見えていた。
     対策: span の実レイアウト幅（offsetWidth = transform 非適用）を読み、--scale-x を
     fa / 実レイアウト幅 に引き直す。span 箱と内部の文字位置が canvas 実描画（= item 幾何）に一致し、
     当たり判定・native 選択・コピー範囲がすべて実描画基準になる。 */
  function calibrateTextLayer(textLayer: HTMLElement, geom: GeomIndex) {
    const spans = textLayer.querySelectorAll<HTMLElement>("span");
    if (!spans.length || spans.length !== geom.items.length) return; // 対応が取れないなら触らない
    for (let i = 0; i < spans.length; i++) {
      if (spans[i].textContent !== geom.items[i].str) return; // 順序ずれ検出時も触らない（安全側）
    }
    const widths: number[] = [];
    for (let i = 0; i < spans.length; i++) widths.push(spans[i].offsetWidth); // 読みを一括（reflow 1 回）
    for (let i = 0; i < spans.length; i++) {
      const fa = geom.items[i].fa;
      if (widths[i] > 0 && fa > 0) spans[i].style.setProperty("--scale-x", String(fa / widths[i]));
    }
  }

  /** 古典的テキスト層（pdf.js v3 以前の renderTextLayer 相当）。transform から位置・サイズを直接計算。 */
  function manualTextLayer(textContent: any, viewport: any, container: HTMLElement): number {
    const measure = document.createElement("canvas").getContext("2d")!;
    let count = 0;
    for (const item of textContent.items as any[]) {
      if (!item.str) continue;
      const tx = (pdfjs as any).Util.transform(viewport.transform, item.transform);
      const fontHeight = Math.hypot(tx[2], tx[3]);
      if (!fontHeight) continue;
      const style = textContent.styles?.[item.fontName];
      const fam = style?.fontFamily || "sans-serif";
      const span = document.createElement("span");
      span.textContent = item.str;
      span.style.left = tx[4] + "px";
      span.style.top = tx[5] - fontHeight + "px";
      span.style.fontSize = fontHeight + "px";
      span.style.fontFamily = fam;
      measure.font = `${fontHeight}px ${fam}`;
      const w = measure.measureText(item.str).width;
      const target = (item.width ?? 0) * viewport.scale;
      if (w > 0 && target > 0) {
        // 公式 CSS の transform: scaleX(var(--scale-x)) 経由にする（calibrateTextLayer が引き直せる形）
        span.style.setProperty("--scale-x", String(target / w));
      }
      container.appendChild(span);
      count++;
    }
    return count;
  }

  // ---------- 選択プレビュー自前描画 + 選択ガード（手動ドラッグの安定化） ----------
  /** 帯の見た目は native ::selection を透明化して sel-layer に自前描画する方式（保存ハイライトと同一の
     item 幾何・行末はみ出し根治）を踏襲。本節は、ドラッグ中に帯がチカチカし、
     ドラッグ開始前後の場所へ反転する問題への根本対策で、以下の点を直す。

     1) 帯の位置を「文字列照合」から「DOM Range の位置」へ:
        旧実装は選択文字列を squash.indexOf で照合していた。ドラッグ序盤の短い/重複する文字列
        （著者欄の "Department of Radiology, " など）はページ内の別の出現 = 直前に選んだ行へ先に一致し、
        帯がカーソルと無関係な場所に出てから正しい位置へ飛ぶ（= 反転・チカチカ）。
        対策 = textLayer 先頭→選択始点の Range.toString() 長をそのまま geom.full 上の offset にする。
        Range.toString はテキストノードの連結のみで <br> を含まない（Selection.toString とは別物）。
        textLayer の文字連結 = item.str 連結 = geom.full であることは描画時に実検査して domAligned に持つ。
        文字列照合をしないので、どこを選んでも帯は native selection と同じ場所に出る。
        domAligned でないページだけ squash 照合へフォールバックする。
     2) DOM の使い回し: 毎 selectionchange に innerHTML="" → 再生成していたのをやめ、矩形キーが同じなら
        DOM に触らず、変わったときも既存 div の style だけ更新する（120Hz のドラッグで合成レイヤの
        生成/破棄を繰り返さない。帯の mix-blend-mode も外した = app.css）。mouseup（onSelect）でも
        1 回描き直し、最終状態を必ず selection と一致させる。
     3) 選択端の安定化 = pdf.js 本家 web/text_layer_builder.js（#bindMouse / selectionchange）の移植:
        span は絶対配置で隙間だらけ。隙間（行間・語間・余白）でのヒットは textLayer 自身のインライン内容
        （<br> の行箱 = 左上に積まれた縦列）へ落ちるため、選択端がポインタ位置と無関係な <br> の DOM 位置へ
        跳ぶ（例: ドラッグ開始直後に帯が 2 行上へ出る）。対策 = textLayer に .endOfContent
        （pdf_viewer.css: z-index 0 = span の z-index 1 より下。選択中はページ全面 + user-select:text）を置き、
        selectionchange ごとに「動いている側の端 span の直後（start 側なら直前）」へ DOM 移動する。
        隙間ヒットは endOfContent に落ち、選択端はその DOM 位置 = 現在の端 span の境界に吸着する。
        span は常に上にあるので span 上の伸縮は従来どおり。pointerup / blur / keyup / 選択解除で末尾へ戻す。
     4) 開始点（anchor）も守る = 本家からの拡張: 本家の endOfContent は選択中だけ全面に広がるため、
        mousedown 自体が隙間に落ちると anchor は <br> 由来の無関係な DOM 位置に置かれる（例:
        "Prostate" の 1px 左で押すと anchor=(textLayer,49)。Chromium はその後の伸長が caret に潰れ、
        WebKit は「2 行上から」の選択になる）。対策 = endOfContent を常時ページ全面に敷き
        （app.css・user-select:text）、隙間での pointerdown（target が endOfContent）で最寄りの span の
        手前/直後へ先に動かす。mousedown 既定動作はヒット節 = endOfContent の DOM 位置に caret を置くので、
        anchor は最寄り行の行頭/行末に決まる。collapsed（mousedown 直後）の selectionchange では動かさない
        （anchor を candidate 位置 (endOfContent,0) のまま保ち、最初の mousemove で伸長させる）。
     native selection 自体は従来どおり生かす（quote 抽出・popover 位置・コピー）。 */
  let selAC: AbortController | null = null; // document / window 側リスナ（painter + guard）の一括解除用
  let selPaintRaf = 0;
  let selPointerDown = false; // document の pointerdown〜pointerup（onScroll がドラッグ中の選択を消さないため）
  let guardPrevRange: Range | null = null; // pdf.js の prevRange（start 側が動いているかの判定）

  /** テキスト層描画後に endOfContent を付け、mousedown で .selecting を立てる（pdf.js #bindMouse 相当）。 */
  function attachSelectionGuard(L: Layer) {
    const textLayer = L.textLayer;
    const end = document.createElement("div");
    end.className = "endOfContent";
    textLayer.append(end);
    L.endDiv = end;
    textLayer.addEventListener("mousedown", () => textLayer.classList.add("selecting"));
    // 隙間で押した: mousedown 既定動作（caret 配置）より先に endOfContent を最寄り span の隣へ
    textLayer.addEventListener("pointerdown", (e) => {
      if (e.target === end && e.isPrimary && e.button === 0) placeGuardNearest(L, e.clientX, e.clientY);
    });
  }

  /** 隙間 pointerdown: ポインタに最も近い span（同じ行帯を優先し、次に距離）を選び、
      左/上側なら手前、右/下側なら直後へ endOfContent を移す。span 箱は textLayer 相対で遅延計測して保持。 */
  function placeGuardNearest(L: Layer, clientX: number, clientY: number) {
    if (!L.endDiv) return;
    const tr = L.textLayer.getBoundingClientRect();
    if (!L.spanBoxes) {
      L.spanBoxes = [];
      for (const el of L.textLayer.querySelectorAll<HTMLElement>(":scope > span")) {
        if (!el.textContent) continue;
        const r = el.getBoundingClientRect();
        if (r.width <= 0 || r.height <= 0) continue;
        L.spanBoxes.push({ el, l: r.left - tr.left, t: r.top - tr.top, r: r.right - tr.left, b: r.bottom - tr.top });
      }
    }
    const x = clientX - tr.left;
    const y = clientY - tr.top;
    let best: NonNullable<Layer["spanBoxes"]>[number] | null = null;
    let bestDy = Infinity;
    let bestDx = Infinity;
    for (const s of L.spanBoxes) {
      const dy = Math.max(s.t - y, y - s.b, 0);
      const dx = Math.max(s.l - x, x - s.r, 0);
      // 行帯（dy）を主、行内距離（dx）を従にする。行帯が同じ（≒同じ行）なら近い方
      if (dy < bestDy - 0.5 || (Math.abs(dy - bestDy) <= 0.5 && dx < bestDx)) {
        best = s;
        bestDy = dy;
        bestDx = dx;
      }
    }
    if (!best) return;
    const before = bestDx > 0 ? x < best.l : y < best.t;
    const parent = best.el.parentNode;
    if (!parent) return;
    parent.insertBefore(L.endDiv, before ? best.el : best.el.nextSibling);
  }

  function ensureSelectionListeners() {
    if (selAC) return;
    selAC = new AbortController();
    const signal = selAC.signal;
    document.addEventListener("pointerdown", () => (selPointerDown = true), { signal });
    const release = () => {
      selPointerDown = false;
      resetSelectionGuards();
    };
    document.addEventListener("pointerup", release, { signal });
    window.addEventListener("blur", release, { signal });
    document.addEventListener(
      "keyup",
      () => {
        if (!selPointerDown) resetSelectionGuards();
      },
      { signal }
    );
    document.addEventListener("selectionchange", onSelectionChange, { signal });
    // コピーも帯と同じ本文テキストにする（native の範囲は柱・キャプション等を DOM 順に含むため）。隠れているタブのビューアは
    // 扱わない（⌘A の全選択は隠れたタブの本文も含む = 後から登録した隠れたビューアが前面の本文を上書きしない）
    document.addEventListener(
      "copy",
      (e) => {
        if (!front) return;
        const text = selectionBodyText(document.getSelection());
        if (text == null || !e.clipboardData) return;
        e.clipboardData.setData("text/plain", text);
        e.preventDefault();
      },
      { signal }
    );
  }

  /** 選択が変わった: 端 span の隣へ endOfContent を動かし（同期 = 次の mousemove のヒットに間に合わせる）、選択の帯を描き直す。
      隠れているタブのビューアは扱わない（前面のビューアの選択で自分の帯や選択ガードを動かさない。前面に戻ったときに呼び直す） */
  function onSelectionChange() {
    if (!front) return;
    guardSelection();
    schedulePaintSelection();
  }

  function schedulePaintSelection() {
    if (selPaintRaf) return;
    selPaintRaf = requestAnimationFrame(() => {
      selPaintRaf = 0;
      paintSelection();
    });
  }

  function resetSelectionGuard(L: Layer) {
    if (L.endDiv) {
      L.textLayer.append(L.endDiv);
      L.endDiv.style.width = "";
      L.endDiv.style.height = "";
    }
    L.textLayer.classList.remove("selecting");
  }
  function resetSelectionGuards() {
    for (const [, L] of layers) resetSelectionGuard(L);
  }

  function layerOfTextLayer(tl: Element): Layer | null {
    for (const [, L] of layers) if (L.textLayer === tl) return L;
    return null;
  }

  /** pdf.js TextLayerBuilder の selectionchange ハンドラ（非 Firefox 分岐）の移植。 */
  function guardSelection() {
    const sel = document.getSelection();
    if (!sel || sel.rangeCount === 0) {
      resetSelectionGuards();
      guardPrevRange = null;
      return;
    }
    const activeL = new Set<Layer>();
    for (let i = 0; i < sel.rangeCount; i++) {
      const r = sel.getRangeAt(i);
      for (const [, L] of layers) if (!activeL.has(L) && r.intersectsNode(L.textLayer)) activeL.add(L);
    }
    for (const [, L] of layers) {
      if (activeL.has(L)) L.textLayer.classList.add("selecting");
      else resetSelectionGuard(L);
    }
    const range = sel.getRangeAt(0);
    if (range.collapsed) {
      // mousedown 直後（caret）: 動かさない。隙間始まりでは anchor が (endOfContent,0) に置かれており、
      // ここで endOfContent を動かすと anchor が (textLayer,k) に化けて最初の mousemove で伸長が潰れる
      guardPrevRange = range.cloneRange();
      return;
    }
    let modifyStart = false;
    if (guardPrevRange) {
      try {
        modifyStart =
          range.compareBoundaryPoints(Range.END_TO_END, guardPrevRange) === 0 ||
          range.compareBoundaryPoints(Range.START_TO_END, guardPrevRange) === 0;
      } catch {
        modifyStart = false; // 前回 Range の節が解放済み（root が異なる）→ 比較不能は「end が動いた」扱い
      }
    }
    let anchor: Node | null = modifyStart ? range.startContainer : range.endContainer;
    if (anchor.nodeType === Node.TEXT_NODE) anchor = anchor.parentNode;
    if (anchor && !modifyStart && range.endOffset === 0) {
      // 端が節の先頭（= 直前の節の末尾と等価）: 直前の「子を持つ」節まで戻る（endOfContent 自身や <br> を飛ばす）
      do {
        while (anchor && !anchor.previousSibling) anchor = anchor.parentNode;
        anchor = anchor ? anchor.previousSibling : null;
      } while (anchor && !anchor.childNodes.length);
    }
    const tl = anchor instanceof Element ? anchor.parentElement?.closest(".textLayer") : null;
    const L = tl ? layerOfTextLayer(tl) : null;
    if (L?.endDiv && anchor?.parentNode) {
      L.endDiv.style.width = L.textLayer.style.width; // TextLayer が付けた inline 寸法（ページ全面）
      L.endDiv.style.height = L.textLayer.style.height;
      L.endDiv.style.userSelect = "text";
      (L.endDiv.style as any).webkitUserSelect = "text";
      anchor.parentNode.insertBefore(L.endDiv, modifyStart ? anchor : anchor.nextSibling);
    }
    guardPrevRange = range.cloneRange();
  }

  function paintSelection() {
    if (disposed) return;
    const sel = document.getSelection();
    const range = sel && sel.rangeCount > 0 && !sel.isCollapsed ? sel.getRangeAt(0) : null;
    // ポップオーバー（保留中の注釈 selInfo）が開いている間、選択が textLayer の外へ移っても帯は保持する。
    // コメント欄にフォーカスすると DOM 選択は入力欄へ移る（= textLayer と交差しない）が、注釈の対象は
    // selInfo.text として確定済みなので帯を消す理由がない（消すとコメント記入中にハイライトが消えてしまう）。
    // 保存 / 閉じる / Esc / スクロールは clearSelection 経由で selInfo を外してから描き直す。
    if (selInfo && (!range || ![...layers.values()].some((L) => range.intersectsNode(L.textLayer)))) return;
    for (const [n, L] of layers) {
      let rects: Rect[] = [];
      if (range && L.geom && range.intersectsNode(L.textLayer)) {
        for (const [s0, e0] of selectionOrigSpans(L, n, range)) {
          rects.push(...fragmentRects(L.geom.items, s0, e0, glyphMeasure)); // 保存ハイライトと同じ帯
        }
      }
      applySelBands(L, rects);
    }
  }

  /** Range をこのページの textLayer 内へクランプし、geom.full 上の元 index 区間の列を返す。
      domAligned なら DOM 位置から直算（文字列照合なし）。そうでなければ squash 照合（squashFlatSpan）。
      区間の決め方（ブロック規則・帯 / quote / コピー / 注釈描画の全経路で同一）:
      - 選択の起点ブロックと終点ブロック（user が実際に指した場所）は、その中の選択範囲をそのまま採用
      - その間にある（起点でも終点でもない）ブロックは kind === "body" のものだけ採用し、
        柱・ノンブル・ロゴ（running）・キャプション・表・脚注など本文より小さい文字（small / caption）は飛ばす
      - ページ跨ぎでは、このページの手前/先にある側のブロックは「起点/終点ではない」ので同じ規則で間引かれる
      この規則が防ぐ例: 次ページの柱 "Abdominal Radiology" が帯に入る / 右カラムから次ページへ
      引くと DOM 順で後ろにある左下の "Fig. 2 …" キャプションを経由する（同型: 表・脚注・2 カラム内の図説）。
      単一ブロック内の選択はこれまでどおり無加工。キャプションや表を取りたいときはその中から / その中まで引く。 */
  function selectionOrigSpans(L: Layer, n: number, range: Range): [number, number][] {
    const geom = L.geom!;
    const pr = document.createRange();
    pr.selectNodeContents(L.textLayer);
    const r2 = range.cloneRange();
    const startsBefore = r2.compareBoundaryPoints(Range.START_TO_START, pr) < 0;
    const endsAfter = r2.compareBoundaryPoints(Range.END_TO_END, pr) > 0;
    if (startsBefore) r2.setStart(pr.startContainer, pr.startOffset);
    if (endsAfter) r2.setEnd(pr.endContainer, pr.endOffset);
    const text = r2.toString();
    if (!text) return [];
    let oStart: number | undefined;
    let oEnd: number | undefined;
    if (L.domAligned) {
      const head = document.createRange();
      head.setStart(L.textLayer, 0);
      head.setEnd(r2.startContainer, r2.startOffset);
      oStart = head.toString().length;
      oEnd = oStart + text.length;
    } else {
      const span = squashFlatSpan(geom, normText(text).replace(/ /g, ""));
      if (!span) return [];
      oStart = geom.posMap[span[0]];
      oEnd = geom.posMap[Math.min(span[1], geom.posMap.length - 1)];
      if (oStart == null || oEnd == null) return [];
    }
    if (oEnd <= oStart) return [];
    const { blocks, itemBlock } = blocksOf(n, L);
    const bStart = startsBefore ? -1 : itemBlock[itemIndexAt(geom, oStart)] ?? -1;
    const bEnd = endsAfter ? -1 : itemBlock[itemIndexAt(geom, oEnd - 1)] ?? -1;
    if (!startsBefore && !endsAfter && bStart === bEnd) return [[oStart, oEnd]]; // 単一ブロック内: 無加工
    const run = runningSet(n);
    const out: [number, number][] = [];
    let cur: [number, number] | null = null;
    let curBlock = -1;
    for (let i = 0; i < geom.items.length; i++) {
      const it = geom.items[i];
      if (it.start >= oEnd) break;
      const s0 = Math.max(oStart, it.start);
      const e0 = Math.min(oEnd, it.start + it.str.length);
      if (e0 <= s0) continue;
      const bi = itemBlock[i];
      const explicit = bi === bStart || bi === bEnd;
      const include = explicit || (blocks[bi]?.kind === "body" && !run.has(i));
      if (!include) {
        if (cur) out.push(cur);
        cur = null;
        continue;
      }
      // ブロック境界でも区間を切る（quote / コピーが段落ごとに空行で分かれ、hasEOL の無い 2 カラム
      // ページでも "…centers withFig. 1 …" のように段落末と次段落が連結しない。帯には影響しない）
      if (cur && cur[1] === s0 && bi === curBlock) cur[1] = e0;
      else {
        if (cur) out.push(cur);
        cur = [s0, e0];
        curBlock = bi;
      }
    }
    if (cur) out.push(cur);
    return out.filter(([s0, e0]) => geom.full.slice(s0, e0).trim().length > 0);
  }

  // ---------- テキストブロック分割（行 → ブロック → 種別） ----------
  /** ページ n のブロック分割（splitBlocks = util.ts）をレイヤにキャッシュする。柱表の構築（buildRunningTable）で破棄される。 */
  function blocksOf(n: number, L: Layer): { blocks: TextBlock[]; itemBlock: number[] } {
    if (L.blocks && L.itemBlock) return { blocks: L.blocks, itemBlock: L.itemBlock };
    const r = splitBlocks(L.geom!, runningSet(n), bodyFontPx(L.geom!, docBodySize, scale));
    L.blocks = r.blocks;
    L.itemBlock = r.itemBlock;
    return r;
  }

  // ---------- 柱・ノンブル（running header / footer）検出 ----------
  /** 「複数ページの同じ位置に同じ文字で出る item」を柱・ノンブル・出版社ロゴとみなす（判定 = util.ts の runningTable / runningIndexes）。
      表は prefetch 完了時に 1 回構築（それまではトリムしない）。 */
  let runningKeys: Map<string, number> | null = null; // key → 出現ページ数
  let docBodySize: number | null = null; // 文書全体の本文文字サイズ（PDF 単位・文字数重みの最頻値）
  const runningByPage = new Map<number, Set<number>>(); // page → running な geom item index（表構築後のみキャッシュ）

  function buildRunningTable() {
    if (disposed) return;
    const t = runningTable([...pageTexts.values()].map((pt) => (pt.content?.items ?? []) as any[]));
    runningKeys = t.counts;
    docBodySize = t.bodySize;
    runningByPage.clear();
    for (const [, L] of layers) {
      L.blocks = null; // 柱・本文サイズが確定したのでブロック種別を引き直す
      L.itemBlock = null;
    }
  }

  function runningSet(n: number): Set<number> {
    const cached = runningByPage.get(n);
    if (cached) return cached;
    if (!runningKeys) return new Set(); // 表未構築: キャッシュせず空（prefetch 完了後に効き始める）
    const s = runningIndexes((pageTexts.get(n)?.content?.items ?? []) as any[], runningKeys);
    runningByPage.set(n, s);
    return s;
  }

  /** 選択の本文テキスト（ブロック規則で間引いた区間列をページ順に連結）。帯と同じ区間を使うので
      quote / コピー / 保存後の注釈描画が帯と一致する。非連続の区間（飛ばしたキャプション等の前後・ページ間）は
      空行（"\n\n"）で区切る = drawAnnotations はこれを区切りに区間ごとへ照合する。描画済みページの
      textLayer と交差しない選択（サイドバー等）は null。 */
  function selectionBodyText(sel: Selection | null): string | null {
    if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return null;
    const range = sel.getRangeAt(0);
    const pages = [...layers].filter(([, L]) => L.geom && range.intersectsNode(L.textLayer));
    if (pages.length < 1) return null;
    pages.sort((a, b) => a[0] - b[0]);
    const parts: string[] = [];
    for (const [n, L] of pages) {
      for (const [s0, e0] of selectionOrigSpans(L, n, range)) parts.push(bodyTextOf(L.geom!, s0, e0).trim());
    }
    return parts.filter((p) => p).join("\n\n");
  }

  /** 絶対配置の div を矩形 r（px）に置く */
  function setBox(div: HTMLElement, r: Rect) {
    div.style.left = r.left + "px";
    div.style.top = r.top + "px";
    div.style.width = r.width + "px";
    div.style.height = r.height + "px";
  }

  /** sel-layer の帯を rects に合わせる。矩形キーが同じなら何もしない。既存 div を使い回し、余りだけ外す。 */
  function applySelBands(L: Layer, rects: Rect[]) {
    const key = rects
      .map((r) => `${r.left.toFixed(2)},${r.top.toFixed(2)},${r.width.toFixed(2)},${r.height.toFixed(2)}`)
      .join("|");
    if (key === L.selKey) return;
    L.selKey = key;
    const layer = L.selLayer;
    for (let i = 0; i < rects.length; i++) {
      const r = rects[i];
      let div = layer.children[i] as HTMLElement | undefined;
      if (!div) {
        div = document.createElement("div");
        div.className = "selband";
        layer.appendChild(div);
      }
      setBox(div, r);
    }
    while (layer.children.length > rects.length) layer.lastElementChild!.remove();
  }

  // ---------- オーバーレイ（注釈 + 検索） ----------
  // 描画幾何（GeomIndex）は util.ts の buildGeom（pageTexts の索引を参照・ensureRendered が組む）

  function redrawOverlays(n: number) {
    const L = layers.get(n);
    if (!L || !L.geom) return;
    drawAnnotations(n, L.hlLayer, L.geom);
    drawSearchHits(n, L.searchLayer, L.geom);
  }

  /** flat 上の文字区間 [flatStart, flatStart+flatLen) を PDF item 幾何の矩形群へ写して layer に描く。
      注釈（drawAnnotations）と検索（drawSearchHits）の両経路が必ずここを通る。選択帯（paintSelection）は
      flat を経ず元 index で同じ fragmentRects + glyphMeasure を呼ぶ＝矩形エンジンと校正は 3 経路で一元。
      glyphMeasure を渡すことで item 内の文字境界を実グリフ幅比で按分（行途中ヒットの横ずれ根治）。 */
  function placeRects(
    layer: HTMLElement,
    geom: GeomIndex,
    flatStart: number,
    flatLen: number,
    style: (div: HTMLElement) => void
  ) {
    const endIdx = Math.min(flatStart + flatLen, geom.posMap.length - 1);
    const oStart = geom.posMap[flatStart];
    const oEnd = geom.posMap[endIdx];
    if (oStart == null || oEnd == null) return;
    for (const r of fragmentRects(geom.items, oStart, oEnd, glyphMeasure)) {
      const div = document.createElement("div");
      setBox(div, r);
      style(div);
      layer.appendChild(div);
    }
  }

  function drawAnnotations(pageNo: number, hlLayer: HTMLElement, geom: GeomIndex) {
    hlLayer.innerHTML = "";
    const targets = anns.filter((a) => a.quote && (a.page == null || a.page === pageNo));
    if (!targets.length || !geom.flat) return;
    for (const a of targets) {
      // quote は空行（"\n\n"）で区切られた区間列になり得る（selectionBodyText: 飛ばしたキャプション等の前後・
      // ページ間）。区間ごとに照合して描く。旧来の quote（区切りなし）は区間 1 つで従来どおり。
      // 照合は squash（空白除去）空間で行う。選択 quote は行境界に <br> 由来の空白を含むが、
      // geom.flat は行末空白なしの無区切り連結になり得るため（buildSquash の注記参照）。
      const col = colorForAnnotation(a.color, a.author);
      for (const seg of a.quote!.split(/\n[ \t]*\n/)) {
        const q = normText(seg).replace(/ /g, "");
        if (q.length < 4) continue;
        // 全体が無ければ先頭 40 文字で探す（部分一致時は一致した接頭辞のみを描く = 余分な後続を着色しない）
        const span = squashFlatSpan(geom, q) ?? squashFlatSpan(geom, q.slice(0, 40));
        if (!span) continue;
        // 種別はハイライトのみ（descender まで塗る帯 + 背景のみ）。下線 / 取消線は扱わない
        // （線が文字位置とずれる + 機能過剰）。DB に残る旧 kind もハイライトとして描く。
        placeRects(hlLayer, geom, span[0], span[1] - span[0], (div) => {
          div.className = "hl";
          div.dataset.kind = "highlight";
          div.dataset.key = a.key; // ページ上クリックで編集対象を引くため
          div.style.background = col + "59";
        });
      }
    }
    if (hlEdit) markHighlightSelected(hlEdit.key); // 編集中のハイライトの破線枠を描き直す（色変更後など）
  }

  function drawSearchHits(pageNo: number, searchLayer: HTMLElement, geom: GeomIndex) {
    searchLayer.innerHTML = "";
    const q = normText(searchQuery);
    if (!q || !geom.flat) return;
    const idxs = allMatchIndexes(geom.flat, q); // geom.flat = pageTexts の full（runSearch が数えた文字列）
    for (let gi = 0; gi < matches.length; gi++) {
      const m = matches[gi];
      if (m.page !== pageNo) continue;
      const start = idxs[m.occ] ?? -1;
      if (start < 0) continue;
      const cur = gi === matchIdx;
      placeRects(
        searchLayer,
        geom,
        start,
        q.length,
        (div) => {
          div.className = cur ? "hit current" : "hit";
        }
      );
    }
  }

  function refreshAnnotationOverlays() {
    for (const n of rendered) {
      const L = layers.get(n);
      if (L && L.geom) drawAnnotations(n, L.hlLayer, L.geom);
    }
  }

  /** 注釈を取り直し、描画済みページの hl-layer だけ描き直す（追加 / 更新 / 削除の後に共通。全再描画はしない）。
      失敗は投げたままにする（toast に出すのは呼び手の mutateAnn）。古い応答が新しい anns を上書きしない（世代）・
      unmount 後に state を触らない（disposed・load() と同じ流儀）。 */
  let annGen = 0;
  async function reloadAnnotations() {
    const gen = ++annGen;
    const [list] = await api.listAnnotations(itemKey);
    if (disposed || gen !== annGen) return;
    anns = list;
    refreshAnnotationOverlays();
  }

  function refreshSearchOverlays() {
    for (const n of rendered) {
      const L = layers.get(n);
      if (L && L.geom) drawSearchHits(n, L.searchLayer, L.geom);
    }
  }

  // ---------- スクロール spy ----------
  function onScroll() {
    // スクロールで座標がずれる → 選択ポップオーバーは閉じる。ただしドラッグ選択の最中（pointer down・
    // 端が画面端に達した autoscroll など）に native selection まで消すとドラッグが途中で切れるので、
    // その間はポップオーバーだけ閉じて選択は残す
    if (selInfo) {
      if (selPointerDown) clearSelection();
      else cancelSelection();
    }
    if (hlEdit) closeHighlightEdit(); // 位置がずれるので閉じる（破線枠も外す）
    if (scrollRaf) return;
    scrollRaf = requestAnimationFrame(() => {
      scrollRaf = 0;
      updateCurrentPage();
    });
  }

  function updateCurrentPage() {
    if (!container || !pageCount) return;
    const crect = container.getBoundingClientRect();
    const mid = crect.top + container.clientHeight / 2;
    let best = currentPage;
    let bestDist = Infinity;
    for (let n = 1; n <= pageCount; n++) {
      const el = pageEls[n];
      if (!el) continue;
      const r = el.getBoundingClientRect();
      const center = r.top + r.height / 2;
      const d = Math.abs(center - mid);
      if (d < bestDist) {
        bestDist = d;
        best = n;
      } else if (center > mid) {
        break; // ページは順序どおり → 中央を過ぎたら以降は遠ざかるだけ
      }
    }
    if (best !== currentPage) currentPage = best;
  }

  // ---------- ズーム / 回転 / モード ----------
  function recomputeFit(force = false) {
    if (!container || !doc) return;
    if (zoomMode === "custom") return;
    const base = rotatedBase(1);
    const s = fitScale({
      mode: zoomMode,
      containerW: container.clientWidth,
      containerH: container.clientHeight,
      pageW: base.w,
      pageH: base.h,
    });
    // ε 抑制（fit 発振の減衰）: RO 由来の微小な clientWidth 揺れ（サブピクセル/±1px）では再フィットしない。
    // 新旧 scale の差が現 scale の 0.5% 未満なら no-op（force = 明示ズーム/モード変更は常に適用）。
    // スクロールバー出入りによる clientWidth 双安定（~15px 級）は .pages の scrollbar-gutter:stable が
    // 構造的に断つ（app.css）。ε はその外側の微振動に対する二段目の保険。
    if (force || Math.abs(s - scale) >= scale * 0.005) {
      scale = s;
      withAnchor(invalidateAll);
    }
  }

  /** custom は recomputeFit が何もしない（倍率はそのまま） */
  function setMode(m: ZoomMode) {
    zoomMode = m;
    recomputeFit(true);
  }

  function zoomStep(dir: 1 | -1) {
    const pct = stepZoomPct(scale, dir);
    zoomMode = "custom";
    scale = pct / 100;
    withAnchor(invalidateAll);
  }

  function rotate(dir: 1 | -1) {
    userRotation = normRotation(userRotation + dir * 90);
    if (zoomMode === "custom") withAnchor(invalidateAll);
    else recomputeFit(true);
  }

  function gotoPage(n: number) {
    const p = Math.min(pageCount, Math.max(1, Math.floor(n)));
    const el = pageEls[p];
    if (!el) return;
    active.add(p);
    void ensureRendered(p);
    el.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  // ---------- 検索（サイドバー「検索」タブ・Paperpile の Search document 相当） ----------
  /** 入力由来の再計算のみ 150ms デバウンス（Enter 巡回と Esc / × のクリアは即応 = デバウンスを経ない） */
  function onSearchInput(v: string) {
    searchQuery = v; // 入力欄の表示は即時
    searchPending = !!normText(v);
    if (searchDebounceTimer) clearTimeout(searchDebounceTimer);
    searchDebounceTimer = window.setTimeout(() => {
      searchDebounceTimer = 0;
      void runSearch(v);
    }, 150);
  }

  /** 全ページを網羅検索し、ページ上のヒット帯（search-layer）と一覧（スニペット）を更新する。
      matches（page, occ）と searchHits は同じループで 1 件ずつ積むので index が一致する。 */
  async function runSearch(v: string) {
    searchQuery = v;
    const q = normText(v);
    // 空語は最初の await より前で終える（clearSearch はこれを同期の消去として使う）
    if (!q) {
      matches = [];
      searchHits = [];
      matchIdx = -1;
      searchPending = false;
      refreshSearchOverlays();
      return;
    }
    if (prefetchPromise) await prefetchPromise; // 全ページのテキストが揃ってから検索
    if (normText(searchQuery) !== q) return; // 待っている間に入力が進んだ: 古い検索は捨てる
    const ms: Match[] = [];
    const hits: SearchHit[] = [];
    for (let n = 1; n <= pageCount; n++) {
      const t = pageTexts.get(n);
      if (!t) continue;
      const idxs = allMatchIndexes(t.full, q);
      for (let occ = 0; occ < idxs.length; occ++) {
        ms.push({ page: n, occ });
        hits.push({ page: n, occ, ...buildSnippet(t.raw, t.posMap, t.itemStarts, idxs[occ], q.length) });
      }
    }
    matches = ms;
    searchHits = hits;
    matchIdx = matches.length ? 0 : -1;
    searchPending = false;
    if (matches.length) goToMatch(0); // goToMatch がヒット帯を描き直す
    else refreshSearchOverlays();
  }

  function goToMatch(i: number) {
    if (i < 0 || i >= matches.length) return;
    matchIdx = i;
    const m = matches[i];
    const el = pageEls[m.page];
    if (el) {
      active.add(m.page);
      el.scrollIntoView({ behavior: "smooth", block: "center" });
      void ensureRendered(m.page).then(() => refreshSearchOverlays());
    }
    refreshSearchOverlays();
  }
  function nextMatch() {
    if (matches.length) goToMatch((matchIdx + 1) % matches.length);
  }
  function prevMatch() {
    if (matches.length) goToMatch((matchIdx - 1 + matches.length) % matches.length);
  }

  /** 検索語と結果を消す（タブは開いたまま）。runSearch の空語の分岐は最初の await より前 = 同期で消える */
  function clearSearch() {
    if (searchDebounceTimer) {
      clearTimeout(searchDebounceTimer);
      searchDebounceTimer = 0;
    }
    void runSearch("");
  }
  /** ⌘F / ツールバー ⌕: サイドバーの検索タブを開いて入力欄にフォーカス */
  function openSearch() {
    sideTab = "search";
    void tick().then(() => searchPanel?.focusInput());
  }
  function toggleSearch() {
    if (sideTab === "search") sideTab = "annotations";
    else openSearch();
  }

  // ---------- ページ上のハイライト編集（クリック → ポップオーバー・Paperpile の highlight toolbar 相当） ----------
  /** クリック位置（client 座標）にある注釈ハイライトの key。hl-layer は pointer-events:none なので矩形で自前判定 */
  function hitHighlightAt(clientX: number, clientY: number): string | null {
    for (const [, L] of layers) {
      const wr = L.wrap.getBoundingClientRect();
      if (clientX < wr.left || clientX > wr.right || clientY < wr.top || clientY > wr.bottom) continue;
      for (const el of L.hlLayer.querySelectorAll<HTMLElement>(".hl")) {
        const r = el.getBoundingClientRect();
        if (clientX >= r.left && clientX <= r.right && clientY >= r.top && clientY <= r.bottom) return el.dataset.key ?? null;
      }
    }
    return null;
  }

  /** 全ページの破線枠（.hl-selbox）を外す */
  function clearHighlightSelboxes() {
    for (const [, L] of layers) L.hlLayer.querySelectorAll(".hl-selbox").forEach((e) => e.remove());
  }

  /** key のハイライト矩形群を破線枠（.hl-selbox）で囲み、その枠の client rect を返す。再描画後も呼び直せる（冪等） */
  function markHighlightSelected(key: string): DOMRect | null {
    clearHighlightSelboxes();
    for (const [, L] of layers) {
      const els = [...L.hlLayer.querySelectorAll<HTMLElement>(`.hl[data-key="${CSS.escape(key)}"]`)];
      if (!els.length) continue;
      let l = Infinity;
      let t = Infinity;
      let r = -Infinity;
      let b = -Infinity;
      for (const el of els) {
        const x = parseFloat(el.style.left);
        const y = parseFloat(el.style.top);
        const w = parseFloat(el.style.width);
        const h = parseFloat(el.style.height);
        l = Math.min(l, x);
        t = Math.min(t, y);
        r = Math.max(r, x + w);
        b = Math.max(b, y + h);
      }
      const box = document.createElement("div");
      box.className = "hl-selbox";
      setBox(box, { left: l - 3, top: t - 3, width: r - l + 6, height: b - t + 6 });
      L.hlLayer.appendChild(box);
      return box.getBoundingClientRect(); // 注釈は 1 ページに閉じる（複数ページ描画は未対応）
    }
    return null;
  }

  /** client 座標の矩形の近くに置くポップオーバーの位置（pages-wrap 相対）。呼び手が pagesWrap の存在を確かめてから呼ぶ */
  function popAt(r: { left: number; top: number; right: number; bottom: number }): PopoverPos {
    const wr = pagesWrap!.getBoundingClientRect();
    return popoverPosition(
      { left: r.left - wr.left, top: r.top - wr.top, right: r.right - wr.left, bottom: r.bottom - wr.top },
      wr.width,
      wr.height,
      POP_W,
      POP_H
    );
  }

  function openHighlightEdit(key: string) {
    const a = anns.find((x) => x.key === key);
    if (!a || !pagesWrap) return;
    const box = markHighlightSelected(key);
    if (!box) return;
    const pos = popAt(box);
    hlComment = a.comment ?? "";
    hlEdit = { key, pos };
    editingKey = null; // サイドバー側の編集と二重にしない
  }

  function closeHighlightEdit() {
    hlEdit = null;
    hlComment = "";
    clearHighlightSelboxes();
  }

  /** 注釈の書き換え → 取り直し → 成功なら ok() の文を toast（訳は完了時に引く）。失敗は「✗ …」を toast */
  async function mutateAnn(fn: () => Promise<unknown>, ok?: () => string) {
    try {
      await fn();
      await reloadAnnotations();
      if (ok) toast(ok());
    } catch (e) {
      toast("✗ " + String(e));
    }
  }

  /** 色ドット: 即適用（Paperpile と同じ）。ポップオーバーは開いたまま */
  async function hlSetColor(c: string) {
    if (!hlEdit) return;
    const key = hlEdit.key;
    await mutateAnn(() => api.updateAnnotation(key, { color: c })); // drawAnnotations が hlEdit を見て破線枠を描き直す
  }

  /** コメント保存（空にして保存 = コメント削除）。変更が無ければ API を呼ばない */
  async function hlSaveComment() {
    if (!hlEdit) return;
    const key = hlEdit.key;
    const a = anns.find((x) => x.key === key);
    const trimmed = hlComment.trim();
    const cur = a?.comment ?? "";
    closeHighlightEdit();
    if (!a || trimmed === cur) return;
    await mutateAnn(() => api.updateAnnotation(key, { comment: trimmed === "" ? null : trimmed }), () => t("✓ 注釈を更新"));
  }

  async function hlDelete() {
    if (!hlEdit) return;
    const key = hlEdit.key;
    closeHighlightEdit();
    await mutateAnn(() => api.deleteAnnotation(key), () => t("✓ ハイライトを削除"));
  }

  // ---------- 注釈（選択ポップオーバー/保存/削除/ジャンプ） ----------
  function clearSelection() {
    selInfo = null;
    popPos = null;
    commentDraft = "";
    schedulePaintSelection(); // 保持していた帯を現在の selection に合わせて描き直す（無ければ消える）
  }
  function cancelSelection() {
    clearSelection();
    window.getSelection()?.removeAllRanges();
  }

  function onSelect(e: MouseEvent) {
    paintSelection(); // mouseup 時点の selection で帯を確定（selectionchange の取りこぼし/遅延に依らない）
    const sel = window.getSelection();
    // quote は帯と同じブロック規則の本文テキスト（柱・キャプション等を飛ばした区間列）。textLayer 外の選択は native の文字列
    const text = (selectionBodyText(sel) ?? sel?.toString() ?? "").trim();
    if (!text || text.length < 4) {
      clearSelection();
      // 選択なし = クリック: 既存ハイライトの上なら編集ポップオーバーを開き、そうでなければ閉じる
      const key = hitHighlightAt(e.clientX, e.clientY);
      if (key) openHighlightEdit(key);
      else closeHighlightEdit();
      return;
    }
    closeHighlightEdit();
    const an = sel!.anchorNode;
    const pageEl = (an instanceof Element ? an : an?.parentElement)?.closest<HTMLElement>(".pdfpage");
    const page = pageEl ? Number(pageEl.dataset.page) : 1;

    // 選択矩形 → pages-wrap 相対でポップオーバー位置を確定
    let rect: DOMRect | null = null;
    try {
      rect = sel!.getRangeAt(0).getBoundingClientRect();
    } catch {
      rect = null;
    }
    if (!rect || !rect.width || !pagesWrap) {
      clearSelection();
      return;
    }
    popPos = popAt(rect);
    // 新規選択ごとに既定へリセット（yellow / コメント空）
    selColor = "yellow";
    commentDraft = "";
    selInfo = { text, page };
  }

  async function saveAnnotation() {
    const sel = selInfo;
    if (!sel) return;
    await mutateAnn(async () => {
      await api.addAnnotation({
        key: itemKey,
        quote: sel.text,
        page: sel.page,
        kind: "highlight",
        comment: commentDraft.trim() || null,
        color: selColor,
      });
      clearSelection();
      window.getSelection()?.removeAllRanges();
    }, () => t("✓ ハイライトを保存"));
  }

  /** 注釈カードの × 削除（ページ上の hlDelete と同じ mutateAnn 経路 = 失敗は「✗ …」の toast）。
      消すカードを編集中 / ページ上で編集ポップオーバーを開いていれば閉じる */
  async function removeAnnotation(key: string) {
    if (editingKey === key) editingKey = null;
    if (hlEdit?.key === key) closeHighlightEdit();
    await mutateAnn(() => api.deleteAnnotation(key), () => t("✓ ハイライトを削除"));
  }

  function startAnnotationEdit(a: AnnotationRow) {
    editingKey = a.key; // 別カードが編集中でも代入するだけで自動的に排他される（draft は単一）
    editColor = normalizedColor(a.color, a.author);
    editComment = a.comment ?? "";
  }

  function cancelAnnotationEdit() {
    editingKey = null;
  }

  async function saveAnnotationEdit(a: AnnotationRow) {
    const curColor = normalizedColor(a.color, a.author);
    const curComment = a.comment ?? "";
    const trimmed = editComment.trim();

    const patch: AnnotationPatch = {};
    if (trimmed !== curComment) patch.comment = trimmed === "" ? null : trimmed;
    if (!isClaudeAuthor(a.author) && editColor !== curColor) patch.color = editColor;

    editingKey = null; // 保存中も UI は即閉じ、連打による二重送信の的を作らない
    if (Object.keys(patch).length === 0) return; // 変更なし: API 呼び出し不要
    await mutateAnn(() => api.updateAnnotation(a.key, patch), () => t("✓ 注釈を更新"));
  }

  /** 注釈のページへ（page が範囲外なら何もしない = 端のページへ寄せない） */
  function jumpTo(a: AnnotationRow) {
    const p = a.page ?? 1;
    if (pageEls[p]) gotoPage(p);
  }

  // ---------- キーボード ----------
  /** この keydown が始まった時点の状態（capture 段で控える = どのリスナーもまだ動いていない）。onKey はこれで判断する:
      - frontAtKey = このビューアが前面だったか。実キーの keydown では window のリスナー 1 つごとに microtask が流れるので、
        前面のビューアが Esc でタブを閉じて隣の PDF タブが前面になった後に、その隣のビューアの onKey が同じ Esc を受ける
        （onKey の時点の front を見ると 1 回の Esc で 2 枚閉じる）
      - coveredAtKey = ビューアの上にエディタ / モーダル / 取込があったか。App と各モーダルの window keydown は onKey より先に走って
        Esc でそれらを閉じるので、onKey の時点の状態を見ると同じ Esc でタブまで閉じてしまう */
  let frontAtKey = false;
  let coveredAtKey = false;
  function onKeyCapture() {
    frontAtKey = front;
    coveredAtKey = !!(app.editorOpen || app.modal || app.importJob);
  }

  function onKey(e: KeyboardEvent) {
    // 隠れていたビューア（前面でないタブ）・上にエディタ / モーダル / 取込があったときは何もしない（Esc はそちらが扱う）
    if (!frontAtKey || coveredAtKey) return;
    // IME の変換中のキー（取消の Esc を含む）は入力欄のもの = 選択の取消・編集の取消・タブを閉じるにしない
    if (composing(e)) return;
    const meta = e.metaKey || e.ctrlKey;
    const el = document.activeElement as HTMLElement | null;
    const inField = !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA");
    if (meta && (e.key === "f" || e.key === "F")) {
      e.preventDefault();
      toggleSearch();
      return;
    }
    if (e.key === "Escape") {
      if (hlEdit) {
        e.preventDefault();
        if (inField) el!.blur();
        closeHighlightEdit();
      } else if (selInfo) {
        e.preventDefault();
        if (inField) el!.blur();
        cancelSelection(); // 選択ポップオーバーを閉じる（ビューアは閉じない）
      } else if (inField) {
        el!.blur(); // 入力中の Esc はフォーカスを外すだけ（ビューアは閉じない）
      } else if (editingKey) {
        // 注釈カード編集中（フォーカスが編集コントロール外でも）: 編集キャンセルのみ。
        // WebKit はボタンクリックでフォーカスを与えないため、✎ 直後の Esc はここに来る
        e.preventDefault();
        cancelAnnotationEdit();
      } else {
        onClose();
      }
      return;
    }
    if (meta && (e.key === "=" || e.key === "+")) {
      e.preventDefault();
      zoomStep(1);
    } else if (meta && e.key === "-") {
      e.preventDefault();
      zoomStep(-1);
    } else if (meta && e.key === "0") {
      e.preventDefault();
      setMode("auto");
    }
  }

  // ---------- 計測エクスポート（デバッグ専用・プロダクション無影響） ----------
  // App.svelte は <PdfViewer> を bind:this 無しで mount するため、この関数は本番では決して
  // 呼ばれない（＝描画・状態に一切副作用なし）。デバッグ・計測時だけ bind:this 経由で内部 $state を読む。
  export function __debugState() {
    return {
      scale,
      zoomPct,
      zoomMode,
      userRotation,
      currentPage,
      pageCount,
      renderGen,
      base1: baseSizes[1] ?? null,
      renderedCount: rendered.size,
      activeCount: active.size,
      keepCount: keep.size,
      pendingCount: pending.size,
      failedCount: failed.size,
      running,
      // 選択帯の DOM 直算が有効なページ（textLayer.textContent === geom.full の実検査結果）
      selDomAligned: [...layers.entries()].map(([n, L]) => [n, L.domAligned]),
      selText: selInfo?.text ?? null, // 直近 onSelect が確定した quote（ページ跨ぎの本文限定の回帰確認用）
      sideTab,
      searchHits: searchHits.length,
      hlEditKey: hlEdit?.key ?? null,
      selPage: selInfo?.page ?? null,
    };
  }

  /** デバッグ専用: ページ n の柱・ノンブル検出結果と本文区間（ページ跨ぎトリムの回帰確認用） */
  export function __debugRunning(n: number) {
    const L = layers.get(n);
    if (!L?.geom) return null;
    const pt = pageTexts.get(n);
    return {
      table: !!runningKeys,
      running: [...runningSet(n)].map((i) => L.geom!.items[i].str),
      fullLen: L.geom.full.length,
      // 索引が 1 組であることの実行時証拠: geom の posMap は pageTexts の posMap と同じ参照（配列の === は参照比較。
      // 文字列の === は値の比較なので full / flat は「同じ内容」までしか言えない = buildGeom が pt.raw / pt.full を代入している）
      sharedIndex: !!pt && L.geom.posMap === pt.posMap && L.geom.full === pt.raw && L.geom.flat === pt.full,
      head: L.geom.full.slice(0, 60),
      tail: L.geom.full.slice(-40),
    };
  }

  /** デバッグ専用: ページ n のテキストブロック分割と種別（本文/小文字/キャプション/柱） */
  export function __debugBlocks(n: number) {
    const L = layers.get(n);
    if (!L?.geom) return null;
    const { blocks } = blocksOf(n, L);
    return {
      bodyFh: +bodyFontPx(L.geom, docBodySize, scale).toFixed(2),
      docBodySize,
      blocks: blocks.map((b) => ({
        kind: b.kind,
        lines: b.lines,
        fh: +b.fh.toFixed(2),
        chars: b.end - b.start,
        firstItem: L.geom!.items[b.first].str,
        text: L.geom!.full.slice(b.start, Math.min(b.end, b.start + 50)),
      })),
    };
  }

  // ---------- ライフサイクル ----------
  $effect(() => {
    if (loaded || !container) return;
    loaded = true;
    load();
  });

  /** 隠れたタブの canvas 解放: 隠れたまま HIDDEN_RELEASE_MS 経ったら描画済みページを全部解放する（プレースホルダの寸法と
      scrollTop は残る = 前面に戻ったとき reconcile が帯の分だけ描き直す。pageTexts は検索に要るので残す）。
      ⌘1 ⌘2 の往復（読み直しの待ちが無い）を壊さないため即時ではなく遅延。1 ページ約 12.5MB（DPR 2・A4・自動ズーム上限
      1.25）× 帯 4〜6 ページ = 約 50〜75MB / タブが、放置したタブの分だけ戻る */
  const HIDDEN_RELEASE_MS = 30_000;

  // 前面になった: 隠れている間に止めていた描画とフィットを追いつかせ、ページにフォーカスする（Space / ↑↓ でスクロール。
  // フォーカスが見えない一覧の検索欄などに残っていると、キーがそこへ入る）。
  // 隠れた: フォーカスがビューアの中なら外す（隠れたページが Space / ↑↓ でスクロールしない・見えない入力欄にキーが入らない）。
  // 30 秒隠れたままなら canvas を解放（前面に戻れば effect の cleanup がタイマーを消す）
  $effect(() => {
    if (!front) {
      const el = document.activeElement;
      if (el instanceof HTMLElement && rootEl?.contains(el)) el.blur();
      const t = setTimeout(() => {
        if (!disposed) untrack(releaseAllRendered);
      }, HIDDEN_RELEASE_MS);
      return () => clearTimeout(t);
    }
    untrack(() => {
      if (fitStale) {
        fitStale = false;
        recomputeFit(false);
      }
      reconcile();
      onSelectionChange(); // 隠れている間に動いた選択に、帯と選択ガードを合わせる
      container?.focus({ preventScroll: true });
    });
  });

  $effect(() => {
    window.addEventListener("keydown", onKeyCapture, true);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKeyCapture, true);
      window.removeEventListener("keydown", onKey);
      io?.disconnect();
      ioKeep?.disconnect();
      ro?.disconnect();
      container?.removeEventListener("scroll", onScroll);
      if (scrollRaf) cancelAnimationFrame(scrollRaf);
      if (searchDebounceTimer) clearTimeout(searchDebounceTimer);
      selAC?.abort(); // 選択プレビュー描画 + 選択ガードの document/window リスナ解除
      selAC = null;
      guardPrevRange = null;
      if (selPaintRaf) cancelAnimationFrame(selPaintRaf);
      disposed = true; // prefetchTexts の継続ループ・以降の reject 診断を打ち切る
      pending.clear(); // pump の継続を止める（in-flight は renderTasks cancel 側で処理）
      for (const n of [...renderTasks.keys()]) cancelRenderTask(n); // in-flight render を全打ち切り
      try {
        loadingTask?.destroy()?.catch((e: unknown) => console.warn("pdf doc destroy failed", e));
      } catch (e) {
        console.warn("pdf doc destroy failed", e);
      }
      loadingTask = null;
      doc = null;
    };
  });
</script>

<div class="viewer-overlay" class:v-hidden={!front} bind:this={rootEl}>
  <div class="viewer">
    <header class="v-head">
      <b>{viewer?.title ?? ""}</b>
      <span class="dim">
        {pageCount ? pageCount + t(" ページ") : ""}
        {#if tlItems > 0}
          · text {tlItems}{tlFallbackPages > 0 ? `${t("（fallback")} ${tlFallbackPages}${t("p）")}` : ""}{#if tlErr}
            <span class="tl-warn">⚠ {tlErr()}</span>{/if}
        {:else if !loading}
          · <span class="tl-warn">{t("⚠ テキスト層なし")}{tlErr ? ": " + tlErr() : ""}</span>
        {/if}
      </span>
      <span class="build-id" title="build fingerprint">b {buildId}</span>
      <button class="x" title={t("タブを閉じる（⌘W）")} onclick={() => onClose()}>×</button>
    </header>

    {#if pageCount > 0}
      <Toolbar
        {zoomPct}
        mode={zoomMode}
        page={currentPage}
        {pageCount}
        searchOpen={sideTab === "search"}
        onZoomIn={() => zoomStep(1)}
        onZoomOut={() => zoomStep(-1)}
        onSetMode={setMode}
        onRotate={rotate}
        onGoto={gotoPage}
        onToggleSearch={toggleSearch}
      />
    {/if}

    <div class="v-body">
      <div class="pages-wrap" bind:this={pagesWrap}>
        <div class="pages" tabindex="-1" bind:this={container} onmouseup={onSelect}></div>
        {#if loading}<div class="pages-overlay dim">{t("PDF を読み込み中…")}</div>{/if}
        {#if error && !pageCount}<div class="pages-overlay err">{error}</div>{/if}
        {#if selInfo && popPos}
          <SelectionPopover
            pos={popPos}
            page={selInfo.page}
            color={selColor}
            comment={commentDraft}
            onColor={(c) => (selColor = c)}
            onComment={(v) => (commentDraft = v)}
            onSave={saveAnnotation}
            onCancel={cancelSelection}
          />
        {/if}
        {#if hlEdit}
          {@const ha = anns.find((x) => x.key === hlEdit!.key)}
          {#if ha}
            <HighlightPopover
              pos={hlEdit.pos}
              a={ha}
              comment={hlComment}
              onColor={hlSetColor}
              onComment={(v) => (hlComment = v)}
              onSave={hlSaveComment}
              onDelete={hlDelete}
              onClose={closeHighlightEdit}
            />
          {/if}
        {/if}
      </div>
      <aside class="v-side">
        <div class="side-tabs">
          <button class:active={sideTab === "search"} title={t("文書内検索 (⌘F)")} onclick={openSearch}>
            {t("検索")}{#if searchHits.length} <span class="dim">{searchHits.length}</span>{/if}
          </button>
          <button class:active={sideTab === "annotations"} onclick={() => (sideTab = "annotations")}>
            {t("注釈")} <span class="dim">{anns.length+embedded.length}</span>
          </button>
          <button class:active={sideTab === "outline"} onclick={() => (sideTab = "outline")}>
            {t("目次")}
          </button>
          <button class:active={sideTab === "pages"} onclick={() => (sideTab = "pages")}>
            {t("ページ::thumbnails")}
          </button>
        </div>

        {#if sideTab === "search"}
          <SearchPanel
            bind:this={searchPanel}
            query={searchQuery}
            hits={searchHits}
            {matchIdx}
            pending={searchPending}
            onInput={onSearchInput}
            onNext={nextMatch}
            onPrev={prevMatch}
            onPick={goToMatch}
            onClear={clearSearch}
          />
        {:else if sideTab === "annotations"}
          <div class="an-search">
            <input
              placeholder={t("注釈を検索")}
              value={annQuery}
              oninput={(e) => (annQuery = e.currentTarget.value)}
              onkeydown={(e) => {
                if (e.key === "Escape" && !composing(e)) {
                  e.preventDefault();
                  e.stopPropagation(); // window の Esc ハンドラ（ビューア閉）まで伝播させない
                  if (annQuery) annQuery = "";
                  else e.currentTarget.blur();
                }
              }}
            />
          </div>
          {#if annQuery.trim()}<div class="an-count">{tn(filteredAnns.length+filteredEmbedded.length, "件::annotations")}</div>{/if}
          <div class="side-scroll">
            {#if embeddedLoading}<p class="dim hint">{t("PDF 内注釈を読み込み中…")}</p>{/if}
            {#if embeddedError}<p class="dim hint">{t("PDF 内注釈の読込に失敗")}: {embeddedError}</p>{/if}
            {#if !anns.length && !embedded.length}
              <p class="dim hint">{t("本文を選択するとハイライト（コメント付き）を作成できます。")}</p>
            {:else if !filteredAnns.length && !filteredEmbedded.length}
              <p class="dim hint">{t("一致する注釈はありません。")}</p>
            {/if}
            {#each filteredAnns as a (a.key)}
              <AnnotationCard
                {a}
                query={annQuery.trim()}
                editing={editingKey === a.key}
                draftColor={editColor}
                draftComment={editComment}
                onJump={jumpTo}
                onDelete={removeAnnotation}
                onEditStart={startAnnotationEdit}
                onEditCancel={cancelAnnotationEdit}
                onEditSave={() => saveAnnotationEdit(a)}
                onDraftColor={(c) => (editColor = c)}
                onDraftComment={(v) => (editComment = v)}
              />
            {/each}
            {#if filteredEmbedded.length}<p class="dim hint">{t("PDF 内注釈（読み取り専用）")}</p>{/if}
            {#each filteredEmbedded as a (`${a.page}:${a.id}`)}
              <article class="embedded-card">
                <button class="link" onclick={()=>gotoPage(a.page)}>p. {a.page} · {a.kind}</button>
                {#if a.author}<p class="dim">{a.author}</p>{/if}
                {#if a.text}<p class="embedded-comment">{a.text}</p>{/if}
              </article>
            {/each}
          </div>
        {:else if sideTab === "outline"}
          <OutlineTab doc={sideDoc} onJump={gotoPage} />
        {:else}
          <ThumbnailsTab doc={sideDoc} {pageCount} {currentPage} {baseSizes} onJump={gotoPage} />
        {/if}
      </aside>
    </div>
  </div>
</div>
