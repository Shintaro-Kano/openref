/* 描画が拾うリンクと core の索引が拾うリンクの一致（node app/src/lib/wiki/links.test.mjs・Node 22.18+ の type stripping）。
   core と同じ fixture crates/openref-core/tests/fixtures/wiki-links.json を読み、md を描画（markdown-it → DOMPurify → finish）した
   DOM から出現順の (kind, target) を取り出して fixture の links と比べる。
   描画と索引は別の解析器なので、一致しない既知の境界を KNOWN に理由付きで置く（その case は「一致しないこと」も確かめる =
   どちらかが直って一致したら KNOWN から外す）。resolve の case は描画側の相対の解決（索引が無いときの fallback）を確かめる。
   チャットの応答の `[[…]]`（chat.ts の chatLinks）は描画と同じ走査 = fixture の全 case と EXTRA で、描画の DOM が拾う wiki / embed と同じ集合 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { JSDOM } from "jsdom";
import { renderMarkdown } from "./render/md.ts";
import { purify, finish, collectLinks, classifyDest } from "./render/sanitize.ts";
import { chatLinks } from "./chat.ts";

const here = dirname(fileURLToPath(import.meta.url));
const fx = JSON.parse(readFileSync(join(here, "../../../../crates/openref-core/tests/fixtures/wiki-links.json"), "utf8"));
const win = new JSDOM("").window;
const labels = { openExternal: "o", playFailed: "f", moreRows: (n) => `${n}`, externalImage: "img", unresolved: "u", pdf: "pdf" };

/** 一致しない既知の境界: case 名 → 理由 */
const KNOWN = {
  "frontmatter は文字列の値の [[…]] だけ（配列・入れ子も・先に数える）。md リンクは取らない":
    "frontmatter は描画しない（右ペインのページ情報に出す）= 本文の [[yes]] だけを拾う。索引は frontmatter の [[…]] もバックリンクに数える",
  "HTML コメントの中は抽出しない（複数行も）":
    "CommonMark の HTML ブロック（type 2）は `-->` を含む行で終わる = 1 行目の末尾の閉じない `<!--` は描画が閉じ（以後を呑まない）、2 行目の [[no2]] は段落 = リンクになる。索引は複数行のコメントの中を除く",
};

let n = 0;
const diffs = [];
for (const c of fx.md) {
  const frag = purify(win, renderMarkdown(c.md));
  finish(frag, { root: "/vault", path: "index.md", kind: "md", links: [], labels });
  const got = collectLinks(frag);
  const want = c.links.map((l) => ({ kind: l.kind, target: l.target }));
  if (KNOWN[c.name]) {
    assert.notDeepEqual(got, want, `KNOWN の case が一致した（KNOWN から外す）: ${c.name}`);
    diffs.push({ name: c.name, got, want, why: KNOWN[c.name] });
  } else {
    assert.deepEqual(got, want, c.name);
  }
  n++;
}

// resolve: 索引（page.links）が無いときの描画側の相対の解決 = core の「書いたファイルからの相対」だけ（basename の探索は索引の仕事）
const ctxOf = (src, kind) => ({ root: fx.resolve.root, path: src, kind, links: [], labels });
let r = 0;
for (const c of fx.resolve.cases) {
  if (c.kind !== "md" && c.kind !== "html" && c.kind !== "file") continue;
  const nav = classifyDest(c.target, ctxOf(c.src, c.kind === "file" ? "md" : c.kind), new Map());
  const rel = nav.t === "page" ? nav.path : null;
  // 相対で直接たどれる物は core と同じ path、たどれない物（basename の探索・vault の外）は null か別の path = 索引の path を使う
  const direct = c.path && fx.resolve.files.includes(rel);
  if (direct) assert.equal(rel, c.path, `${c.src} → ${c.target}`);
  else if (c.kind === "file") assert.equal(nav.t, c.path ? "page" : "open", `${c.src} → ${c.target}`);
  r++;
}

