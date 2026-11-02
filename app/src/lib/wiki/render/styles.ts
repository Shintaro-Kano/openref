/* OpenNote: shadow root の中の体裁（adoptedStyleSheets で入れる。app.css は shadow の中へ届かない）。
   色は app.css の CSS 変数（--text --dim --border --accent …）を継承して使う（custom property は shadow の境界を越えて継承される）。
   base = md / html 共通（リンク・埋め込み・media・検索の強調）、md = md の文章の体裁（html ページは作者の CSS に任せる）。
   md の :host(.wk-hc) = 見出しの色分け（host の class = WikiPage のツールバーの切替。色は app.css の --wk-h1〜6 = ライト / ダーク） */

const BASE = `
:host { display: block; contain: layout paint; position: relative; color: var(--text); }
.wk-root { position: relative; contain: layout paint; overflow-wrap: anywhere; }
:where(a) { color: var(--accent); text-decoration: none; cursor: pointer; }
:where(a:hover) { text-decoration: underline; }
a.wk-l-unresolved, .wk-embed.wk-unresolved { color: var(--dim); text-decoration: underline dashed; cursor: default; }
a.wk-l-item::before { content: ""; display: inline-block; width: 0.8em; height: 0.95em; margin-right: 0.2em; vertical-align: -0.1em;
  border: 1.5px solid currentColor; border-radius: 2px; box-sizing: border-box; background: linear-gradient(currentColor, currentColor) 50% 35% / 50% 1.5px no-repeat; }
a.wk-l-ext::after, a.wk-l-open::after, a.wk-l-ref::after { content: "\\2197"; font-size: 0.8em; margin-left: 0.1em; color: var(--dim); }
a.wk-ext-media { display: inline-block; padding: 1px 6px; border: 1px dashed var(--border); border-radius: 4px; font-size: 0.92em; }
a.wk-embed-pdf::before { content: "PDF"; font-size: 0.7em; font-weight: 700; margin-right: 0.35em; padding: 0 3px; border-radius: 3px;
  color: #fff; background: #c0392b; vertical-align: 0.1em; }
:where(img) { max-width: 100%; height: auto; }
:where(video, audio) { max-width: 100%; }
.wk-media-video video { display: block; max-height: 60vh; background: #000; }
.wk-media { display: block; margin: 0.6em 0; }
.wk-media-inline.wk-media-image { display: inline-block; margin: 0; max-width: 100%; }
.wk-media-bar { display: flex; align-items: center; gap: 8px; margin-top: 4px; font-size: 12px; color: var(--dim); }
.wk-media-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.wk-media-open, .wk-tbl-more { font: inherit; font-size: 12px; padding: 2px 10px; border: 1px solid var(--border); border-radius: 5px;
  background: var(--panel); color: var(--text); cursor: pointer; }
.wk-media-open:hover, .wk-tbl-more:hover { border-color: var(--accent); }
.wk-media-fail { padding: 12px; border: 1px dashed var(--border); border-radius: 6px; color: var(--dim); }
.wk-tbl-more { display: block; margin: 6px 0 14px; }
.wk-embed-page { margin: 0.8em 0; padding: 4px 12px 8px; border-left: 3px solid var(--accent); background: color-mix(in srgb, var(--accent) 5%, transparent); border-radius: 0 6px 6px 0; }
.wk-embed-page-title { display: block; font-size: 12px; font-weight: 600; margin: 2px 0 4px; }
.wk-embed-error { color: var(--dim); font-size: 12px; }
::highlight(wk-find) { background-color: rgba(255, 213, 0, 0.45); color: inherit; }
::highlight(wk-find-cur) { background-color: rgba(255, 140, 0, 0.85); color: #000; }
`;

