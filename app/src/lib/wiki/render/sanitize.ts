/* OpenNote: 掃除（DOMPurify）と掃除後の DOM 走査。
   順序 = DOMPurify（許可リスト・ALLOWED_URI_REGEXP）→ finish（相対パスの asset URL への書換え・リンクの分類・`<style>` / style 属性の
   自前の掃除・`![[…]]` の展開・見出しの id・大きな表）。hook ではなく掃除後の DOM を走査する。
   DOMPurify は RETURN_DOM_FRAGMENT で自分の inert な文書（DOMParser）の中に結果を作る = finish が http(s) の src を外すまで
   画面の文書には入らない（入れた瞬間の画像の読込 = 外部ビーコンを起こさない）。
   window は呼び手が渡す（画面 = window・node test = jsdom）。 */
import createDOMPurify from "dompurify";
import type { WikiLink } from "../types.ts";
import { absOf, assetInRoot, assetPath, assetUrl, filePath, normalizeAbs, refRootOf, refViewable, relInRoot, resolveRelative, safeDecode, schemeOf, splitHash, viewKindOf } from "./asset.ts";
import { cleanStyleAttr, cleanStyleSheet } from "./css.ts";
import { buildMedia, type MediaLabels } from "./media.ts";
import { scanWikiText, wikiLabel } from "./md.ts";

/** 許可する要素（計画書 §5.1 の一覧 + markdown-it が出す em strong del section + KB が使う small） */
export const ALLOWED_TAGS = [
  "img", "details", "summary", "table", "thead", "tbody", "tfoot", "tr", "td", "th", "caption", "video", "audio", "source",
  "a", "span", "div", "p", "b", "i", "u", "s", "sub", "sup", "br", "hr", "code", "pre", "kbd", "mark", "ul", "ol", "li",
  "h1", "h2", "h3", "h4", "h5", "h6", "blockquote", "figure", "figcaption", "style",
  "em", "strong", "del", "section", "small",
];
/** 許可する属性（計画書 §5.1 + ol の start・自作の data-wk-wiki / data-wk-embed / data-wk-size / data-wk-toc） */
export const ALLOWED_ATTR = [
  "href", "src", "alt", "title", "width", "height", "colspan", "rowspan", "open", "controls", "style", "class", "id", "start",
  "data-wk-wiki", "data-wk-embed", "data-wk-size", "data-wk-toc",
];
/** URL の規則（DOMPurify の既定 = http(s) / mailto / tel と相対 を差し替え）。data: は画像だけ。後ろ 2 つは DOMPurify の既定と同じ相対の形 */
export const ALLOWED_URI_REGEXP =
  /^(?:(?:https?|mailto|openref|asset|file):|data:image\/(?:png|jpe?g|gif|webp|avif|svg\+xml|bmp)[;,]|[^a-z]|[a-z+.-]+(?:[^a-z+.\-:]|$))/i;

const FORBID_TAGS = ["script", "iframe", "object", "embed", "form", "input", "link", "meta", "base", "svg", "math", "template"];

type Purifier = ReturnType<typeof createDOMPurify>;
const purifiers = new WeakMap<object, Purifier>();

function purifierFor(win: Window): Purifier {
  let p = purifiers.get(win);
  if (!p) {
    p = createDOMPurify(win as unknown as Window & typeof globalThis);
    purifiers.set(win, p);
  }
  return p;
}

/** HTML 文字列 → 掃除済みの DocumentFragment（inert な文書の物） */
export function purify(win: Window, html: string): DocumentFragment {
  return purifierFor(win).sanitize(html, {
    ALLOWED_TAGS,
    ALLOWED_ATTR,
    ALLOW_DATA_ATTR: false,
    ALLOW_ARIA_ATTR: false,
    ALLOWED_URI_REGEXP,
    ADD_URI_SAFE_ATTR: ["data-wk-wiki", "data-wk-embed", "data-wk-size", "data-wk-toc", "width", "height", "colspan", "rowspan", "start"],
    FORBID_TAGS,
    FORCE_BODY: true,
    RETURN_DOM_FRAGMENT: true,
  }) as unknown as DocumentFragment;
}