// 索引の path がある場合は必ずそれを使う（未解決は見た目で区別）
{
  const links = [
    { kind: "wiki", target: "alpha", path: "notes/Alpha.md", ambiguous: false },
    { kind: "wiki", target: "missing", path: null, ambiguous: false },
    { kind: "md", target: "gamma.md", path: "other/gamma.md", ambiguous: false },
    { kind: "embed", target: "pic.jpg", path: "img/pic.JPG", ambiguous: false },
  ];
  const frag = purify(win, renderMarkdown("[[alpha]] [[missing]] [g](gamma.md) ![[pic.jpg]] [[ALPHA#h]]"));
  finish(frag, { root: "/vault", path: "notes/x.md", kind: "md", links, labels });
  const as = frag.querySelectorAll("a");
  assert.equal(as[0].getAttribute("data-wk-path"), "notes/Alpha.md");
  assert.ok(as[1].classList.contains("wk-l-unresolved"));
  assert.equal(as[1].getAttribute("data-wk-path"), null);
  assert.equal(as[2].getAttribute("data-wk-path"), "other/gamma.md"); // 相対では notes/gamma.md になるが索引に従う
  const img = frag.querySelector(".wk-media img");
  assert.equal(img.getAttribute("src"), "asset://localhost//vault/img/pic.JPG");
  // 索引に無い target（大小違い）は未解決（索引の解決だけに従う・推測しない）
  assert.ok(as[3].classList.contains("wk-l-unresolved"));
}

// 参照フォルダ: ワークスペースの外の file:// は、参照フォルダの中で内蔵で開ける種類（pdf / html / 画像 / 動画 / 音声）だけ ref・
// それ以外（md・参照フォルダの外・隠しの区画・フォルダそのもの）は open のまま。vault の中は今どおり page
{
  const ctx = { root: "/vault", path: "notes/x.md", kind: "md", links: [], labels, refs: ["/Users/u/kb"] };
  const nav = (d) => classifyDest(d, ctx, new Map());
  assert.deepEqual(nav("file:///Users/u/kb/journals/a.pdf"), { t: "ref", abs: "/Users/u/kb/journals/a.pdf" });
  assert.deepEqual(nav("file:///Users/u/kb/posts/view.html"), { t: "ref", abs: "/Users/u/kb/posts/view.html" });
  assert.deepEqual(nav("file:///Users/u/kb/notes-web/p.png"), { t: "ref", abs: "/Users/u/kb/notes-web/p.png" });
  assert.deepEqual(nav("file:///Users/u/kb/a/../journals/b.PDF"), { t: "ref", abs: "/Users/u/kb/journals/b.PDF" });
  assert.deepEqual(nav("file:///Users/u/kb/notes/n.md"), { t: "open", abs: "/Users/u/kb/notes/n.md" }, "md は索引が無い = ホストの open");
  assert.deepEqual(nav("file:///Users/u/other/a.pdf"), { t: "open", abs: "/Users/u/other/a.pdf" }, "参照フォルダの外");
  assert.deepEqual(nav("file:///Users/u/kb/.git/a.pdf"), { t: "open", abs: "/Users/u/kb/.git/a.pdf" }, "隠しの区画");
  assert.deepEqual(nav("file:///Users/u/kb2/a.pdf"), { t: "open", abs: "/Users/u/kb2/a.pdf" }, "接頭辞だけ一致");
  assert.deepEqual(nav("file:///Users/u/kb"), { t: "open", abs: "/Users/u/kb" }, "フォルダそのもの");
  assert.deepEqual(nav("file:///vault/a.pdf"), { t: "page", path: "a.pdf", heading: null }, "vault の中は page");
  assert.deepEqual(classifyDest("file:///Users/u/kb/a.pdf", { ...ctx, refs: undefined }, new Map()), { t: "open", abs: "/Users/u/kb/a.pdf" }, "参照フォルダなし");
  // 参照フォルダのページ（root = そのフォルダ）の中の相対リンクは root 相対の page（WikiPage が絶対に戻す）
  const inRef = { ...ctx, root: "/Users/u/kb", path: "posts/x/view.html", kind: "html" };
  assert.deepEqual(classifyDest("../y/view.html", inRef, new Map()), { t: "page", path: "posts/y/view.html", heading: null });
  // 描いた <a> から ref を読み戻せる
  const frag = purify(win, renderMarkdown("[p](file:///Users/u/kb/journals/a.pdf)"));
  finish(frag, ctx);
  const a = frag.querySelector("a");
  assert.equal(a.getAttribute("data-wk-ref"), "/Users/u/kb/journals/a.pdf");
  assert.ok(a.classList.contains("wk-l-ref"));
}

