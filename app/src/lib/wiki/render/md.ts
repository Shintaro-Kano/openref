/* OpenNote: md → HTML 文字列。DOM に依存しない（node test が直接読む）。
   markdown-it + footnote + task-lists + 自作の wikilink / embed / callout。出力は必ず sanitize.ts の purify → finish を通す。
   - `[[name|alias]]` `[[name#h]]` → <a data-wk-wiki="name#h">（解決は finish が page.links で行う = 索引の解決に従う）
   - `![[x|560]]` → <span data-wk-embed="x" data-wk-size="560">（画像・動画・音声・ページ・PDF の展開は finish / MdView）
   - target の切り出しは core parse_md.rs の wikilinks と同じ（最初の `|` の前・末尾の `\` を落とす・trim・同じ行の中・中に `[[` があれば外側は閉じない）。
     wikiClose / splitWikiInner / scanWikiText を sanitize（HTML ブロックの文字列）と chat の chatLinks（wikiLinksOf）が共用する
   - callout `> [!type] title`（`[!type]-` / `[!type]+` = 折畳）→ <div class="wk-callout"> か <details class="wk-callout">
   - `[toc]` だけの段落 → <div class="wk-toc" data-wk-toc>（見出しの一覧は finish が id を付けた後に埋める）
   - validateLink を差し替え（既定は file: と画像以外の data: を拒否 = 参照フォルダの file:// がリンクにならない）。normalizeLink は恒等
     （href に書かれたままの dest を残し、finish が core と同じ target で page.links を引く） */
import markdownIt, { type MarkdownIt, type Token } from "markdown-it";
import footnote from "markdown-it-footnote";
import taskLists from "markdown-it-task-lists";
import { schemeOf } from "./asset.ts";


/** frontmatter（先頭の `---` 〜 `---` / `...`）を外した本文の開始位置（core frontmatter.rs の split と同じ） */
export function frontmatterEnd(text: string): number {
  const bom = text.startsWith("﻿") ? 1 : 0;
  const t = text.slice(bom);
  const first = t.indexOf("\n");
  if (first < 0 || t.slice(0, first).trimEnd() !== "---") return 0;
  let pos = first + 1;
  while (pos <= t.length) {
    let end = t.indexOf("\n", pos);
    if (end < 0) end = t.length;
    const line = t.slice(pos, end).trimEnd();
    if (line === "---" || line === "...") return bom + Math.min(end + 1, t.length);
    if (end === t.length) break;
    pos = end + 1;
  }
  return 0;
}

const LINK_SCHEMES = ["http", "https", "mailto", "openref", "asset", "file"];

/** markdown-it の validateLink の代わり。許可 = 相対・http https mailto openref asset file・data は画像だけ */
export function linkAllowed(url: string): boolean {
  const u = url.trim();
  const s = schemeOf(u);
  if (!s) return !/^[\u0000-\u001f]/.test(u);
  if (LINK_SCHEMES.includes(s)) return true;
  return s === "data" && /^data:image\/(png|jpe?g|gif|webp|avif|svg\+xml|bmp)[;,]/i.test(u);
}

/** `[[…]]` の中身 → (target, alias)。core の規則と同じ切り出し */
export function splitWikiInner(inner: string): { target: string; alias: string | null } {
  const bar = inner.indexOf("|");
  const head = bar < 0 ? inner : inner.slice(0, bar);
  const target = head.replace(/\\+$/, "").trim();
  const alias = bar < 0 ? null : inner.slice(bar + 1).trim();
  return { target, alias: alias === "" ? null : alias };
}

/** `[[` の直後 start から閉じの `]]` を探す（core と同じ = 同じ行の中・中に `[[` があれば外側は閉じない）。
    end = 閉じの `]]` の位置（閉じなければ -1）・stop = 走査が止まった位置 */
export function wikiClose(s: string, start: number, max = s.length): { end: number; stop: number } {
  let j = start;
  while (j + 1 < max) {
    const c = s.charCodeAt(j);
    if (c === 0x0a) break; // 行をまたがない（core は行ごと）
    if (c === 0x5b && s.charCodeAt(j + 1) === 0x5b) break;
    if (c === 0x5d && s.charCodeAt(j + 1) === 0x5d) return { end: j, stop: j };
    j++;
  }
  return { end: -1, stop: j };
}

