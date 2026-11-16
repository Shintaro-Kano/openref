// CodeMirror のエディタの「焦点の戻し方」の表（.cm-editor の要素 → view.focus()）。wiki/editor/cm.ts の createEditor が登録・destroy で外す。
// CodeMirror に依らない（import しない）= 窓を閉じた後に焦点を戻す focusReturn.ts がこれだけを読む → 起動時の chunk に CodeMirror が入らない
// （CodeMirror は編集を始めたときに WikiEditor の動的 import で読む = 前のまま）
export const editorFocus = new WeakMap<Element, () => void>();
