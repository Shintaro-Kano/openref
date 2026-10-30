/* util.ts の純関数を node で固定する（依存なし・`node app/src/lib/viewer/util.test.mjs`）。
   **Node 22.18+ が要る**: .ts を直接 import し、Node の type stripping（型注釈を消すだけ・22.18+ / 23.6+ で既定）に依存する。
   util.ts は型を消すだけで動く TS（enum・namespace・parameter property を使わない）であることが前提。
   入力は手組みの小さな item 列。PdfViewer.svelte から移した ブロック分割・ブロック規則（bodySpans）・柱検出・読み順・本文組立・索引 の既存の挙動と、
   その閾値（行送り 0.5 / 2.2・文字高比 0.88 / 1.14・本文比 0.95 / 0.97・同行 0.45・meta 4 行・読み順 8 item・柱 2 ページ）を固定する。
   あわせて 文書内検索の出現列・ハイライト矩形（fragmentRects）・注釈の色・選択と注釈 quote の照合（squash 空間 = squashOrigSpan / quoteSpans）・
   注釈タブの検索（matchParts = 絞り込みと強調が同じ一致）を固定する。 */
import assert from "node:assert/strict";
import {
  makePageText,
  itemsFull,
  applyReadingOrder,
  itemIndexAt,
  splitBlocks,
  majorLargestSize,
  bodyFontPx,
  runningKey,
  runningTable,
  runningIndexes,
  bodyTextOf,
  buildGeom,
  transform,
  allMatchIndexes,
  makeGeomItem,
  fragmentRects,
  colorForAnnotation,
  normalizedColor,
  isClaudeAuthor,
  normText,
  matchParts,
  squashOrigSpan,
  quoteSpans,
  bodySpans,
  fitScale,
  AUTO_MAX,
} from "./util.ts";

let n = 0;
function test(name, fn) {
  try {
    fn();
    n++;
    console.log("ok", n, name);
  } catch (e) {
    console.log("FAIL", name);
    throw e;
  }
}

/** pdf.js の TextItem もどき（transform = [a b c d e f]・fontHeight = hypot(c, d)） */
function item(str, x, y, size, opts = {}) {
  return { str, transform: [size, 0, 0, size, x, y], width: opts.width ?? str.length * size * 0.5, height: size, hasEOL: opts.hasEOL ?? false, fontName: "f1" };
}

/** 回転なし・高さ 800 のページの viewport（pdf.js の getViewport({scale}) と同じ transform = [s 0 0 -s 0 800·s]・y を反転して下向き正に） */
function viewportOf(scale = 1) {
  return { transform: [scale, 0, 0, -scale, 0, 800 * scale], scale };
}

/** item 列 → 索引（makePageText）→ 幾何（buildGeom）。PdfViewer.ensureRendered と同じ経路 */
function geomOf(items, scale = 1, styles) {
  return buildGeom(makePageText({ items, styles }), viewportOf(scale));
}

// ---------- 索引 ----------
test("transform: pdfjs Util.transform と同じ式（実物で取った値）", () => {
  assert.deepEqual(transform([1.5, 0, 0, -1.5, 0, 1200], [1, 2, 3, 4, 5, 6]), [1.5, -3, 4.5, -6, 7.5, 1191]);
});

test("buildGeom: 同じ item 列から作った索引（makePageText）と幾何が一致する（本物の pdf.js に近い item 列）", () => {
  // pdf.js v6 の textContent: 行末に str "" + hasEOL の item が入る・styles は fontName → {fontFamily, ascent, descent}
  const styles = { g_d0_f1: { fontFamily: "serif", ascent: 0.9, descent: -0.22 }, g_d0_f2: { fontFamily: "sans-serif", ascent: 1.4, descent: -0.8 } };
  const items = [
    { str: "Prostate cancer is", dir: "ltr", width: 90, height: 10, transform: [10, 0, 0, 10, 72, 700], fontName: "g_d0_f1", hasEOL: false },
    { str: "", dir: "ltr", width: 0, height: 0, transform: [10, 0, 0, 10, 162, 700], fontName: "g_d0_f1", hasEOL: true },
    { str: " ", dir: "ltr", width: 2.5, height: 10, transform: [10, 0, 0, 10, 162, 700], fontName: "g_d0_f1", hasEOL: false },
    { str: "the most common", dir: "ltr", width: 80, height: 10, transform: [10, 0, 0, 10, 72, 688], fontName: "g_d0_f1", hasEOL: true },
    { str: "12", dir: "ltr", width: 8, height: 6, transform: [6, 0, 0, 6, 152, 692], fontName: "g_d0_f2", hasEOL: false },
    { str: "", dir: "ltr", width: 0, height: 0, transform: [10, 0, 0, 10, 72, 676], fontName: "g_d0_f1", hasEOL: true },
    { dir: "ltr", width: 0, height: 0, transform: [10, 0, 0, 10, 72, 676], fontName: "g_d0_f1" }, // str 無し
  ];
  const pt = makePageText({ items, styles });
  const g = buildGeom(pt, viewportOf(1.5));
  assert.equal(g.posMap, pt.posMap); // 同じ参照（配列の === は参照比較）
  assert.equal(g.full, pt.raw);
  assert.equal(g.flat, pt.full);
  assert.equal(g.full, "Prostate cancer is the most common12");
  assert.deepEqual(g.items.map((it) => it.start), pt.itemStarts); // 非空 item の raw 上の開始 offset = 索引の itemStarts
  const last = g.items[g.items.length - 1];
  assert.equal(last.start + last.str.length, g.full.length); // 末尾 offset = raw の長さ（ensureRendered の前提検査と同じ）
  assert.deepEqual(g.items.map((it) => it.str), ["Prostate cancer is", " ", "the most common", "12"]);
  assert.deepEqual(g.eol, [false, false, true, false]); // 空 str の hasEOL は幾何に入らない（span を持たない）
  assert.deepEqual(g.items.map((it) => [it.font, it.ascent, it.descent]), [["serif", 0.9, 0.22], ["serif", 0.9, 0.22], ["serif", 0.9, 0.22], ["sans-serif", 1.25, 0.5]]); // クランプ
  assert.deepEqual(g.items.map((it) => [it.ox, it.oy, it.fh, it.fa]), [[108, 150, 15, 135], [243, 150, 15, 3.75], [108, 168, 15, 120], [228, 162, 9, 12]]);
  assert.equal(g.squash, "prostatecanceristhemostcommon12");
  assert.equal(g.sqMap[g.squash.length], g.flat.length);
  // pt 不在 = 空の幾何
  assert.deepEqual(buildGeom(undefined, viewportOf()), { items: [], full: "", flat: "", posMap: [0], squash: "", sqMap: [0], eol: [] });
});

