/* OpenNote の描画の安全と URL の退行（node app/src/lib/wiki/render.test.mjs・Node 22.18+ の type stripping・jsdom）。
   ① DOMPurify の許可リストと URL 規則: on*・script 等の要素・javascript: と外部 http の src が消え、asset: / openref: / file: / data 画像が残る
   ② `<style>` と style 属性の自前の掃除: @import・CSS エスケープした url()（`\75 rl(` 等）・:host の規則・image-set が消える
   ③ asset URL の組立て（和文・`:`・空白・`#`・`?`・`%`）と相対パスの解決
   ④ 大きな表（content-visibility の印・続きを表示）・見出しの id・閉じない HTML コメント・埋め込み
   ⑤ [toc]（目次）
   ⑥ 作者の HTML が遷移の印（data-wk-path / ext / open / ref / item …）を作れないこと・許可している data-wk-wiki / embed / toc を作者が書いたときの振る舞い
   ⑦ `contain: layout paint`（作者の position: fixed をページの箱に閉じ込める唯一の壁）が styles.ts と ShadowPage.svelte に残っていること（jsdom では配置を確かめられないので文字列で固定）
   ⑧ ページ内検索（Finder）: CSS.highlights は文書で 1 つ = 持ち主でない Finder の reset / clear が強調を消さない（偽の CSS.highlights / Highlight）
   ⑨ 失敗を握りつぶさない・検査を 1 つに: checkPdf の 3 値（PDF / PDF でない / 読めなかった = 理由）・CSS の url(asset:) は img の src と
     同じく vault の中だけ（<style>・style 属性・link の stylesheet）・link の stylesheet が読めないと console.warn に残して描画は続ける
   ⑩ 見出しの照合が 1 つの規則（headingKey）: `[[p#見出し]]` の着地（findHeading）・`![[p#見出し]]` の節（keepSection）・同じページの `#…`
     （anchorTarget）が同じ組で一致し、目次のクリックと `[[p#見出し]]` の着地が同じ id
   出力 = `ok: wiki render (N tests)`（N = 列 0 のブロックの数） */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { JSDOM } from "jsdom";
import { renderMarkdown, linkAllowed, frontmatterEnd, calloutType } from "./render/md.ts";
import { purify, finish, navOf, parseOpenref, ALLOWED_TAGS, MAX_PAGE_EMBEDS, headingKey, findHeading, keepSection, anchorTarget } from "./render/sanitize.ts";
import { checkPdf } from "./render/media.ts";
import { prepareHtml } from "./render/html.ts";
import { cleanStyleSheet, cleanStyleAttr, decodeCss, MAX_CSS_DEPTH } from "./render/css.ts";
import { assetUrl, absOf, resolveRelative, filePath, relInRoot, viewKindOf, normalizeRel, assetPath, normalizeAbs, assetInRoot, dirOf, baseName } from "./render/asset.ts";
import { Finder } from "./render/find.ts";

const win = new JSDOM("").window;
const labels = { openExternal: "open", playFailed: "fail", moreRows: (n) => `more ${n}`, externalImage: "img:", unresolved: "unres", pdf: "pdf" };
const ctx = (over = {}) => ({ root: "/Users/me/vault", path: "notes/page.md", kind: "md", links: [], labels, ...over });
function render(md, over) {
  const frag = purify(win, renderMarkdown(md));
  const res = finish(frag, ctx(over));
  const box = win.document.createElement("div");
  box.append(frag);
  return { box, res, html: box.innerHTML };
}
function renderHtml(html, over) {
  const frag = purify(win, html);
  const res = finish(frag, ctx({ kind: "html", path: "pages/p.html", ...over }));
  const box = win.document.createElement("div");
  box.append(frag);
  return { box, res, html: box.innerHTML };
}
let tests = 0;

// ---- ① 要素と属性 ----
{
  const { box, html } = render(
    [
      `<img src="a.png" onerror="alert(1)" onload="x()"> <script>alert(1)</script> <iframe src="https://e.org"></iframe>`,
      `<object data="x"></object><embed src="x"><form action="https://e.org"><input value="x"></form>`,
      `<link rel="stylesheet" href="https://e.org/x.css"><meta http-equiv="refresh" content="0;url=https://e.org"><base href="https://e.org/">`,
      `<svg><image href="https://e.org/x.png"/></svg><math><mi>x</mi></math><template><img src="https://e.org/t.png"></template>`,
      `<div onclick="x()" data-x="1" aria-label="a" srcset="https://e.org/s.png" background="https://e.org/b.png">d</div>`,
      `<a href="javascript:alert(1)">j</a> <a href="JaVaScRiPt:alert(1)">J</a> <a href="vbscript:x">v</a> <a href="data:text/html,<script>alert(1)</script>">d</a>`,
      `<a href="  java\tscript:alert(1)">ws</a>`,
    ].join("\n\n"),
  );
  for (const bad of ["script", "iframe", "object", "embed", "form", "input", "link", "meta", "base", "svg", "math", "template", "image"]) {
    assert.equal(box.querySelector(bad), null, `要素 ${bad} が残った`);
  }
  assert.ok(!/\son[a-z]+=/i.test(html), "on* 属性が残った");
  assert.ok(!/srcset|background=|data-x|aria-label/.test(html), "許可外の属性が残った");
  assert.ok(!/javascript:|vbscript:|data:text/i.test(html), "javascript: / vbscript: / data:text が残った");
  assert.ok(!/https:\/\/e\.org/.test(html), "外部 URL が属性に残った");
  // javascript: の <a> は href ごと消える（クリックしても何もしない）
  for (const a of box.querySelectorAll("a")) assert.equal(navOf(a).t, "none", a.outerHTML);
  tests++;
}
{
  // 残る物: asset: / openref: / file: / data 画像・相対（asset に書換え）・mailto
  const { box } = render(
    [
      `<img src="asset://localhost//Users/me/vault/a.png"> <img src="data:image/png;base64,iVBORw0KGgo="> <img src="data:image/svg+xml;utf8,<svg/>">`,
      `<img src="data:text/html;base64,PHNjcmlwdD4="> <img src="file:///Users/me/vault/img/b.png"> <img src="file:///etc/x.png">`,
      `[r](openref://open?key=ABCD2345&page=3&att=ATT1) [f](file:///Users/me/vault/notes/x.md#h) [o](file:///Users/me/Desktop/a%20b.pdf) [m](mailto:a@example.org) [w](https://example.org/a_(b))`,
      `![rel](../img/c%20d.png) ![abs](/img/e.png) ![up](../../../x.png) ![ext](https://e.org/beacon.png)`,
      `<video src="v.mp4"></video> <audio src="https://e.org/a.mp3"></audio> <video><source src="https://e.org/v.mp4"></video>`,
    ].join("\n\n"),
  );
  const imgs = [...box.querySelectorAll("img")].map((i) => i.getAttribute("src"));
  assert.deepEqual(imgs, [
    "asset://localhost//Users/me/vault/a.png",
    "data:image/png;base64,iVBORw0KGgo=",
    "data:image/svg+xml;utf8,<svg/>",
    null, // data:text は消える
    "asset://localhost//Users/me/vault/img/b.png", // vault 内の file: は asset へ
    null, // vault 外の file: の画像は読まない
    "asset://localhost//Users/me/vault/img/c%20d.png",
    "asset://localhost//Users/me/vault/img/e.png",
    null, // root より上
  ]);
  // http(s) の画像は読まずにリンク表示（外部ビーコン禁止）
  const ext = [...box.querySelectorAll("a.wk-ext-media")].map((a) => navOf(a));
  assert.deepEqual(ext, [
    { t: "ext", url: "https://e.org/beacon.png" },
    { t: "ext", url: "https://e.org/a.mp3" },
    { t: "ext", url: "https://e.org/v.mp4" },
  ]);
  assert.equal(box.querySelector("video").getAttribute("src"), "asset://localhost//Users/me/vault/notes/v.mp4");
  assert.equal(box.querySelectorAll("video").length, 1);
  assert.equal(box.querySelector("audio"), null);
  const navs = [...box.querySelectorAll("a:not(.wk-ext-media)")].map((a) => navOf(a));
  assert.deepEqual(navs, [
    { t: "item", key: "ABCD2345", page: 3, att: "ATT1" },
    { t: "page", path: "notes/x.md", heading: "h" },
    { t: "open", abs: "/Users/me/Desktop/a b.pdf" },
    { t: "ext", url: "mailto:a@example.org" },
    { t: "ext", url: "https://example.org/a_(b)" },
  ]);
  // href は横取りの印（# だけ）・元の dest は data-wk-href
  for (const a of box.querySelectorAll("a")) assert.equal(a.getAttribute("href"), "#");
  // 画面の文書に入る前の src に http(s) が無い
  assert.ok(![...box.querySelectorAll("[src]")].some((e) => /^https?:/i.test(e.getAttribute("src"))));
  tests++;
}
{
  // markdown-it の validateLink の差し替え
  for (const ok of ["a.md", "../x/y.md", "#h", "https://a", "http://a", "mailto:a@b", "openref://open?key=K", "asset://localhost//a", "file:///a", "data:image/png;base64,AA", "C:\\x"]) {
    assert.ok(linkAllowed(ok), ok);
  }
  for (const ng of ["javascript:alert(1)", "vbscript:x", "data:text/html,x", "tel:1", "ftp://x", "chrome://x"]) assert.ok(!linkAllowed(ng), ng);
  // 参照フォルダの file:// がリンクになる（既定の validateLink は拒否する）
  const { box } = render("[教科書](file:///Users/me/ref/raw/和文の教科書.pdf)");
  assert.deepEqual(navOf(box.querySelector("a")), { t: "open", abs: "/Users/me/ref/raw/和文の教科書.pdf" });
  assert.deepEqual(parseOpenref("openref://open?key=K%20X"), { key: "K X", page: null, att: null });
  assert.ok(ALLOWED_TAGS.includes("details") && !ALLOWED_TAGS.includes("input"));
  tests++;
}