export interface Labels extends MediaLabels {
  /** 「続きを表示（残り n 行）」 */
  moreRows: (n: number) => string;
  /** 外部の画像（読まずにリンク表示）の頭 */
  externalImage: string;
  /** 未解決のリンクの title */
  unresolved: string;
  /** PDF の埋め込み（リンク + アイコン）の title */
  pdf: string;
}

export interface FinishCtx {
  /** vault の root（絶対パス）。参照フォルダのページではそのフォルダ */
  root: string;
  /** このページの root 相対パス */
  path: string;
  /** 参照フォルダ。その下の file:// で内蔵で開ける種類は ref（無ければ open） */
  refs?: readonly string[];
  /** md = 相対は md 種で page.links を引く・html = html 種 */
  kind: "md" | "html";
  links: WikiLink[];
  /** `![[ページ]]` の展開の中（もう展開しない・見出しの id を付けない） */
  nested?: boolean;
  labels: Labels;
  /** 表を分ける行数（既定 2000）と content-visibility を付ける行数（既定 200） */
  tableSplit?: number;
  tableBig?: number;
}

/** `![[ページ]]` を展開する上限（1 ページあたり・超えた分はリンクのまま = readPage の連打を避ける） */
export const MAX_PAGE_EMBEDS = 20;

export interface Heading {
  level: number;
  text: string;
  id: string;
}

export interface FinishResult {
  headings: Heading[];
  /** `![[ページ]]` の置き場（MdView が readPage で 1 段だけ埋める） */
  pageEmbeds: { el: HTMLElement; path: string; heading: string | null }[];
}

/** リンクの分類の結果（data-wk-* に載せる） */
export type Nav =
  | { t: "page"; path: string; heading: string | null }
  | { t: "anchor"; id: string }
  | { t: "item"; key: string; page: number | null; att: string | null }
  | { t: "ext"; url: string }
  | { t: "open"; abs: string }
  | { t: "ref"; abs: string }
  | { t: "unresolved" }
  | { t: "none" };

const NAV_ATTRS = ["data-wk-path", "data-wk-heading", "data-wk-anchor", "data-wk-item", "data-wk-page", "data-wk-att", "data-wk-ext", "data-wk-open", "data-wk-ref"];

export function parseOpenref(url: string): { key: string; page: number | null; att: string | null } | null {
  const q = url.indexOf("?");
  if (q < 0) return null;
  const params = new URLSearchParams(url.slice(q + 1).split("#")[0]);
  const key = params.get("key")?.trim();
  if (!key) return null;
  const page = params.get("page");
  const n = page && /^\d+$/.test(page) ? Number(page) : null;
  return { key, page: n, att: params.get("att") || null };
}

/** page.links の引き当て表（kind + target → 最初の 1 本） */
function linkTable(links: WikiLink[]): Map<string, WikiLink> {
  const m = new Map<string, WikiLink>();
  for (const l of links) {
    const k = `${l.kind}\u0000${l.target}`;
    if (!m.has(k)) m.set(k, l);
  }
  return m;
}

/** dest（href / src に書かれたまま）→ 分類。table = page.links の引き当て表・rel = 相対の種類（md / html） */
export function classifyDest(dest: string, ctx: FinishCtx, table: Map<string, WikiLink>): Nav {
  const d = dest.trim();
  if (!d) return { t: "none" };
  if (d.startsWith("#")) return { t: "anchor", id: safeDecode(d.slice(1)) };
  const s = schemeOf(d);
  if (s === "openref") {
    const o = parseOpenref(d);
    return o ? { t: "item", ...o } : { t: "none" };
  }
  if (s === "http" || s === "https" || s === "mailto") return { t: "ext", url: d };
  if (s === "file" || s === "asset") {
    // `..` を畳んだ素直な絶対パスにする（安全の判定は骨組みの wiki_open_path）
    const raw = s === "file" ? filePath(d) : assetPath(d);
    const abs = raw ? normalizeAbs(raw) : null;
    if (!abs) return { t: "none" };
    const rel = relInRoot(ctx.root, abs);
    if (rel) return { t: "page", path: rel, heading: splitHash(d).hash };
    // 参照フォルダの中の pdf / html / 画像 / 動画 / 音声は内蔵の表示で（実在・symlink の検査は wiki_read_ref）
    if (refRootOf(ctx.refs, abs) && refViewable(abs)) return { t: "ref", abs };
    return { t: "open", abs };
  }
  if (s) return { t: "none" }; // 未知の scheme（javascript: 等）は何もしない
  const hit = table.get(`${ctx.kind}\u0000${d}`);
  const { hash } = splitHash(d);
  if (hit) return hit.path ? { t: "page", path: hit.path, heading: hash } : { t: "unresolved" };
  const rel = resolveRelative(ctx.path, d);
  return rel ? { t: "page", path: rel, heading: hash } : { t: "unresolved" };
}

