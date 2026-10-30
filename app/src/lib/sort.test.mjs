/* sort.ts の並びを固定する(node app/src/lib/sort.test.mjs・Node 22.18+ の type stripping)。
   core の `ORDER BY name COLLATE NOCASE, id`(NOCASE = ASCII だけ大小無視・UTF-8 バイト順)と同じ並びになること */
import assert from "node:assert/strict";
import { byName, byCount, cmpFor, nocase } from "./sort.ts";

let id = 0;
const mk = (name, nItems = 0) => ({ id: ++id, name, nItems });
const names = (rows, cmp) => rows.slice().sort(cmp).map((r) => r.name);

// ASCII の大小は無視・同名(大小違い)は id 順
const a1 = mk("beta"), a2 = mk("Alpha"), a3 = mk("alpha"), a4 = mk("Gamma");
assert.deepEqual(names([a1, a2, a3, a4], byName), ["Alpha", "alpha", "beta", "Gamma"]);
assert.equal(nocase("ÀBC"), "Àbc"); // 非 ASCII は畳まない(SQLite NOCASE と同じ)

// アクセント・和文: コードポイント順(É > z、和文は ASCII の後ろ、ひらがな < カタカナ < 漢字)
const m = [mk("胆道"), mk("カテーテル"), mk("あいう"), mk("Zeta"), mk("Éclair"), mk("zebra")];
assert.deepEqual(names(m, byName), ["zebra", "Zeta", "Éclair", "あいう", "カテーテル", "胆道"]);

// UTF-16 の `<` と違い、補助面(😀 U+1F600)は BMP 上位(ｱ U+FF71)より後ろ = UTF-8 バイト順
assert.deepEqual(names([mk("😀"), mk("ｱ")], byName), ["ｱ", "😀"]);

// 前方一致は短い方が先
assert.deepEqual(names([mk("abc"), mk("ab")], byName), ["ab", "abc"]);

// 件数順: 多い順・同数は名前順(大小無視)・名前も同じなら id 順
const c = [mk("b", 2), mk("A", 5), mk("a", 5), mk("c", 5), mk("z", 0)];
assert.deepEqual(names(c, byCount), ["A", "a", "c", "b", "z"]);
assert.equal(cmpFor("count"), byCount);
assert.equal(cmpFor("alpha"), byName);
console.log("ok: sort");
