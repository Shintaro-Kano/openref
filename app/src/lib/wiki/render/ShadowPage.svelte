<script lang="ts">
  /* OpenNote: md / html ページを shadow DOM に描く共通の部品（MdView / HtmlView の中身）。
     閉じ込めは 2 重: 外箱 .wk-md-box（contain: layout paint）+ host（.wk-md・同じ contain）。体裁は adoptedStyleSheets。
     描画 = md: markdown-it / html: prepareHtml → purify（DOMPurify・inert な文書）→ finish（URL の書換え・リンクの分類）→ shadow へ。
     リンクのクリックは shadow root で横取りして onNavigate に（WebView には遷移させない・⌘クリック / 中クリック = 新しいタブ）。
     リンクの右クリックは WebView の既定のメニュー（押しても何も起きない）の代わりに OpenNote のメニュー（linkmenu.svelte.ts）。`![[ページ]]` は readPage で 1 段だけ埋める
     （読んだページは世代 wk.embedGen（索引の変更と GUI の保存の成功で進む）の間だけ覚える = 分割のプレビューが入力の一時停止ごとに描き直しても IPC は世代に 1 回）。
     描画の時間の計測（performance.measure "wk-render <path>"・初回の描画（次のフレーム）まで = "wk-paint <path>"）は旗 `window.__wkMeasure = true`
     のときだけ残す（既定は切る = User Timing の項目が描画のたびに溜まり続けない。計測は Inspector で旗を立ててから） */
  import { onDestroy, untrack } from "svelte";
  import type { WikiPageData, WikiViewProps } from "../types";
  import { t } from "../../i18n.svelte";
  import { composing } from "../../input";
  import { wk } from "../wiki.svelte";
  import { renderMarkdown } from "./md";
  import { anchorTarget, finish, keepSection, navOf, purify, type FinishResult, type Labels } from "./sanitize";
  import { linkItems, openLinkMenu, toWikiNav, withSelection } from "../linkmenu.svelte";
  import { prepareHtml } from "./html";
  import { sheets } from "./styles";
  import { Finder } from "./find";

  let {
    page,
    root,
    refs,
    onNavigate,
    readPage,
    onHeadings,
    scrollTo = null,
    findRequest = 0,
    headingColors = false,
    mode,
  }: WikiViewProps & { mode: "md" | "html" } = $props();

  let host: HTMLDivElement | undefined = $state();
  let shadow: ShadowRoot | null = null;
  let finder: Finder | null = null;
  let rendered = $state(false);
  let failed = $state<string | null>(null);
  let gen = 0;

  let findOpen = $state(false);
  let findQ = $state("");
  let findN = $state(0);
  let findCur = $state(0);
  let findInput: HTMLInputElement | undefined = $state();

  function labels(): Labels {
    return {
      openExternal: t("外部で開く"),
      playFailed: t("ここでは再生できません。外部のアプリで開いてください"),
      moreRows: (n: number) => `${t("続きを表示")} (${n})`,
      externalImage: t("外部の画像"),
      unresolved: t("未解決のリンク"),
      pdf: t("PDF を開く"),
    };
  }

  function scrollToAnchor(id: string) {
    if (!shadow) return;
    const el = anchorTarget(shadow, id);
    el?.scrollIntoView({ block: "start" });
  }

  function onClick(e: Event) {
    const me = e as MouseEvent;
    const el = (me.target as Element | null)?.closest?.("a, button[data-wk-open]");
    if (!el) return;
    me.preventDefault();
    // auxclick = 左以外のボタン。中ボタン = 新しいタブ（一覧・サイドバーと同じ）・右ボタンは contextmenu が扱う
    const middle = me.type === "auxclick";
    if (middle && me.button !== 1) return;
    const n = navOf(el);
    if (n.t === "anchor") return scrollToAnchor(n.id);
    const w = toWikiNav(n);
    if (w) onNavigate(w, middle || me.metaKey || me.ctrlKey);
  }

  /** リンクの右クリック = OpenNote のメニュー（リンクの外 = WebView の既定のメニュー = 選択のコピー・調べる 等のまま）。
      文字を選んでいれば頭に「コピー」（既定のメニューを止めた分を戻す） */
  function onContext(e: Event) {
    const me = e as MouseEvent;
    const a = (me.target as Element | null)?.closest?.("a");
    if (!a) return;
    const n = navOf(a);
    const written = a.getAttribute("data-wk-href") ?? "";
    const items = linkItems(n, written, { go: onNavigate, anchor: scrollToAnchor, root });
    if (!items.length) return;
    me.preventDefault();
    const head = n.t === "unresolved" ? `${t("未解決のリンク")}: ${written}` : written;
    const selected = window.getSelection()?.toString() ?? "";
    openLinkMenu({ x: me.clientX, y: me.clientY, head, items: withSelection(items, selected), origin: host });
  }

  function ensureShadow(): ShadowRoot {
    if (shadow) return shadow;
    shadow = host!.attachShadow({ mode: "open" });
    shadow.adoptedStyleSheets = sheets(document, mode);
    shadow.addEventListener("click", onClick);
    shadow.addEventListener("auxclick", onClick);
    shadow.addEventListener("contextmenu", onContext);
    finder = new Finder(shadow);
    return shadow;
  }

  async function fetchText(url: string): Promise<string> {
    const r = await fetch(url);
    if (!r.ok) throw new Error(`${r.status}`);
    return r.text();
  }

  /** 計測の旗（Inspector で `window.__wkMeasure = true`）。立っているときだけ performance.measure を残す */
  function measuring(): boolean {
    return (globalThis as { __wkMeasure?: unknown }).__wkMeasure === true;
  }

  /** `![[ページ]]` の読み（readPage）を世代（wk.embedGen = 索引の変更 + GUI の保存の成功）の間だけ覚える。readPage ごと（WeakMap）に path → 約束。
      失敗した読みは覚えない（次の描画でもう一度読む）。世代が進んだら全部捨てる */
  let embedStamp = -1;
  let embedCache = new WeakMap<WikiViewProps["readPage"], Map<string, Promise<WikiPageData>>>();
  function readEmbed(read: WikiViewProps["readPage"], path: string): Promise<WikiPageData> {
    const stamp = untrack(() => wk.embedGen);
    if (stamp !== embedStamp) {
      embedStamp = stamp;
      embedCache = new WeakMap();
    }
    let m = embedCache.get(read);
    if (!m) {
      m = new Map();
      embedCache.set(read, m);
    }
    const hit = m.get(path);
    if (hit) return hit;
    const p = read(path);
    m.set(path, p);
    p.catch(() => {
      if (m!.get(path) === p) m!.delete(path);
    });
    return p;
  }

  async function render(p: WikiPageData, r: string, g: number) {
    const sr = ensureShadow();
    failed = null;
    const t0 = performance.now();
    try {
      const links = p.links ?? [];
      const html =
        mode === "md"
          ? renderMarkdown(p.content ?? "")
          : await prepareHtml(window, p.content ?? "", { root: r, path: p.path, links }, fetchText);
      if (g !== gen) return;
      const frag = purify(window, html);
      const res = finish(frag, { root: r, path: p.path, refs, kind: mode, links, labels: labels() });
      const box = document.createElement("div");
      box.className = "wk-root";
      box.append(frag);
      sr.replaceChildren(box);
      void host!.offsetHeight; // 配置まで（初回描画の計測に入れる）
      if (measuring()) {
        const t1 = performance.now();
        const name = `${p.path}`;
        performance.measure(`wk-render ${name}`, { start: t0, end: t1 });
        requestAnimationFrame(() => {
          setTimeout(() => performance.measure(`wk-paint ${name}`, { start: t0, end: performance.now() }), 0);
        });
      }
      rendered = true;
      finder?.reset();
      if (findOpen && findQ) runFind();
      onHeadings?.(res.headings);
      if (pendingScroll) {
        scrollToAnchor(pendingScroll);
        pendingScroll = null;
      }
      await expandEmbeds(res, r, g);
    } catch (err) {
      if (g === gen) failed = String(err);
    }
  }

  async function expandEmbeds(res: FinishResult, r: string, g: number) {
    if (!res.pageEmbeds.length) return;
    let i = 0;
    for (const e of res.pageEmbeds) {
      i++;
      try {
        const p = await readEmbed(readPage, e.path);
        if (g !== gen) return;
        if (p.kind !== "md" || p.content == null) {
          e.el.remove();
          continue;
        }
        const frag = purify(window, renderMarkdown(p.content, `e${i}`));
        finish(frag, { root: r, path: p.path, refs, kind: "md", links: p.links ?? [], nested: true, labels: labels() });
        if (e.heading) keepSection(frag, e.heading);
        e.el.replaceChildren(frag);
      } catch (err) {
        e.el.className = "wk-embed-error";
        e.el.textContent = `${t("埋め込みを読めません")}: ${String(err)}`;
      }
    }
    finder?.reset();
  }

  // 描画 = page・root・refs（参照フォルダ = リンクの分類）が変わったとき。
  // props を effect で直接読むと、親が spread で渡している物（WikiPage の viewBase）の全部に依存する（spread_props の has が全部の元を評価する）
  // = ⌘F・目次クリック・見出しの色・refDirs の再代入のたびに全描画 → 値を derived に写し、effect はそれだけを読む（値が同じなら走らない）。
  // refs は中身の文字列で比べる（同内容の新配列 = wiki-state ごとの loadRefDirs で描き直さない）。render の中（untrack）で読むだけだと依存にならない
  const pageD = $derived(page);
  const rootKey = $derived(root);
  const refsKey = $derived((refs ?? []).join("\n"));
  $effect(() => {
    const p = pageD;
    const r = rootKey;
    void refsKey;
    if (!host) return;
    const g = ++gen;
    untrack(() => void render(p, r, g));
  });

  // 目次クリック等（nonce が変わるたび）。描画の前に来たら描画の後に
  let pendingScroll: string | null = null;
  let seenScroll: number | null = null;
  $effect(() => {
    const s = scrollTo;
    if (!s || s.nonce === seenScroll) return;
    seenScroll = s.nonce;
    if (rendered) untrack(() => scrollToAnchor(s.id));
    else pendingScroll = s.id;
  });

  // ⌘F（nonce が変わるたび）。初めの値では開かない
  let seenFind = untrack(() => findRequest ?? 0);
  $effect(() => {
    const n = findRequest ?? 0;
    if (n === seenFind) return;
    seenFind = n;
    untrack(openFind);
  });

  function openFind() {
    findOpen = true;
    queueMicrotask(() => {
      findInput?.focus();
      findInput?.select();
    });
  }

  function runFind() {
    if (!finder) return;
    findN = finder.search(findQ);
    findCur = finder.cur + 1;
  }

  function stepFind(d: 1 | -1) {
    finder?.step(d);
    findCur = (finder?.cur ?? -1) + 1;
  }

  function closeFind() {
    findOpen = false;
    finder?.clear();
    findN = 0;
    findCur = 0;
  }

  function findKey(e: KeyboardEvent) {
    if (composing(e)) return; // かなの確定 Enter で次の一致へ飛ばない・変換の取消 Esc で欄を閉じない
    if (e.key === "Enter") {
      e.preventDefault();
      stepFind(e.shiftKey ? -1 : 1);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      closeFind();
    }
  }

  onDestroy(() => {
    gen++;
    finder?.clear();
  });