const MD = `
.wk-root { font-size: 14px; line-height: 1.7; padding: 4px 2px 40px; }
h1, h2, h3, h4, h5, h6 { line-height: 1.35; margin: 1.3em 0 0.5em; font-weight: 650; scroll-margin-top: 12px; }
h1 { font-size: 1.7em; margin-top: 0.4em; } h2 { font-size: 1.4em; padding-bottom: 0.2em; border-bottom: 1px solid var(--border); }
h3 { font-size: 1.2em; } h4 { font-size: 1.05em; } h5, h6 { font-size: 1em; color: var(--dim); }
h1 small, h2 small, h3 small, h4 small { font-size: 0.6em; font-weight: 400; color: var(--dim); }
p, ul, ol, pre, table, blockquote, details, figure { margin: 0 0 0.85em; }
ul, ol { padding-left: 1.6em; }
li > p { margin: 0; }
li.task-list-item { list-style: none; margin-left: -1.3em; }
.wk-task { display: inline-block; width: 0.95em; height: 0.95em; margin-right: 0.35em; vertical-align: -0.12em; border: 1.5px solid var(--dim);
  border-radius: 3px; box-sizing: border-box; }
.wk-task-on { background: var(--accent); border-color: var(--accent);
  background-image: linear-gradient(45deg, transparent 45%, #fff 45%, #fff 58%, transparent 58%); }
code, kbd, pre { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 0.88em; }
:not(pre) > code { padding: 0.1em 0.35em; border-radius: 4px; background: color-mix(in srgb, var(--text) 8%, transparent); }
pre { padding: 10px 12px; border-radius: 6px; overflow-x: auto; background: color-mix(in srgb, var(--text) 6%, transparent); line-height: 1.5; }
kbd { padding: 0 4px; border: 1px solid var(--border); border-bottom-width: 2px; border-radius: 4px; }
blockquote { padding: 0 0 0 12px; border-left: 3px solid var(--border); color: var(--dim); }
hr { border: 0; border-top: 1px solid var(--border); margin: 1.5em 0; }
mark { background: rgba(255, 213, 0, 0.4); color: inherit; }
table { border-collapse: collapse; display: block; max-width: 100%; overflow-x: auto; font-size: 0.93em; line-height: 1.5; }
th, td { border: 1px solid var(--border); padding: 4px 8px; vertical-align: top; }
th { background: color-mix(in srgb, var(--text) 5%, transparent); font-weight: 600; text-align: left; }
table.wk-tbl-big { content-visibility: auto; contain-intrinsic-size: auto 2000px; }
details { border: 1px solid var(--border); border-radius: 6px; padding: 4px 10px; }
details > summary { cursor: pointer; font-weight: 600; }
details[open] > summary { margin-bottom: 4px; }
sup, sub { line-height: 0; }
.footnotes { font-size: 0.9em; color: var(--dim); }
.footnotes-sep { margin-top: 2em; }

:host(.wk-hc) h1 { color: var(--wk-h1); } :host(.wk-hc) h2 { color: var(--wk-h2); } :host(.wk-hc) h3 { color: var(--wk-h3); }
:host(.wk-hc) h4 { color: var(--wk-h4); } :host(.wk-hc) h5 { color: var(--wk-h5); } :host(.wk-hc) h6 { color: var(--wk-h6); }

.wk-toc { border: 1px solid var(--border); border-radius: 6px; padding: 8px 14px; margin: 0 0 1em; font-size: 0.92em; }
.wk-toc ul { list-style: none; margin: 0; padding: 0; }
.wk-toc li { margin: 2px 0; }
.wk-toc a { text-decoration: none; }
.wk-toc a:hover { text-decoration: underline; }
.wk-callout { --wk-c: 68, 138, 255; border: 0; border-left: 4px solid rgb(var(--wk-c)); border-radius: 4px; padding: 6px 12px;
  margin: 0 0 0.85em; background: rgba(var(--wk-c), 0.08); color: var(--text); }
.wk-callout > .wk-callout-title { font-weight: 650; color: rgb(var(--wk-c)); margin: 0; }
.wk-callout > summary.wk-callout-title { list-style-position: inside; }
.wk-callout-body > :last-child { margin-bottom: 0; }
.wk-callout-body > :first-child { margin-top: 4px; }
details.wk-callout { border-top: 0; border-right: 0; border-bottom: 0; }
.wk-callout-abstract, .wk-callout-todo { --wk-c: 0, 176, 255; }
.wk-callout-info { --wk-c: 0, 122, 255; }
.wk-callout-tip { --wk-c: 0, 191, 165; }
.wk-callout-success { --wk-c: 0, 160, 80; }
.wk-callout-question { --wk-c: 236, 117, 0; }
.wk-callout-warning { --wk-c: 236, 117, 0; }
.wk-callout-failure, .wk-callout-danger, .wk-callout-bug { --wk-c: 233, 49, 71; }
.wk-callout-example { --wk-c: 120, 82, 238; }
.wk-callout-quote { --wk-c: 158, 158, 158; }
`;

const HTML = `
.wk-root { font-size: 14px; line-height: 1.6; padding: 4px 2px 40px; }
`;

const cache = new WeakMap<Document, Record<string, CSSStyleSheet>>();

/** 文書ごとに 1 つ作って共有（全ページの shadow root が同じ CSSStyleSheet を参照する） */
export function sheets(doc: Document, kind: "md" | "html"): CSSStyleSheet[] {
  let c = cache.get(doc);
  if (!c) {
    const win = doc.defaultView as (Window & typeof globalThis) | null;
    const mk = (css: string) => {
      const s = new (win ?? globalThis).CSSStyleSheet();
      s.replaceSync(css);
      return s;
    };
    c = { base: mk(BASE), md: mk(MD), html: mk(HTML) };
    cache.set(doc, c);
  }
  return [c.base, c[kind]];
}