/** 文字列の中の `[[…]]` / `![[…]]`（from〜to = 直前の `!` を含む範囲）。空の target は飛ばす */
export interface WikiHit {
  from: number;
  to: number;
  embed: boolean;
  target: string;
  alias: string | null;
}
export function scanWikiText(s: string): WikiHit[] {
  const out: WikiHit[] = [];
  let i = 0;
  while (i < s.length - 1) {
    if (!(s[i] === "[" && s[i + 1] === "[")) {
      i++;
      continue;
    }
    const { end, stop } = wikiClose(s, i + 2);
    if (end < 0) {
      i = stop > i + 2 ? stop : i + 2;
      continue;
    }
    const { target, alias } = splitWikiInner(s.slice(i + 2, end));
    const embed = i > 0 && s[i - 1] === "!";
    if (target) out.push({ from: embed ? i - 1 : i, to: end + 2, embed, target, alias });
    i = end + 2;
  }
  return out;
}

/** 表示名: alias → `page#h` は `page > h` → `#h` は `h` */
export function wikiLabel(target: string, alias: string | null): string {
  if (alias) return alias;
  if (target.startsWith("#")) return target.slice(1);
  const h = target.indexOf("#");
  const name = h < 0 ? target : `${target.slice(0, h)} > ${target.slice(h + 1)}`;
  return name.replace(/\.md$/i, "");
}

function wikilinkPlugin(md: MarkdownIt) {
  md.inline.ruler.before("link", "wk_wikilink", (state, silent) => {
    const src = state.src;
    let pos = state.pos;
    const embed = src.charCodeAt(pos) === 0x21; /* ! */
    if (embed) pos++;
    if (src.charCodeAt(pos) !== 0x5b || src.charCodeAt(pos + 1) !== 0x5b) return false;
    const start = pos + 2;
    const { end } = wikiClose(src, start, state.posMax);
    if (end < 0) return false;
    const { target, alias } = splitWikiInner(src.slice(start, end));
    if (!target) return false;
    if (!silent) {
      const tok = state.push(embed ? "wk_embed" : "wk_wikilink", "", 0);
      tok.meta = { target, alias };
    }
    state.pos = end + 2;
    return true;
  });
  const esc = md.utils.escapeHtml;
  md.renderer.rules.wk_wikilink = (tokens, idx) => {
    const { target, alias } = tokens[idx].meta as { target: string; alias: string | null };
    return `<a class="wk-wikilink" data-wk-wiki="${esc(target)}">${esc(wikiLabel(target, alias))}</a>`;
  };
  md.renderer.rules.wk_embed = (tokens, idx) => {
    const { target, alias } = tokens[idx].meta as { target: string; alias: string | null };
    const size = alias && /^\d+(x\d+)?$/.test(alias) ? alias : "";
    const label = alias && !size ? alias : "";
    return `<span class="wk-embed" data-wk-embed="${esc(target)}" data-wk-size="${esc(size)}" title="${esc(label)}">${esc(wikiLabel(target, label || null))}</span>`;
  };
}

/** Obsidian の callout の 12 種と別名。未知の type は note */
const CALLOUT: Record<string, string> = {
  note: "note", abstract: "abstract", summary: "abstract", tldr: "abstract", info: "info", todo: "todo",
  tip: "tip", hint: "tip", important: "tip", success: "success", check: "success", done: "success",
  question: "question", help: "question", faq: "question", warning: "warning", caution: "warning", attention: "warning",
  failure: "failure", fail: "failure", missing: "failure", danger: "danger", error: "danger", bug: "bug",
  example: "example", quote: "quote", cite: "quote",
};

export function calloutType(t: string): string {
  return CALLOUT[t.toLowerCase()] ?? "note";
}

