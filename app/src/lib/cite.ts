/* citeproc-js ラッパ（引用コピー・編集画面のライブプレビュー・スタイル設定のプレビュー）。
   citeproc は 1MB 近いので動的ロード＋シングルトン（public/cite/citeproc.js）。
   CSL と locale は Rust（core::citestyle）から取る: 同梱 4 種 + CSL リポジトリ約 1 万誌（必要時に取得・キャッシュ・
   dependent は親で解決済み）。locale は citeproc が同期で要求するため、1 度目の構築で要求された言語を記録 →
   取得 → 本番の構築、の 2 段で用意する（取れない言語は en-US の語彙で代用）。
   「DOI / URL を常に含める」は Word アドインと同じ実装（word/cite-core.js の withIdentifiers）を使う。 */
import { api, type StyleEntry } from "./api";
// OpenRefCite（globalThis）を定義する。node テスト（word/test-*.cjs）と connectd の Word 整形（word_v2.rs）も同じファイル
import "../../../word/cite-core.js";
import { t } from "./i18n.svelte";

const OpenRefCite = (globalThis as any).OpenRefCite as {
  withIdentifiers: (entries: string[], entryIds: string[][], itemsById: Record<string, any>, styleXml: string) => string[];
};

/** 言語の上書き（Paperpile の Language）。"" = スタイルの既定。CSL locales リポジトリの収録言語 */
export const LOCALES: [string, string][] = [
  ["", ""], // 表示は StyleModal（t("既定（スタイルに従う）")）
  ["en-US", "English (US)"],
  ["en-GB", "English (UK)"],
  ["ja-JP", "日本語"],
  ["de-DE", "Deutsch (Deutschland)"],
  ["de-AT", "Deutsch (Österreich)"],
  ["de-CH", "Deutsch (Schweiz)"],
  ["fr-FR", "Français (France)"],
  ["fr-CA", "Français (Canada)"],
  ["es-ES", "Español (España)"],
  ["es-MX", "Español (México)"],
  ["es-CL", "Español (Chile)"],
  ["it-IT", "Italiano"],
  ["pt-BR", "Português (Brasil)"],
  ["pt-PT", "Português (Portugal)"],
  ["nl-NL", "Nederlands"],
  ["ko-KR", "한국어"],
  ["zh-CN", "中文 (中国大陆)"],
  ["zh-TW", "中文 (台灣)"],
  ["ru-RU", "Русский"],
  ["pl-PL", "Polski"],
  ["cs-CZ", "Čeština"],
  ["sk-SK", "Slovenčina"],
  ["hu-HU", "Magyar"],
  ["ro-RO", "Română"],
  ["bg-BG", "Български"],
  ["hr-HR", "Hrvatski"],
  ["sl-SI", "Slovenščina"],
  ["uk-UA", "Українська"],
  ["el-GR", "Ελληνικά"],
  ["tr-TR", "Türkçe"],
  ["da-DK", "Dansk"],
  ["sv-SE", "Svenska"],
  ["nb-NO", "Norsk bokmål"],
  ["nn-NO", "Norsk nynorsk"],
  ["fi-FI", "Suomi"],
  ["is-IS", "Íslenska"],
  ["et-EE", "Eesti keel"],
  ["lv-LV", "Latviešu"],
  ["lt-LT", "Lietuvių kalba"],
  ["ca-AD", "Català"],
  ["eu", "Euskara"],
  ["gl-ES", "Galego"],
  ["cy-GB", "Cymraeg"],
  ["la", "Latina"],
  ["af-ZA", "Afrikaans"],
  ["ar", "العربية"],
  ["he-IL", "עברית"],
  ["fa-IR", "فارسی"],
  ["hi-IN", "हिंदी"],
  ["th-TH", "ไทย"],
  ["vi-VN", "Tiếng Việt"],
  ["id-ID", "Bahasa Indonesia"],
  ["ms-MY", "Bahasa Melayu"],
  ["tl-PH", "Tagalog"],
  ["km-KH", "ភាសាខ្មែរ"],
  ["mn-MN", "Монгол"],
  ["hy-AM", "Հայերեն"],
];

let cslLoaded: Promise<any> | null = null;
const styleCache = new Map<string, string>();
/** 取得済み locale（取れなかった言語は null を入れて再試行しない） */
const localeCache = new Map<string, string | null>();

async function loadCSL(): Promise<any> {
  if ((globalThis as any).CSL) return (globalThis as any).CSL;
  if (!cslLoaded) {
    cslLoaded = new Promise((resolve, reject) => {
      const s = document.createElement("script");
      s.src = "/cite/citeproc.js";
      s.onload = () => resolve((globalThis as any).CSL);
      s.onerror = () => reject(new Error(t("citeproc の読み込みに失敗")));
      document.head.appendChild(s);
    });
  }
  return cslLoaded;
}

/** .csl を読み込み直したときなど、同じ id の XML を取り直させる */
export function forgetStyle(id: string) {
  styleCache.delete(id);
}

/** 解決済み CSL（未取得なら Rust 側がリポジトリから取得してキャッシュ） */
export async function styleXml(id: string): Promise<string> {
  const hit = styleCache.get(id);
  if (hit) return hit;
  const x = await api.citeStyleXml(id);
  styleCache.set(id, x);
  return x;
}