/** `[[target]]`（wiki / embed）→ 分類。page.links の path（= 索引の解決）に従う */
export function classifyWiki(target: string, kind: "wiki" | "embed", ctx: FinishCtx, table: Map<string, WikiLink>): Nav {
  const h = target.indexOf("#");
  const heading = h < 0 ? null : target.slice(h + 1);
  if (h === 0) return { t: "anchor", id: heading ?? "" };
  const hit = table.get(`${kind}\u0000${target}`) ?? table.get(`${kind === "wiki" ? "embed" : "wiki"}\u0000${target}`);
  if (hit?.path) return { t: "page", path: hit.path, heading };
  return { t: "unresolved" };
}

function setNav(el: Element, nav: Nav) {
  for (const a of NAV_ATTRS) el.removeAttribute(a);
  switch (nav.t) {
    case "page":
      el.setAttribute("data-wk-path", nav.path);
      if (nav.heading) el.setAttribute("data-wk-heading", nav.heading);
      break;
    case "anchor":
      el.setAttribute("data-wk-anchor", nav.id);
      break;
    case "item":
      el.setAttribute("data-wk-item", nav.key);
      if (nav.page != null) el.setAttribute("data-wk-page", String(nav.page));
      if (nav.att) el.setAttribute("data-wk-att", nav.att);
      break;
    case "ext":
      el.setAttribute("data-wk-ext", nav.url);
      break;
    case "open":
      el.setAttribute("data-wk-open", nav.abs);
      break;
    case "ref":
      el.setAttribute("data-wk-ref", nav.abs);
      break;
  }
  el.classList.add(`wk-l-${nav.t}`);
}

/** 書き換えた後の `<a>` から遷移先を読む（MdView / HtmlView のクリックの横取り） */
export function navOf(el: Element): Nav {
  const p = el.getAttribute("data-wk-path");
  if (p) return { t: "page", path: p, heading: el.getAttribute("data-wk-heading") };
  const a = el.getAttribute("data-wk-anchor");
  if (a != null) return { t: "anchor", id: a };
  const k = el.getAttribute("data-wk-item");
  if (k) {
    const pg = el.getAttribute("data-wk-page");
    return { t: "item", key: k, page: pg ? Number(pg) : null, att: el.getAttribute("data-wk-att") };
  }
  const x = el.getAttribute("data-wk-ext");
  if (x) return { t: "ext", url: x };
  const o = el.getAttribute("data-wk-open");
  if (o) return { t: "open", abs: o };
  const r = el.getAttribute("data-wk-ref");
  if (r) return { t: "ref", abs: r };
  return el.classList.contains("wk-l-unresolved") ? { t: "unresolved" } : { t: "none" };
}