test("itemsFull: str 連結（空 str と undefined は寄与しない）", () => {
  assert.equal(itemsFull({ items: [{ str: "ab" }, { str: "" }, {}, { str: " c" }] }), "ab c");
});

test("makePageText: raw / full（normText）/ posMap / itemStarts（空 str を飛ばす）", () => {
  const pt = makePageText({ items: [{ str: "Ab  " }, { str: "" }, { str: "C" }] });
  assert.equal(pt.raw, "Ab  C");
  assert.equal(pt.full, "ab c"); // 連続空白は 1 個・per-char lowercase
  assert.deepEqual(pt.posMap, [0, 1, 2, 4, 5]); // flat index → raw index（末尾は排他終端）
  assert.deepEqual(pt.itemStarts, [0, 4]);
});

test("itemIndexAt: start 昇順の二分探索・範囲外は端にクランプ", () => {
  const g = geomOf([item("abc", 0, 100, 10), item("de", 0, 88, 10), item("fgh", 0, 76, 10)]);
  assert.equal(itemIndexAt(g, 0), 0);
  assert.equal(itemIndexAt(g, 2), 0);
  assert.equal(itemIndexAt(g, 3), 1);
  assert.equal(itemIndexAt(g, 4), 1);
  assert.equal(itemIndexAt(g, 5), 2);
  assert.equal(itemIndexAt(g, 99), 2);
  assert.equal(itemIndexAt(g, -1), 0);
});

test("bodyTextOf: 区間を item 単位で組み立て hasEOL の item 末尾でだけ改行", () => {
  const g = geomOf([item("the", 0, 100, 10, { hasEOL: true }), item("United", 0, 88, 10, { hasEOL: true }), item("States", 0, 76, 10)]);
  assert.equal(bodyTextOf(g, 0, g.full.length), "the\nUnited\nStates");
  assert.equal(bodyTextOf(g, 1, 5), "he\nUn"); // 途中で切れた item の末尾では改行しない
  assert.equal(bodyTextOf(g, 3, 9), "United\n");
});

// ---------- 本文サイズ ----------
test("majorLargestSize: シェア 20% 以上のうち最大 / 無ければ最頻値 / 空は null", () => {
  assert.equal(majorLargestSize(new Map([[8.5, 300], [10, 500], [14, 50]]), 850), 10);
  assert.equal(majorLargestSize(new Map([[8.5, 300], [10, 500], [12, 200]]), 1000), 12); // 12 が 20% ちょうど
  assert.equal(majorLargestSize(new Map([[8.5, 1], [10, 2]]), 0), 10); // total 0 → 最頻値
  assert.equal(majorLargestSize(new Map(), 0), null);
});

test("bodyFontPx: 文書全体の推定があれば docBodySize × scale・無ければページ内の同じ規則", () => {
  const g = geomOf([item("body text line one", 0, 100, 10), item("body text line two", 0, 88, 10), item("Title", 0, 120, 18), item("caption", 0, 60, 8)]);
  assert.equal(bodyFontPx(g, 10, 1.5), 15);
  assert.equal(bodyFontPx(g, null, 1.5), 10); // 18pt は 5/48 < 20%・8pt は 7/48 < 20% → 10
  assert.equal(bodyFontPx({ items: [], full: "" }, null, 1), 1); // 空ページは 1（0 除算を避ける）
});

// ---------- 柱・ノンブル ----------
test("runningKey: 小文字・数字→#・空白畳み + y の丸め / 空白のみは null", () => {
  assert.equal(runningKey(item("Abdominal  Radiology (2023) 48:3610", 72, 770.4, 10)), "abdominal radiology (####) ##:####|770");
  assert.equal(runningKey(item("   ", 0, 0, 10)), null);
  assert.equal(runningKey({ str: "x" }), "x|0");
});

test("runningTable: 2 ページ以上の同位置同文字を数え、本文サイズを文書全体で決める", () => {
  const header = (p) => item(`Journal ${p}`, 72, 770, 9); // 数字→# で同じ key
  const pages = [
    [header(1), item("1", 300, 30, 9), item("Body line on page one about something", 72, 700, 10), item("Body line two", 72, 688, 10)],
    [header(2), item("2", 300, 30, 9), item("Body line on page two about something", 72, 700, 10), item("Fig. 1 caption", 72, 100, 8)],
    [header(3), item("Body line on page three about it", 72, 700, 10), item("", 0, 0, 10)],
  ];
  const t = runningTable(pages);
  assert.equal(t.counts.get("journal #|770"), 3);
  assert.equal(t.counts.get("#|30"), 2);
  assert.equal(t.counts.get("body line two|688"), 1);
  assert.equal(t.bodySize, 10);
  // 同じ key が 1 ページに 2 回出ても 1 ページ分しか数えない
  const t2 = runningTable([[item("1 3", 50, 50, 17), item("1 3", 50, 50, 17)], [item("1 3", 50, 50, 17)]]);
  assert.equal(t2.counts.get("# #|50"), 2);
});

test("runningIndexes: 2 ページ以上に出る item の index（空 str を飛ばした geom 番号）", () => {
  const counts = new Map([["journal #|770", 3], ["#|30", 2], ["once|10", 1]]);
  const items = [item("", 0, 0, 10), item("Journal 1", 72, 770, 9), item("Body", 72, 700, 10), item("", 0, 0, 10), item("7", 300, 30, 9), item("once", 0, 10, 9)];
  assert.deepEqual([...runningIndexes(items, counts)], [0, 2]);
  assert.deepEqual([...runningIndexes(items, new Map())], []);
});

