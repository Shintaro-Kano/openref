/* 本棚の純関数（node app/src/lib/shelf/tree.test.mjs・Node 22.18+ の type stripping）。dir・上限つきの設定を含む */
import assert from "node:assert/strict";
import { ASSET_MAX, breadcrumbOf, collectBooks, dirOf, filterBooks, fmtSize, hueOf, indexDirs, indexFolders, indexParents, norm, pushPos, pushRecent, rangeParts, repBooks, searchTerms, sortBooks } from "./tree.ts";

const book = (id, title, folder, size = 1, mtime = 1) => ({ id, title, folder, path: `/r/${id}.pdf`, size, mtime });
const fld = (id, path, name, books = [], children = []) => ({ id, name, path, books, children, totalBooks: books.length + children.reduce((n, c) => n + c.totalBooks, 0) });

const b1 = book("b1", "ＭＲＩ の基礎", "画像", 3, 10);
const b2 = book("b2", "胸部 CT 読影", "画像", 1, 30);
const b3 = book("b3", "Sample MRI", "2024:3:10-11 特集", 2, 20);
const sub = fld("f2", "/r/画像/2024:3:10-11 特集", "2024:3:10-11 特集", [b3]);
const rad = fld("f1", "/r/画像", "画像", [b1, b2], [sub]);
const tree = fld("r", "/r", "r", [], [rad]);

// 索引: id → フォルダ（root を含む）・子 → 親（root は入らない）・dir（root 相対・`:` 入りも）
const byId = new Map();
indexFolders(tree, byId);
assert.deepEqual([...byId.keys()], ["r", "f1", "f2"]);
const parent = new Map();
indexParents(tree, parent);
assert.deepEqual([...parent], [["f1", "r"], ["f2", "f1"]]);
const byDir = new Map();
indexDirs(tree, byDir);
assert.deepEqual([...byDir.keys()], ["画像", "画像/2024:3:10-11 特集"]);
assert.equal(dirOf(tree, tree), null, "root の dir が null でない");
assert.equal(dirOf(tree, sub), "画像/2024:3:10-11 特集");

// 全部の本・パンくず・代表の表紙
const all = [];
collectBooks(tree, all);
assert.deepEqual(all.map((b) => b.id), ["b1", "b2", "b3"]);
assert.deepEqual(breadcrumbOf(sub, byId, parent).map((f) => f.id), ["r", "f1", "f2"]);
assert.deepEqual(breadcrumbOf(null, byId, parent), []);
assert.deepEqual(repBooks(tree, 2).map((b) => b.id), ["b1", "b2"], "代表が直下 → 子孫の順でない");
assert.deepEqual(repBooks(sub).map((b) => b.id), ["b3"]);

// 検索 = NFKC + 小文字 + AND（書名と親フォルダ名の両方を見る）・全角の語でも半角の語でも当たる
assert.equal(norm("ＭＲＩ"), "mri");
assert.deepEqual(searchTerms("  mri  胸部　"), ["mri", "胸部"], "全角の空白で分けない");
assert.deepEqual(searchTerms("   "), []);
assert.deepEqual(filterBooks(all, "mri").map((b) => b.id), ["b1", "b3"]);
assert.deepEqual(filterBooks(all, "ｍｒｉ 画像").map((b) => b.id), ["b1"], "AND にならない");
assert.deepEqual(filterBooks(all, "特集").map((b) => b.id), ["b3"], "親フォルダ名を見ない");
assert.deepEqual(filterBooks(all, "").map((b) => b.id), ["b1", "b2", "b3"]);

// 並び = 新しい配列（引数は変えない）・名前は昇順・更新とサイズは大きい順
const copy = [...all];
assert.deepEqual(sortBooks(all, "mtime").map((b) => b.id), ["b2", "b3", "b1"]);
assert.deepEqual(sortBooks(all, "size").map((b) => b.id), ["b1", "b3", "b2"]);
assert.deepEqual(sortBooks(all, "title").map((b) => b.id), ["b1", "b3", "b2"], "全角の Ｍ は半角の M と同じ位置（localeCompare ja）");
assert.deepEqual(all, copy, "引数の配列を変えた");

// 最近開いた本 = 同じ本は先頭へ・別の本棚の同じパスは別の本・上限
let rec = [];
for (const path of ["a", "b", "a"]) rec = pushRecent(rec, { root: "/r", path });
assert.deepEqual(rec.map((r) => r.path), ["a", "b"]);
rec = pushRecent(rec, { root: "/s", path: "a" });
assert.equal(rec.length, 3, "別の本棚の本を同じ本にした");
rec = [];
for (let i = 0; i < 15; i++) rec = pushRecent(rec, { root: "/r", path: String(i) });
assert.equal(rec.length, 12, "上限 12 で切れない");
assert.equal(rec[0].path, "14");

// 読書位置 = 上限 500・古い物から落とす・読み直した本は新しい扱い
let pos = {};
for (let i = 0; i < 500; i++) pos = pushPos(pos, `id${i}`, i);
assert.equal(Object.keys(pos).length, 500);
pos = pushPos(pos, "id0", 7); // id0 を読み直した = 落ちるのは id1
pos = pushPos(pos, "new", 1);
assert.equal(Object.keys(pos).length, 500, "上限で切れない");
assert.equal(pos.id0, 7);
assert.equal(pos.id1, undefined, "古い物から落ちない");
assert.equal(pos.new, 1);

// asset_reply の上限で分ける = つなぐと元の区間・どれも上限以下・空の区間は無し
const MAX = ASSET_MAX;
assert.deepEqual(rangeParts(0, 65536, MAX), [[0, 65536]]);
assert.deepEqual(rangeParts(5, 5, MAX), []);
const parts = rangeParts(100, 100 + MAX * 2 + 7, MAX);
assert.deepEqual(parts, [[100, 100 + MAX], [100 + MAX, 100 + MAX * 2], [100 + MAX * 2, 100 + MAX * 2 + 7]]);
assert.ok(parts.every(([b, e]) => e - b <= MAX && e > b));

assert.equal(fmtSize(1500000000), "1.4 GB");
assert.equal(fmtSize(5 << 20), "5 MB");
assert.equal(fmtSize(100), "1 KB");
assert.equal(hueOf("題名"), hueOf("題名"));
assert.ok(hueOf("題名") >= 0 && hueOf("題名") < 360);

console.log("shelf tree: ok");