/** src（img / video / audio / source）→ asset URL か data:image か null（= 読まない） */
function srcUrl(raw: string, ctx: FinishCtx, table: Map<string, WikiLink>, tag: string): { url: string | null; ext: string | null } {
  const d = raw.trim();
  const s = schemeOf(d);
  if (s === "data") return { url: tag === "img" && /^data:image\//i.test(d) ? d : null, ext: null };
  if (s === "asset") {
    // vault の外を指す asset: は読まない（scope の外は 403 だが、描画側でも vault の中だけに絞る。CSS の url() も同じ assetInRoot）
    return { url: assetInRoot(ctx.root, d), ext: null };
  }
  if (s === "http" || s === "https") return { url: null, ext: d };
  if (s === "file") {
    const abs = filePath(d);
    const rel = abs ? relInRoot(ctx.root, abs) : null;
    return { url: rel ? assetUrl(absOf(ctx.root, rel)) : null, ext: null };
  }
  if (s || d.startsWith("//")) return { url: null, ext: null };
  const hit = table.get(`${ctx.kind}\u0000${d}`);
  const rel = hit ? hit.path : resolveRelative(ctx.path, d);
  return { url: rel ? assetUrl(absOf(ctx.root, rel)) : null, ext: null };
}

/** `<style>` / style 属性の相対 url() → asset URL（このページからの相対・`/` 始まりは root から） */
function cssRewriter(ctx: FinishCtx) {
  return (v: string) => {
    const rel = resolveRelative(ctx.path, v);
    return rel ? assetUrl(absOf(ctx.root, rel)) : null;
  };
}

/** HTML ブロックの文字列の `[[…]]` を拾わない要素（textarea は ALLOWED_TAGS に無い = DOMPurify が中身を text に残すので拾う = md.ts の HTML_NO_WIKI と同じ） */
const SKIP_TEXT = new Set(["A", "CODE", "PRE", "STYLE", "SCRIPT", "KBD"]);

/** HTML ブロックの中に残った `[[…]]` / `![[…]]` の文字列（markdown-it は HTML ブロックの中を解かない）を md と同じ要素にする */
function wikilinkTextNodes(root: DocumentFragment | Element, doc: Document) {
  const walker = doc.createTreeWalker(root, 4 /* NodeFilter.SHOW_TEXT */);
  const hits: Text[] = [];
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    if (!(n as Text).data.includes("[[")) continue;
    let p = n.parentNode as Element | null;
    let skip = false;
    while (p && p.nodeType === 1) {
      if (SKIP_TEXT.has(p.nodeName.toUpperCase())) {
        skip = true;
        break;
      }
      p = p.parentNode as Element | null;
    }
    if (!skip) hits.push(n as Text);
  }
  for (const t of hits) {
    const s = t.data;
    const found = scanWikiText(s);
    if (!found.length) continue;
    const frag = doc.createDocumentFragment();
    let last = 0;
    for (const w of found) {
      frag.append(s.slice(last, w.from));
      if (w.embed) {
        const sp = doc.createElement("span");
        sp.className = "wk-embed";
        sp.setAttribute("data-wk-embed", w.target);
        const size = w.alias && /^\d+(x\d+)?$/.test(w.alias) ? w.alias : "";
        sp.setAttribute("data-wk-size", size);
        sp.textContent = wikiLabel(w.target, size ? null : w.alias);
        frag.append(sp);
      } else {
        const a = doc.createElement("a");
        a.className = "wk-wikilink";
        a.setAttribute("data-wk-wiki", w.target);
        a.textContent = wikiLabel(w.target, w.alias);
        frag.append(a);
      }
      last = w.to;
    }
    frag.append(s.slice(last));
    t.replaceWith(frag);
  }
}

function replaceWithExternal(el: Element, url: string, text: string, ctx: FinishCtx, doc: Document) {
  const a = doc.createElement("a");
  a.className = "wk-ext-media";
  a.href = "#";
  setNav(a, { t: "ext", url });
  a.textContent = `${ctx.labels.externalImage} ${text || url}`;
  const raw = el.getAttribute("data-wk-src");
  if (raw != null) a.setAttribute("data-wk-src", raw);
  el.replaceWith(a);
}

/** 見出しの id（同じ文書の中で一意・shadow root の中なので外と衝突しない） */
export function headingId(text: string, used: Set<string>): string {
  const base = "h-" + (text.trim().replace(/\s+/g, "-").slice(0, 80) || "x");
  let id = base;
  for (let n = 1; used.has(id); n++) id = `${base}-${n}`;
  used.add(id);
  return id;
}

