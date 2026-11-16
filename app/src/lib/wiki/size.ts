// OpenNote のファイルの大きさの表示（木の Ingest / Chat log 箱と右ペインのページ情報 = 同じファイルが同じ数字に見える）。
// B（1024 未満）/ KB（小数 0 桁）/ MB・GB（小数 1 桁）。本棚（shelf/tree.ts の fmtSize）は別の桁（lib 直下に共通の物を置くまで）
export function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(1)} GB`;
}