// chatLinks と描画: 同じ md で同じ wiki / embed の集合（出現順・重複を除く）。名前の解決（chatLinks の path）はここでは見ない
const EXTRA = [
  "\\[[esc]]",
  "[[a\\]] [[b\\\\]] [[c\\|x]]",
  "[[a|b]] [[a#h]] ![[a]] ![[a|300x200]]",
  "[[a]b]] [[x [[y]] [[z\n]]",
  "`[[no]]` <code>[[il]]</code>\n\n```\n[[no2]]\n```",
  '<div title="[[no]]">[[yes]] <code>[[no2]]</code> <pre>[[no3]]</pre> <a href="u">[[no4]]</a></div>',
  "<div>\n![[e]] <!-- [[c]] --> [[z|q]]\n</div>",
  "[^1]\n\n[^1]: [[fn]]",
  "[l [[in]]](u.md)",
];
let m = 0;
const uniq = (l) => [...new Set(l.map((x) => `${x.kind}:${x.target}`))];
for (const md of [...fx.md.map((c) => c.md), ...EXTRA]) {
  const frag = purify(win, renderMarkdown(md));
  finish(frag, { root: "/vault", path: "index.md", kind: "md", links: [], labels });
  const want = uniq(collectLinks(frag).filter((l) => l.kind === "wiki" || l.kind === "embed"));
  assert.deepEqual(uniq(chatLinks(md, [])), want, `chatLinks ≠ 描画: ${JSON.stringify(md)}`);
  m++;
}
// HTML ブロックの中: 期待を固定（両側が同じ向きに変わっても落ちる）= `![[…]]` は embed・textarea の中は拾う（DOMPurify が中身を text に残す）・
// `<a-b>` `<code-block>` は a / code ではない・中身の無いコメント `<!-->` `<!--->` は閉じている
for (const [md, want] of [
  ["<div>\n![[e]] <!-- [[c]] --> [[z|q]]\n</div>", ["embed:e", "wiki:z"]],
  ["<div><textarea>[[t]]</textarea> [[u]]</div>", ["wiki:t", "wiki:u"]],
  ["<div><a-b>[[x]]</a-b> <code-block>[[y]]</code-block> <code>[[no]]</code> [[z]]</div>", ["wiki:x", "wiki:y", "wiki:z"]],
  ["<div><!-->[[c1]] <!--->[[c2]] <!-- [[no]] --></div>", ["wiki:c1", "wiki:c2"]],
]) {
  const frag = purify(win, renderMarkdown(md));
  finish(frag, { root: "/vault", path: "index.md", kind: "md", links: [], labels });
  assert.deepEqual(uniq(collectLinks(frag)), want, `描画: ${md}`);
  assert.deepEqual(uniq(chatLinks(md, [])), want, `chatLinks: ${md}`);
  m++;
}
// 末尾の `\` は全部落とす（core と描画と同じ）・同じ target は 1 つ
assert.deepEqual(chatLinks("[[a\\\\]] [[a]] [[b\\|x]]", [{ path: "a.md" }]).map((l) => [l.target, l.path]), [["a", "a.md"], ["b", null]]);

console.log(`ok: wiki links (md ${n} cases・KNOWN ${diffs.length}・resolve ${r} cases・chat ${m} cases)`);
if (process.argv.includes("--list")) console.log(JSON.stringify(diffs, null, 1));
