/* 参照エディタ（ReferenceEditor.svelte）の CSL ⇄ フォーム値の変換。純関数（node の cslForm.test.mjs が固定する）。
   型を消すだけで動く TS であること（Node の type stripping で直接 import される）。 */
import type { FieldDef, TypeDef } from "./api";

export type Kind = FieldDef["kind"];
/** 読んだときの欄の文字列と元の値。欄が読んだときのままなら、保存は元の値をそのまま出す */
export type Orig = Record<string, { text: any; value: any }>;

/** names → 欄の文字列（1 行 1 名）。配列でない値（取込の想定外 = 文字列 1 個など）でも例外を出さない */
export function namesToText(v: any): string {
  if (v == null) return "";
  if (typeof v === "string") return v;
  const arr = Array.isArray(v) ? v : [v];
  return arr
    .map((a) => {
      if (a == null) return "";
      if (typeof a !== "object") return String(a);
      if (a.literal) return String(a.literal);
      return [a.family, a.given].filter(Boolean).join(", ");
    })
    .filter((l) => l !== "")
    .join("\n");
}

export function textToNames(s: string): any[] {
  return String(s)
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean)
    .map((l) => {
      if (l.includes(",")) {
        const [fam, ...rest] = l.split(",");
        const given = rest.join(",").trim();
        return given ? { family: fam.trim(), given } : { family: fam.trim() };
      }
      const parts = l.split(/\s+/);
      if (parts.length === 2 && /^[A-Za-z.\- ]+$/.test(l)) {
        return { family: parts[0], given: parts[1] };
      }
      return { literal: l }; // 和文氏名・団体名はそのまま
    });
}

/** date → 欄の文字列。date-parts（先頭）→ raw → literal の順。文字列・数値はそのまま。例外を出さない */
export function dateToText(d: any): string {
  if (d == null) return "";
  if (typeof d !== "object") return String(d);
  const p = d["date-parts"]?.[0];
  if (Array.isArray(p) && p.some((x: any) => x != null)) return p.filter((x: any) => x != null).join("-");
  return String(d.raw ?? d.literal ?? "");
}

export function textToDate(s: string): any {
  const parts = String(s)
    .trim()
    .split(/[-/.]/)
    .map((x) => parseInt(x, 10))
    .filter((n) => !isNaN(n));
  return parts.length ? { "date-parts": [parts] } : null;
}

/** エディタで開く型。型カタログに無い `article`（1.1.0 までの GUI 取込 stub・Zotero 移行の preprint）は document の
 *  フォームで開く（空のフォームにならない）。core types.rs の canonical_type と同じ読み替え。保存時は core も同じ読み替えをする */
export function editorType(csl: any): string {
  const t = csl?.type || "article-journal";
  return t === "article" ? "document" : t;
}

/** 全型のカタログの キー → kind（同じキーの kind は全型で同じ = core types.rs のテストが固定） */
export function kindsOf(types: TypeDef[]): Map<string, Kind> {
  const m = new Map<string, Kind>();
  for (const tp of types) for (const f of tp.fields) m.set(f.key, f.kind);
  return m;
}

function toText(kind: Kind, val: any): any {
  if (kind === "names") return namesToText(val);
  if (kind === "date") return dateToText(val);
  return val;
}

/** CSL → フォーム値。CSL の全キーのうち、どれかの型のカタログにあるものを、その kind の変換をして読む
 *  （開いた時点の型の欄だけを読むと、型を変えたとき新しい型の欄に旧値が見えない）。
 *  1 つの欄の変換が失敗しても全体を止めない（その欄は JSON の文字列で出す） */
export function cslToForm(csl: any, types: TypeDef[]): { values: Record<string, any>; orig: Orig } {
  const kinds = kindsOf(types);
  const values: Record<string, any> = {};
  const orig: Orig = {};
  if (csl == null || typeof csl !== "object") return { values, orig };
  for (const [key, val] of Object.entries(csl)) {
    const kind = kinds.get(key);
    if (kind == null || val == null) continue;
    let text: any;
    try {
      text = toText(kind, val);
    } catch {
      text = typeof val === "string" ? val : JSON.stringify(val);
    }
    values[key] = text;
    orig[key] = { text, value: val };
  }
  return { values, orig };
}

/** フォーム値 → CSL。現在の型（typeId）の欄だけを出す。欄が読んだときのままなら元の値をそのまま出す
 *  （names の suffix・particle、date の literal / raw、配列でない names 等をフォームの変換で失わない） */
export function formToCsl(
  typeId: string,
  subtype: string,
  values: Record<string, any>,
  orig: Orig,
  types: TypeDef[]
): any {
  const out: any = { type: typeId };
  const fields = types.find((t) => t.id === typeId)?.fields ?? [];
  for (const f of fields) {
    const raw = values[f.key];
    if (raw == null || (typeof raw === "string" && !raw.trim())) continue;
    const o = orig[f.key];
    if (o && o.text === raw) {
      out[f.key] = o.value;
      continue;
    }
    if (f.kind === "names") out[f.key] = textToNames(raw);
    else if (f.kind === "date") {
      const d = textToDate(raw);
      if (d) out[f.key] = d;
    } else out[f.key] = raw;
  }
  if (subtype) out.custom = { subtype };
  return out;
}

/** 値の入っている、その型の任意欄（core でない欄）。開いたとき・型を変えたときに出す欄 */
export function filledExtras(types: TypeDef[], id: string, values: Record<string, any>): string[] {
  return (types.find((t) => t.id === id)?.fields ?? [])
    .filter((f) => !f.core && values[f.key] != null && String(values[f.key]).trim() !== "")
    .map((f) => f.key);
}
