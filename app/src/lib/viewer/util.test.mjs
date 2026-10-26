/* util.ts の純関数を node で固定する（依存なし・`node app/src/lib/viewer/util.test.mjs`）。
   **Node 22.18+ が要る**: .ts を直接 import し、Node の type stripping（型注釈を消すだけ・22.18+ / 23.6+ で既定）に依存する。
   util.ts は型を消すだけで動く TS（enum・namespace・parameter property を使わない）であることが前提。
   入力は手組みの小さな item 列。PdfViewer.svelte から移した ブロック分割・柱検出・読み順・本文組立・索引 の既存の挙動と、
   その閾値（行送り 0.5 / 2.2・文字高比 0.88 / 1.14・本文比 0.95 / 0.97・同行 0.45・meta 4 行・読み順 8 item・柱 2 ページ）を固定する。 */
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

console.log(`${n} tests passed`);