/** 掃除後の DOM の走査（frag は purify の結果。その場で書き換える） */
export function finish(frag: DocumentFragment, ctx: FinishCtx): FinishResult {
  const doc = frag.ownerDocument;
  const table = linkTable(ctx.links);
  const rewrite = cssRewriter(ctx);
  const asset = (v: string) => assetInRoot(ctx.root, v);
  wikilinkTextNodes(frag, doc);

  // style 属性・<style> の中身
  for (const el of Array.from(frag.querySelectorAll("[style]"))) {
    const v = cleanStyleAttr(el.getAttribute("style") ?? "", rewrite, asset);
    if (v.trim()) el.setAttribute("style", v);
    else el.removeAttribute("style");
  }
  for (const st of Array.from(frag.querySelectorAll("style"))) st.textContent = cleanStyleSheet(st.textContent ?? "", rewrite, asset);

  // リンク
  for (const a of Array.from(frag.querySelectorAll("a"))) {
    const wiki = a.getAttribute("data-wk-wiki");
    let nav: Nav;
    if (wiki != null) nav = classifyWiki(wiki, "wiki", ctx, table);
    else if (a.hasAttribute("href")) nav = classifyDest(a.getAttribute("href") ?? "", ctx, table);
    else continue;
    // href は横取りの印だけにする（WebView に任せると「リンクを開く」で画面ごと遷移しうる）
    a.setAttribute("data-wk-href", wiki != null ? `[[${wiki}]]` : (a.getAttribute("href") ?? ""));
    a.setAttribute("href", "#");
    setNav(a, nav);
    if (nav.t === "unresolved") a.setAttribute("title", ctx.labels.unresolved);
  }

  // 画像・動画・音声の src
  for (const el of Array.from(frag.querySelectorAll("img, video, audio, source"))) {
    const tag = el.nodeName.toLowerCase();
    const raw = el.getAttribute("src");
    if (raw == null) continue;
    el.setAttribute("data-wk-src", raw);
    const { url, ext } = srcUrl(raw, ctx, table, tag);
    if (url) {
      el.setAttribute("src", url);
      if (tag === "img") el.setAttribute("loading", "lazy");
      if (tag === "video" || tag === "audio") el.setAttribute("preload", "metadata");
      continue;
    }
    el.removeAttribute("src");
    if (ext && tag !== "source") replaceWithExternal(el, ext, el.getAttribute("alt") ?? "", ctx, doc);
    else if (ext) {
      const media = el.parentElement;
      if (media && /^(video|audio)$/i.test(media.nodeName) && !media.querySelector("source[src]") && !media.getAttribute("src")) {
        replaceWithExternal(media, ext, "", ctx, doc);
      }
    }
  }

  // `![[…]]`
  const pageEmbeds: FinishResult["pageEmbeds"] = [];
  for (const sp of Array.from(frag.querySelectorAll("span[data-wk-embed]"))) {
    const target = sp.getAttribute("data-wk-embed") ?? "";
    const size = sp.getAttribute("data-wk-size") ?? "";
    const nav = classifyWiki(target, "embed", ctx, table);
    if (nav.t !== "page") {
      if (nav.t === "anchor") continue;
      sp.className = "wk-embed wk-unresolved";
      sp.setAttribute("title", ctx.labels.unresolved);
      continue;
    }
    const kind = viewKindOf(nav.path);
    const abs = absOf(ctx.root, nav.path);
    const name = nav.path.slice(nav.path.lastIndexOf("/") + 1);
    if (kind === "image" || kind === "video" || kind === "audio") {
      const m = buildMedia(doc, { kind, src: assetUrl(abs), abs, name, labels: ctx.labels, inline: true });
      if (size) {
        const [w, h] = size.split("x");
        const target = kind === "image" ? m.querySelector("img") ?? m : m;
        (target as HTMLElement).style.width = `${Number(w)}px`;
        if (h && kind === "image") (target as HTMLElement).style.height = `${Number(h)}px`;
      }
      m.setAttribute("data-wk-target", target);
      sp.replaceWith(m);
    } else if (kind === "page" && !ctx.nested && pageEmbeds.length < MAX_PAGE_EMBEDS) {
      const box = doc.createElement("div");
      box.className = "wk-embed-page";
      const head = doc.createElement("a");
      head.className = "wk-embed-page-title";
      head.href = "#";
      setNav(head, nav);
      head.textContent = wikiLabel(target, null);
      const body = doc.createElement("div");
      body.className = "wk-embed-page-body";
      box.append(head, body);
      head.setAttribute("data-wk-target", target);
      sp.replaceWith(box);
      pageEmbeds.push({ el: body as HTMLElement, path: nav.path, heading: nav.heading });
    } else {
      // PDF・入れ子のページ・その他 = リンク（PDF はアイコン付き）
      const a = doc.createElement("a");
      a.href = "#";
      a.className = kind === "pdf" ? "wk-embed-link wk-embed-pdf" : "wk-embed-link";
      if (kind === "pdf") a.title = ctx.labels.pdf;
      setNav(a, nav);
      a.textContent = wikiLabel(target, null);
      a.setAttribute("data-wk-target", target);
      sp.replaceWith(a);
    }
  }

  // 見出しの id（埋め込みの中は付けない）
  const headings: Heading[] = [];
  if (!ctx.nested) {
    const used = new Set<string>();
    for (const h of Array.from(frag.querySelectorAll("h1, h2, h3, h4, h5, h6"))) {
      const text = (h.textContent ?? "").trim();
      const id = headingId(text, used);
      h.setAttribute("id", id);
      headings.push({ level: Number(h.nodeName.slice(1)), text, id });
    }
  }

  // `[toc]` の箱 = 見出しの一覧（同じページの見出しへの anchor）。埋め込みの中（id を付けない）と見出しの無いページでは消す
  for (const box of Array.from(frag.querySelectorAll("div[data-wk-toc]"))) {
    if (ctx.nested || !headings.length) {
      box.remove();
      continue;
    }
    const min = Math.min(...headings.map((h) => h.level));
    const ul = doc.createElement("ul");
    for (const h of headings) {
      const li = doc.createElement("li");
      li.className = `wk-toc-l${h.level}`;
      if (h.level > min) li.setAttribute("style", `padding-left: ${(h.level - min) * 1.1}em`);
      const a = doc.createElement("a");
      a.href = `#${h.id}`;
      a.textContent = h.text;
      setNav(a, { t: "anchor", id: h.id });
      li.append(a);
      ul.append(li);
    }
    box.replaceChildren(ul);
  }

  // 大きな表: big 行超 = content-visibility・split 行超 = 続きを表示
  const big = ctx.tableBig ?? 200;
  const split = ctx.tableSplit ?? 2000;
  for (const tbl of Array.from(frag.querySelectorAll("table"))) {
    const rows = Array.from(tbl.querySelectorAll(":scope > tbody > tr"));
    if (rows.length > big) tbl.classList.add("wk-tbl-big");
    if (rows.length > split) {
      const rest = rows.slice(split);
      for (const r of rest) r.remove();
      const btn = doc.createElement("button");
      btn.className = "wk-tbl-more";
      btn.type = "button";
      btn.textContent = ctx.labels.moreRows(rest.length);
      const tbody = tbl.querySelector(":scope > tbody");
      btn.addEventListener("click", () => {
        const chunk = rest.splice(0, split);
        tbody?.append(...chunk);
        if (rest.length) btn.textContent = ctx.labels.moreRows(rest.length);
        else btn.remove();
      });
      tbl.after(btn);
    }
  }
  return { headings, pageEmbeds };
}