// ---- ② style ----
{
  const css = [
    `@import "https://e.org/a.css";`,
    `@import url(https://e.org/b.css) screen;`,
    `@\\69 mport "https://e.org/c.css";`,
    `@im\\port 'https://e.org/d.css';`,
    `@import/**/"https://e.org/e.css";`,
    `a { background: url(https://e.org/1.png) }`,
    `b { background: \\75 rl(https://e.org/2.png) }`,
    `c { background: \\000075\\000072\\00006c(https://e.org/3.png) }`,
    `d { background: u\\rl("https://e.org/4.png") }`,
    `e { background: \\5c 75rl(https://e.org/5.png) }`,
    `f { background: URL( 'https://e.org/6.png' ) }`,
    `g { background: url(//e.org/7.png) }`,
    `h { background: image-set("https://e.org/8.png" 1x) }`,
    `i { background: -webkit-image-set("https://e.org/9.png" 1x) }`,
    `@font-face { font-family: x; src: url(https://e.org/f.woff) }`,
    `:host { position: fixed; inset: 0 }`,
    `:host(.x) { display: block } :host-context(body) { color: red }`,
    `@media screen { :host { position: fixed } p { color: blue } }`,
    `j { background: url(img/ok.png) }`,
    `k { background: url("asset://localhost//Users/me/vault/ok.png") }`,
    `l { background: url(data:image/png;base64,AA) }`,
    `m { background: url(data:text/html,x) }`,
    `n { color: red }`,
    `o { background: url(https://e.org/unclosed.png`,
  ].join("\n");
  const rewrite = (v) => "asset://localhost//Users/me/vault/notes/" + v;
  const out = cleanStyleSheet(css, rewrite, (v) => assetInRoot("/Users/me/vault", v));
  assert.ok(!/e\.org/.test(out), `外部が残った:\n${out}`);
  assert.ok(!/@import/i.test(out), "@import が残った");
  assert.ok(!/:host/i.test(out), ":host が残った");
  assert.ok(!/image-set\(/i.test(out), "image-set が残った");
  assert.ok(!/\\/.test(out), "エスケープが残った");
  assert.ok(out.includes(`url("asset://localhost//Users/me/vault/notes/img/ok.png")`), out);
  assert.ok(out.includes(`url("asset://localhost//Users/me/vault/ok.png")`), out);
  assert.ok(out.includes(`url("data:image/png;base64,AA")`), out);
  assert.ok(/p \{ color: blue \}/.test(out) && /n \{ color: red \}/.test(out), "無害な規則まで消えた");
  assert.equal(decodeCss("\\75 rl("), "url(");
  assert.equal(decodeCss("\\5c 75rl("), "url(");
  // style 属性
  const attr = cleanStyleAttr(`color:red; background:\\75 rl(https://e.org/x.png); background-image: url(a.png)`, (v) => "asset://localhost//v/" + v);
  assert.ok(!/e\.org/.test(attr) && attr.includes(`url("asset://localhost//v/a.png")`) && attr.includes("color:red"), attr);
  // 描画の通しで
  const { box, html } = render(
    `<style>@import "https://e.org/a.css"; :host{position:fixed} .x{background:\\75 rl(https://e.org/z.png)}</style>\n\n<div style="background:url(https://e.org/s.png);color:red">s</div>`,
  );
  assert.ok(!/e\.org/.test(html) && !/:host/.test(box.querySelector("style").textContent), html);
  assert.equal(box.querySelector("div").getAttribute("style").includes("color:red"), true);
  tests++;
}

// ---- ③ asset URL ----
{
  assert.equal(assetUrl("/Users/me/vault/a.png"), "asset://localhost//Users/me/vault/a.png");
  assert.equal(
    assetUrl("/Users/me/参考 コピー/画像: 1#2?3%4.png"),
    "asset://localhost//Users/me/%E5%8F%82%E8%80%83%20%E3%82%B3%E3%83%94%E3%83%BC/%E7%94%BB%E5%83%8F%3A%201%232%3F3%254.png",
  );
  // 区画ごとに decode すると元の絶対パスに戻る（asset.rs は先頭 1 byte を落として decode）
  const p = "/Users/me/和文 : #?%/x y.mp4";
  assert.equal(decodeURIComponent(assetUrl(p).slice("asset://localhost/".length)), p);
  assert.equal(absOf("/v/", "/a/b.md"), "/v/a/b.md");
  assert.equal(resolveRelative("notes/a.md", "../img/x%20y.png#h"), "img/x y.png");
  assert.equal(resolveRelative("notes/a.md", "/img/x.png"), "img/x.png");
  assert.equal(resolveRelative("a.md", "../x.png"), null);
  assert.equal(resolveRelative("notes/a.md", "caf%C3%A9.md"), "notes/café.md");
  assert.equal(resolveRelative("notes/a.md", "cafe\u0301.md"), "notes/café.md"); // NFC
  assert.equal(filePath("file:///Users/me/a%20b.pdf"), "/Users/me/a b.pdf");
  assert.equal(filePath("file://localhost/x.pdf"), "/x.pdf");
  assert.equal(relInRoot("/Users/me/vault", "/Users/me/vault/a/b.md"), "a/b.md");
  assert.equal(relInRoot("/Users/me/vault", "/Users/me/vault2/a.md"), null);
  assert.equal(relInRoot("/Users/me/vault", "/Users/me/vault/../x.md"), null);
  assert.equal(normalizeRel("a/./b/../c"), "a/c");
  assert.deepEqual(["a.PNG", "b.mov", "c.m4a", "d.pdf", "e.md", "f.html", "g.zip"].map(viewKindOf), ["image", "video", "audio", "pdf", "page", "page", "other"]);
  tests++;
}

// ---- dirOf / baseName は寄せる前の写し（wiki.svelte.ts と asset.ts の dirOf・WikiPage / WikiSidebar / WikiDialogs の basename）と同じ出力 ----
{
  const wikiDirOf = (p) => (p && p.includes("/") ? p.slice(0, p.lastIndexOf("/")) : "");
  const assetDirOf = (rel) => {
    const i = rel.lastIndexOf("/");
    return i < 0 ? "" : rel.slice(0, i);
  };
  const oldBase = (p) => p.split("/").pop() ?? p;
  const inputs = ["", "/", "//", "a", "a/", "a//", "/a", "a/b", "a/b/", "a/b/c.md", "a//b", ".hidden", "a/.hidden", "./a", "../a/b", "/Users/me/vault/x.pdf", "ノート/所見.md", "a b/c d.html", "a\\b", "https://x/y/", ".."];
  for (const p of inputs) {
    assert.equal(dirOf(p), wikiDirOf(p), `dirOf(${JSON.stringify(p)}) が wiki の写しと違う`);
    assert.equal(dirOf(p), assetDirOf(p), `dirOf(${JSON.stringify(p)}) が asset の写しと違う`);
    assert.equal(baseName(p), oldBase(p), `baseName(${JSON.stringify(p)}) が写しと違う`);
  }
  for (const p of [null, undefined]) assert.equal(dirOf(p), wikiDirOf(p));
  assert.equal(dirOf("a/b/c.md"), "a/b");
  assert.equal(baseName("a/b/c.md"), "c.md");
  assert.equal(baseName("a/"), "");
  tests++;
}

// ---- ④ 表・見出し・コメント・埋め込み・callout ----
{
  const rows = (n) => "| a | b |\n|---|---|\n" + Array.from({ length: n }, (_, i) => `| ${i} | x |`).join("\n");
  const { box } = render(rows(250) + "\n\n" + rows(2500));
  const [t1, t2] = box.querySelectorAll("table");
  assert.ok(t1.classList.contains("wk-tbl-big"));
  assert.equal(t1.querySelectorAll("tbody tr").length, 250);
  assert.equal(t2.querySelectorAll("tbody tr").length, 2000);
  const more = box.querySelector("button.wk-tbl-more");
  assert.equal(more.textContent, "more 500");
  more.click();
  assert.equal(t2.querySelectorAll("tbody tr").length, 2500);
  assert.equal(box.querySelector("button.wk-tbl-more"), null);
  tests++;
}
{
  const { res } = render("# A\n## B c\n## B c\n### `x` y\n");
  assert.deepEqual(res.headings, [
    { level: 1, text: "A", id: "h-A" },
    { level: 2, text: "B c", id: "h-B-c" },
    { level: 2, text: "B c", id: "h-B-c-1" },
    { level: 3, text: "x y", id: "h-x-y" },
  ]);
  // 閉じない <!-- が以後のページを呑まない
  const { box } = render("<!-- a --> b <!--\n\n# after\n\ntext");
  assert.ok(box.querySelector("h1")?.textContent === "after", box.innerHTML);
  assert.equal(frontmatterEnd("---\na: 1\n---\nbody"), 13);
  assert.equal(frontmatterEnd("--- \nx"), 0);
  tests++;
}
{
  const links = [
    { kind: "embed", target: "pic.png", path: "img/pic.png", ambiguous: false },
    { kind: "embed", target: "v.mp4", path: "media/v.mp4", ambiguous: false },
    { kind: "embed", target: "doc.pdf", path: "docs/doc.pdf", ambiguous: false },
    { kind: "embed", target: "other", path: "other.md", ambiguous: false },
    { kind: "embed", target: "gone.png", path: null, ambiguous: false },
  ];
  const { box, res } = render("![[pic.png|300]] ![[v.mp4]] ![[doc.pdf]] ![[other]] ![[gone.png]]", { links });
  assert.equal(box.querySelector(".wk-media-image img").getAttribute("src"), "asset://localhost//Users/me/vault/img/pic.png");
  assert.equal(box.querySelector(".wk-media-image img").style.width, "300px");
  assert.equal(box.querySelector("video").getAttribute("src"), "asset://localhost//Users/me/vault/media/v.mp4");
  assert.equal(box.querySelector(".wk-media-video [data-wk-open]").getAttribute("data-wk-open"), "/Users/me/vault/media/v.mp4");
  assert.deepEqual(navOf(box.querySelector(".wk-embed-pdf")), { t: "page", path: "docs/doc.pdf", heading: null });
  assert.equal(res.pageEmbeds.length, 1);
  assert.equal(res.pageEmbeds[0].path, "other.md");
  assert.ok(box.querySelector(".wk-embed.wk-unresolved"));
  // 入れ子（埋め込みの中）ではページを展開しない
  const nested = render("![[other]]", { links, nested: true });
  assert.equal(nested.res.pageEmbeds.length, 0);
  assert.ok(nested.box.querySelector(".wk-embed-link"));
  tests++;
}
{
  const { box } = render("> [!warning]- 注意\n> 本文\n\n> [!unknown] x\n\n> [!INFO]\n> y");
  const [a, b, c] = box.querySelectorAll(".wk-callout");
  assert.equal(a.nodeName, "DETAILS");
  assert.ok(a.classList.contains("wk-callout-warning"));
  assert.equal(a.querySelector("summary").textContent, "注意");
  assert.ok(b.classList.contains("wk-callout-note"));
  assert.equal(c.querySelector(".wk-callout-title").textContent, "Info");
  assert.equal(calloutType("cite"), "quote");
  tests++;
}
{
  // html ページ: 相対は html 種で引く・<a href> の相対は内部遷移
  const links = [{ kind: "html", target: "../notes/page.md", path: "notes/page.md", ambiguous: false }];
  const { box } = renderHtml(`<h1>T</h1><a href="../notes/page.md">p</a><img src="../img/x.png"><a href="https://e.org">e</a><p style="position:fixed">f</p>`, { links });
  const [p, e] = box.querySelectorAll("a");
  assert.deepEqual(navOf(p), { t: "page", path: "notes/page.md", heading: null });
  assert.deepEqual(navOf(e), { t: "ext", url: "https://e.org" });
  assert.equal(box.querySelector("img").getAttribute("src"), "asset://localhost//Users/me/vault/img/x.png");
  tests++;
}

// ---- 掃除の規則の回帰 ----
{
  // 中身が PDF でない x.pdf は iframe に載せない（先頭 5 バイトの検査・Range で先頭だけ）
  const res = (bytes, status = 206) => new Response(new Uint8Array(bytes), { status });
  const enc = (s) => [...new TextEncoder().encode(s)];
  let sentRange = null;
  const f = (body, status) => async (_u, init) => ((sentRange = init?.headers?.Range ?? null), res(body, status));
  const kind = async (fn) => (await checkPdf("asset://x", fn)).kind;
  assert.equal(await kind(f(enc("%PDF-1.7\n..."))), "pdf");
  assert.equal(sentRange, "bytes=0-4");
  assert.equal(await kind(f(enc("%PDF-"), 200)), "pdf"); // Range が効かず 200 でも先頭で判定
  assert.equal(await kind(f(enc("<!doctype html><script>fetch('http://e')</script>"))), "other");
  assert.equal(await kind(f(enc("%PDF"))), "other"); // 短すぎる
  assert.equal(await kind(f(enc(" %PDF-1.4"))), "other"); // 先頭に空白
  // 中身が %PDF- でも応答が ok でなければ PDF にしない（読めなかった）
  assert.equal(await kind(f(enc("%PDF-1.4"), 404)), "unreadable");
  assert.equal(await kind(async () => { throw new Error("net"); }), "unreadable");
  tests++;
}
{
  // ワークスペースの外を指す asset: の src は外す・open に渡す絶対パスは `..` を畳む
  const { box } = render(
    `<img src="asset://localhost//etc/passwd"> <img src="asset://localhost/%2FUsers%2Fme%2Fvault%2Fa.png"> <img src="asset://localhost//Users/me/vault/../secret.png"> <img src="file:///Users/me/vault/x/../img/b.png">\n\n` +
      `[o](file:///Users/me/vault/../Desktop/a.pdf) [p](file:///Users/me/x/./y//z.pdf) [q](asset://localhost//etc/hosts) [r](file:///../../etc/x)`,
  );
  const srcs = [...box.querySelectorAll("img")].map((i) => i.getAttribute("src"));
  assert.deepEqual(srcs, [null, "asset://localhost//Users/me/vault/a.png", null, "asset://localhost//Users/me/vault/img/b.png"]);
  const navs = [...box.querySelectorAll("a")].map((a) => navOf(a));
  assert.deepEqual(navs, [
    { t: "open", abs: "/Users/me/Desktop/a.pdf" },
    { t: "open", abs: "/Users/me/x/y/z.pdf" },
    { t: "open", abs: "/etc/hosts" },
    { t: "none" }, // `/` より上
  ]);
  assert.equal(assetPath("asset://localhost//a/../b"), "/b");
  assert.equal(normalizeAbs("/a/../../b"), null);
  tests++;
}
{
  // @starting-style ほか入れ子を持つ at-rule・CSS Nesting の中の :host も落とす・接頭辞付きの関数も塞ぐ
  const css = [
    `@starting-style { :host { position: fixed } p { opacity: 0 } }`,
    `@container (min-width: 1px) { :host { position: fixed } }`,
    `@layer x { :host { position: fixed } }`,
    `@scope (.a) { :host { position: fixed } }`,
    `@supports (display: grid) { @media screen { :host(.x) { inset: 0 } } }`,
    `@document url-prefix() { :host { position: fixed } }`,
    `@unknown-rule { :host { position: fixed } }`,
    `p { color: red; :host & { position: fixed } & .b { color: blue } }`,
    `a { background: -webkit-cross-fade(url(x.png), "https://e.org/1.png", 50%) }`,
    `b { background: -moz-image-set("https://e.org/2.png" 1x) }`,
    `c { background: -webkit-image-set("https://e.org/3.png" 1x) }`,
  ].join("\n");
  const out = cleanStyleSheet(css);
  assert.ok(!/:host/i.test(out), out);
  assert.ok(!/e\.org/.test(out), out);
  assert.ok(/p \{ opacity: 0 \}/.test(out) && /& \.b \{ color: blue \}/.test(out), out);
  // 入れ子の深さの上限を超えたら <style> を丸ごと捨てる・深くても線形に近い時間
  const deep = "a{".repeat(MAX_CSS_DEPTH + 5) + "color:red" + "}".repeat(MAX_CSS_DEPTH + 5);
  assert.equal(cleanStyleSheet(deep), "");
  const ok = "a{".repeat(MAX_CSS_DEPTH) + "color:red" + "}".repeat(MAX_CSS_DEPTH);
  assert.ok(cleanStyleSheet(ok).includes("color:red"));
  const t0 = performance.now();
  assert.ok(!/:host/.test(cleanStyleSheet("@media x {".repeat(5000) + ":host{position:fixed}")));
  cleanStyleSheet(".x{color:red}\n".repeat(20000) + "@media a{".repeat(30) + ".y{color:blue}" + "}".repeat(30));
  assert.ok(performance.now() - t0 < 2000, `css の掃除が遅い: ${performance.now() - t0}ms`);
  tests++;
}
{
  // `![[ページ]]` の展開は 1 ページ 20 個まで（超えた分はリンク）
  const links = Array.from({ length: 25 }, (_, i) => ({ kind: "embed", target: `p${i}`, path: `p${i}.md`, ambiguous: false }));
  const { box, res } = render(links.map((l) => `![[${l.target}]]`).join("\n\n"), { links });
  assert.equal(MAX_PAGE_EMBEDS, 20);
  assert.equal(res.pageEmbeds.length, 20);
  assert.equal(box.querySelectorAll(".wk-embed-page").length, 20);
  assert.equal(box.querySelectorAll("a.wk-embed-link").length, 5);
  assert.deepEqual(navOf(box.querySelectorAll("a.wk-embed-link")[0]), { t: "page", path: "p20.md", heading: null });
  tests++;
}
// ---- ⑤ [toc]（目次 = 「/」メニューの「目次」が入れる） ----
{
  const { box, res } = render("[toc]\n\n# A\n\n## B 1\n\n### C\n\n## B 2\n\n[TOC] は段落の中では x\n");
  const toc = box.querySelector(".wk-toc");
  assert.ok(toc, "箱が残る");
  const items = Array.from(toc.querySelectorAll("li"));
  assert.deepEqual(items.map((li) => li.textContent), ["A", "B 1", "C", "B 2"], "見出しの順");
  assert.deepEqual(items.map((li) => li.getAttribute("style")), [null, "padding-left: 1.1em", "padding-left: 2.2em", "padding-left: 1.1em"], "最小の階層から相対に字下げ");
  const a = items[1].querySelector("a");
  assert.equal(a.getAttribute("href"), "#" + res.headings[1].id);
  assert.deepEqual(navOf(a), { t: "anchor", id: res.headings[1].id }, "同じページの見出しへの anchor（ShadowPage が scrollToAnchor）");
  assert.equal(res.headings.length, 4, "目次の中の文字は見出しに数えない");
  assert.ok(box.textContent.includes("[TOC] は段落の中では x"), "文の一部の [TOC] は触らない");
  // 見出しが無いページ・埋め込みの中（nested）では箱ごと消す
  assert.equal(render("[toc]\n\n本文だけ").box.querySelector(".wk-toc"), null);
  assert.equal(render("[toc]\n\n# A", { nested: true }).box.querySelector(".wk-toc"), null);
  // 大小は無視・前後の空白も可
  assert.ok(render("  [Toc]  \n\n# A").box.querySelector(".wk-toc"));
  tests++;
}
// ---- ⑥ 作者が書く data-wk-*: 遷移の印は finish だけが付ける（ALLOWED_ATTR に無い data-* は ALLOW_DATA_ATTR: false で消える） ----
{
  // finish が読む印（NAV_ATTRS + data-wk-href / src / target）を作者が全部書いても、purify の後には 1 つも残らない
  const NAV = ["data-wk-path", "data-wk-heading", "data-wk-anchor", "data-wk-item", "data-wk-page", "data-wk-att", "data-wk-ext", "data-wk-open", "data-wk-ref", "data-wk-href", "data-wk-src", "data-wk-target"];
  const authored =
    `<a href="a.md" data-wk-path="../../etc/x" data-wk-ext="https://e.org" data-wk-open="/etc/passwd" data-wk-ref="/x" data-wk-item="K" data-wk-anchor="h" data-wk-heading="y" data-wk-page="3" data-wk-att="A" data-wk-href="[[z]]" class="wk-l-open">a</a>` +
    `<span data-wk-path="x" data-wk-open="/etc/passwd" data-wk-src="https://e.org/b.png" data-wk-target="t" class="wk-media">s</span>` +
    `<img src="a.png" data-wk-src="https://e.org/x.png" data-wk-open="/etc/passwd">`;
  for (const [name, frag] of [
    ["md", purify(win, renderMarkdown(authored))],
    ["html", purify(win, authored)],
  ]) {
    for (const el of frag.querySelectorAll("*")) {
      for (const attr of NAV) assert.equal(el.getAttribute(attr), null, `${name}: 作者の ${attr} が残った: ${el.outerHTML}`);
    }
  }
  // finish の後の遷移先は href / src だけから決まる（作者の印は効かない）。class の wk-l-* は残るが navOf は data-wk-* で決める = none か href の分類
  {
    const { box } = render(authored); // md の相対: notes/page.md → notes/a.md
    assert.deepEqual(navOf(box.querySelector("a")), { t: "page", path: "notes/a.md", heading: null });
    assert.ok(!box.innerHTML.includes("e.org") && !box.innerHTML.includes("/etc/passwd"), box.innerHTML);
    assert.equal(box.querySelector("img").getAttribute("data-wk-src"), "a.png", "data-wk-src は finish が元の src から付け直す");
    assert.deepEqual(navOf(box.querySelector("span")), { t: "none" }, "class だけの span は遷移先なし");
  }
  {
    const { box } = renderHtml(authored); // html の相対: pages/p.html → pages/a.md
    assert.deepEqual(navOf(box.querySelector("a")), { t: "page", path: "pages/a.md", heading: null });
    assert.ok(!box.innerHTML.includes("e.org") && !box.innerHTML.includes("/etc/passwd"), box.innerHTML);
    assert.equal(box.querySelector("img").getAttribute("data-wk-src"), "a.png");
  }
  // 作者の class="wk-l-unresolved" は navOf を unresolved にする（遷移は起きない = 害なし）。固定しておく
  assert.deepEqual(navOf(renderHtml(`<a class="wk-l-unresolved">u</a>`).box.querySelector("a")), { t: "unresolved" });
  tests++;
}
{
  // 許可している data-wk-wiki / data-wk-embed / data-wk-size / data-wk-toc を作者が直接書いた場合 = md の [[…]] / ![[…]] / [toc] と同じ道を通る
  // （解決は索引 page.links だけ・href の javascript: は消える・索引に無い target は unresolved = vault の外を指せない）
  const links = [
    { kind: "wiki", target: "alpha", path: "notes/Alpha.md", ambiguous: false },
    { kind: "embed", target: "pic.png", path: "img/pic.png", ambiguous: false },
    { kind: "embed", target: "other", path: "other.md", ambiguous: false },
  ];
  const authored =
    `<a data-wk-wiki="alpha" href="javascript:alert(1)">w</a> <a data-wk-wiki="nope">n</a> <a data-wk-wiki="#h">anchor</a>` +
    `<span data-wk-embed="pic.png" data-wk-size="200">e</span> <span data-wk-embed="other">page</span>` +
    `<span data-wk-embed="../../etc/passwd">bad</span> <span data-wk-embed="/etc/passwd">abs</span> <span data-wk-embed="asset://localhost//etc/passwd">asset</span>` +
    `<div data-wk-toc="1">x</div><h1>T</h1>`;
  for (const [name, out] of [
    ["html", renderHtml(authored, { links })],
    ["md", render(authored.replace("<div data-wk-toc", "\n\n<div data-wk-toc").replace("<h1>T</h1>", "\n\n# T"), { links })],
  ]) {
    const { box, res } = out;
    const as = [...box.querySelectorAll("a")];
    assert.deepEqual(navOf(as[0]), { t: "page", path: "notes/Alpha.md", heading: null }, `${name}: data-wk-wiki は索引で解く`);
    assert.equal(as[0].getAttribute("href"), "#", `${name}: javascript: の href は # に`);
    assert.equal(as[0].getAttribute("data-wk-href"), "[[alpha]]");
    assert.deepEqual(navOf(as[1]), { t: "unresolved" }, `${name}: 索引に無い target`);
    assert.deepEqual(navOf(as[2]), { t: "anchor", id: "h" });
    const img = box.querySelector(".wk-media-image img");
    assert.equal(img.getAttribute("src"), "asset://localhost//Users/me/vault/img/pic.png", `${name}: data-wk-embed は ![[…]] と同じ展開`);
    assert.equal(img.style.width, "200px", `${name}: data-wk-size`);
    assert.equal(res.pageEmbeds.length, 1, `${name}: ページの埋め込みは索引の path`);
    assert.equal(res.pageEmbeds[0].path, "other.md");
    // 索引に無い（vault の外を狙う）target は未解決のまま = src も path も付かない
    const bad = [...box.querySelectorAll("span.wk-embed.wk-unresolved")].map((s) => s.getAttribute("data-wk-embed"));
    assert.deepEqual(bad, ["../../etc/passwd", "/etc/passwd", "asset://localhost//etc/passwd"], `${name}: ${box.innerHTML}`);
    // 外のパスは作者の data-wk-embed の文字として残るだけで、遷移の印（data-wk-path / open / ref）にも src にも出ない
    for (const e of box.querySelectorAll("*")) {
      for (const attr of ["data-wk-path", "data-wk-open", "data-wk-ref", "src"]) {
        assert.ok(!/etc\/passwd/.test(e.getAttribute(attr) ?? ""), `${name}: 外のパスが ${attr} に出た: ${e.outerHTML}`);
      }
    }
    // data-wk-toc は [toc] と同じ箱（中身は作者の文字を捨てて見出しの一覧）
    const toc = box.querySelector("[data-wk-toc]");
    assert.deepEqual([...toc.querySelectorAll("li a")].map((a) => [a.textContent, navOf(a)]), [["T", { t: "anchor", id: "h-T" }]], `${name}: toc`);
    assert.ok(!toc.textContent.includes("x"), `${name}: 作者の中身が残った`);
  }
  tests++;
}
// ---- ⑦ contain: layout paint: css.ts は position を消さない = 作者の position: fixed を箱に閉じ込めるのは host と外箱の contain だけ ----
{
  // コメント（/* … */ と // 行）を除いてから照合する = コメントアウトした宣言で通らない
  const src = (rel) => readFileSync(new URL(rel, import.meta.url), "utf8").replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
  const styles = src("./render/styles.ts");
  // adoptedStyleSheets の base（md / html 共通）: host と .wk-root の両方に contain（+ position: relative = fixed の基準になる）
  assert.match(styles, /^:host \{[^\n]*\bcontain: layout paint\b[^\n]*\bposition: relative\b/m, "styles.ts の :host に contain: layout paint が無い");
  assert.match(styles, /^\.wk-root \{[^\n]*\bposition: relative\b[^\n]*\bcontain: layout paint\b/m, "styles.ts の .wk-root に contain: layout paint が無い");
  // ShadowPage の外箱と host の要素（component の <style> = 2 重の閉じ込め）
  const page = src("./render/ShadowPage.svelte");
  assert.match(page, /^\s*\.wk-md-box \{[^\n]*\bcontain: layout paint\b[^\n]*\bposition: relative\b/m, "ShadowPage の .wk-md-box に contain が無い");
  assert.match(page, /^\s*\.wk-md \{[^\n]*\bcontain: layout paint\b[^\n]*\bposition: relative\b/m, "ShadowPage の .wk-md に contain が無い");
  assert.ok(/class="wk-md-box"/.test(page) && /class="wk-md"/.test(page), "ShadowPage の箱の要素が無い");
  // `contain:` の宣言はこの 4 か所だけ（増減したら壁の説明を見直す。contain-intrinsic-size は別物）
  const decls = (s) => (s.match(/\bcontain:/g) ?? []).length;
  assert.equal(decls(styles) + decls(page), 4, `contain: の宣言の数が変わった: styles.ts ${decls(styles)} + ShadowPage ${decls(page)}`);
  // 前提の確認: css.ts は作者の position を消さない（消すようになったらこの壁の説明が変わる = 気づけるように）
  assert.ok(cleanStyleSheet("p { position: fixed; inset: 0 }").includes("position: fixed"), "css.ts が position を消すようになった（contain のテストの前提が変わる）");
  assert.ok(cleanStyleAttr("position:fixed;inset:0", (v) => v).includes("position:fixed"));
  tests++;
}
// ---- ⑧ Finder の強調の持ち主: 中央のページ（A）が検索中に、チャットの応答・埋め込み（B）の描き直し（reset）や破棄（clear）が A の強調を消さない ----
{
  const reg = new Map(); // 偽の CSS.highlights（文書で 1 つ）
  globalThis.CSS = { highlights: reg };
  globalThis.Highlight = class {
    constructor(...r) {
      this.ranges = r;
    }
  };
  const hadNF = "NodeFilter" in globalThis;
  if (!hadNF) globalThis.NodeFilter = win.NodeFilter;
  const siv = win.HTMLElement.prototype.scrollIntoView;
  win.HTMLElement.prototype.scrollIntoView = () => {};
  const mk = (html) => {
    const d = win.document.createElement("div");
    d.innerHTML = html;
    win.document.body.append(d);
    return d;
  };
  const A = new Finder(mk("<p>alpha beta</p><p>alpha</p>"));
  const B = new Finder(mk("<p>alpha（応答）</p>"));
  assert.equal(A.search("alpha"), 2);
  assert.equal(reg.get("wk-find")?.ranges.length, 2, "強調を登録しない");
  assert.equal(reg.get("wk-find-cur")?.ranges[0], A.hits[0]);
  B.reset(); // 応答の描き直し
  B.clear(); // 応答の破棄
  assert.equal(reg.get("wk-find")?.ranges.length, 2, "別の Finder の reset / clear が A の強調を消した");
  assert.equal(reg.get("wk-find-cur")?.ranges[0], A.hits[0], "別の Finder の reset / clear が現在の 1 件を消した");
  A.step(1);
  assert.equal(reg.get("wk-find-cur")?.ranges[0], A.hits[1], "step の後も A が持ち主");
  // B が検索すれば持ち主は B（登録表は 1 つ）→ A の clear は B の強調を消さない・B の clear で消える
  assert.equal(B.search("alpha"), 1);
  assert.equal(reg.get("wk-find")?.ranges.length, 1);
  A.clear();
  assert.equal(reg.get("wk-find")?.ranges.length, 1, "持ち主でなくなった A の clear が B の強調を消した");
  assert.equal(A.hits.length, 0, "clear で自分の一致は捨てる");
  B.clear();
  assert.equal(reg.has("wk-find"), false, "持ち主の clear で消えない");
  assert.equal(reg.has("wk-find-cur"), false);
  // 持ち主が消した後の reset は無害（二重の delete をしない = Map の delete は冪等だが、持ち主が null に戻っていること）
  A.reset();
  assert.equal(reg.size, 0);
  // 登録表が無い環境（API なし）でも例外なく動く
  delete globalThis.CSS;
  delete globalThis.Highlight;
  assert.equal(A.search("beta"), 1);
  A.clear();
  if (!hadNF) delete globalThis.NodeFilter;
  win.HTMLElement.prototype.scrollIntoView = siv;
  tests++;
}

// ---- ⑨ 失敗を握りつぶさない・検査を 1 つに ----
{
  // checkPdf の 3 値: 読めなかったときは理由（HTTP の status・例外の文言）を返し、「PDF でない」と分ける
  const res = (text, status) => async () => new Response(text, { status });
  assert.deepEqual(await checkPdf("asset://x", res("%PDF-1.7\n", 206)), { kind: "pdf" });
  assert.deepEqual(await checkPdf("asset://x", res("<html>", 206)), { kind: "other" });
  assert.deepEqual(await checkPdf("asset://x", res("", 206)), { kind: "other" }); // 空のファイルは読めた・PDF でない
  assert.deepEqual(await checkPdf("asset://x", res("forbidden", 403)), { kind: "unreadable", reason: "HTTP 403" });
  assert.deepEqual(await checkPdf("asset://x", async () => { throw new TypeError("Load failed"); }), { kind: "unreadable", reason: "Load failed" });
  assert.deepEqual(await checkPdf("asset://x", async () => { throw "x"; }), { kind: "unreadable", reason: "x" });
  // 本文の読み出しの途中で throw しても読めなかった扱い（PDF にしない）
  const broken = async () => new Response(new ReadableStream({ pull(c) { c.error(new Error("io")); } }), { status: 206 });
  assert.deepEqual(await checkPdf("asset://x", broken), { kind: "unreadable", reason: "io" });
  tests++;
}
{
  // 0 バイトのファイル: 自前の配信は bytes=0-4 を満たせず 416（範囲外）を返す → 読めなかったでなく PDF でない
  assert.deepEqual(await checkPdf("asset://x", async () => new Response("", { status: 416 })), { kind: "other" });
  // 最初の塊で止める: 終わらない本文（Range が効かず全量が来る形）でも 5 バイト揃ったら読み止めて cancel する。
  // 読み続けると 50 回目の pull で error = unreadable になる（読み止めないと終わらないので、テストが止まらずに落ちる形にしておく）
  let pulls = 0;
  let cancelled = false;
  const endless = async () =>
    new Response(
      new ReadableStream({
        pull(c) {
          if (++pulls > 50) return c.error(new Error("read past the head"));
          c.enqueue(new TextEncoder().encode(pulls === 1 ? "%P" : pulls === 2 ? "DF" : "-1.7\n"));
        },
        cancel() {
          cancelled = true;
        },
      }, { highWaterMark: 0 }),
      { status: 200 },
    );
  assert.deepEqual(await checkPdf("asset://x", endless), { kind: "pdf" });
  await new Promise((r) => setTimeout(r, 0));
  assert.ok(pulls <= 4, `先頭の後も読み続けた: pull ${pulls} 回`);
  assert.ok(cancelled, "読み止めた後に cancel していない");
  tests++;
}
{
  // CSS の url(asset:) は img の src と同じく vault の中だけ（外・`..` で外へ出る物は none）。中の物は正規化して残す
  const root = "/Users/me/vault";
  const asset = (v) => assetInRoot(root, v);
  const IN = "asset://localhost//Users/me/vault/img/a.png";
  const OUT = "asset://localhost//etc/passwd";
  const UP = "asset://localhost//Users/me/vault/../secret.png";
  const ENC = "asset://localhost/%2FUsers%2Fme%2Fvault%2Fimg%2Fb.png"; // convertFileSrc の形
  const SIB = "asset://localhost//Users/me/vault2/x.png"; // 名前が vault で始まる兄弟のフォルダ
  const sheet = cleanStyleSheet(`a{background:url(${IN})} b{background:url("${OUT}")} c{background:url('${UP}')} d{background:url(${ENC})} e{background:url(${SIB})} @font-face{src:url(${OUT})}`, (v) => v, asset);
  assert.ok(sheet.includes(`a{background:url("${IN}")}`), sheet);
  assert.ok(sheet.includes(`d{background:url("asset://localhost//Users/me/vault/img/b.png")}`), sheet);
  for (const sel of ["b", "c", "e"]) assert.ok(sheet.includes(`${sel}{background:none}`), `${sel} が none にならない: ${sheet}`);
  assert.ok(sheet.includes("@font-face{src:none}"), sheet);
  assert.ok(!/etc\/passwd|secret|vault2/.test(sheet), sheet);
  // style 属性も同じ検査
  const attr = cleanStyleAttr(`background:url(${IN}); border-image:url(${OUT}); list-style-image:url(${UP})`, (v) => v, asset);
  assert.equal(attr, `background:url("${IN}"); border-image:none; list-style-image:none`);
  // 検査を渡さなければ asset: は全部 none（root を知らない呼び手が素通しにしない）
  assert.equal(cleanStyleAttr(`background:url(${IN})`), "background:none");
  assert.ok(cleanStyleSheet(`a{background:url(${IN})}`, (v) => v).includes("a{background:none}"));
  // CSS エスケープで書いた asset: も解いた後で同じ検査
  assert.equal(cleanStyleAttr(`background:\\75 rl(asset://localhost//etc/x)`, (v) => v, asset), "background:none");
  // 描画の通し（finish の <style> と style 属性）
  const { box } = renderHtml(`<style>.x{background:url(${IN})} .y{background:url(${OUT})} .z{background:url(${UP})}</style><div style="background:url(${OUT});color:red">s</div><p style="background:url(${IN})">p</p>`);
  const st = box.querySelector("style").textContent;
  assert.ok(st.includes(`.x{background:url("${IN}")}`) && st.includes(".y{background:none}") && st.includes(".z{background:none}"), st);
  assert.equal(box.querySelector("div").getAttribute("style"), "background:none;color:red");
  assert.equal(box.querySelector("p").getAttribute("style"), `background:url("${IN}")`);
  tests++;
}
{
  // link の stylesheet: 読めない物は console.warn に理由（href と error）を残して捨て、ほかの stylesheet と本文の描画は続ける。
  // 読めた CSS の url(asset:) も vault の中だけ（finish を通す前の 1 段目でも同じ検査）
  const root = "/Users/me/vault";
  const fetched = [];
  const fetchText = async (url) => {
    fetched.push(url);
    if (url.endsWith("/bad.css")) throw new Error("403");
    if (url.endsWith("/ok.css")) return `.a{color:red} .b{background:url(asset://localhost//etc/x)} .c{background:url(img/c.png)} .d{background:url(asset://localhost//Users/me/vault/d.png)}`;
    throw new Error("unexpected " + url);
  };
  const warned = [];
  const orig = console.warn;
  console.warn = (...a) => warned.push(a);
  let out;
  try {
    out = await prepareHtml(
      win,
      `<html><head><link rel="stylesheet" href="css/bad.css"><link rel="stylesheet" href="css/ok.css"><style>.s{color:blue}</style></head><body><p>本文</p></body></html>`,
      { root, path: "pages/p.html", links: [] },
      fetchText,
    );
  } finally {
    console.warn = orig;
  }
  assert.deepEqual(fetched, [assetUrl(root + "/pages/css/bad.css"), assetUrl(root + "/pages/css/ok.css")]);
  assert.equal(warned.length, 1, `warn の回数: ${JSON.stringify(warned)}`);
  assert.ok(warned[0].includes("css/bad.css") && warned[0].some((x) => x instanceof Error && x.message === "403"), `warn に href と error が無い: ${warned[0]}`);
  assert.ok(out.includes("<p>本文</p>"), out);
  assert.ok(out.includes(".s{color:blue}") && out.includes(".a{color:red}"), out);
  assert.ok(out.includes(".b{background:none}"), out);
  assert.ok(out.includes(`.c{background:url("${assetUrl(root + "/pages/css/img/c.png")}")}`), out);
  assert.ok(out.includes(`.d{background:url("${assetUrl(root + "/d.png")}")}`), out);
  // 全部読めたら warn は出ない
  console.warn = (...a) => warned.push(a);
  try {
    await prepareHtml(win, `<link rel="stylesheet" href="css/ok.css"><p>x</p>`, { root, path: "pages/p.html", links: [] }, fetchText);
  } finally {
    console.warn = orig;
  }
  assert.equal(warned.length, 1);
  tests++;
}

// ---- ⑩ 見出しの照合: 着地・埋め込みの節・同じページの #… が同じ規則 ----
function shadowRender(md) {
  const frag = purify(win, renderMarkdown(md));
  const res = finish(frag, ctx());
  const box = win.document.createElement("div").attachShadow({ mode: "open" });
  box.append(frag);
  return { box, res };
}
{
  // [書いた見出し, ページの見出し, 一致するか]。記号（: # | ^ [ ] \ / % * " < > ?）と `-` は空白・空白の連なりは 1 つ・大小無視・NFC。全角は畳まない
  const PAIRS = [
    ["A B", "A: B", true],
    ["a b", "A: B", true],
    ["A B", "A B", true],
    ["a-b", "A B", true],
    ["A-B", "A-B", true],
    ["A B", "A-B", true],
    ["A B", "A - B", true],
    ["A/B", "A B", true],
    ["A B", "[A] B?", true],
    ["Q&A", "Q&A", true],
    ["a  b", "A B", true],
    ["A\u3000B", "A B", true],
    [" A B ", "A B", true],
    ["ct 所見", "CT 所見", true],
    ["ガ", "カ\u3099", true],
    ["Ａ", "A", false],
    ["A：B", "A B", false],
    ["A.B", "A B", false],
    ["A_B", "A B", false],
    ["AB", "A B", false],
  ].map(([w, h, ok]) => [JSON.parse(`"${w}"`), JSON.parse(`"${h}"`), ok]);
  for (const [want, text, ok] of PAIRS) {
    const tag = `${JSON.stringify(want)} ⇔ ${JSON.stringify(text)}`;
    assert.equal(headingKey(want) === headingKey(text), ok, `headingKey: ${tag}`);
    // 埋め込みの節: その見出しの節だけ（前と次の同じ階層の見出しは残らない）・一致しなければ全体のまま
    const frag = purify(win, renderMarkdown(`# 前\n\nx\n\n## ${text}\n\n本文\n\n## 次\n\ny`));
    finish(frag, ctx({ nested: true }));
    keepSection(frag, want);
    const hs = Array.from(frag.children).filter((k) => /^H/.test(k.nodeName)).map((k) => k.textContent);
    assert.deepEqual(hs, ok ? [text.trim()] : ["前", text.trim(), "次"], `keepSection: ${tag}`);
    // 別のページからの着地（finish の見出しの一覧）と同じページの #…（ShadowPage と同じく shadow root の中）
    const { box, res } = shadowRender(`# 前\n\n## ${text}\n\n## 次`);
    const h = findHeading(res.headings, want);
    assert.equal(h?.text ?? null, ok ? text.trim() : null, `findHeading: ${tag}`);
    const el = anchorTarget(box, want);
    assert.equal(el?.textContent ?? null, ok ? text.trim() : null, `anchorTarget: ${tag}`);
    if (ok) assert.equal(el.id, h.id, `着地の id: ${tag}`);
  }
  // 代表的な組: keepSection("A B") ⇔ 見出し「A: B」の節（本文が残り、次の見出しは消える）
  {
    const frag = purify(win, renderMarkdown("# 表題\n\n## A: B\n\n節の本文\n\n## C\n\n別"));
    finish(frag, ctx({ nested: true }));
    keepSection(frag, "A B");
    assert.deepEqual(Array.from(frag.children).map((k) => [k.nodeName, k.textContent]), [["H2", "A: B"], ["P", "節の本文"]]);
  }
  // 目次のクリック（data-wk-anchor = 見出しの id）と `[[p#見出し]]` の着地（findHeading）と同じページの `[[#見出し]]`（anchorTarget）が同じ id
  {
    const { box, res } = shadowRender("[toc]\n\n# 表題\n\n## A: B\n\n## 所見 - CT\n\n## A: B\n\n[[#A B]] [[#所見 CT]]");
    const toc = Array.from(box.querySelectorAll(".wk-toc a")).map((a) => [a.textContent, a.getAttribute("data-wk-anchor")]);
    assert.equal(toc.length, 4);
    for (const [text, id] of toc) {
      const first = res.headings.find((x) => x.text === text);
      assert.equal(findHeading(res.headings, text).id, first.id, `目次と着地: ${text}`);
      assert.equal(findHeading(res.headings, id).id, id, `id の完全一致が先: ${id}`);
      assert.equal(anchorTarget(box, id).id, id, `目次のクリック: ${id}`);
    }
    assert.equal(findHeading(res.headings, "a b").id, toc[1][1], "同じ見出しが 2 つなら最初");
    assert.equal(findHeading(res.headings, "所見-ct").id, toc[2][1]);
    const anchors = Array.from(box.querySelectorAll("a.wk-l-anchor")).filter((a) => !a.closest(".wk-toc")).map((a) => a.getAttribute("data-wk-anchor"));
    assert.deepEqual(anchors.map((a) => anchorTarget(box, a).id), [toc[1][1], toc[2][1]], "[[#…]] の着地");
  }
  // 厳密な鍵が先: 記号の違いで書き分けた見出しは書いたとおりの方へ（文書順で前の緩い一致に取られない）・記号だけの見出しも引ける・
  // 緩い鍵が空（記号だけ）で厳密にも一致しなければ何にも一致しない。着地（findHeading）・同じページ（anchorTarget）・埋め込みの節（keepSection）が同じ見出し
  for (const [heads, want, land] of [
    [["A-B", "A B"], "A B", "A B"],
    [["A B", "A-B"], "A-B", "A-B"],
    [["T-1", "T1", "T 1"], "T 1", "T 1"],
    [["CT:", "CT"], "CT", "CT"],
    [["A B", "A: B"], "A: B", "A: B"],
    [["A B", "A: B"], "a  b", "A B"],
    [["x", "?", "*"], "?", "?"],
    [["x", "?", "*"], "*", "*"],
    [["x", "?", "*"], "[", null],
    [["x", "?"], "", null],
    [["・"], "・", "・"],
    [["---x"], "---x", "---x"],
  ]) {
    const md = heads.map((h, i) => `## ${h}\n\nbody${i}`).join("\n\n");
    const tag = `${JSON.stringify(heads)} ← ${JSON.stringify(want)}`;
    const { box, res } = shadowRender(md);
    assert.equal(findHeading(res.headings, want)?.text ?? null, land, `findHeading: ${tag}`);
    assert.equal(anchorTarget(box, want)?.textContent ?? null, land, `anchorTarget: ${tag}`);
    const frag = purify(win, renderMarkdown(md));
    finish(frag, ctx({ nested: true }));
    keepSection(frag, want);
    const kept = Array.from(frag.children).map((k) => k.textContent);
    assert.deepEqual(kept, land == null ? heads.flatMap((h, i) => [h, `body${i}`]) : [land, `body${heads.indexOf(land)}`], `keepSection: ${tag}`);
  }
  tests++;
}

console.log(`ok: wiki render (${tests} tests)`);
