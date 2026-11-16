// 英語辞書の網羅(node app/src/lib/i18n.test.mjs)。app/src の .svelte / .ts を svelte の parser で読み、
// ① t("…") / tn(n, "…") の鍵と core types.rs の日本語ラベル（ReferenceEditor が t(`${label}::type`) で引く）が全部 en 辞書にある
// ② 日本語(CJK)を含む文字列・テンプレートの文が t() の外に残っていない ③ 辞書に使われていない鍵が無い、を確かめる。
// --list で日本語の文字列の一覧(JSON)を stdout に出す
import { parse } from "svelte/compiler";
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url)); // app/src/lib
const src = join(here, "..");
const CJK = /[　-ヿ㐀-鿿＀-￯]/;
/** t() の外に残してよい日本語(画面に出ない = 照合・データ)。[ファイル, 文字列, 理由] */
const KEEP = [
  ["lib/cite.ts", "日本語", "引用の言語の一覧は各言語の自称（Deutsch・한국어 と同じ）"],
  ["lib/cite.ts", "中文 (中国大陆)", "同上"],
  ["lib/cite.ts", "中文 (台灣)", "同上"],
];

/** 古い呼び名（MY LIBRARY 等）を残してよい行 = [ファイル, 行の一部, 理由]。画面に出ないコメントだけ */
const OLD_NAME_KEEP = [["lib/wiki/WikiPage.svelte", "MY LIBRARY の .list と同じ", "CSS のコメント（画面に出ない）"]];

function files(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((d) => {
    const p = join(dir, d.name);
    if (d.isDirectory()) return files(p);
    return /\.(svelte|ts)$/.test(d.name) && d.name !== "i18n-en.ts" && !d.name.endsWith(".d.ts") ? [p] : [];
  });
}

/** 1 ファイルの日本語の文字列。.ts は <script lang="ts"> で包んで同じ parser に通す(行番号は変えない) */
function scan(path) {
  const rel = relative(src, path);
  const text = readFileSync(path, "utf8");
  const head = path.endsWith(".svelte") ? "" : '<script lang="ts">';
  const code = head + text + (head ? "</script>" : "");
  const ast = parse(code, { modern: true });
  const lineOf = (pos) => code.slice(0, pos).split("\n").length;
  const out = [];
  const callee = (n) => (n?.type === "CallExpression" ? (n.callee.type === "Identifier" ? n.callee.name : n.callee.type === "MemberExpression" ? `${n.callee.object.name ?? ""}.${n.callee.property.name ?? ""}` : "") : null);
  /** 鍵の引数の位置（t(鍵) = 0・tn(件数, 鍵) = 1・それ以外 = -1） */
  const keyArg = (n) => ({ t: 0, tn: 1 })[callee(n)] ?? -1;
  function ctx(stack) {
    for (let i = stack.length - 1; i >= 0; i--) {
      const c = callee(stack[i]);
      if (c === "toast" || c?.startsWith("console.")) return c;
      if (stack[i].type === "Attribute") return `attr:${stack[i].name}`;
    }
    return "script";
  }
  function visit(node, stack) {
    if (!node || typeof node !== "object") return;
    if (Array.isArray(node)) return node.forEach((n) => visit(n, stack));
    if (!node.type) return;
    if (node.type === "Comment" || node.type === "StyleSheet") return;
    const parent = stack[stack.length - 1];
    const wrapped = keyArg(parent) >= 0 && parent.arguments[keyArg(parent)] === node;
    if (node.type === "Text" && CJK.test(node.data)) {
      out.push({ file: rel, line: lineOf(node.start), kind: parent?.type === "Attribute" ? `attr:${parent.name}` : "text", text: node.data.trim(), wrapped: false });
    } else if (node.type === "Literal" && typeof node.value === "string" && !node.regex && (wrapped || CJK.test(node.value))) {
      out.push({ file: rel, line: lineOf(node.start), kind: wrapped ? "t" : ctx(stack), text: node.value, wrapped });
    } else if (node.type === "TemplateLiteral" && node.quasis.some((q) => CJK.test(q.value.cooked ?? ""))) {
      out.push({ file: rel, line: lineOf(node.start), kind: wrapped ? "t-template" : ctx(stack), text: code.slice(node.start, node.end), wrapped: false });
    } else if (keyArg(node) >= 0 && node.arguments[keyArg(node)] && node.arguments[keyArg(node)].type !== "Literal") {
      out.push({ file: rel, line: lineOf(node.start), kind: "t-dynamic", text: code.slice(node.start, node.end), wrapped: true });
    }
    stack.push(node);
    for (const [k, v] of Object.entries(node)) if (k !== "loc" && k !== "metadata" && v && typeof v === "object") visit(v, stack);
    stack.pop();
  }
  visit(ast, []);
  return out;
}

