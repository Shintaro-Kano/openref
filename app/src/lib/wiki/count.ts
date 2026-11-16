// OpenNote の字数（ページのツールバーの「n 字」「選択 m 字」）。数え方は自前で単純に決める:
// - 本文 = 描画したときに見える文字。表示と同じ markdown-it の走査（render/md.ts の markdown()）で text・コード・
//   `[[ページ]]` の表示名・HTML の地の文を集める。frontmatter・リンクの URL（地の文の素の URL と `<URL>` も = 本文ではない）・
//   画像（alt は見えない）・埋め込み `![[…]]`・
//   記法の記号・HTML のタグとコメントは数えない
// - 選択 = 選んだ文字のそのまま（frontmatter の中も）
// どちらも空白と改行（全角の空白も）は数えない。1 字 = Unicode のコードポイント 1 つ

import type { Token } from "markdown-it";
import { frontmatterEnd, markdown, wikiLabel } from "./render/md.ts";

/** 空白と改行を除いた字数 */
export function countChars(s: string): number {
  let n = 0;
  for (const c of s) if (!/\s/u.test(c)) n++;
  return n;
}

/** HTML の地の文（コメント・タグを落とし、実体参照は 1 字に） */
function htmlText(s: string): string {
  return s
    .replace(/<!--[\s\S]*?(?:-->|$)/g, "")
    .replace(/<(script|style)[\s>][\s\S]*?(?:<\/\1\s*>|$)/gi, "")
    .replace(/<[^>]*>/g, "")
    .replace(/&(?:#x?[0-9a-f]+|[a-z][a-z0-9]*);/gi, "x");
}

/** md 全文（frontmatter 付きでよい）→ 描画したときに見える文字の数 */
export function countBody(md: string): number {
  let n = 0;
  /** 素の URL（linkify）・`<URL>`（autolink）のリンクの中 = 数えない */
  let bare = 0;
  const walk = (toks: Token[]) => {
    for (const t of toks) {
      if (t.type === "link_open" && (t.markup === "linkify" || t.markup === "autolink")) {
        bare++;
        continue;
      }
      if (t.type === "link_close" && bare) {
        bare--;
        continue;
      }
      if (bare) continue;
      switch (t.type) {
        case "text":
        case "code_inline":
        case "code_block":
        case "fence":
          n += countChars(t.content);
          break;
        case "wk_wikilink": {
          const m = t.meta as { target: string; alias: string | null };
          n += countChars(wikiLabel(m.target, m.alias));
          break;
        }
        case "html_inline":
        case "html_block":
          n += countChars(htmlText(t.content));
          break;
        case "image": // alt は見えない（children = alt の text）
        case "wk_embed":
          continue;
      }
      if (t.children) walk(t.children);
    }
  };
  walk(markdown().parse(md.slice(frontmatterEnd(md)), {}));
  return n;
}
