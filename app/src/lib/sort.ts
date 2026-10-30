/* 名前順・件数順の比較関数(フォルダ・ラベルのサイドバー / Manage の表で共用)。依存なし = node で直接テストする
   (app/src/lib/sort.test.mjs)。型を消すだけで動く TS(enum 等を使わない)であること。 */

/** core の COLLATE NOCASE と同じ畳み方(ASCII だけ大小無視)。兄弟の同名判定と名前順に使う */
export const nocase = (s: string) => s.replace(/[A-Z]/g, (c) => c.toLowerCase());

/** nocase で畳んだ名前をコードポイント順に比べる(SQLite の NOCASE は UTF-8 のバイト順 = コードポイント順。
    UTF-16 の `<` だと BMP 上位の字と補助面の字の順が逆になる) */
function cmpCodePoints(x: string, y: string): number {
  let i = 0;
  let j = 0;
  while (i < x.length && j < y.length) {
    const a = x.codePointAt(i)!;
    const b = y.codePointAt(j)!;
    if (a !== b) return a < b ? -1 : 1;
    i += a > 0xffff ? 2 : 1;
    j += b > 0xffff ? 2 : 1;
  }
  return i < x.length ? 1 : j < y.length ? -1 : 0;
}

/** 名前順(大小無視・同名は id 順)。core の `ORDER BY name COLLATE NOCASE, id` と同じ */
export function byName(a: { name: string; id: number }, b: { name: string; id: number }): number {
  return cmpCodePoints(nocase(a.name), nocase(b.name)) || a.id - b.id;
}

/** 件数の多い順(同数は名前順)。core の `ORDER BY n_items DESC, name COLLATE NOCASE, id` と同じ */
export function byCount(
  a: { name: string; id: number; nItems: number },
  b: { name: string; id: number; nItems: number }
): number {
  return b.nItems - a.nItems || byName(a, b);
}

/** sort("alpha" | "count") の比較関数 */
export function cmpFor(sort: "alpha" | "count") {
  return sort === "count" ? byCount : byName;
}
