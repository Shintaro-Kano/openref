/* folders.ts の名前の規則を固定する（node app/src/lib/folders.test.mjs・Node 22.18+ の type stripping）。
   core collections.rs の create_collection / rename_collection（trim・空と `/` は不可・同じ親の下で COLLATE NOCASE 一意）・
   tags.rs（trim・tags.name UNIQUE COLLATE NOCASE）と同じ判定になること */
import assert from "node:assert/strict";
import { folderNameProblem, labelTaken, sameName } from "./folders.ts";

// 同名 = 前後の空白を落として ASCII だけ大小無視（SQLite NOCASE）。非 ASCII の大小は別名
assert.ok(sameName("Liver", " liver "));
assert.ok(sameName("LIVER ct", "liver CT"));
assert.ok(!sameName("Äab", "äab"));
assert.ok(!sameName("Ωmega", "ωmega"));
assert.ok(sameName("胆道", "胆道"));

// 木: Liver(1) › liver(3)、Pancreas(2)、Äab(4)
const cols = [
  { id: 1, name: "Liver", parentId: null },
  { id: 2, name: "Pancreas", parentId: null },
  { id: 3, name: "liver", parentId: 1 },
  { id: 4, name: "Äab", parentId: null },
];

// 空・空白だけ・`/` を含む名前は不可（root でも子でも。ツールバーの「作成して追加」も `A/B` で入れ子を作らない）
assert.equal(folderNameProblem(cols, "", null), "empty");
assert.equal(folderNameProblem(cols, "   ", 1), "empty");
assert.equal(folderNameProblem(cols, "A/B", null), "slash");
assert.equal(folderNameProblem(cols, " / ", null), "slash");
assert.equal(folderNameProblem(cols, "Liver/MRI", 1), "slash");

// 同じ親の下で大小無視一意
assert.equal(folderNameProblem(cols, "LIVER", null), "taken");
assert.equal(folderNameProblem(cols, " pancreas ", null), "taken");
assert.equal(folderNameProblem(cols, "Liver", 1), "taken"); // 子の liver と同名
// 別の親の下の同名は可: サブフォルダにだけ「X」がある状態で root に「X」を作れる（逆も）
const sub = [{ id: 10, name: "Teaching", parentId: null }, { id: 11, name: "X", parentId: 10 }];
assert.equal(folderNameProblem(sub, "X", null), null);
assert.equal(folderNameProblem(sub, "x", 11), null);
assert.equal(folderNameProblem(cols, "Pancreas", 1), null);
// 非 ASCII の大小違いは別名（core の create_collection が「Äab」と「äab」を両方作れるのと同じ）
assert.equal(folderNameProblem(cols, "äab", null), null);
assert.equal(folderNameProblem(cols, "Äab", null), "taken");

// 改名: 自分は数えない（大小だけ変える改名は可）・兄弟とは衝突
assert.equal(folderNameProblem(cols, "LIVER", null, 1), null);
assert.equal(folderNameProblem(cols, "pancreas", null, 1), "taken");
assert.equal(folderNameProblem(cols, "LIVER", 1, 3), null);

// ラベル: 全ラベルの中で大小無視一意（ASCII だけ。Unicode の toLowerCase で比べていた頃は「Äab」があると「äab」を拒否したが、
// DB の UNIQUE COLLATE NOCASE は別名として通す）・空は重複にしない・改名は自分を数えない
const tags = [{ id: 1, name: "Äab" }, { id: 2, name: "Liver" }, { id: 3, name: "胆道" }];
assert.ok(labelTaken(tags, "ÄAB")); // 非 ASCII は畳まず、ASCII の AB だけ畳む → 「Äab」と同名
assert.ok(!labelTaken(tags, "äab"));
assert.ok(labelTaken(tags, " liver "));
assert.ok(labelTaken(tags, "胆道"));
assert.ok(!labelTaken(tags, "   "));
assert.ok(!labelTaken(tags, "LIVER", 2));
assert.ok(labelTaken(tags, "liver", 1));

console.log("ok: folders");
