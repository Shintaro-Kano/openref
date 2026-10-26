// cite-core の node 実弾テスト: 4 スタイル × クラスタ整形 × bibliography
// 実行: node word/test-cite.cjs
const fs = require("fs");
const path = require("path");
const vm = require("vm");
const assert = require("assert");

const dir = __dirname;
vm.runInThisContext(fs.readFileSync(path.join(dir, "vendor/citeproc.js"), "utf8"));
vm.runInThisContext(fs.readFileSync(path.join(dir, "cite-core.js"), "utf8"));
const locale = fs.readFileSync(path.join(dir, "vendor/locales-en-US.xml"), "utf8");

const items = {
  K1: {
    type: "article-journal",
    title: "Pancreatic cancer: changing epidemiology",
    author: [{ family: "Stoffel", given: "Elena M" }, { family: "Goggins", given: "Michael" }],
    "container-title": "Gastroenterology",
    "container-title-short": "Gastroenterology",
    issued: { "date-parts": [[2023, 2]] },
    volume: "164", issue: "5", page: "752-765",
    DOI: "10.1053/j.gastro.2023.02.012", PMID: "36804602",
  },
  K2: {
    type: "article-journal",
    title: "The peritoneum: anatomy and patterns of disease spread",
    author: [{ family: "Gaballah", given: "Ayman H" }],
    "container-title": "RadioGraphics",
    "container-title-short": "Radiographics",
    issued: { "date-parts": [[2024]] },
    volume: "44", issue: "8", page: "e230216",
  },
};

