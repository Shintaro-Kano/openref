// dotm-render.js（Startup テンプレート版の整形 = connectd が JavaScriptCore で動かすもの）の node 実弾テスト。
// HTML → 本文 + 書式 run の変換と、本物の citeproc（vendor/citeproc.js）+ 同梱スタイルでの render。
// 実行: node word/test-dotm-render.cjs
"use strict";
const fs = require("fs");
const path = require("path");
const vm = require("vm");
const assert = require("assert");

const dir = __dirname;
vm.runInThisContext(fs.readFileSync(path.join(dir, "vendor/citeproc.js"), "utf8"));
vm.runInThisContext(fs.readFileSync(path.join(dir, "cite-core.js"), "utf8"));
vm.runInThisContext(fs.readFileSync(path.join(dir, "dotm-render.js"), "utf8"));
const EN = fs.readFileSync(path.join(dir, "vendor/locales-en-US.xml"), "utf8");
const style = (id) => fs.readFileSync(path.join(dir, "styles", id + ".csl"), "utf8");
const R = OpenRefDotm;

let failures = 0;
function check(name, fn) {
  try {
    fn();
    console.log("OK   " + name);
  } catch (e) {
    failures++;
    console.log("FAIL " + name + ": " + e.message);
  }
}
/** runs の文字列 → [[本文の部分, 書式]] */
function spans(r) {
  if (!r.runs) return [];
  return r.runs.split(",").map((s) => {
    const [a, b, f] = s.split(":");
    return [r.text.substr(+a, +b), f];
  });
}

// ---------- HTML → run ----------
const CASES = [
  // [名前, html, block, 期待 text, 期待 runs]
  ["上付きの引用", "<sup>1,2</sup>", false, "1,2", "0:3:s"],
  ["書式なし", "(1)", false, "(1)", ""],
  ["打ち消し（font-style:normal）", '<i>Title <span style="font-style:normal;">Species</span> end</i>', false, "Title Species end", "0:6:i,13:4:i"],
  ["入れ子の太字 + 斜体", "<b><i>x</i>y</b>", false, "xy", "0:1:ib,1:1:b"],
  ["small-caps / 下線 / 下付き", '<span style="font-variant:small-caps;">Abc</span><span style="text-decoration:underline;">u</span>H<sub>2</sub>O', false, "AbcuH2O", "0:3:c,3:1:u,5:1:d"],
  ["font-weight:normal は太字を消す", '<b>A<span style="font-weight:normal;">B</span>C</b>', false, "ABC", "0:1:b,2:1:b"],
  ["baseline（citeproc の style=\"baseline\"）", '<sup>1<span style="baseline">x</span></sup>', false, "1x", "0:1:s"],
  ["nocase は書式なし", '<span class="nocase">pH</span> value', false, "pH value", ""],
  ["文字参照", "A &#38; B &#60;C&#62; &amp;&#160;x &#x1F600;", false, "A & B <C> & x \u{1F600}", ""],
  ["リンクは文字だけ", '<a href="https://doi.org/10.1/x">https://doi.org/10.1/x</a>', false, "https://doi.org/10.1/x", ""],
  ["サロゲートペアは 2 単位", "\u{1F600}<i>a</i>", false, "\u{1F600}a", "2:1:i"],
  ["本文中の改行・タブは空白 1 つ・制御文字は落とす", "a\n  b\tc\u0001d", false, "a b cd", ""],
  ["タグでない < は文字", "a < b", false, "a < b", ""],
  ["引用に div が来たら空白で続ける", '<div class="csl-block">A</div><div class="csl-block">B</div>', false, "A B", ""],
  [
    "文献リスト: entry ごとに改行・番号と本文は空白 1 つ（Vancouver の 1.）",
    '  <div class="csl-entry">\n    <div class="csl-left-margin">1.</div><div class="csl-right-inline">Stoffel EM. <i>Gastro</i>.</div>\n  </div>\n  <div class="csl-entry">\n    <div class="csl-left-margin">2.</div><div class="csl-right-inline">Müller J. 小児.</div>\n  </div>\n',
    true,
    "1. Stoffel EM. Gastro.\n2. Müller J. 小児.",
    "15:6:i",
  ],
  [
    "文献リスト: 番号が空白で終わるなら足さない（Radiology の 1. ）",
    '<div class="csl-entry">\n    <div class="csl-left-margin">1. </div><div class="csl-right-inline">A.</div>\n  </div>',
    true,
    "1. A.",
    "",
  ],
  ["文献リスト: csl-block / csl-indent は空白で続ける", '<div class="csl-entry"><div class="csl-block">Title</div>\n<div class="csl-indent">Body</div></div>', true, "Title Body", ""],
  ["文献リスト: 番号なしスタイル（div 1 段）", '<div class="csl-bib-body">\n  <div class="csl-entry">Smith J. <i>A</i>.</div>\n  <div class="csl-entry">Doe K.</div>\n</div>', true, "Smith J. A.\nDoe K.", "9:1:i"],
  ["(no items)", "<p>(no items)</p>", true, "(no items)", ""],
];
for (const [name, html, block, wantText, wantRuns] of CASES) {
  check("htmlToRuns " + name, () => {
    const r = R.htmlToRuns(html, block);
    assert.strictEqual(r.text, wantText);
    assert.strictEqual(r.runs, wantRuns);
  });
}