// ---------- ブロック分割 ----------
test("splitBlocks: 行送り・文字高・x 重なりでブロックを切り、種別を付ける", () => {
  // 見出し 14pt → 本文 10pt（行送り 12）3 行 → キャプション 8.5pt 2 行 → 柱 → meta（email）
  const items = [
    item("Introduction heading text", 72, 700, 14),
    item("Prostate cancer is the most common cancer in men in the", 72, 680, 10),
    item("United States and the second leading cause of cancer", 72, 668, 10),
    item("death. Imaging plays a central role in staging.", 72, 656, 10),
    item("Fig. 1 Example of normal biodistribution on PSMA PET", 72, 600, 8.5),
    item("with physiologic uptake in the salivary glands.", 72, 590, 8.5),
    item("Abdominal Radiology (2023) 48:3610", 72, 770, 9),
    item("Corresponding author: someone@example.org", 72, 100, 10),
  ];
  const g = geomOf(items);
  const run = new Set([6]); // 柱 = geom index 6
  const { blocks, itemBlock } = splitBlocks(g, run, 10);
  assert.deepEqual(
    blocks.map((b) => [b.kind, b.lines, b.fh, b.first, b.last]),
    [
      ["body", 1, 14, 0, 0],
      ["body", 3, 10, 1, 3],
      ["small", 2, 8.5, 4, 5],
      ["running", 1, 9, 6, 6],
      ["meta", 1, 10, 7, 7],
    ]
  );
  assert.deepEqual(itemBlock, [0, 1, 1, 1, 2, 2, 3, 4]);
  assert.equal(g.full.slice(blocks[1].start, blocks[1].end).slice(0, 15), "Prostate cancer");
  // caption: 本文の 95〜97% のサイズで "Fig. N" 始まり
  const g2 = geomOf([item("Fig. 2 74-year-old with high-risk prostate cancer", 72, 600, 9.6), item("Body text at ten point size here", 72, 500, 10)]);
  assert.deepEqual(splitBlocks(g2, new Set(), 10).blocks.map((b) => b.kind), ["caption", "body"]);
});

test("splitBlocks: 同じ行の item（上付き）は 1 行にまとまり、代表文字高は文字数重みの最頻値", () => {
  const items = [item("Body text with a reference", 72, 700, 10), item("12", 200, 704, 6), item("and more body text on the next line", 72, 688, 10)];
  const { blocks, itemBlock } = splitBlocks(geomOf(items), new Set(), 10);
  assert.equal(blocks.length, 1);
  assert.deepEqual([blocks[0].lines, blocks[0].fh, blocks[0].kind], [2, 10, "body"]);
  assert.deepEqual(itemBlock, [0, 0, 0]);
});

test("splitBlocks: 2 カラム（x が重ならない）と 大きな行送り は別ブロック", () => {
  const items = [item("left column line one", 72, 700, 10), item("left column line two", 72, 688, 10), item("right column line one", 320, 700, 10), item("right column line two", 320, 688, 10), item("far below after a gap", 72, 500, 10)];
  const { blocks } = splitBlocks(geomOf(items), new Set(), 10);
  assert.deepEqual(blocks.map((b) => [b.first, b.last, b.lines]), [[0, 1, 2], [2, 3, 2], [4, 4, 1]]);
});

test("splitBlocks: 閾値の境界（行送り 0.5 / 2.2・同行 0.45・x 重なり 0）", () => {
  const two = (y2, fh2 = 10) => splitBlocks(geomOf([item("first body line of text", 72, 700, 10), item("second body line of text", 72, y2, fh2)]), new Set(), 10).blocks.length;
  assert.equal(two(679), 1); // 行送り 2.1 文字高 → 同じブロック（ダブルスペースの本文）
  assert.equal(two(677), 2); // 2.3 → 切る
  assert.equal(two(695), 1); // 0.5 ちょうど → 同じブロック
  assert.equal(two(695.1), 2); // 0.49 → 切る（別の行だが詰まりすぎ）
  assert.equal(two(695.5), 1); // 差 4.5 = 0.45 文字高 → 同じ行（1 行 1 ブロック）
  assert.equal(splitBlocks(geomOf([item("a", 72, 700, 10), item("b", 72, 695.5, 10)]), new Set(), 10).blocks[0].lines, 1);
  assert.equal(splitBlocks(geomOf([item("a", 72, 700, 10), item("b", 72, 695.4, 10)]), new Set(), 10).blocks.length, 2); // 差 4.6 → 別の行・行送り 0.46 → 切る
  // x 範囲が接するだけ（重なり 0）は別の列
  assert.equal(splitBlocks(geomOf([item("abcd", 72, 700, 10, { width: 100 }), item("efgh", 172, 688, 10, { width: 100 })]), new Set(), 10).blocks.length, 2);
  assert.equal(splitBlocks(geomOf([item("abcd", 72, 700, 10, { width: 100 }), item("efgh", 171, 688, 10, { width: 100 })]), new Set(), 10).blocks.length, 1);
});

test("splitBlocks: 文字高の比 0.88 / 1.14（大 → 小・小 → 大）", () => {
  const two = (fh2) => splitBlocks(geomOf([item("first line of text here", 72, 700, 10), item("second line of text here", 72, 686, fh2)]), new Set(), 10).blocks.length;
  assert.equal(two(8.9), 1); // 0.89 → 同じ
  assert.equal(two(8.7), 2); // 0.87 → 切る
  assert.equal(two(11.3), 1); // 1.13 → 同じ
  assert.equal(two(11.5), 2); // 1.15 → 切る（キャプション 8.5 → 本文 10 の遷移はこちらの判定）
});

test("splitBlocks: 種別の境界（本文比 0.95 / 0.97・meta 4 行・柱と本文が混ざる）", () => {
  // 代表文字高は 0.25 刻みに丸まるので本文 100px で測る
  const kind = (fh, str = "plain body text line") => splitBlocks(geomOf([item(str, 72, 700, fh)]), new Set(), 100).blocks[0].kind;
  assert.equal(kind(94.75), "small");
  assert.equal(kind(95), "body");
  assert.equal(kind(96.75, "Fig. 3 something"), "caption");
  assert.equal(kind(97, "Fig. 3 something"), "body"); // 97% 以上は本文扱い
  assert.equal(kind(94.75, "Fig. 3 something"), "small"); // small が caption より先
  assert.equal(kind(100, "Received: 1 May 2024"), "meta");
  const lines = (k) => Array.from({ length: k }, (_, i) => item(i === 0 ? "Contact: someone@example.org" : "more lines of the same block", 72, 700 - 12 * i, 10));
  assert.equal(splitBlocks(geomOf(lines(4)), new Set(), 10).blocks[0].kind, "meta");
  assert.equal(splitBlocks(geomOf(lines(5)), new Set(), 10).blocks[0].kind, "body");
  // 柱 item と本文 item が同じブロックに混ざれば running ではない（全部が柱のときだけ）
  const mixed = geomOf([item("Journal 2024", 72, 700, 10), item("Body line in the same block", 72, 688, 10)]);
  assert.equal(splitBlocks(mixed, new Set([0]), 10).blocks[0].kind, "body");
  assert.equal(splitBlocks(mixed, new Set([0, 1]), 10).blocks[0].kind, "running");
  // 空白だけの item は running の判定に数えない
  const blank = geomOf([item("Journal 2024", 72, 700, 10), item("   ", 200, 700, 10)]);
  assert.equal(splitBlocks(blank, new Set([0]), 10).blocks[0].kind, "running");
});