let failures = 0;
for (const styleId of ["vancouver", "american-medical-association", "radiology", "american-journal-of-roentgenology"]) {
  const styleXml = fs.readFileSync(path.join(dir, "styles", styleId + ".csl"), "utf8");
  try {
    const core = new OpenRefCite.CiteCore(CSL, styleXml, locale);
    core.setItems(items);
    core.register(["K1", "K2"]);
    const c1 = core.clusterHtml(["K1"]);
    const c12 = core.clusterHtml(["K1", "K2"]);
    const bib = core.bibliographyHtml();
    assert(c1.includes("1"), `${styleId}: c1 に 1 がない: ${c1}`);
    assert(c12.includes("2"), `${styleId}: c12 に 2 がない: ${c12}`);
    assert((bib.match(/csl-entry/g) || []).length === 2, `${styleId}: bib entry 数`);
    assert(bib.includes("Stoffel"), `${styleId}: bib に著者がない`);
    console.log(`OK  ${styleId.padEnd(36)} c1=${c1}  c12=${c12}`);
  } catch (e) {
    failures++;
    console.log(`FAIL ${styleId}: ${e.message}`);
  }
}
// ---- 「DOI / URL を常に含める」（withIdentifiers）: 同梱 4 スタイル ----
// D1 = DOI + doi.org の URL（典型）/ D2 = DOI なし・J-STAGE の URL だけ（和文誌の型）/ D3 = 識別子なし
const DOI = "10.1053/j.gastro.2023.02.012";
const JSTAGE = "https://www.jstage.jst.go.jp/article/jspr/42/1/42_5/_pdf/-char/ja";
const idItems = {
  D1: { ...items.K1, URL: "http://dx.doi.org/" + DOI },
  D2: {
    type: "article-journal",
    title: "小児の中枢神経感染症",
    author: [{ family: "山田", given: "太郎" }],
    "container-title": "日本小児放射線学会雑誌",
    issued: { "date-parts": [[2026]] },
    volume: "42", issue: "1", page: "5-12",
    URL: JSTAGE,
  },
  D3: items.K2,
};
function plain(html) {
  return html.replace(/<div class="csl-left-margin">([^<]*)<\/div>/g, "$1 ").replace(/<[^>]+>/g, "").replace(/&#38;/g, "&").replace(/\s+/g, " ").trim();
}
/** entry ごとのプレーンテキスト（著者で引く） */
function entries(core, on) {
  const html = core.bibliographyHtml(on ? { doiAlways: true } : undefined);
  const out = {};
  html.split(/(?=<div class="csl-entry">)/).filter((s) => s.includes("csl-entry")).forEach((e) => {
    const t = plain(e);
    out[t.includes("Stoffel") ? "D1" : t.includes("山田") ? "D2" : "D3"] = { t, html: e };
  });
  return out;
}
const count = (hay, needle) => hay.toLowerCase().split(needle.toLowerCase()).length - 1;
// [style, D1 に付け足されるべき文字列（null = スタイル自身が既に DOI を出すので不変）, D2 に URL を付け足すか]
const DOI_CASES = [
  ["vancouver", null, false],
  ["american-medical-association", null, false],
  ["radiology", null, false],
  ["american-journal-of-roentgenology", "DOI: " + DOI, true],
];
for (const [styleId, wantD1, wantD2] of DOI_CASES) {
  const styleXml = fs.readFileSync(path.join(dir, "styles", styleId + ".csl"), "utf8");
  try {
    const core = new OpenRefCite.CiteCore(CSL, styleXml, locale);
    core.setItems(idItems);
    core.register(["D1", "D2", "D3"]);
    const off = entries(core, false);
    const on = entries(core, true);
    // 付け足しは末尾に「区切り + 識別子」だけ（他は 1 文字も変えない）
    const added = (k, want) => {
      if (!want) return assert.strictEqual(on[k].t, off[k].t, `${k} は不変のはず: ${on[k].t}`);
      assert.ok([off[k].t + " " + want, off[k].t + ". " + want].includes(on[k].t), `${k}: ${off[k].t} → ${on[k].t}`);
    };
    added("D1", wantD1);
    added("D2", wantD2 ? JSTAGE : null);
    added("D3", null);
    assert.strictEqual(count(on.D1.t, DOI), 1, "DOI はちょうど 1 回");
    assert.strictEqual(count(on.D2.t, JSTAGE), 1, "URL はちょうど 1 回");
    // 番号付きは本文側（csl-right-inline）の中に入る
    if (wantD1 && on.D1.html.includes("csl-right-inline"))
      assert.ok(new RegExp(wantD1.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&") + "</div>\\s*</div>").test(on.D1.html), on.D1.html);
    console.log(`OK  doiAlways ${styleId.padEnd(36)} ${on.D1.t.slice(-60)}`);
  } catch (e) {
    failures++;
    console.log(`FAIL doiAlways ${styleId}: ${e.message}`);
  }
}
// 境界: citeproc は ' を ’ に変える / DOI が URL 形で入っている（編集画面の保存前プレビュー）
const SICI = "10.1002/(SICI)1522-2586(199901)9:1<44::AID-JMRI6>3.0.CO;2-V";
for (const [styleId, doi, wantTail] of [
  ["american-medical-association", "10.1000/it's-1", null], // AMA は DOI を出す → ’ でも不変
  ["vancouver", "10.1000/it's-1", null],
  ["american-journal-of-roentgenology", "https://doi.org/" + DOI, "DOI: " + DOI], // URL 形は素の DOI に
  ["american-journal-of-roentgenology", SICI, "DOI: " + SICI], // DOI: 形は符号化しない（HTML では &#60; に）
]) {
  try {
    const styleXml = fs.readFileSync(path.join(dir, "styles", styleId + ".csl"), "utf8");
    const core = new OpenRefCite.CiteCore(CSL, styleXml, locale);
    core.setItems({ D1: { ...items.K1, DOI: doi } });
    core.register(["D1"]);
    const off = plain(core.bibliographyHtml());
    const onHtml = core.bibliographyHtml({ doiAlways: true });
    const on = plain(onHtml).replace(/&#60;/g, "<").replace(/&#62;/g, ">");
    if (!wantTail) assert.strictEqual(on, off.replace(/&#60;/g, "<").replace(/&#62;/g, ">"), `不変のはず: ${on}`);
    else assert.ok(on.endsWith(". " + wantTail) || on.endsWith(" " + wantTail), on);
    assert.ok(!/<[^>]*44::/.test(onHtml), "DOI の < はタグにならない");
    console.log(`OK  doiAlways edge ${styleId.padEnd(34)} ${on.slice(-70)}`);
  } catch (e) {
    failures++;
    console.log(`FAIL doiAlways edge ${styleId} ${doi}: ${e.message}`);
  }
}

// スタイルの DOI 表記の読み取り（前置の区切りは落とす・無ければ https://doi.org/）
for (const [xml, want] of [
  ['<text variable="DOI" prefix="DOI: "/>', "DOI: "],
  ['<text prefix="doi:" variable="DOI"/>', "doi:"],
  ['<text prefix=". https://doi.org/" variable="DOI"/>', "https://doi.org/"],
  ['<text variable="DOI" prefix=" https://doi.org/"/>', "https://doi.org/"],
  ['<text variable="DOI"/>', "https://doi.org/"],
  ["", "https://doi.org/"],
]) {
  const got = OpenRefCite.doiPrefix(xml);
  if (got === want) console.log(`OK  doiPrefix ${JSON.stringify(xml).padEnd(54)} → ${got}`);
  else {
    failures++;
    console.log(`FAIL doiPrefix ${xml}: ${got} (want ${want})`);
  }
}

console.log(failures ? `\n${failures} style(s) FAILED` : "\nALL STYLES PASS");
process.exit(failures ? 1 : 0);
