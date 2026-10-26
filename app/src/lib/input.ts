/* 入力欄の共通処理（TagModals / FolderModals / StyleModal で共用。複製を 1 つにまとめたもの） */

/** IME の変換中（確定の Enter・取消の Esc・候補を選ぶ ↑↓ を含む）。WebKit は確定の Enter を compositionend の後に
    isComposing=false・keyCode=229 で送る = word/cite-dialog.js と同じ判定 */
export const composing = (e: KeyboardEvent) => e.isComposing || e.keyCode === 229;

/** Enter（IME の確定を除く）で fn を呼ぶ */
export function onEnter(e: KeyboardEvent, fn: () => unknown) {
  if (e.key === "Enter" && !composing(e)) {
    e.preventDefault();
    void fn();
  }
}

/** 開いた入力欄へ確実にフォーカスし現在値を全選択（use:focusMe）。autofocus は他要素がフォーカス中だと効かず、
    WKWebView はボタンのクリックでフォーカスを移さないので検索欄・背後のメモ欄に残る → 明示 focus */
export function focusMe(node: HTMLInputElement) {
  queueMicrotask(() => {
    node.focus();
    node.select();
  });
}