test("majorLargestSize / 代表文字高: 同数は先に出た方", () => {
  assert.equal(majorLargestSize(new Map([[10, 5], [12, 5]]), 0), 10);
  const { blocks } = splitBlocks(geomOf([item("abcde", 72, 700, 10), item("fghij", 72, 688, 11)]), new Set(), 10);
  assert.equal(blocks[0].fh, 10);
});

// ---------- ブロック規則（bodySpans = 選択帯・quote・コピー・保存後の注釈描画の全経路） ----------
/** 柱 / 本文 A（2 行）/ 図説 / 表（小さい字）/ 本文 B（2 行目はノンブル = run）/ 空白だけのブロック / 本文 C。bodyFh = 10 */
function blockPage() {
  const g = geomOf([
    item("Abdominal Radiology", 72, 780, 8), // 0 柱（run）
    item("First body paragraph line one", 72, 700, 10), // 1 本文 A
    item("first body paragraph line two", 72, 688, 10), // 2 本文 A
    item("Fig. 1 Axial CT of the liver", 72, 650, 9.6), // 3 caption（本文の 95〜97%・Fig. N）
    item("Small table cell text here", 72, 620, 8), // 4 small
    item("Second body paragraph begins", 72, 580, 10), // 5 本文 B
    item("12", 72, 568, 10), // 6 本文 B の中の run（ノンブル）
    item("   ", 72, 500, 10), // 7 空白だけ（body）
    item("Third body paragraph here", 72, 470, 10), // 8 本文 C
  ]);
  const run = new Set([0, 6]);
  const { blocks, itemBlock } = splitBlocks(g, run, 10);
  const at = (s, k = 0) => {
    const i = g.full.indexOf(s);
    assert.ok(i >= 0, s);
    return i + k;
  };
  const spans = (oStart, oEnd, startsBefore = false, endsAfter = false) =>
    bodySpans(g, blocks, itemBlock, run, oStart, oEnd, startsBefore, endsAfter).map(([a, b]) => g.full.slice(a, b));
  return { g, run, blocks, itemBlock, at, spans };
}

test("bodySpans: 前提のブロック分割（柱 / 本文 / caption / small / 本文 / 空白 / 本文）", () => {
  const { blocks, itemBlock } = blockPage();
  assert.deepEqual(blocks.map((b) => b.kind), ["running", "body", "caption", "small", "body", "body", "body"]);
  assert.deepEqual(itemBlock, [0, 1, 1, 2, 3, 4, 4, 5, 6]);
});

test("bodySpans: 単一ブロック内は無加工（run の item・空白だけでも区間 1 つ）・空区間は []", () => {
  const { g, run, blocks, itemBlock, at, spans } = blockPage();
  const a = at("paragraph line one");
  const b = at("first body paragraph line two", 10);
  assert.deepEqual(bodySpans(g, blocks, itemBlock, run, a, b, false, false), [[a, b]]);
  assert.deepEqual(spans(at("begins"), at("12", 2)), ["begins12"]); // run（ノンブル）も起点 = 終点ブロックなら採る
  assert.deepEqual(spans(at("   ", 1), at("   ", 2)), [" "]); // 空白だけの区間も単一ブロックなら落とさない
  assert.deepEqual(spans(a, a), []);
  assert.deepEqual(spans(b, a), []);
});

test("bodySpans: 起点 / 終点ブロックはそのまま・間は body だけ（柱・図説・表・本文中の run を飛ばす）・ブロック境界で切る・空白だけの区間は落とす", () => {
  const { at, spans } = blockPage();
  // 本文 A の途中 → 本文 C の途中: 図説と表を飛ばし、本文 B のノンブルを飛ばし、空白ブロックは落ちる
  assert.deepEqual(spans(at("paragraph line one"), at("Third body", 10)), ["paragraph line onefirst body paragraph line two", "Second body paragraph begins", "Third body"]);
  // 起点が図説・終点が表（user が指したブロックは種別を問わず採る）: 隣り合う 2 ブロックでも境界で切る
  assert.deepEqual(spans(at("Axial CT"), at("Small table", 11)), ["Axial CT of the liver", "Small table"]);
  // 起点ブロック（本文 B）の中の run は採る・終点ブロック C も採る
  assert.deepEqual(spans(at("begins"), at("Third", 5)), ["begins12", "Third"]);
  // 起点が柱（user が柱から引いた）
  assert.deepEqual(spans(at("Radiology"), at("First body", 10)), ["Radiology", "First body"]);
});

test("bodySpans: ページ跨ぎ（startsBefore / endsAfter）では手前 / 先の側に起点・終点が無い = 同じ規則で間引く", () => {
  const { g, at, spans } = blockPage();
  // 前ページから来て本文 B で終わる: 柱は飛ばす・本文 A は採る
  assert.deepEqual(spans(0, at("Second body", 11), true, false), ["First body paragraph line onefirst body paragraph line two", "Second body"]);
  // 本文 A の途中から次ページへ: 先頭ブロック A は起点・以後は body だけ
  assert.deepEqual(spans(at("line two"), g.full.length, false, true), ["line two", "Second body paragraph begins", "Third body paragraph here"]);
  // 柱から引いて次ページへ: 起点の柱は採る
  assert.deepEqual(spans(at("Radiology"), g.full.length, false, true), ["Radiology", "First body paragraph line onefirst body paragraph line two", "Second body paragraph begins", "Third body paragraph here"]);
  // 3 ページ以上の選択の中間ページ: 起点も終点も無い → body だけ（run・空白は落ちる）
  assert.deepEqual(spans(0, g.full.length, true, true), ["First body paragraph line onefirst body paragraph line two", "Second body paragraph begins", "Third body paragraph here"]);
  // 1 ブロックしか無いページでも跨ぎなら規則を通す（柱だけのページは何も採らない）
  const solo = geomOf([item("Abdominal Radiology", 72, 780, 8)]);
  const sb = splitBlocks(solo, new Set([0]), 10);
  assert.deepEqual(bodySpans(solo, sb.blocks, sb.itemBlock, new Set([0]), 0, solo.full.length, true, true), []);
  assert.deepEqual(bodySpans(solo, sb.blocks, sb.itemBlock, new Set([0]), 0, 5, false, false), [[0, 5]]);
});

