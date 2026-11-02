// OpenNote のエディタの改行コード（CodeMirror に依存しない = セッション側と共用）。
// エディタの中は常に \n（CodeMirror の既定 = \r\n・\r・\n のどれでも行を分ける）。書き出すときにファイルの改行コードへ戻す

/** ファイルの改行コード（\r\n を含めば CRLF・それ以外は LF） */
export const eolOf = (s: string): string => (s.includes("\r\n") ? "\r\n" : "\n");

/** 改行をすべて eol に揃える（孤立した \r も = エディタが行を分けるのと同じ規則） */
export const withEol = (s: string, eol: string): string => s.split(/\r\n|\r|\n/).join(eol);