// ---------- render（本物の citeproc + 同梱スタイル） ----------
const K1 = {
  type: "article-journal",
  title: "Pancreatic cancer: changing epidemiology",
  author: [{ family: "Stoffel", given: "Elena M" }, { family: "Goggins", given: "Michael" }],
  "container-title": "Gastroenterology",
  "container-title-short": "Gastroenterology",
  issued: { "date-parts": [[2023, 2]] },
  volume: "164", issue: "5", page: "752-765",
  DOI: "10.1053/j.gastro.2023.02.012",
};
const K2 = {
  type: "article-journal",
  title: "小児の中枢神経感染症",
  author: [{ family: "Corrêa", given: "José" }],
  "container-title": "日本小児放射線学会雑誌",
  issued: { "date-parts": [[2024]] },
  volume: "44", issue: "8", page: "e230216",
};
const K3 = { ...K1, title: "Old snapshot title", DOI: "10.1000/k3", author: [{ family: "Zed", given: "A" }] };
function input(styleId, cites, extra) {
  return Object.assign({ styleXml: style(styleId), locales: { "en-US": EN }, doiAlways: false, cites: cites, fresh: {} }, extra || {});
}
const P = (...keys) => ({ items: keys.map((k) => ({ key: k, csl: { K1, K2, K3 }[k] })) });

check("render vancouver: 番号は文書順の初出順・複製 CC も 1 個ずつ・アクセントと和文", () => {
  // 文書順 = [K2], [K1,K2], [K2]（複製）→ K2 = 1・K1 = 2
  const out = R.render(input("vancouver", [P("K2"), P("K1", "K2"), P("K2")]));
  assert.deepStrictEqual(out.cites.map((c) => c.text), ["(1)", "(1,2)", "(1)"]);
  assert.strictEqual(out.items, 2);
  const lines = out.bib.text.split("\n");
  assert.strictEqual(lines.length, 2);
  assert.ok(lines[0].startsWith("1. Corrêa J. 小児の中枢神経感染症. 日本小児放射線学会雑誌. 2024;44(8):e230216"), lines[0]);
  assert.ok(lines[1].startsWith("2. Stoffel EM, Goggins M. Pancreatic cancer: changing epidemiology. Gastroenterology."), lines[1]);
  assert.strictEqual(out.bib.runs, "", "Vancouver の文献リストは書式なし");
});

check("render AMA: 上付きの引用と斜体の誌名（run の位置）", () => {
  const out = R.render(input("american-medical-association", [P("K1"), P("K1", "K2")]));
  assert.deepStrictEqual(out.cites.map((c) => [c.text, c.runs]), [["1", "0:1:s"], ["1,2", "0:3:s"]]);
  const sp = spans(out.bib);
  assert.ok(sp.some(([t, f]) => t === "Gastroenterology" && f === "i"), JSON.stringify(sp));
  assert.ok(out.bib.text.startsWith("1. Stoffel EM"), out.bib.text);
});

check("render: 蔵書（fresh）の最新を優先・無い key は snapshot（後の payload の snapshot が勝つ）", () => {
  const fresh = { K1: { ...K1, title: "Fresh title from library" } };
  const oldK3 = { items: [{ key: "K3", csl: { ...K3, title: "First snapshot" } }] };
  const out = R.render(input("vancouver", [P("K1"), oldK3, P("K3")], { fresh }));
  assert.ok(out.bib.text.includes("Fresh title from library"), out.bib.text);
  assert.ok(!out.bib.text.includes("Pancreatic cancer"), "snapshot の題名は使わない");
  assert.ok(out.bib.text.includes("Old snapshot title") && !out.bib.text.includes("First snapshot"), out.bib.text);
});

check("render AJR + doiAlways: DOI を末尾に足す（cite-core の withIdentifiers）", () => {
  const off = R.render(input("american-journal-of-roentgenology", [P("K2")]));
  const k2 = { ...K2, DOI: "10.1000/xyz" };
  const on = R.render(input("american-journal-of-roentgenology", [{ items: [{ key: "K2", csl: k2 }] }], { doiAlways: true }));
  assert.ok(!off.bib.text.includes("DOI"), off.bib.text);
  assert.ok(on.bib.text.endsWith("DOI: 10.1000/xyz"), on.bib.text);
  assert.strictEqual(on.cites[0].text, "[1]");
});

check("render radiology: 番号の後ろの空白はスタイルのまま（足さない）", () => {
  const out = R.render(input("radiology", [P("K1")]));
  assert.ok(out.bib.text.startsWith("1. Stoffel EM"), out.bib.text);
  assert.ok(!out.bib.text.startsWith("1.  "), "空白は 1 つ");
});

check("locales: スタイルの要求する locale（cite-core と同じ）", () => {
  assert.deepStrictEqual(R.locales(style("vancouver"), EN), ["en-US"]);
});

console.log(failures ? `\n${failures} FAILED` : "\nALL PASS");
process.exit(failures ? 1 : 0);