// ---------- 読み順 ----------
function twoColumnInterleaved() {
  // 左右の行を交互に織り込んだ 6 バンド（x 50..250 と 300..500・ガター 275）
  const items = [];
  for (let k = 0; k < 6; k++) {
    items.push({ str: `L${k} left column text`, transform: [10, 0, 0, 10, 50, 700 - 12 * k], width: 200, height: 10, hasEOL: true });
    items.push({ str: `R${k} right column text`, transform: [10, 0, 0, 10, 300, 700 - 12 * k], width: 200, height: 10, hasEOL: true });
  }
  return items;
}

test("applyReadingOrder: 交互織り込みの 2 カラムは左→右へ並べ替え、hasEOL を捨てる", () => {
  const tc = { items: twoColumnInterleaved() };
  applyReadingOrder({ rotate: 0 }, tc);
  assert.deepEqual(
    tc.items.map((it) => it.str.slice(0, 2)),
    ["L0", "L1", "L2", "L3", "L4", "L5", "R0", "R1", "R2", "R3", "R4", "R5"]
  );
  assert.ok(tc.items.every((it) => it.hasEOL === false));
});

test("applyReadingOrder: 回転ページ・8 item 未満・整列済みは元順のまま（hasEOL も保持）", () => {
  const rot = { items: twoColumnInterleaved() };
  applyReadingOrder({ rotate: 90 }, rot);
  assert.equal(rot.items[1].str.slice(0, 2), "R0");
  assert.ok(rot.items.every((it) => it.hasEOL === true));
  const few = { items: twoColumnInterleaved().slice(0, 7) }; // 7 item → 見ない
  applyReadingOrder({ rotate: 0 }, few);
  assert.equal(few.items[1].str.slice(0, 2), "R0");
  const eight = { items: twoColumnInterleaved().slice(0, 8) }; // 8 item（4 バンド）→ 並べ替える
  applyReadingOrder({ rotate: 0 }, eight);
  assert.deepEqual(eight.items.map((it) => it.str.slice(0, 2)), ["L0", "L1", "L2", "L3", "R0", "R1", "R2", "R3"]);
  // 左塊 → 右塊 に整列済み（スイッチ 1 回）: no-op
  const src = twoColumnInterleaved();
  const aligned = { items: [...src.filter((_, i) => i % 2 === 0), ...src.filter((_, i) => i % 2 === 1)] };
  const before = aligned.items.slice();
  applyReadingOrder({ rotate: 0 }, aligned);
  assert.deepEqual(aligned.items, before);
  assert.ok(aligned.items.every((it) => it.hasEOL === true));
  // items 無し
  const none = { items: null };
  applyReadingOrder({ rotate: 0 }, none);
  assert.equal(none.items, null);
});

// ---------- 文書内検索 ----------
/** 決定論の擬似乱数（線形合同法 mod 2^31。下位ビットは周期が短いので上位 15 bit を使う） */
function lcg(seed) {
  let s = seed;
  return (k) => {
    s = (Math.imul(s, 1103515245) + 12345) & 0x7fffffff;
    return (s >>> 16) % k;
  };
}

test("allMatchIndexes: 重ならない出現の開始 index（i + q.length で前進）・空の q は []", () => {
  assert.deepEqual(allMatchIndexes("aaaa", "aa"), [0, 2]);
  assert.deepEqual(allMatchIndexes("aaa", "aa"), [0]);
  assert.deepEqual(allMatchIndexes("abcabc", "bc"), [1, 4]);
  assert.deepEqual(allMatchIndexes("abc", ""), []);
  assert.deepEqual(allMatchIndexes("", "a"), []);
  assert.deepEqual(allMatchIndexes("ab", "abc"), []);
  // 1 文字ずつ見る素朴な走査（一致したら q.length 進む）と同じ列
  const rnd = lcg(7);
  for (let k = 0; k < 20000; k++) {
    let s = "";
    for (let i = rnd(40); i > 0; i--) s += "ab a"[rnd(4)];
    let q = "";
    for (let i = 1 + rnd(3); i > 0; i--) q += "ab "[rnd(3)];
    const naive = [];
    for (let i = 0; i + q.length <= s.length; ) {
      if (s.startsWith(q, i)) {
        naive.push(i);
        i += q.length;
      } else i++;
    }
    assert.deepEqual(allMatchIndexes(s, q), naive, JSON.stringify([s, q]));
  }
});

// ---------- ハイライト矩形 ----------
/** 横書き（ascent/descent あり・なし）・回転 90°・斜め 30°（ascent だけ）の 4 item。viewport は scale 1.5・高さ 800 */
function rectItems() {
  const vp = [1.5, 0, 0, -1.5, 0, 1200];
  return [
    makeGeomItem("Prostate cancer", 0, transform(vp, [10, 0, 0, 10, 72, 700]), 75, 1.5, "serif", 0.9, -0.22),
    makeGeomItem("is common", 15, transform(vp, [10, 0, 0, 10, 160, 700]), 45, 1.5),
    makeGeomItem("rotated", 24, transform(vp, [0, 10, -10, 0, 300, 500]), 35, 1.5, "serif", 0.8, -0.2),
    makeGeomItem("slanted", 31, transform(vp, [8.660254037844387, 5, -5, 8.660254037844387, 100, 400]), 35, 1.5, undefined, 1.0),
  ];
}
const RECT_CASES = [[0, 38], [3, 12], [9, 20], [26, 33], [0, 15], [15, 24], [31, 38], [5, 5]];
const R = (left, top, width, height) => ({ left, top, width, height });