</script>

{#if findOpen}
  <div class="wk-find">
    <input
      class="wk-find-input"
      bind:this={findInput}
      bind:value={findQ}
      oninput={runFind}
      onkeydown={findKey}
      placeholder={t("ページ内を検索")}
      spellcheck="false"
    />
    <span class="wk-find-count">{findQ ? (findN ? `${findCur} / ${findN}` : t("該当なし")) : ""}</span>
    <button class="wk-find-btn" onclick={() => stepFind(-1)} disabled={!findN} title={t("前へ")}>&#x2191;</button>
    <button class="wk-find-btn" onclick={() => stepFind(1)} disabled={!findN} title={t("次へ")}>&#x2193;</button>
    <button class="wk-find-btn" onclick={closeFind} title={t("閉じる")}>&#x2715;</button>
  </div>
{/if}
{#if page.truncated}
  <div class="wk-note">{t("ページが大きいため途中までを表示しています")}</div>
{/if}
{#if failed}
  <div class="wk-note wk-note-error">{t("このページを描けません")}: {failed}</div>
{/if}
<div class="wk-md-box">
  <div class="wk-md" class:wk-hc={mode === "md" && headingColors} bind:this={host}></div>
</div>

<style>
  .wk-md-box { contain: layout paint; position: relative; }
  .wk-md { contain: layout paint; position: relative; }
  .wk-find {
    position: sticky; top: 0; z-index: 5; display: flex; align-items: center; gap: 6px; margin: 0 0 8px;
    padding: 6px 8px; border: 1px solid var(--border); border-radius: 8px; background: var(--panel);
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.08);
  }
  .wk-find-input {
    flex: 1; min-width: 0; font: inherit; padding: 3px 8px; border: 1px solid var(--border); border-radius: 5px;
    background: var(--bg); color: var(--text);
  }
  .wk-find-input:focus { outline: none; border-color: var(--accent); }
  .wk-find-count { font-size: 12px; color: var(--dim); min-width: 4em; text-align: right; }
  .wk-find-btn {
    font: inherit; width: 26px; height: 24px; padding: 0; border: 1px solid var(--border); border-radius: 5px;
    background: var(--bg); color: var(--text); cursor: pointer;
  }
  .wk-find-btn:disabled { opacity: 0.4; cursor: default; }
  .wk-note { font-size: 12px; color: var(--dim); padding: 4px 8px; margin-bottom: 6px; border-radius: 5px; background: var(--accent-soft); }
  .wk-note-error { background: var(--banner); color: var(--text); }
</style>
