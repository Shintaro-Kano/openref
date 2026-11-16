/* OpenNote: 編集中の本文（エディタの原文）→ 見出しの一覧と、その行（右ペインの目次 = 編集中はこれを出し、押すとエディタがその行へ）。
   id は表示の描画（sanitize の finish）と同じ規則 = 見出しの文字（描画したときの textContent）を文書の順に headingId へ通す
   （同名の見出しの連番・記号・和文が表示と同じ。分割のプレビューへ同じ id で scrollTo が届く）。
   md: markdown-it の走査（描画と同じ設定 = frontmatter・コードの中は見出しでない・引用 / リストの中の見出しも数える）+ HTML ブロックの中の <h1>〜<h6>。
   html: 原文の <h1>〜<h6>（コメント・script・style の中を除く）。
   描画との近似の外（id がずれうる = 編集中のエディタの移動は効く・分割のプレビューだけ動かない）: 見出しの中の画像の埋め込み `![[x.png]]`・
   外部の画像・インラインの HTML の見出し（段落の途中の <h2>）・purify が落とす中身（見出しの中の <script> 等）・
   HTML の見出しを正規表現で拾う所の近似 = 属性値に `>` のある開始タグ（<h2 title='>'>）・<template> / <textarea> の中の <h2>・閉じない <h2> */
import type { Token } from "markdown-it";
import { frontmatterEnd, markdown, scanWikiText, wikiLabel } from "./md.ts";
import { headingId } from "./sanitize.ts";

export interface SourceHeading {
  level: number;
  text: string;
  id: string;
  /** エディタの行（1 始まり・原文の全体 = frontmatter を含む） */
  line: number;
}

type Raw = Omit<SourceHeading, "id">;

const lines = (s: string) => {
  let n = 0;
  for (let i = s.indexOf("\n"); i >= 0; i = s.indexOf("\n", i + 1)) n++;
  return n;
};

/** 見出しの中身の文字（描画の textContent と同じになるように: リンクの記号・強調・インラインの HTML のタグは字を持たない・
    wikilink は表示名・脚注の参照は [n]・改行は \n） */
function inlineText(children: Token[] | null): string {
  let s = "";
  let inLink = 0;
  let run = ""; // リンクの外の続いた字 = 描画の 1 つの text node（finish の wikilinkTextNodes が `\[[a]]` の残りもリンクの表示名にする）
  const flush = () => {
    let last = 0;
    for (const w of scanWikiText(run)) {
      const size = w.embed && w.alias && /^\d+(x\d+)?$/.test(w.alias);
      s += run.slice(last, w.from) + wikiLabel(w.target, size ? null : w.alias);
      last = w.to;
    }
    s += run.slice(last);
    run = "";
  };
  for (const c of children ?? []) {
    if ((c.type === "text" || c.type === "text_special") && !inLink) {
      run += c.content;
      continue;
    }
    flush();
    if (c.type === "link_open") inLink++;
    else if (c.type === "link_close") inLink--;
    if (c.type === "text" || c.type === "code_inline" || c.type === "text_special") s += c.content;
    else if (c.type === "softbreak" || c.type === "hardbreak") s += "\n";
    else if (c.type === "wk_wikilink" || c.type === "wk_embed") {
      const { target, alias } = c.meta as { target: string; alias: string | null };
      const size = c.type === "wk_embed" && alias && /^\d+(x\d+)?$/.test(alias);
      s += wikiLabel(target, size ? null : alias);
    } else if (c.type === "footnote_ref") {
      const m = c.meta as { id: number; subId: number };
      s += `[${m.id + 1}${m.subId > 0 ? `:${m.subId}` : ""}]`;
    }
  }
  flush();
  return s;
}

/** HTML の文字列の中の <h1>〜<h6>（コメント・script・style の中は同じ長さの空白に潰して数えない = 行の位置は保つ） */
function htmlHeadings(src: string, firstLine: number, doc: Document): Raw[] {
  const blank = (m: string) => m.replace(/[^\n]/g, " ");
  const s = src.replace(/<!--[\s\S]*?(?:-->|$)/g, blank).replace(/<(script|style)(?=[\s>/])[\s\S]*?(?:<\/\1\s*>|$)/gi, blank);
  const out: Raw[] = [];
  const tpl = doc.createElement("template");
  for (const m of s.matchAll(/<h([1-6])(?=[\s>/])[^>]*>([\s\S]*?)<\/h\1\s*>/gi)) {
    tpl.innerHTML = m[2];
    out.push({ level: Number(m[1]), text: tpl.content.textContent ?? "", line: firstLine + lines(s.slice(0, m.index)) });
  }
  return out;
}

/** 本文 → 見出し（文書の順）。doc = HTML の中身の字を読む（実体参照・入れ子のタグ） */
export function sourceHeadings(text: string, kind: "md" | "html", doc: Document): SourceHeading[] {
  let raw: Raw[];
  if (kind === "html") raw = htmlHeadings(text, 1, doc);
  else {
    const start = frontmatterEnd(text);
    const base = 1 + lines(text.slice(0, start));
    const toks = markdown().parse(text.slice(start), {});
    raw = [];
    for (let i = 0; i < toks.length; i++) {
      const t = toks[i];
      if (t.type === "heading_open" && t.map) raw.push({ level: Number(t.tag.slice(1)), text: inlineText(toks[i + 1]?.children ?? null), line: base + t.map[0] });
      else if (t.type === "html_block" && t.map) raw.push(...htmlHeadings(t.content, base + t.map[0], doc));
    }
  }
  const used = new Set<string>();
  return raw.map((h) => {
    const text = h.text.trim();
    return { level: h.level, text, id: headingId(text, used), line: h.line };
  });
}
