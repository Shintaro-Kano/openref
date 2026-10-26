/* UI の表記の言語（Settings › Language・DB の settings.ui_lang = "ja" 既定 / "en"）。
   鍵は日本語の原文: ja は原文そのまま・en は i18n-en.ts の辞書を引き、無ければ原文（落ちない）。
   同じ原文で訳を分けたいときだけ鍵に "::文脈" を付ける（例 t("ページ::thumbnails")。日本語では外して表示する）。
   t() は $state を読むので、テンプレート・$derived の中で呼べば言語の切替で描き直る。
   辞書の網羅 = node app/src/lib/i18n.test.mjs */
import { en } from "./i18n-en";

export const i18n = $state({ lang: "ja" as "ja" | "en" });

export function t(ja: string): string {
  if (i18n.lang === "en" && en[ja] !== undefined) return en[ja];
  const i = ja.indexOf("::");
  return i < 0 ? ja : ja.slice(0, i);
}

/** 件数 + 単位（`${n} ${t(ja)}` と同じ）。英語で 1 のときだけ訳の最初の複数形を単数に（items → item・PDFs → PDF） */
export function tn(n: number, ja: string): string {
  const s = t(ja);
  return `${n} ${i18n.lang === "en" && n === 1 ? s.replace(/\b(item|folder|subfolder|label|name|hit|annotation|PDF)s\b/, "$1") : s}`;
}

/** 起動時の DB の値・メニューの切替（Rust の emit "ui-lang"）の両方から呼ぶ。"en" 以外は日本語 */
export function setLang(v: unknown) {
  i18n.lang = v === "en" ? "en" : "ja";
}