test("fragmentRects: 文字ボックス帯（上 = ascent·fh / 無ければ fh・下 = descent·fh / 無ければ 0）・回転と斜めは 4 隅の AABB", () => {
  const items = rectItems();
  const whole = [R(108, 136.5, 112.5, 16.80000000000001), R(240, 135, 67.5, 15), R(438, 397.5, 15, 52.5), R(142.5, 560.7596189432334, 52.96633369868303, 39.24038105676664)];
  assert.deepEqual(
    RECT_CASES.map(([a, b]) => fragmentRects(items, a, b)),
    [
      whole,
      [R(130.5, 136.5, 67.5, 16.80000000000001)],
      [R(175.5, 136.5, 45, 16.80000000000001), R(240, 135, 37.5, 15)],
      [R(438, 397.5, 15, 37.5), R(142.5, 579.5096189432334, 20.49038105676658, 20.490381056766637)],
      [whole[0]],
      [whole[1]],
      [whole[3]],
      [],
    ]
  );
  // グリフ幅の校正（measure）は item 内の文字境界だけを動かす（item 境界は同じ）
  const measure = (it, s) => s.length * 7 + (s.includes("o") ? 3 : 0);
  assert.deepEqual(
    RECT_CASES.map(([a, b]) => fragmentRects(items, a, b, measure)),
    [
      whole,
      [R(133, 136.5, 65.625, 16.80000000000001)],
      [R(176.75, 136.5, 43.75, 16.80000000000001), R(240, 135, 38.863636363636374, 15)],
      [R(438, 397.5, 15, 35.33653846153845), R(142.5, 579.5096189432334, 20.49038105676658, 20.490381056766637)],
      [whole[0]],
      [whole[1]],
      [whole[3]],
      [],
    ]
  );
});

// ---------- 注釈の色 ----------
test("注釈の色（colorForAnnotation / normalizedColor / isClaudeAuthor）: パレットの色はそのまま・それ以外は author で決まる（claude で始まる = 紫・他 = 黄）", () => {
  const hex = { yellow: "#f5c518", green: "#34a853", blue: "#4285f4", red: "#ea4335", purple: "#a855f7" };
  const authors = ["user", "claude", "claude-opus", "Claude", ""];
  const table = [
    [null, "yellow purple purple yellow yellow"],
    [undefined, "yellow purple purple yellow yellow"],
    ["", "yellow purple purple yellow yellow"],
    ["yellow", "yellow yellow yellow yellow yellow"],
    ["green", "green green green green green"],
    ["blue", "blue blue blue blue blue"],
    ["red", "red red red red red"],
    ["purple", "purple purple purple purple purple"],
    ["pink", "yellow purple purple yellow yellow"],
    ["Yellow", "yellow purple purple yellow yellow"],
  ];
  for (const [c, names] of table) {
    names.split(" ").forEach((name, i) => {
      assert.equal(colorForAnnotation(c, authors[i]), hex[name], `${c} × ${authors[i]}`);
      assert.equal(normalizedColor(c, authors[i]), name, `${c} × ${authors[i]}`); // 色ドットの選択中・編集 draft の初期値
    });
  }
  assert.deepEqual(authors.map(isClaudeAuthor), [false, true, true, false, false]);
});

// ---------- 注釈 quote の照合（squash 空間） ----------
/** 照合の参照: quote 区間 seg を squash（空白除去）空間で探し、無ければ（headFallback のとき）先頭 40 文字で探して、一致した区間を flat の [start, end) で返す */
function refQuoteSpan(geom, seg, headFallback) {
  const q = normText(seg).replace(/ /g, "");
  if (q.length < 1) return null;
  let i = geom.squash.indexOf(q);
  let len = q.length;
  if (i < 0) {
    if (!headFallback) return null;
    const head = q.slice(0, 40);
    i = geom.squash.indexOf(head);
    if (i < 0) return null;
    len = head.length;
  }
  len = Math.min(len, geom.squash.length - i);
  if (len <= 0) return null;
  return [geom.sqMap[i], geom.sqMap[i + len - 1] + 1];
}

function quoteGeom() {
  // 行末に空白を持たない item 連結（"theUnited"）+ 行末空白あり + 語中の改行ハイフン
  return geomOf([
    item("Prostate cancer is the most common cancer in men in the", 72, 700, 10, { hasEOL: true }),
    item("United States and the second leading cause of cancer ", 72, 688, 10, { hasEOL: true }),
    item("death. Imaging plays a central role in staging and re-", 72, 676, 10, { hasEOL: true }),
    item("staging of the disease.", 72, 664, 10),
  ]);
}

test("quote の照合: 行境界の空白の有無を無視・一致は最終一致文字の直後まで・40 文字超は先頭 40 文字・不一致は null", () => {
  const g = quoteGeom();
  const flatOf = (sp) => g.flat.slice(sp[0], sp[1]);
  // 選択の改行（<br> 由来）と item 連結（行末空白なし）の差
  assert.deepEqual(refQuoteSpan(g, "in the\nUnited States", false), [49, 68]);
  assert.equal(flatOf(refQuoteSpan(g, "in the\nUnited States", false)), "in theunited states");
  assert.deepEqual(refQuoteSpan(g, "in theUnited States", false), [49, 68]);
  assert.deepEqual(refQuoteSpan(g, "IN  THE \t united\n\nSTATES", false), [49, 68]);
  // 行末空白ありの境界: 末尾の空白は範囲に含めない
  assert.equal(flatOf(refQuoteSpan(g, "cause of cancer\n", false)), "cause of cancer");
  // 40 文字超で後半が不一致: 先頭 40 文字（squash）だけ
  const long = "the second leading cause of cancer death. Imaging plays a central role in NOTHERE";
  assert.equal(refQuoteSpan(g, long, false), null);
  const sp = refQuoteSpan(g, long, true);
  assert.equal(flatOf(sp).replace(/ /g, ""), normText(long).replace(/ /g, "").slice(0, 40));
  // 40 文字以下の不一致は先頭で探しても同じく不一致
  assert.equal(refQuoteSpan(g, "the second leading cause of cancers", true), null);
  assert.equal(refQuoteSpan(g, "no such words", true), null);
  // 空 / 空白だけ
  assert.equal(refQuoteSpan(g, "", false), null);
  assert.equal(refQuoteSpan(g, " \n ", true), null);
  // 行末ハイフンの改行
  assert.equal(flatOf(refQuoteSpan(g, "re-\nstaging", false)), "re-staging");
  // 先に出る方へ一致する（"the" は 1 行目）
  assert.deepEqual(refQuoteSpan(g, "the", false), [19, 22]);
});

const QUOTE_FIXED = [
  "in the\nUnited States",
  "in theUnited States",
  "IN  THE \t united\n\nSTATES",
  "cause of cancer\n",
  "the second leading cause of cancer death. Imaging plays a central role in NOTHERE",
  "the second leading cause of cancers",
  "no such words",
  "",
  " \n ",
  "re-\nstaging",
  "the",
];