function calloutPlugin(md: MarkdownIt) {
  // block の後・inline の前: 引用の最初の段落の 1 行目が `[!type]` なら callout にする
  md.core.ruler.after("block", "wk_callout", (state) => {
    const toks = state.tokens;
    for (let i = 0; i < toks.length; i++) {
      const open = toks[i];
      if (open.type !== "blockquote_open") continue;
      const p = toks[i + 1];
      const inl = toks[i + 2];
      if (!p || p.type !== "paragraph_open" || !inl || inl.type !== "inline") continue;
      const m = /^\[!([^\]\s]+)\]([-+]?)[ \t]*(.*)$/.exec(inl.content.split("\n", 1)[0]);
      if (!m) continue;
      const type = m[1];
      const fold = m[2];
      const titleSrc = m[3].trim();
      // 対の blockquote_close
      let depth = 0;
      let close = -1;
      for (let k = i; k < toks.length; k++) {
        if (toks[k].type === "blockquote_open") depth++;
        else if (toks[k].type === "blockquote_close" && --depth === 0) {
          close = k;
          break;
        }
      }
      if (close < 0) continue;
      const meta = { type: calloutType(type), raw: type.toLowerCase(), fold };
      open.meta = { ...(open.meta ?? {}), callout: meta };
      toks[close].meta = { ...(toks[close].meta ?? {}), callout: meta };
      const rest = inl.content.includes("\n") ? inl.content.slice(inl.content.indexOf("\n") + 1) : "";
      const titleOpen = new state.Token("wk_callout_title_open", "", 1);
      titleOpen.meta = meta;
      const title = new state.Token("inline", "", 0);
      title.content = titleSrc || type.charAt(0).toUpperCase() + type.slice(1).toLowerCase();
      title.map = inl.map;
      title.children = [];
      const titleClose = new state.Token("wk_callout_title_close", "", -1);
      titleClose.meta = meta;
      if (rest.trim()) {
        inl.content = rest;
        toks.splice(i + 1, 0, titleOpen, title, titleClose);
      } else {
        // 1 行目だけの段落 = 段落ごと title に置き換える
        toks.splice(i + 1, 3, titleOpen, title, titleClose);
      }
    }
  });
  const esc = md.utils.escapeHtml;
  const prevOpen = md.renderer.rules.blockquote_open;
  const prevClose = md.renderer.rules.blockquote_close;
  md.renderer.rules.blockquote_open = (tokens, idx, opts, env, slf) => {
    const c = tokens[idx].meta?.callout as { type: string; fold: string } | undefined;
    if (!c) return prevOpen ? prevOpen(tokens, idx, opts, env, slf) : slf.renderToken(tokens, idx, opts);
    const cls = `wk-callout wk-callout-${esc(c.type)}`;
    return c.fold ? `<details class="${cls}"${c.fold === "+" ? " open" : ""}>` : `<div class="${cls}">`;
  };
  md.renderer.rules.blockquote_close = (tokens, idx, opts, env, slf) => {
    const c = tokens[idx].meta?.callout as { type: string; fold: string } | undefined;
    if (!c) return prevClose ? prevClose(tokens, idx, opts, env, slf) : slf.renderToken(tokens, idx, opts);
    return c.fold ? "</div></details>\n" : "</div></div>\n";
  };
  md.renderer.rules.wk_callout_title_open = (tokens, idx) =>
    (tokens[idx].meta as { fold: string }).fold ? `<summary class="wk-callout-title">` : `<div class="wk-callout-title">`;
  md.renderer.rules.wk_callout_title_close = (tokens, idx) =>
    ((tokens[idx].meta as { fold: string }).fold ? `</summary>` : `</div>`) + `<div class="wk-callout-body">\n`;
}

/** task-lists の <input type=checkbox>（sanitize が input を落とす）を描画だけの span に置き換える */
function taskSpanPlugin(md: MarkdownIt) {
  md.core.ruler.after("github-task-lists", "wk_task_span", (state) => {
    for (const t of state.tokens) {
      if (t.type !== "inline" || !t.children) continue;
      for (const c of t.children as Token[]) {
        if (c.type === "html_inline" && c.content.startsWith('<input class="task-list-item-checkbox"')) {
          c.content = c.content.includes("checked") ? '<span class="wk-task wk-task-on"></span>' : '<span class="wk-task"></span>';
        }
      }
    }
  });
}

