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
  console.log(`t() の鍵 ${keys.size}(呼び出し ${found.filter((f) => f.kind === "t").length})・types.rs のラベル ${catalog.size}・辞書 ${Object.keys(en).length}・辞書に無い ${missing.length}・t() の外 ${loose.length}・未使用 ${unused.length}`);
  process.exitCode = missing.length || loose.length || unused.length ? 1 : 0;
}