/** 区切り（空行）を含まない 1 区間の quote を乱択で作る: 語を並べた行（行末空白あり / なし）から切り出し、空白の揺れと後半の書き換えを加える */
function randomQuoteCases(seed, count) {
  const rnd = lcg(seed);
  const words = ["the", "PSMA", "cancer", "prostate", "of", "in", "İstanbul", "ΟΣ", "and", "a", "re-", "staging"];
  const out = [];
  for (let k = 0; k < count; k++) {
    const items = [];
    for (let li = 0, lines = 1 + rnd(6); li < lines; li++) {
      let str = "";
      for (let w = 0, m = 1 + rnd(8); w < m; w++) str += (w ? " " : "") + words[rnd(words.length)];
      if (rnd(2)) str += " ";
      items.push(item(str, 72, 700 - 12 * li, 10, { hasEOL: true }));
    }
    const gg = geomOf(items);
    const a = rnd(gg.full.length + 1);
    let seg = gg.full.slice(a, a + rnd(80)).replace(/ /g, () => [" ", "  ", "\n", "", " \t "][rnd(5)]);
    if (rnd(3) === 0 && seg.length > 10) seg = seg.slice(0, -5) + "zzzzz";
    out.push([gg, seg]);
  }
  return out;
}

test("squashOrigSpan（domAligned でないページの選択）: 全体だけで照合し、一致を geom.full の元 index 区間で返す（固定例 + 乱択）", () => {
  const g = quoteGeom();
  const raw = (sp) => g.full.slice(sp[0], sp[1]);
  assert.equal(raw(squashOrigSpan(g, "in the\nUnited States")), "in theUnited States");
  assert.equal(raw(squashOrigSpan(g, "IN  THE \t united\n\nSTATES")), "in theUnited States");
  assert.equal(raw(squashOrigSpan(g, "cause of cancer\n")), "cause of cancer");
  assert.equal(raw(squashOrigSpan(g, "re-\nstaging")), "re-staging");
  // 先頭 40 文字への退避はしない（注釈の描画とは違う）
  assert.equal(squashOrigSpan(g, "the second leading cause of cancer death. Imaging plays a central role in NOTHERE"), null);
  for (const seg of ["no such words", "", " \n "]) assert.equal(squashOrigSpan(g, seg), null, seg);
  // 参照（refQuoteSpan = flat 区間）を posMap で元 index へ写したものと一致
  const viaRef = (gg, seg) => {
    const sp = refQuoteSpan(gg, seg, false);
    return sp && [gg.posMap[sp[0]], gg.posMap[Math.min(sp[1], gg.posMap.length - 1)]];
  };
  for (const seg of QUOTE_FIXED) assert.deepEqual(squashOrigSpan(g, seg), viaRef(g, seg), seg);
  let found = 0;
  for (const [gg, seg] of randomQuoteCases(3, 3000)) {
    const want = viaRef(gg, seg);
    if (want) found++;
    assert.deepEqual(squashOrigSpan(gg, seg), want, JSON.stringify(seg));
  }
  assert.ok(found > 1000, `found ${found}`);
});

test("quoteSpans（保存した注釈の描画）: 空行で区間に分け、区間ごとに 全体 → 先頭 40 文字 で照合・4 文字未満と不一致の区間は飛ばす", () => {
  const g = quoteGeom();
  const ref = (seg) => refQuoteSpan(g, seg, true);
  // 区切りなしの quote は区間 1 つ（固定例）: squash で 4 文字以上なら参照と同じ 1 区間・それ以外は []
  for (const seg of QUOTE_FIXED.filter((x) => !/\n[ \t]*\n/.test(x))) {
    const want = normText(seg).replace(/ /g, "").length >= 4 && ref(seg) ? [ref(seg)] : [];
    assert.deepEqual(quoteSpans(g, seg), want, seg);
  }
  assert.deepEqual(quoteSpans(g, "the"), []); // 一致はするが 3 文字
  assert.deepEqual(quoteSpans(g, "deat"), [ref("deat")]); // 4 文字は描く
  // 空行（間に空白・タブがあってよい）で区間に分け、quote の順に返す（選択では 1 続きに一致する "…united\n\nSTATES" も 2 区間）
  assert.deepEqual(quoteSpans(g, "IN  THE \t united\n\nSTATES"), [ref("in the united"), ref("states")]);
  assert.deepEqual(quoteSpans(g, "cause of cancer\n\nin the\nUnited States"), [ref("cause of cancer"), ref("in the\nUnited States")]);
  assert.deepEqual(quoteSpans(g, "re-\nstaging\n \t\nProstate cancer"), [ref("re-\nstaging"), ref("Prostate cancer")]);
  // 区間ごとに飛ばす: 3 文字の区間・不一致の区間は落ち、残りは描く
  assert.deepEqual(quoteSpans(g, "the\n\nno such words\n\nUnited States"), [ref("United States")]);
  // 区間ごとに先頭 40 文字へ退避
  const long = "the second leading cause of cancer death. Imaging plays a central role in NOTHERE";
  assert.deepEqual(quoteSpans(g, `${long}\n\nstaging of the disease.`), [ref(long), ref("staging of the disease.")]);
  // 改行 1 つは区切りでない（行境界）
  assert.deepEqual(quoteSpans(g, "in the\nUnited"), [ref("in the United")]);
  assert.deepEqual(quoteSpans(g, ""), []);
  assert.deepEqual(quoteSpans(g, "\n\n\n\n"), []);
  // 乱択（区切りを含まない 1 区間）: 参照と同じ
  let found = 0;
  for (const [gg, seg] of randomQuoteCases(5, 3000)) {
    assert.ok(!/\n[ \t]*\n/.test(seg), JSON.stringify(seg));
    const q = normText(seg).replace(/ /g, "");
    const sp = refQuoteSpan(gg, seg, true);
    const want = q.length >= 4 && sp ? [sp] : [];
    if (want.length) found++;
    assert.deepEqual(quoteSpans(gg, seg), want, JSON.stringify(seg));
  }
  assert.ok(found > 1000, `found ${found}`);
});