/** `[toc]`（大小無視）だけの段落を目次の箱に。中身は sanitize の finish が見出しの id を付けた後に作る */
function tocPlugin(md: MarkdownIt) {
  md.core.ruler.after("block", "wk_toc", (state) => {
    const toks = state.tokens;
    for (let i = 0; i + 2 < toks.length; i++) {
      if (toks[i].type !== "paragraph_open" || toks[i + 1].type !== "inline" || toks[i + 2].type !== "paragraph_close") continue;
      if (!/^\[toc\]$/i.test(toks[i + 1].content.trim())) continue;
      const t = new state.Token("wk_toc", "div", 0);
      t.block = true;
      t.map = toks[i].map;
      toks.splice(i, 3, t);
    }
  });
  md.renderer.rules.wk_toc = () => `<div class="wk-toc" data-wk-toc="1"></div>\n`;
}

let shared: MarkdownIt | null = null;

export function markdown(): MarkdownIt {
  if (shared) return shared;
  const md = markdownIt({ html: true, linkify: true, typographer: false, breaks: false });
  md.validateLink = linkAllowed;
  md.normalizeLink = (u: string) => u;
  md.normalizeLinkText = (u: string) => u;
  md.use(footnote);
  md.use(taskLists, { enabled: false });
  md.use(wikilinkPlugin);
  md.use(calloutPlugin);
  md.use(tocPlugin);
  md.use(taskSpanPlugin);
  // HTML ブロックが閉じない `<!--` で終わると（`<!-- a --> b <!--` の行）、HTML の構文解析が以後のページ全体をコメントに呑む → 閉じる
  md.renderer.rules.html_block = (tokens, idx) => {
    const s = tokens[idx].content;
    return s.lastIndexOf("<!--") > s.lastIndexOf("-->") ? s + " -->\n" : s;
  };
  // linkify は http(s) / mailto だけ（`www.` や `//` を外部リンクにしない）
  md.linkify.set({ fuzzyLink: false, fuzzyEmail: false, fuzzyIP: false });
  shared = md;
  return md;
}

/** HTML ブロックの中で finish が `[[…]]` を拾わない所（コメント（中身の無い `<!-->` `<!--->` も）・タグそのもの・
    a code pre style script kbd の中 = sanitize の SKIP_TEXT。`<a-b>` `<code-block>` は別の要素）= 改行に置き換える。
    近似の外 = 属性値の中の `>`・実体参照（`&#91;&#91;` は描画だけが [[ に戻す） */
const HTML_NO_WIKI = /<!--(?:-?>|[\s\S]*?(?:-->|$))|<(a|code|pre|style|script|kbd)(?=[\s/>])[\s\S]*?(?:<\/\1\s*>|$)|<[^>]*>/gi;

/** md 全文（frontmatter 付きでよい）→ 描画が作る `[[…]]` / `![[…]]` の (kind, target)（出現順・重複あり）。
    描画と同じ markdown-it の走査（code・fence・frontmatter の中は無い）+ finish の wikilinkTextNodes と同じ scanWikiText を
    HTML ブロックの文字列と、リンクの外の text（`\[[a]]` = エスケープの後に残った `[[…]]` も描画はリンクにする）にかける */
export function wikiLinksOf(text: string): { kind: "wiki" | "embed"; target: string }[] {
  const out: { kind: "wiki" | "embed"; target: string }[] = [];
  const scan = (s: string) => {
    for (const w of scanWikiText(s)) out.push({ kind: w.embed ? "embed" : "wiki", target: w.target });
  };
  const walk = (toks: Token[]) => {
    let inLink = 0;
    for (const t of toks) {
      if (t.type === "wk_wikilink" || t.type === "wk_embed") {
        out.push({ kind: t.type === "wk_embed" ? "embed" : "wiki", target: (t.meta as { target: string }).target });
      } else if (t.type === "html_block") scan(t.content.replace(HTML_NO_WIKI, "\n"));
      else if (t.type === "link_open") inLink++;
      else if (t.type === "link_close") inLink--;
      else if (t.type === "text" && !inLink && t.content.includes("[[")) scan(t.content);
      if (t.children) walk(t.children);
    }
  };
  walk(markdown().parse(text.slice(frontmatterEnd(text)), {}));
  return out;
}

/** md 全文（frontmatter 付きでよい）→ HTML 文字列（未掃除）。docId = 埋め込みの脚注 id の衝突除け */
export function renderMarkdown(text: string, docId?: string): string {
  const body = text.slice(frontmatterEnd(text));
  return markdown().render(body, docId ? { docId } : {});
}
