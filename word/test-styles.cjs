// 誌名スタイル（CSL リポジトリから取得 → core::citestyle で dependent を親に解決した実 CSL）の node 実弾テスト。
// fixture は `openref style get <id>` / `openref style locale <lang>` の出力（app/harness/cite/・CC BY-SA 3.0 の CSL）。
// 実行: node word/test-styles.cjs
const fs = require("fs");
const path = require("path");
const vm = require("vm");
const assert = require("assert");

const dir = __dirname;
const fx = path.join(dir, "../app/harness/cite");
vm.runInThisContext(fs.readFileSync(path.join(dir, "vendor/citeproc.js"), "utf8"));
vm.runInThisContext(fs.readFileSync(path.join(dir, "cite-core.js"), "utf8"));
const en = fs.readFileSync(path.join(dir, "vendor/locales-en-US.xml"), "utf8");
const LOCALES = {
  "en-US": en,
  "de-DE": fs.readFileSync(path.join(fx, "locales-de-DE.xml"), "utf8"),
  "ja-JP": fs.readFileSync(path.join(fx, "locales-ja-JP.xml"), "utf8"),
};

const authors = ["Miyayama", "Yamashiro", "Hashimoto", "Suzuki", "Ikeda", "Toshima", "Matsui"].map((f, i) => ({
  family: f,
  given: "S" + String.fromCharCode(65 + i),
}));
const items = {
  K1: {
    type: "article-journal",
    title: "Ultraselective transcatheter arterial chemoembolization for small hepatocellular carcinoma",
    author: authors,
    "container-title": "CardioVascular and Interventional Radiology",
    "container-title-short": "Cardiovasc Intervent Radiol",
    "journalAbbreviation": "Cardiovasc Intervent Radiol",
    issued: { "date-parts": [[2006, 3]] },
    volume: "29", issue: "1", page: "39-48",
    DOI: "10.1007/s00270-004-0287-1",
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

function text(html) {
  return html.replace(/<div class="csl-left-margin">([^<]*)<\/div>/g, "$1 ").replace(/<[^>]+>/g, "").replace(/\s+/g, " ").trim();
}

// [style id, 期待する本文中の引用形（K1,K2 クラスタ）, 期待する locale 要求]
const CASES = [
  ["european-radiology", /^\[1, 2\]$/, ["en-US"]],
  ["japanese-journal-of-radiology", /^\[1,2\]$/, ["en-US"]],
  ["american-journal-of-neuroradiology", /^1,2$|^<sup>1,2<\/sup>$/, ["en-US"]],
  ["korean-journal-of-radiology", /\b1\b.*\b2\b/, ["en-US"]],
  ["der-radiologe", /^\[1, 2\]$/, ["de-DE"]],
];

let failures = 0;
for (const [id, citeRe, wantLocales] of CASES) {
  try {
    const xml = fs.readFileSync(path.join(fx, id + ".csl"), "utf8");
    assert.ok(!/rel="independent-parent"/.test(xml), "解決済み（親の CSL 本体）であること");
    const req = OpenRefCite.requestedLocales(CSL, xml, en);
    for (const l of wantLocales) assert.ok(req.includes(l), `locale 要求に ${l}（実際: ${req}）`);
    const core = new OpenRefCite.CiteCore(CSL, xml, LOCALES);
    core.setItems(items);
    core.register(["K1", "K2"]);
    const c12 = core.clusterHtml(["K1", "K2"]);
    assert.ok(citeRe.test(c12), `クラスタ ${c12} が ${citeRe}`);
    const bib = text(core.bibliographyHtml());
    assert.ok(bib.includes("Miyayama"), "文献リストに筆頭著者");
    assert.ok(/Cardiovasc Intervent Radiol|CardioVascular/.test(bib), "誌名");
    if (id === "der-radiologe") assert.ok(/et al|u\. ?a\./.test(bib), "7 著者は省略形（de-DE）");
    console.log(`OK  ${id.padEnd(36)} c12=${c12}  locales=${req.join(",")}`);
    console.log(`    ${bib.slice(0, 220)}`);
  } catch (e) {
    failures++;
    console.log(`NG  ${id}: ${e.message}`);
  }
}

// 「DOI / URL を常に含める」: 誌名スタイル。null = スタイル自身が既に DOI を出す（不変）
{
  const DOI = items.K1.DOI;
  const WANT = {
    "european-radiology": null,
    "japanese-journal-of-radiology": null,
    "der-radiologe": null,
    "american-journal-of-neuroradiology": "https://doi.org/" + DOI, // prefix=" https://doi.org/"（前の空白を落とす）
    "korean-journal-of-radiology": "https://doi.org/" + DOI, // DOI の書き方が無い → 既定
  };
  for (const [id, want] of Object.entries(WANT)) {
    try {
      const xml = fs.readFileSync(path.join(fx, id + ".csl"), "utf8");
      const core = new OpenRefCite.CiteCore(CSL, xml, LOCALES);
      core.setItems(items);
      core.register(["K1", "K2"]);
      const split = (html) => {
        const es = html.split(/(?=<div class="csl-entry">)/).filter((s) => s.includes("csl-entry")).map(text);
        return { K1: es.find((t) => t.includes("Miyayama")), K2: es.find((t) => t.includes("Gaballah")) };
      };
      const off = split(core.bibliographyHtml());
      const on = split(core.bibliographyHtml({ doiAlways: true }));
      if (want) assert.ok([off.K1 + " " + want, off.K1 + ". " + want].includes(on.K1), `${off.K1} → ${on.K1}`);
      else assert.strictEqual(on.K1, off.K1);
      assert.strictEqual(on.K1.split(DOI).length - 1, 1, "DOI はちょうど 1 回");
      assert.strictEqual(on.K2, off.K2, "識別子の無い item は不変");
      console.log(`OK  doiAlways ${id.padEnd(36)} ${on.K1.slice(-64)}`);
    } catch (e) {
      failures++;
      console.log(`NG  doiAlways ${id}: ${e.message}`);
    }
  }
}

// 境界: SICI 形の DOI（< > を含む）。doi.org 形のスタイルは citeproc が DOI を URL 符号化して出す
{
  const SICI = "10.1002/(SICI)1522-2586(199901)9:1<44::AID-JMRI6>3.0.CO;2-V";
  const ENC = "10.1002/(SICI)1522-2586(199901)9:1%3C44::AID-JMRI6%3E3.0.CO;2-V";
  for (const [id, want] of [
    ["european-radiology", null], // 既に https://doi.org/<符号化 DOI> を出す → 二重に付けない
    ["korean-journal-of-radiology", "https://doi.org/" + ENC], // 付け足す側も citeproc と同じ符号化
  ]) {
    try {
      const xml = fs.readFileSync(path.join(fx, id + ".csl"), "utf8");
      const core = new OpenRefCite.CiteCore(CSL, xml, LOCALES);
      core.setItems({ K1: { ...items.K1, DOI: SICI } });
      core.register(["K1"]);
      const off = text(core.bibliographyHtml());
      const on = text(core.bibliographyHtml({ doiAlways: true }));
      if (want) assert.ok([off + " " + want, off + ". " + want].includes(on), `${off} → ${on}`);
      else assert.strictEqual(on, off);
      assert.strictEqual(on.split("AID-JMRI6").length - 1, 1, "DOI はちょうど 1 回: " + on);
      console.log(`OK  doiAlways SICI ${id.padEnd(31)} ${on.slice(-72)}`);
    } catch (e) {
      failures++;
      console.log(`NG  doiAlways SICI ${id}: ${e.message}`);
    }
  }
}

// 言語の上書き（Paperpile の Language）: 英文誌スタイルを ja-JP で強制
{
  const xml = fs.readFileSync(path.join(fx, "japanese-journal-of-radiology.csl"), "utf8");
  const eng = new CSL.Engine(
    { retrieveLocale: (l) => LOCALES[l] || en, retrieveItem: (k) => ({ ...items[k], id: k }) },
    xml,
    "ja-JP",
    true
  );
  eng.updateItems(["K1"]);
  const bib = text(eng.makeBibliography()[1].join(""));
  if (/2006年/.test(bib)) console.log(`OK  forceLang ja-JP                     ${bib.slice(0, 120)}`);
  else {
    failures++;
    console.log(`NG  forceLang ja-JP: ${bib}`);
  }
}

if (failures) {
  console.log(`\n${failures} 件失敗`);
  process.exit(1);
}
console.log("\nALL JOURNAL STYLES PASS");