// ---------- 注釈タブの検索（matchParts） ----------
// 旧（検索の正規化を共通化する前）: 絞り込み = normText(text).includes(normText(q).trim())・カードの強調 = per-char lowercase の indexOf
// （q は trim + 文字列全体の toLowerCase・長さの変わる lowercase があれば強調しない）。2 つの正規化が食い違う入力で
// 「絞り込みに残るのに <mark> が付かない」が起きていた。新は両方とも matchParts の結果（null か否か / m の断片）
const oldFilter = (text, q) => normText(text).includes(normText(q).trim());
function oldParts(text, q) {
  q = q.trim();
  if (!q) return [{ t: text, m: false }];
  const lower = text.split("").map((c) => c.toLowerCase()).join("");
  const ql = q.toLowerCase();
  if (lower.length !== text.length || !ql) return [{ t: text, m: false }];
  const out = [];
  let from = 0;
  for (;;) {
    const i = lower.indexOf(ql, from);
    if (i < 0) break;
    if (i > from) out.push({ t: text.slice(from, i), m: false });
    out.push({ t: text.slice(i, i + ql.length), m: true });
    from = i + ql.length;
  }
  if (from < text.length) out.push({ t: text.slice(from), m: false });
  return out;
}
const marked = (parts) => (parts ?? []).filter((p) => p.m).map((p) => p.t);
const joined = (parts) => parts.map((p) => p.t).join("");

test("matchParts: 旧の 2 つの正規化が食い違う入力（空白 2 個・改行・İ・ΟΣ）で、絞り込みと強調が同じ一致を見る", () => {
  const cases = [
    // [text, q, 新の強調の断片]
    ["seen in  the liver", "in the", ["in  the"]], // text 側の空白 2 個
    ["seen in the liver", "in  the", ["in the"]], // 検索語側の空白 2 個
    ["seen in\nthe liver", "in the", ["in\nthe"]], // 改行（選択由来の quote）
    ["İstanbul hepatic", "stan", ["stan"]], // 長さの変わる lowercase（'İ' → "i̇"）
    ["ΟΣ sign", "ΟΣ", ["ΟΣ"]], // 語末シグマ（文字列全体の toLowerCase は "ος"・per-unit は "οσ"）
  ];
  for (const [text, q, want] of cases) {
    // 旧: 絞り込みには残るのに強調が無い（= 食い違いの実例であることを固定）
    assert.equal(oldFilter(text, q), true, `old filter ${JSON.stringify([text, q])}`);
    assert.deepEqual(marked(oldParts(text, q)), [], `old parts ${JSON.stringify([text, q])}`);
    // 新: 一致あり（絞り込みに残る）+ 強調はその断片・断片を繋ぐと元の text
    const p = matchParts(text, q);
    assert.ok(p, `new ${JSON.stringify([text, q])}`);
    assert.deepEqual(marked(p), want, JSON.stringify([text, q]));
    assert.equal(joined(p), text);
  }
});

test("matchParts: 空の検索語は絞り込まない・一致なしは null・大小無視・非重複の全出現・断片は元の text に戻る", () => {
  assert.deepEqual(matchParts("Liver CT", ""), [{ t: "Liver CT", m: false }]);
  assert.deepEqual(matchParts("Liver CT", "  \n "), [{ t: "Liver CT", m: false }]);
  assert.deepEqual(matchParts("", ""), [{ t: "", m: false }]);
  assert.equal(matchParts("Liver CT", "mri"), null);
  assert.equal(matchParts("", "ct"), null);
  assert.deepEqual(matchParts("CT and ct", " Ct "), [
    { t: "CT", m: true },
    { t: " and ", m: false },
    { t: "ct", m: true },
  ]);
  assert.deepEqual(marked(matchParts("aaaa", "aa")), ["aa", "aa"]); // allMatchIndexes と同じ非重複
  // 'İ'（"i̇" = 2 単位）の途中で始まる / 終わる一致は元の字を割らない: "i" は 'İ' 1 字を強調
  assert.deepEqual(matchParts("İ CT", "i"), [{ t: "İ", m: true }, { t: " CT", m: false }]);
  // 隣の一致が同じ元の字を共有する（"̇i" の 2 回目は 2 字目の 'İ' の途中から）: 1 つの強調に延ばす
  assert.deepEqual(matchParts("İİİ", "̇i"), [{ t: "İİİ", m: true }]);
  // 先頭 / 末尾の空白・連続空白を含む text でも断片を繋ぐと元に戻る（照合空間の畳み込みは表示に漏れない）
  for (const [text, q] of [["  a  b  ", "a b"], ["\tx\n\ny ", "y"], ["İı ΟΣ", "ı"]]) {
    const p = matchParts(text, q);
    assert.ok(p, JSON.stringify([text, q]));
    assert.equal(joined(p), text);
  }
});

test("matchParts: 乱択 2,000 件で 一致あり ⇔ normText(text).includes(normText(q).trim())（旧の絞り込みと同じ件数が残る）", () => {
  let seed = 7;
  const rnd = (k) => ((seed = (seed * 1103515245 + 12345) & 0x7fffffff) % k);
  const alpha = ["a", "B", "c", " ", "  ", "\n", "İ", "Σ", "ς", "\t"];
  let hit = 0;
  for (let it = 0; it < 2000; it++) {
    let text = "";
    for (let k = 0, m = rnd(20); k < m; k++) text += alpha[rnd(alpha.length)];
    let q = "";
    for (let k = 0, m = 1 + rnd(4); k < m; k++) q += alpha[rnd(alpha.length)];
    const p = matchParts(text, q);
    assert.equal(p !== null, oldFilter(text, q), JSON.stringify([text, q]));
    if (p) {
      hit++;
      assert.equal(joined(p), text, JSON.stringify([text, q]));
      if (normText(q).trim()) assert.ok(p.some((x) => x.m), JSON.stringify([text, q]));
    }
  }
  assert.ok(hit > 200, `hit ${hit}`);
});

// ---------- ズーム ----------
test("fitScale: 幅 / ページ全体 / 自動（幅フィットを AUTO_MAX で上限クリップ）・片側 padding 16・寸法 0 は 1", () => {
  const base = { containerW: 800, containerH: 600, pageW: 612, pageH: 792 };
  assert.equal(fitScale({ ...base, mode: "width" }), 768 / 612);
  assert.equal(fitScale({ ...base, mode: "page" }), 568 / 792);
  assert.equal(fitScale({ ...base, mode: "auto" }), AUTO_MAX);
  assert.equal(fitScale({ ...base, mode: "auto", containerW: 500 }), 468 / 612);
  assert.equal(fitScale({ ...base, mode: "width", pad: 0 }), 800 / 612);
  assert.equal(fitScale({ ...base, mode: "width", containerW: 10 }), 1 / 612); // 使える幅は 1px 未満にしない
  assert.equal(fitScale({ ...base, mode: "page", pageW: 0 }), 1);
});

console.log(`${n} tests passed`);
