<script lang="ts">
  /* OpenNote: ページへのリンクのプレビューの浮き窓（状態と時計 = preview.svelte.ts）。App に 1 つ = 乗せた側の ShadowPage の
     `contain: layout paint` の外（中に置くと切り取られる）。中身は同じ ShadowPage を小さく（中でスクロール・`![[埋め込み]]` は既存の 1 段・
     中のリンクは乗せた側と同じ onNavigate = 同じ規則で開く・中では次のプレビューを出さない = inPreview）。`#見出し` ならそこへ
     スクロール（浮き窓の中の箱だけを動かす = 外の画面は動かさない）。まだ無いページは案内だけ。
     置き場 = 乗せた要素の下（入らなければ上）・横はポインタの位置から画面の内側へ。
     閉じる = 離れてから少し（浮き窓の上に移れば保つ）・Esc（奪う = 背後へ流さない）・外側の押下・乗せた要素を含む箱のスクロール
     （チャットの自動追従など別の箱のスクロールでは閉じない）・乗せた要素が DOM から消えた / 見えなくなった（描き直し・遷移・集中モード）・窓のフォーカスが外れた /
     大きさが変わった・タブの切替・右クリックのメニュー / アプリのモーダルが出た・中のリンクで移った */
  import { tick, untrack } from "svelte";
  import { t } from "../i18n.svelte";
  import { composing } from "../input";
  import { app, modalOpen } from "../state.svelte";
  import ShadowPage from "./render/ShadowPage.svelte";
  import { anchorTarget, findHeading } from "./render/sanitize";
  import type { WikiHeading, WikiNav, WikiPageData } from "./types";
  import { linkMenu } from "./linkmenu.svelte";
  import { anchorVisible, cancelClose, closePreview, preview, scheduleClose, type Preview } from "./preview.svelte";

  let box: HTMLDivElement | undefined = $state();
  let scroller: HTMLDivElement | undefined = $state();
  let page = $state<WikiPageData | null>(null);
  let failed = $state<string | null>(null);

  /** 浮き窓の大きさ（画面が狭ければ縮む） */
  const W = 480;
  const H = 420;

  /** 中の ShadowPage に渡す物 = 最後に開いた要求の root・refs・readPage・onNavigate だけ（閉じても次に開くまで置き換えない）。
      中の ShadowPage は closePreview の後で props を読む（リンクのクリック・右クリックのメニューの項目・画像の覆い）= preview.cur から直に
      渡すと null を読んで空振り / TypeError になる。要求そのもの（乗せた要素 = 前のページの DOM）は持たない */
  type Held = Pick<Preview, "root" | "refs" | "readPage" | "onNavigate">;
  let kept: Held | null = null;
  const held = $derived.by(() => {
    const c = preview.cur;
    if (c) kept = { root: c.root, refs: c.refs, readPage: c.readPage, onNavigate: c.onNavigate };
    return kept;
  });

  // 開いた（seq が変わった）= 読む。読む間に閉じた / 別の物を開いたら捨てる
  $effect(() => {
    const c = preview.cur;
    untrack(() => {
      page = null;
      failed = null;
      if (!c?.path) return;
      const my = c.seq;
      c.readPage(c.path).then(
        (p) => {
          if (preview.cur?.seq === my) page = p;
        },
        (e) => {
          if (preview.cur?.seq === my) failed = String(e);
        }
      );
    });
  });

  // 置き場（乗せた要素の下 = 入らなければ空きの広い方）
  const place = $derived.by(() => {
    const c = preview.cur;
    if (!c) return null;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const w = Math.min(W, vw - 8);
    const h = Math.min(H, Math.round(vh * 0.6));
    const below = vh - c.rect.bottom - 8;
    const above = c.rect.top - 8;
    const top = below >= h || below >= above ? Math.max(4, Math.min(c.rect.bottom + 6, vh - h - 4)) : Math.max(4, c.rect.top - 6 - h);
    const left = Math.max(4, Math.min(c.x - 24, vw - w - 4));
    return { left, top, w, h };
  });

  // 開いている間だけ外側の押下・キー・スクロール等を見張る（capture = 中の部品が止めても届く）
  $effect(() => {
    if (!preview.cur) return;
    const outside = (e: Event) => !box?.contains(e.target as Node);
    const down = (e: MouseEvent) => {
      if (outside(e)) closePreview();
    };
    const key = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || composing(e)) return;
      e.preventDefault();
      e.stopPropagation();
      closePreview();
    };
    // 乗せた要素を含む箱（shadow の境を越えて親を辿る）のスクロールだけで閉じる
    const scroll = (e: Event) => {
      const tg = e.target;
      const a = preview.cur?.anchor;
      if (!a || tg === document || (tg instanceof Node && containsDeep(tg, a))) closePreview();
    };
    // 乗せた要素が消えた（ページの描き直し・遷移）/ 見えなくなった（集中モードで隠した右ペイン・ヘッダー等）= 閉じる
    const gone = setInterval(() => {
      if (preview.cur && !anchorVisible(preview.cur.anchor)) closePreview();
    }, 300);
    const close = () => closePreview();
    window.addEventListener("mousedown", down, true);
    window.addEventListener("keydown", key, true);
    window.addEventListener("scroll", scroll, true);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    return () => {
      clearInterval(gone);
      window.removeEventListener("mousedown", down, true);
      window.removeEventListener("keydown", key, true);
      window.removeEventListener("scroll", scroll, true);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
    };
  });

  // タブを切り替えた・右クリックのメニューが出た = 閉じる
  let seenTab = untrack(() => app.tab);
  $effect(() => {
    const tab = app.tab;
    if (tab === seenTab) return;
    seenTab = tab;
    untrack(closePreview);
  });
  $effect(() => {
    if (linkMenu.cur) untrack(closePreview);
  });
  // アプリのモーダル（引用の窓 等）が開いた = 閉じる（窓の下に隠れた浮き窓が最初の Esc を capture で奪わない）
  $effect(() => {
    if (modalOpen()) untrack(closePreview);
  });

  /** outer が node を含むか（shadow root の host を辿る = ShadowPage の中のリンクも外の箱から見つける） */
  function containsDeep(outer: Node, node: Node): boolean {
    for (let n: Node | null = node; n; n = n.parentNode ?? (n instanceof ShadowRoot ? n.host : null)) if (n === outer) return true;
    return false;
  }

  function go(nav: WikiNav, newTab?: boolean) {
    const to = (preview.cur ?? held)?.onNavigate;
    closePreview();
    void to?.(nav, newTab);
  }

  function openTitle(e: MouseEvent) {
    const c = preview.cur;
    if (!c?.path) return;
    if (e.button !== 0 && e.button !== 1) return;
    e.preventDefault();
    go(c.heading ? { kind: "page", path: c.path, heading: c.heading } : { kind: "page", path: c.path }, e.button === 1 || e.metaKey || e.ctrlKey);
  }

  /** `#見出し` へ（描画の後の見出し一覧から引く）。浮き窓の中の箱だけを動かす（scrollIntoView は外の箱まで動かしうる） */
  function onHeadings(hs: WikiHeading[]) {
    const c = preview.cur;
    if (!c?.heading) return;
    const h = findHeading(hs, c.heading);
    if (!h) return;
    const my = c.seq;
    void tick().then(() => {
      if (preview.cur?.seq !== my || !scroller) return;
      const sr = scroller.querySelector(".wk-md")?.shadowRoot;
      const el = sr ? anchorTarget(sr, h.id) : null;
      if (!el) return;
      scroller.scrollTop += el.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
    });
  }

  const title = $derived(page?.title || preview.cur?.path?.replace(/^.*\//, "").replace(/\.md$/i, "") || "");
</script>

{#if preview.cur && held && place}
  {@const c = preview.cur}
  {@const h = held}
  <div
    class="wk-pv"
    class:wk-pv-missing={!c.path}
    role="dialog"
    aria-label={t("リンクのプレビュー")}
    tabindex="-1"
    bind:this={box}
    style:left="{place.left}px"
    style:top="{place.top}px"
    style:width="{place.w}px"
    style:height={c.path ? `${place.h}px` : null}
    onmouseenter={cancelClose}
    onmouseleave={scheduleClose}
  >
    {#if !c.path}
      <div class="wk-pv-note">
        <div class="wk-pv-name">{c.missing ?? ""}</div>
        <div>{t("このページはまだありません")}</div>
      </div>
    {:else}
      <button class="wk-pv-head" title={t("開く（⌘クリック = 新しいタブ）")} onclick={openTitle} onauxclick={openTitle}>
        <span class="wk-pv-title">{title}</span>
        <span class="wk-pv-path">{c.path}{c.heading ? ` # ${c.heading}` : ""}</span>
      </button>
      <div class="wk-pv-body" bind:this={scroller}>
        {#if failed}
          <p class="wk-pv-err">{t("プレビューを読めません")}: {failed.slice(0, 200)}</p>
        {:else if !page}
          <p class="wk-pv-dim">{t("読込中…")}</p>
        {:else if page.kind !== "md" || page.content == null}
          <p class="wk-pv-dim">{t("このページはプレビューできません")}</p>
        {:else}
          {#key c.seq}
            <ShadowPage {page} root={h.root} refs={h.refs} onNavigate={go} readPage={h.readPage} {onHeadings} mode="md" inPreview />
          {/key}
        {/if}
      </div>
    {/if}
  </div>
{/if}

<style>
  .wk-pv {
    position: fixed; z-index: 55; display: flex; flex-direction: column; overflow: hidden; border: 1px solid var(--border); border-radius: 10px;
    background: var(--panel); box-shadow: 0 10px 30px rgba(0, 0, 0, 0.22);
  }
  .wk-pv-missing { height: auto; }
  .wk-pv-head {
    display: flex; flex-direction: column; align-items: flex-start; gap: 1px; flex-shrink: 0; padding: 7px 12px 6px; border: 0;
    border-bottom: 1px solid var(--border); background: transparent; text-align: left; cursor: pointer; min-width: 0;
  }
  .wk-pv-head:hover .wk-pv-title { text-decoration: underline; }
  .wk-pv-title { font-weight: 600; font-size: 13px; color: var(--accent); max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wk-pv-path { font-size: 11px; color: var(--dim); max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wk-pv-body { flex: 1; min-height: 0; overflow: auto; padding: 8px 14px 4px; font-size: 13px; }
  .wk-pv-note { padding: 10px 14px; font-size: 12.5px; color: var(--dim); }
  .wk-pv-name { color: var(--text); font-weight: 600; margin-bottom: 2px; overflow-wrap: anywhere; }
  .wk-pv-dim { color: var(--dim); font-size: 12px; margin: 6px 0; }
  .wk-pv-err { color: var(--danger); font-size: 12px; margin: 6px 0; overflow-wrap: anywhere; }
</style>
