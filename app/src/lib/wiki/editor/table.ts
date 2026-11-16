// OpenNote のエディタ: 表（GFM）の中の Tab / Shift+Tab でセルを移る（md だけ）。
// - Tab = 次のセル。行の最後なら次の行の最初のセル。表の最後のセルなら新しい行を足してそこへ。Shift+Tab = 前のセル
// - 範囲は lezer の Table ノード。空のセルには TableCell が無いので、区切りの `|`（TableDelimiter）の位置でセルを決める
//   （空のセルを飛ばさない）。区切りの行（`| --- |`）は見出しの行の後ろとして扱う
// - 表の外は今どおり（false = 次の keymap = indentWithTab の字下げ）。「/」の表の直後の欄の移動（snippet の Tab）は
//   Prec.highest なので先に効く。IME の変換中の Tab は奪わない。`|` をそろえる整形はしない
// caret はセルの中身の終わり（空のセルなら最初の空白の後）に置く = 打てば足される（選択して上書きにはしない）
// 選択が複数行にまたがるとき（複数行を選んで Tab = 字下げ）は取らない。1 行の中の選択（セルの中の 1 語など）は caret と同じくセルを移る。足す行は表の最後の行（本文の行が無ければ区切りの行）の後ろに、
// その行の頭（引用の `> `・リストの字下げ）を写して置く = 引用やリストの中の表でも表の外に出ない

import { syntaxTree } from "@codemirror/language";
import { EditorSelection, type EditorState } from "@codemirror/state";
import type { EditorView, KeyBinding } from "@codemirror/view";

interface Span {
  from: number;
  to: number;
}
interface Row extends Span {
  cells: Span[];
}
type Node = { name: string; from: number; to: number; parent: Node | null; firstChild: Node | null; nextSibling: Node | null };

/** pos を含む Table（両側から探す = 表の頭・行の終わりの caret も） */
function tableAt(state: EditorState, pos: number): Node | null {
  for (const side of [-1, 1] as const) {
    for (let n: Node | null = syntaxTree(state).resolveInner(pos, side) as unknown as Node; n; n = n.parent) {
      if (n.name === "Table") return n;
    }
  }
  return null;
}

/** 行のセル = 区切りの `|` の間（頭と終わりの `|` は省けるので、無ければ行の端までを 1 セルに） */
function cellsOf(row: Node): Span[] {
  const pipes: Span[] = [];
  for (let c = row.firstChild; c; c = c.nextSibling) if (c.name === "TableDelimiter") pipes.push({ from: c.from, to: c.to });
  const cells: Span[] = [];
  let start = row.from;
  let i = 0;
  if (pipes.length && pipes[0].from === row.from) {
    start = pipes[0].to;
    i = 1;
  }
  for (; i < pipes.length; i++) {
    cells.push({ from: start, to: pipes[i].from });
    start = pipes[i].to;
  }
  if (start < row.to) cells.push({ from: start, to: row.to });
  return cells;
}

/** 表の行（見出し + 本文の行）と区切りの行 */
function rowsOf(table: Node): { rows: Row[]; sep: Span | null } {
  const rows: Row[] = [];
  let sep: Span | null = null;
  for (let c = table.firstChild; c; c = c.nextSibling) {
    if (c.name === "TableHeader" || c.name === "TableRow") rows.push({ from: c.from, to: c.to, cells: cellsOf(c) });
    else if (c.name === "TableDelimiter") sep = { from: c.from, to: c.to };
  }
  return { rows, sep };
}

/** セルの中の caret の位置: 中身の終わり・空なら最初の空白の後 */
function caretIn(state: EditorState, c: Span): number {
  const s = state.sliceDoc(c.from, c.to);
  const body = s.trimEnd();
  if (!body.trim()) return c.from + Math.min(1, s.length);
  return c.from + body.length;
}

/** セルを移る（dir = 1 次 / -1 前）。表の外・複数の選択なら false */
export function moveCell(view: EditorView, dir: 1 | -1): boolean {
  const st = view.state;
  const main = st.selection.main;
  if (st.selection.ranges.length !== 1 || st.doc.lineAt(main.from).number !== st.doc.lineAt(main.to).number) return false;
  const pos = main.head;
  const table = tableAt(st, pos);
  if (!table) return false;
  const { rows, sep } = rowsOf(table);
  if (!rows.length) return false;
  let r = rows.findIndex((x) => pos >= x.from && pos <= x.to);
  let i: number;
  if (r >= 0) {
    const cells = rows[r].cells;
    i = cells.findIndex((c) => pos >= c.from && pos <= c.to);
    if (i < 0) i = cells.length && pos < cells[0].from ? -1 : cells.length;
  } else if (sep && pos >= sep.from && pos <= sep.to) {
    r = 0; // 区切りの行 = 見出しの行の最後のセルの後ろ
    i = rows[0].cells.length;
  } else return false;

  let nr = r;
  let ni = i + dir;
  if (dir > 0) {
    while (nr < rows.length && ni >= rows[nr].cells.length) {
      nr++;
      ni = 0;
    }
    if (nr >= rows.length) return addRow(view, rows, sep);
  } else if (ni < 0) {
    nr--;
    if (nr < 0) return true; // 最初のセル = 留まる（表の中の Shift+Tab を字下げに流さない）
    ni = rows[nr].cells.length - 1;
  }
  const cell = rows[nr].cells[Math.max(0, ni)];
  if (!cell) return true;
  view.dispatch({ selection: EditorSelection.cursor(caretIn(st, cell)), scrollIntoView: true, userEvent: "select" });
  return true;
}

/** 表の最後に空の行を足して、その最初のセルへ（列の数 = 見出しの行のセルの数・頭の `|` の有無は見出しの行に合わせる）。
    置く所 = 最後の行と区切りの行のうち後ろの方の行の終わり（本文の行が無い表で見出しと区切りの行の間に入れない）。
    行の頭はその行の頭を写す（`> ` は残し、リストの記号などは同じ幅の空白に） */
function addRow(view: EditorView, rows: Row[], sep: Span | null): boolean {
  const st = view.state;
  const cols = Math.max(1, rows[0].cells.length);
  const lead = st.sliceDoc(rows[0].from, rows[0].from + 1) === "|";
  const row = lead ? "|" + "  |".repeat(cols) : Array.from({ length: cols }, () => " ").join("|");
  const last = rows[rows.length - 1];
  const after = sep && sep.from > last.from ? sep : last;
  const line = st.doc.lineAt(after.from);
  const prefix = st.sliceDoc(line.from, after.from).replace(/[^\s>]/g, " ");
  const at = st.doc.lineAt(after.to).to;
  view.dispatch({
    changes: { from: at, insert: "\n" + prefix + row },
    selection: EditorSelection.cursor(at + 1 + prefix.length + (lead ? 2 : 1)),
    scrollIntoView: true,
    userEvent: "input",
  });
  return true;
}

/** Tab / Shift+Tab（修飾なし）。IME の変換中（isComposing・WebKit の keyCode 229・view.composing）は奪わない */
export const tableKeymap: KeyBinding[] = [
  {
    any(view, e) {
      if (e.key !== "Tab" || e.metaKey || e.ctrlKey || e.altKey) return false;
      if (e.isComposing || e.keyCode === 229 || view.composing) return false;
      return moveCell(view, e.shiftKey ? -1 : 1);
    },
  },
];
