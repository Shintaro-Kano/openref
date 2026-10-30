/* cslForm.ts（参照エディタの CSL ⇄ フォーム値）を node で固定する（`node app/src/lib/cslForm.test.mjs`・Node 22.18+ の type stripping）。
   カタログは手組みの小さな 2 型（core types.rs の形の部分集合）。 */
import assert from "node:assert/strict";
import { namesToText, dateToText, cslToForm, formToCsl, filledExtras, editorType } from "./cslForm.ts";

const f = (key, kind, core) => ({ key, label: key, kind, core });
const types = [
  { id: "article-journal", label: "", desc: "", fields: [
    f("title", "text", true), f("author", "names", true), f("editor", "names", false),
    f("issued", "date", true), f("volume", "text", true), f("URL", "text", false) ] },
  { id: "webpage", label: "", desc: "", fields: [
    f("title", "text", true), f("author", "names", true), f("URL", "text", true),
    f("issued", "date", true), f("accessed", "date", true), f("note", "multiline", false) ] },
];
types.push({ id: "document", label: "", desc: "", fields: [
    f("title", "text", true), f("publisher", "text", true), f("genre", "text", true), f("number", "text", false) ] });
const roundtrip = (csl, to = csl.type) => {
  const { values, orig } = cslToForm(csl, types);
  return formToCsl(to, "", values, orig, types);
};

// 1. 配列でない names・崩れた要素でも例外を出さない（onMount が止まって空の画面で開き、保存で書誌が消える経路を断つ）
assert.equal(namesToText("Smith J"), "Smith J");
assert.equal(namesToText({ family: "Kano", given: "S" }), "Kano, S");
assert.equal(namesToText([null, 3, { family: "A" }, { literal: "日本医学放射線学会" }]), "3\nA\n日本医学放射線学会");
assert.equal(namesToText(undefined), "");
{
  const csl = { type: "article-journal", title: "T", editor: "Smith J" };
  const { values } = cslToForm(csl, types);
  assert.equal(values.editor, "Smith J");
  assert.equal(values.title, "T");
  assert.deepEqual(roundtrip(csl), csl, "触らなければ元の値（文字列の editor）のまま");
}

// 2. date の literal / raw / 文字列は欄に見える形で出す。触らなければ元の値のまま保存（型を変えても）
assert.equal(dateToText({ literal: "Spring 2020" }), "Spring 2020");
assert.equal(dateToText({ raw: "2020 Spring" }), "2020 Spring");
assert.equal(dateToText({ "date-parts": [[]], raw: "x" }), "x");
assert.equal(dateToText({ "date-parts": [[2026, 7, null]] }), "2026-7");
assert.equal(dateToText("2020"), "2020");
assert.equal(dateToText({}), "");
{
  const csl = { type: "article-journal", title: "T", accessed: { literal: "Spring 2020" } };
  const { values } = cslToForm(csl, types);
  assert.equal(values.accessed, "Spring 2020", "開いた型のカタログに無い欄も読む");
  assert.deepEqual(roundtrip(csl, "webpage").accessed, { literal: "Spring 2020" }, "型を変えても見えたまま残る");
  assert.equal(roundtrip(csl).accessed, undefined, "現在の型の欄だけを出す（温存は core の仕事）");
}

// 3. 触らない names は suffix・particle を落とさない。直した欄はフォームの変換を通る
{
  const author = [{ family: "King", given: "Martin Luther", suffix: "Jr." }, { family: "Gogh", given: "Vincent", "non-dropping-particle": "van" }];
  const csl = { type: "article-journal", title: "T", author, issued: { "date-parts": [[2020, 5]] } };
  assert.deepEqual(roundtrip(csl), csl);
  const { values, orig } = cslToForm(csl, types);
  values.author = "Kano, S\n山田太郎";
  values.issued = "2021-02-03";
  const out = formToCsl("article-journal", "review", values, orig, types);
  assert.deepEqual(out.author, [{ family: "Kano", given: "S" }, { literal: "山田太郎" }]);
  assert.deepEqual(out.issued, { "date-parts": [[2021, 2, 3]] });
  assert.deepEqual(out.custom, { subtype: "review" });
  values.issued = "  ";
  assert.equal(formToCsl("article-journal", "", values, orig, types).issued, undefined, "空にした欄は出さない");
}

// 4. どの型のカタログにも無いキー（citation-key・custom）は読まない。値の入った任意欄だけ出す
{
  const csl = { type: "article-journal", title: "T", "citation-key": "k", custom: { a: 1 }, URL: "u", editor: [] , note: " " };
  const { values } = cslToForm(csl, types);
  assert.ok(!("citation-key" in values) && !("custom" in values));
  assert.deepEqual(filledExtras(types, "article-journal", values), ["URL"]);
  assert.deepEqual(filledExtras(types, "webpage", values), [], "webpage の URL は core・note は空白だけ");
}

// 5. 想定外の形でも全体は止まらない
assert.doesNotThrow(() => cslToForm(null, types));
assert.doesNotThrow(() => cslToForm({ type: "webpage", title: 5, author: 7, issued: [2020], accessed: true }, types));

// 型カタログに無い article（旧 GUI 取込 stub・Zotero 移行の preprint）は document のフォームで開き、document の欄が埋まる
{
  const csl = { type: "article", title: "Preprint", publisher: "medRxiv", number: "2024.01.01.1" };
  assert.equal(editorType(csl), "document");
  assert.equal(editorType({ type: "webpage" }), "webpage");
  assert.equal(editorType({}), "article-journal", "型が無いときの既定は従来どおり");
  assert.equal(editorType(null), "article-journal");
  const { values, orig } = cslToForm(csl, types);
  const out = formToCsl(editorType(csl), "", values, orig, types);
  assert.deepEqual(out, { type: "document", title: "Preprint", publisher: "medRxiv", number: "2024.01.01.1" });
}

console.log("cslForm: all ok");