/** 見出しの照合の緩い鍵（`[[p#見出し]]` の着地・`![[p#見出し]]` の節・同じページの `#…` の全部が findHeading でこれを使う・両側に同じ変換）:
    NFC・Obsidian と同じく見出しのリンクに使えない記号（: # | ^ [ ] \ / % * " < > ?）と `-`（md の `#a-b` の書き方）を空白に・
    空白の連なりを 1 つに・前後の空白を除き・大小を無視。全角は畳まない（NFC のまま） */
export function headingKey(s: string): string {
  return s
    .normalize("NFC")
    .replace(/[-:#|^\[\]\\/%*"<>?]/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .toLowerCase();
}

/** 厳密な鍵（記号は残す）= 緩い鍵の前に引く（記号の違いで書き分けた見出しは書いたとおりの方へ・記号だけの見出しも引ける） */
const strictKey = (s: string) => s.normalize("NFC").trim().replace(/\s+/g, " ").toLowerCase();

/** id か見出しの文字列 → 見出し。① id の完全一致 ② 厳密な鍵が同じ最初の見出し ③ headingKey が同じ最初の見出し（空の鍵 = 記号だけは引かない） */
export function findHeading<H extends { id: string; text: string }>(headings: H[], idOrText: string): H | undefined {
  const strict = strictKey(idOrText);
  const loose = headingKey(idOrText);
  return (
    headings.find((h) => h.id !== "" && h.id === idOrText) ??
    headings.find((h) => strictKey(h.text) === strict) ??
    (loose ? headings.find((h) => headingKey(h.text) === loose) : undefined)
  );
}

const isHeading = (el: Element) => /^H[1-6]$/i.test(el.nodeName);
const headingsOf = (els: Element[]) => els.filter(isHeading).map((el) => ({ id: el.getAttribute("id") ?? "", text: el.textContent ?? "", el }));

/** `![[page#見出し]]` の節だけを残す（frag の直下で、その見出しから同じか上の階層の次の見出しの手前まで）。見出しが無ければ全体のまま */
export function keepSection(frag: DocumentFragment, heading: string) {
  const kids = Array.from(frag.children);
  const hit = findHeading(headingsOf(kids), heading);
  if (!hit) return;
  const start = kids.indexOf(hit.el);
  const level = Number(kids[start].nodeName.slice(1));
  let end = kids.length;
  for (let i = start + 1; i < kids.length; i++) {
    if (isHeading(kids[i]) && Number(kids[i].nodeName.slice(1)) <= level) {
      end = i;
      break;
    }
  }
  const keep = new Set(kids.slice(start, end));
  for (const n of Array.from(frag.childNodes)) if (!keep.has(n as Element)) n.remove();
}

/** 同じページの `#…`（脚注の id・見出しの id・見出しの文字列 = findHeading）→ 着地する要素 */
export function anchorTarget(root: ShadowRoot | DocumentFragment | Element, id: string): Element | null {
  const byId = "getElementById" in root ? (root as ShadowRoot).getElementById(id) : (root as Element).querySelector(`[id="${CSS.escape(id)}"]`);
  if (byId) return byId;
  return findHeading(headingsOf(Array.from(root.querySelectorAll("h1, h2, h3, h4, h5, h6"))), id)?.el ?? null;
}

/** md の描画で拾ったリンク（node test が core の fixture と比べる）: 出現順の (kind, target) */
export function collectLinks(frag: DocumentFragment | Element): { kind: string; target: string }[] {
  const out: { kind: string; target: string }[] = [];
  const els = frag.querySelectorAll("[data-wk-href], [data-wk-src], .wk-embed, .wk-embed-link, .wk-embed-page-title, .wk-media");
  for (const el of Array.from(els)) {
    const wiki = el.getAttribute("data-wk-href");
    if (el.classList.contains("wk-embed-page-title") || el.classList.contains("wk-embed-link") || el.classList.contains("wk-media") || el.classList.contains("wk-embed")) {
      const t = el.getAttribute("data-wk-target") ?? el.getAttribute("data-wk-embed");
      if (t) out.push({ kind: "embed", target: t });
      continue;
    }
    if (wiki != null) {
      if (wiki.startsWith("[[") && wiki.endsWith("]]")) out.push({ kind: "wiki", target: wiki.slice(2, -2) });
      else {
        const s = schemeOf(wiki);
        if (wiki.startsWith("#") || !wiki.trim()) continue;
        out.push({ kind: s === "openref" ? "openref" : s === "file" ? "file" : s ? "external" : "md", target: wiki });
      }
      continue;
    }
    const src = el.getAttribute("data-wk-src");
    if (src != null) {
      const s = schemeOf(src);
      out.push({ kind: s === "file" ? "file" : s ? "external" : "md", target: src });
    }
  }
  return out;
}
