// OpenNote のエディタ: md の単純な書式（⌘B / ⌘I / ⌘K / リスト）と caret への挿入。
// CodeMirror の値を使うので editor/cm.ts と一緒に動的 import される（WikiEditor が編集を開くまで読まない）。
// WYSIWYG は作らない = 記号を足す・外すだけ

import { EditorSelection, type ChangeSpec } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";

/** 選択を mark で囲む / 外す（⌘B = "**"・⌘I = "*"）。選択が空なら mark の対を置いて間に caret。
    選択の内側の両端か、外側の直前直後に mark があれば外す */
export function toggleWrap(view: EditorView, mark: string): boolean {
  const n = mark.length;
  const doc = view.state.doc;
  const tr = view.state.changeByRange((r) => {
    const sel = doc.sliceString(r.from, r.to);
    if (r.empty) {
      return { changes: { from: r.from, insert: mark + mark }, range: EditorSelection.cursor(r.from + n) };
    }
    // 内側: "**abc**" を選んでいる
    if (sel.length >= 2 * n && sel.startsWith(mark) && sel.endsWith(mark) && !(n === 1 && sel.startsWith(mark + mark) && !sel.startsWith(mark + mark + mark))) {
      return {
        changes: [{ from: r.from, to: r.from + n }, { from: r.to - n, to: r.to }],
        range: EditorSelection.range(r.from, r.to - 2 * n),
      };
    }
    // 外側: "abc" を選び、その前後が "**"
    const before = doc.sliceString(Math.max(0, r.from - n), r.from);
    const after = doc.sliceString(r.to, Math.min(doc.length, r.to + n));
    if (before === mark && after === mark) {
      return {
        changes: [{ from: r.from - n, to: r.from }, { from: r.to, to: r.to + n }],
        range: EditorSelection.range(r.from - n, r.to - n),
      };
    }
    return {
      changes: [{ from: r.from, insert: mark }, { from: r.to, insert: mark }],
      range: EditorSelection.range(r.from + n, r.to + n),
    };
  });
  view.dispatch(view.state.update(tr, { scrollIntoView: true, userEvent: "input" }));
  return true;
}

const URLISH = /^(https?:|mailto:|openref:|file:)\S*$/i;

/** ⌘K: 選択を [選択](|) に（caret は URL の位置）。選択が URL なら [|](URL)・空なら [|]() */
export function insertLink(view: EditorView): boolean {
  const doc = view.state.doc;
  const tr = view.state.changeByRange((r) => {
    const sel = doc.sliceString(r.from, r.to);
    if (sel && URLISH.test(sel.trim())) {
      const insert = `[](${sel.trim()})`;
      return { changes: { from: r.from, to: r.to, insert }, range: EditorSelection.cursor(r.from + 1) };
    }
    const insert = `[${sel}]()`;
    return { changes: { from: r.from, to: r.to, insert }, range: EditorSelection.cursor(r.from + sel.length + 3) };
  });
  view.dispatch(view.state.update(tr, { scrollIntoView: true, userEvent: "input" }));
  return true;
}

const BULLET = /^(\s*)([-*+]) /;
const ORDERED = /^(\s*)(\d+)[.)] /;

/** リストの付け外し（選択の各行）。全行が同じ種類のリストなら外す・それ以外は付ける（番号は 1. から振る） */
export function toggleList(view: EditorView, ordered: boolean): boolean {
  const { state } = view;
  const lines = new Set<number>();
  for (const r of state.selection.ranges) {
    const a = state.doc.lineAt(r.from).number;
    const b = state.doc.lineAt(r.to).number;
    for (let i = a; i <= b; i++) lines.add(i);
  }
  const nums = [...lines].sort((x, y) => x - y);
  const re = ordered ? ORDERED : BULLET;
  const all = nums.every((i) => re.test(state.doc.line(i).text));
  const changes: ChangeSpec[] = [];
  let k = 1;
  for (const i of nums) {
    const line = state.doc.line(i);
    const m = re.exec(line.text);
    if (all && m) {
      changes.push({ from: line.from + m[1].length, to: line.from + m[0].length });
      continue;
    }
    // 他の種類のリスト記号は置き換える
    const other = (ordered ? BULLET : ORDERED).exec(line.text);
    const indent = (other ?? /^(\s*)/.exec(line.text)!)[1].length;
    const mark = ordered ? `${k++}. ` : "- ";
    changes.push({ from: line.from + indent, to: line.from + (other ? other[0].length : indent), insert: mark });
  }
  view.dispatch(state.update({ changes, scrollIntoView: true, userEvent: "input" }));
  return true;
}

/** caret（主の選択）に文字列を入れて、その後ろへ caret を置く。選択があれば置き換える（貼付・ドロップの `![[…]]`）。
    マウスの位置ではなく caret（Tauri の dragDropEnabled では HTML5 の drop がエディタに届かない） */
export function insertAtCaret(view: EditorView, text: string): void {
  const r = view.state.selection.main;
  view.dispatch({
    changes: { from: r.from, to: r.to, insert: text },
    selection: EditorSelection.cursor(r.from + text.length),
    scrollIntoView: true,
    userEvent: "input",
  });
}
