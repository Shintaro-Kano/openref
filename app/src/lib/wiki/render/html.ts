/* OpenNote: html ページの前処理。purify の前に inert な文書（DOMParser）で:
   - `<link rel="stylesheet" href="相対">` は vault 内のファイルなら中身を読み（fetchText = asset URL の fetch）、`<style>` にする。
     中の相対 url() は CSS ファイルからの相対で asset URL に書き換え、同じ掃除（cleanStyleSheet）を通す。vault 外・http(s) は除去。
     読めない（403・無い）ものは捨てて描画を続ける（理由は console.warn に残す）
   - `<head>` の `<style>` は body の先頭へ（DOMPurify は body だけを返す）。中身の掃除は finish が行う
   結果は HTML 文字列（このあと purify → finish）。script は purify が落とす（常に無効） */
import type { WikiLink } from "../types.ts";
import { absOf, assetInRoot, assetUrl, filePath, relInRoot, resolveRelative, schemeOf } from "./asset.ts";
import { cleanStyleSheet } from "./css.ts";

export interface HtmlCtx {
  root: string;
  path: string;
  links: WikiLink[];
}

/** link の href → vault 相対パス（相対は page.links の html 種 → このページからの相対・file: は vault 内だけ）。読めない物は null */
export function stylesheetPath(href: string, ctx: HtmlCtx): string | null {
  const d = href.trim();
  const s = schemeOf(d);
  if (s === "file") {
    const abs = filePath(d);
    return abs ? relInRoot(ctx.root, abs) : null;
  }
  if (s || d.startsWith("//") || !d) return null;
  const hit = ctx.links.find((l) => l.kind === "html" && l.target === d);
  return hit ? hit.path : resolveRelative(ctx.path, d);
}

export async function prepareHtml(win: Window & typeof globalThis, html: string, ctx: HtmlCtx, fetchText: (url: string) => Promise<string>): Promise<string> {
  const doc = new win.DOMParser().parseFromString(html, "text/html");
  const styles: string[] = [];
  for (const el of Array.from(doc.querySelectorAll("link, head style") as NodeListOf<Element>)) {
    if (el.nodeName.toLowerCase() === "style") {
      styles.push(el.textContent ?? "");
      el.remove();
      continue;
    }
    const rel = (el.getAttribute("rel") ?? "").toLowerCase().split(/\s+/);
    const href = el.getAttribute("href");
    el.remove();
    if (!rel.includes("stylesheet") || !href) continue;
    const p = stylesheetPath(href, ctx);
    if (!p || !/\.css$/i.test(p)) continue;
    let css: string;
    try {
      css = await fetchText(assetUrl(absOf(ctx.root, p)));
    } catch (e) {
      console.warn("wiki stylesheet:", href, e);
      continue;
    }
    styles.push(
      cleanStyleSheet(
        css,
        (v) => {
          const r = resolveRelative(p, v);
          return r ? assetUrl(absOf(ctx.root, r)) : null;
        },
        (v) => assetInRoot(ctx.root, v),
      ),
    );
  }
  const head = styles.map((s) => `<style>${s.replace(/<\/style/gi, "<\\/style")}</style>`).join("");
  return head + (doc.body?.innerHTML ?? "");
}