/** core types.rs（参照タイプのカタログ）の日本語ラベル → 辞書の鍵 `${label}::type`（テストのモジュールとコメントは除く） */
function typeLabels() {
  const rs = readFileSync(join(src, "../../crates/openref-core/src/types.rs"), "utf8").split("#[cfg(test)]")[0];
  const out = new Set();
  for (const line of rs.split("\n"))
    for (const m of line.replace(/\/\/.*$/, "").matchAll(/"((?:[^"\\]|\\.)*)"/g)) if (CJK.test(m[1])) out.add(`${m[1]}::type`);
  return out;
}

const found = files(src).flatMap(scan);
// 終了は exitCode で（process.exit はパイプへの書き残しを捨てる）
if (process.argv.includes("--list")) console.log(JSON.stringify(found, null, 1));
else await check();

async function check() {
  const { en } = await import("./i18n-en.ts");
  const keys = new Set(found.filter((f) => f.kind === "t").map((f) => f.text));
  const catalog = typeLabels();
  const missing = [...keys, ...catalog].filter((k) => !(k in en));
  const kept = (f) => KEEP.some(([file, s]) => file === f.file && s === f.text);
  const loose = found.filter((f) => !f.wrapped && !f.kind.startsWith("console.") && !kept(f));
  const unused = Object.keys(en).filter((k) => !keys.has(k) && !catalog.has(k));
  for (const k of missing) console.log(`辞書に無い: ${JSON.stringify(k)}`);
  for (const f of loose) console.log(`t() の外: ${f.file}:${f.line} [${f.kind}] ${JSON.stringify(f.text)}`);
  for (const k of unused) console.log(`使われていない鍵: ${JSON.stringify(k)}`);
  // 3 つのモードの呼び名は LIBRARY / NOTE / SHELF = 古い呼び名「MY LIBRARY」「MY NOTE」
  // 「MY SHELF」を app/src の画面の部品・文言（英語の辞書の値も）に残さない。大文字小文字を問わない（英語の値の "My Library" も）。
  // 残してよいのは OLD_NAME_KEEP（画面に出ないコメント）だけ
  const oldName = /\bmy (library|note|shelf)\b/i;
  const old = [];
  for (const path of files(src)) {
    const rel = relative(src, path);
    readFileSync(path, "utf8").split("\n").forEach((line, i) => {
      if (oldName.test(line) && !OLD_NAME_KEEP.some(([f, frag]) => f === rel && line.includes(frag))) old.push(`${rel}:${i + 1}`);
    });
  }
  for (const [k, v] of Object.entries(en)) if (oldName.test(k) || oldName.test(v)) old.push(`i18n-en.ts ${JSON.stringify(k)}`);
  for (const o of old) console.log(`古い呼び名（MY …）: ${o}`);
  // 画面の t() の鍵（= 日本語の表示の字・OpenNote の lib/wiki も）に、許した名前の外の英単語を残さない
  // （画面に出ない toast の文も鍵で守る = 「もう一度 Replace を押して」が漏れた前例）。許す = 固有の名前・形式 / 製品の名前・
  // 開発者向けの印（PdfViewer）・他のアプリの画面の語（Zotero の Export Library）・例の文の名前
  const EN_ALLOW = new Set(("LIBRARY NOTE SHELF LABELS OpenRef OpenNote DOI PMID PMCID ISSN URL BibTeX MeSH PDF PubMed Crossref Google Drive Finder Word " +
    "Markdown LaTeX CSL Claude Code Paperpile Zotero JSON RDF ZIP Export Library files Unpaywall MCP Mac OK html md csl json citeproc bibliography " +
    "en-US https doi org fallback geom Gaballah Ayman European Radiology AJNR Vancouver Enter Esc Tab ID " +
    // チャットの固有の名前（製品）
    "CLI Anthropic " +
    // OpenNote の固有の名前（OS・ファイルの形式 = Markdown の frontmatter・文字の符号化・新規ページの既定の名前 Untitled = ファイル名・
    // slash の項目の ToDo（チェックリストの記法の名前）
    "macOS HTML frontmatter Untitled ToDo").split(" "));
  // 語に切る前に、まとまりで外す固有の名前（コマンド・道具・ファイル名・置き場・キーの組み合わせ・置き換えの印）。
  // 一般の英単語（login・write・auth 等）は単独では許さない
  const CODE_ALLOW = ["claude auth login", "claude auth logout", "claude を", "claude コマンド", "openref-mcp", "wiki_write", "wiki_move", "CLAUDE.md",
    "~/.claude/projects/", "Shift+Enter", "{name}",
    // OpenNote のフォルダの実名（ワークスペースの中のフォルダ = 利用者のデータの置き場）・リンクの形式・符号化
    "ingest/", "chat-log/", "file://", "UTF-8"];
  const enLeft = [];
  for (const f of found) {
    if (!(f.kind === "t" || f.kind === "tn")) continue;
    const bare = CODE_ALLOW.reduce((x, c) => x.split(c).join(" "), f.text.replace(/::[\w-]+$/, ""));
    const words = (bare.match(/[A-Za-z][A-Za-z-]+/g) ?? []).filter((w) => !EN_ALLOW.has(w));
    if (words.length) enLeft.push(`${f.file}:${f.line} ${JSON.stringify(f.text)} [${words.join(", ")}]`);
  }
  for (const o of enLeft) console.log(`日本語の鍵に英単語: ${o}`);
  // 蔵書の 1 件（paper / reference / item）を指す語は「文献」に揃える = 日本語の鍵（t() の鍵と辞書の鍵の全部・lib/wiki|chat|shelf も）に
  // 「論文」「アイテム」を残さない。残してよい物 = WORD_KEEP（理由つき）。「参照」は別の概念（参照フォルダ・参照する・参照エディタ）なので縛らない
  // 鍵を名前で並べる（接尾辞「::type」だけで許すと「ゴミ箱内の論文です::type」のような偽の鍵も通る）
  const WORD_KEEP = [
    // 参照タイプ（core types.rs の型の和名・ReferenceEditor が t(`${label}::type`) で引く）= 種類の名前
    ...["雑誌論文::type", "学術雑誌に掲載された論文::type", "論文題目::type", "学位論文::type", "博士・修士論文::type"].map((n) => [(k) => k === n, "参照タイプの名前"]),
  ];
  const wordLeft = [];
  // 場所は部品の側を優先（辞書にしか無い鍵 = 辞書の名前）
  const jaKeys = new Map([...Object.keys(en).map((k) => [k, "i18n-en.ts"]), ...found.filter((f) => f.kind === "t" || f.kind === "tn").map((f) => [f.text, `${f.file}:${f.line}`])]);
  // 書誌を編集する窓（ReferenceEditor）も蔵書の 1 件 =「文献の編集」「文献を編集」「文献を手動で作成」。
  // 「参照」全体は縛らない（OpenNote の参照フォルダ・動詞の「参照する」・リンク先の参照は別の概念）
  const BOOK_REF = /参照エディタ|参照を(編集|新規作成)/;
  for (const [k, at] of jaKeys) if ((/論文|アイテム/.test(k) || BOOK_REF.test(k)) && !WORD_KEEP.some(([ok]) => ok(k))) wordLeft.push(`${at} ${JSON.stringify(k)}`);
  for (const o of wordLeft) console.log(`日本語の鍵に「論文」「アイテム」「参照エディタ」等（蔵書の 1 件は「文献」）: ${o}`);
  console.log(`t() の鍵 ${keys.size}(呼び出し ${found.filter((f) => f.kind === "t").length})・types.rs のラベル ${catalog.size}・辞書 ${Object.keys(en).length}・辞書に無い ${missing.length}・t() の外 ${loose.length}・未使用 ${unused.length}`);
  process.exitCode = missing.length || loose.length || unused.length || old.length || enLeft.length || wordLeft.length ? 1 : 0;
}