async function localeXml(lang: string): Promise<string | null> {
  if (localeCache.has(lang)) return localeCache.get(lang) ?? null;
  try {
    const x = await api.citeLocaleXml(lang);
    localeCache.set(lang, x);
    return x;
  } catch (e) {
    // 収録外（404）は以後問い合わせない。オフライン等の一時的な失敗は覚えず次回また取りに行く
    if (/HTTP 404|不正/.test(String(e))) localeCache.set(lang, null);
    lastMissing.add(lang);
    return null; // en-US の語彙で代用
  }
}

/** 直近の整形で取れなかった locale（en-US の語彙で代用した言語。UI の注意書き用） */
let lastMissing = new Set<string>();
export function missingLocales(): string[] {
  return [...lastMissing];
}

async function engineFor(styleId: string, items: Record<string, any>, lang = "") {
  const CSL = await loadCSL();
  const xml = await styleXml(styleId);
  const en = (await localeXml("en-US"))!;
  const withIds: Record<string, any> = {};
  for (const [k, v] of Object.entries(items)) withIds[k] = { ...v, id: k };
  const make = (requested?: Set<string>) =>
    new CSL.Engine(
      {
        retrieveLocale: (l: string) => {
          requested?.add(l);
          return localeCache.get(l) ?? en;
        },
        retrieveItem: (id: string) => withIds[id],
      },
      xml,
      lang || undefined,
      !!lang
    );
  const requested = new Set<string>();
  const eng = make(requested);
  lastMissing = new Set([...requested].filter((l) => localeCache.has(l) && localeCache.get(l) === null));
  const missing = [...requested].filter((l) => !localeCache.has(l));
  if (missing.length === 0) return eng;
  await Promise.all(missing.map(localeXml));
  return make();
}

function stripHtml(html: string): string {
  const d = document.createElement("div");
  d.innerHTML = html;
  // 番号付きスタイルは番号（csl-left-margin）と本文（csl-right-inline）が別 div。textContent だと
  // "1.Miyayama" と詰まるので間に空白を入れる（連続空白は下で 1 つに畳む）
  d.querySelectorAll(".csl-left-margin").forEach((el) => el.append(" "));
  return (d.textContent || "").replace(/\s+/g, " ").trim();
}

/** 文献リスト形式の 1 件（コピー用・プレーンテキスト）。doiAlways = DOI（無ければ URL）を常に含める */
export async function formatBibliography(
  csl: any,
  styleId = "vancouver",
  lang = "",
  doiAlways = false
): Promise<string> {
  return formatBibliographyMany([{ key: "ITEM", csl }], styleId, lang, doiAlways);
}

/** 複数件の文献リスト（一括コピー用）。番号付きスタイルは items の順で採番される */
export async function formatBibliographyMany(
  items: { key: string; csl: any }[],
  styleId = "vancouver",
  lang = "",
  doiAlways = false
): Promise<string> {
  if (items.length === 0) return "";
  const map: Record<string, any> = {};
  for (const it of items) map[it.key] = it.csl;
  const eng = await engineFor(styleId, map, lang);
  eng.updateItems(items.map((it) => it.key));
  const res = eng.makeBibliography();
  // 本文中の引用形しか定義していないスタイル（リポジトリに約 150）は false を返す
  if (res === false) throw new Error(t("このスタイルには文献リスト（bibliography）の定義がありません。別のスタイルを選んでください"));
  if (!res || !res[1] || !res[1].length) return "";
  const entries: string[] = doiAlways
    ? OpenRefCite.withIdentifiers(res[1], res[0].entry_ids, map, await styleXml(styleId))
    : res[1];
  return entries.map((html) => stripHtml(html)).join("\n");
}

/** 本文中の引用形（(1) / [1] / (Smith 2020) など。プレビュー用） */
export async function formatCitation(csl: any, styleId = "vancouver", lang = ""): Promise<string> {
  const key = "ITEM";
  const eng = await engineFor(styleId, { [key]: csl }, lang);
  eng.updateItems([key]);
  return stripHtml(eng.makeCitationCluster([{ id: key }]));
}

/** 索引の絞り込み（語は AND・title / 略称 / id の部分一致・大小無視）。完全一致 → 前方一致 → 部分一致 → 短い順 */
export function searchStyles(catalog: StyleEntry[], query: string, limit = 200): StyleEntry[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [];
  const q = words.join(" ");
  const scored: { s: StyleEntry; rank: number }[] = [];
  for (const s of catalog) {
    const title = s.title.toLowerCase();
    const short = (s.short ?? "").toLowerCase();
    const hay = `${title} ${short} ${s.id}`;
    if (!words.every((w) => hay.includes(w))) continue;
    const rank =
      title === q || short === q ? 0 : title.startsWith(q) || short.startsWith(q) ? 1 : hay.includes(q) ? 2 : 3;
    scored.push({ s, rank });
  }
  scored.sort((a, b) => a.rank - b.rank || a.s.title.length - b.s.title.length || a.s.title.localeCompare(b.s.title));
  return scored.slice(0, limit).map((x) => x.s);
}
