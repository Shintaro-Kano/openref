// エラーを画面の文（toast・欄）にする。String(e) と同じ文字列化（Error = "Error: …" の前置きつき・Tauri の invoke の失敗 = 文字列のまま・
// undefined / null / オブジェクト = String のとおり）を、UTF-16 の単位で n 字に切る（絵文字・結合文字の途中でも切る）。
// 前置きを剥がさない・改行を畳まない・末尾に … を足さない = 置き換えた String(e).slice(0, n) と 1 字も変わらない。
// n は呼び手ごとの今の値（160 / 200 / 240 / 300）を毎回書く = 既定値を持たない（揃えると切り詰めが変わる所が出る）。
// 前置きを剥がして文だけを出す所（wiki/editor/cite.ts の errText = e.message）は別の処理 = ここには寄せていない
export function errText(e: unknown, n: number): string {
  return String(e).slice(0, n);
}
