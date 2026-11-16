// 窓を閉じた後に、開く前の焦点へ戻す（救出の窓 WikiRescue と取込の窓 ImportModal、「＋ 追加 ▾」の浮き枠 AddMenu が使う。骨格 Modal は使わない = ほかの窓は不変）。
// 窓が開いたとき（枠が DOM に入り、まだ枠へ焦点を移す前）に holdFocus(枠) を呼び、返った関数を枠が DOM から外れるときに呼ぶ。
// 戻すのは次の全部を満たすときだけ（どれかが外れたら何もしない = body のまま・別の要素へ勝手に移さない）:
//   - 開いたときに焦点があった（body でない）・その要素がまだ DOM にあり、見えていて、無効でない（disabled / inert でない）
//   - 閉じる時点の焦点が窓の中か body（開いている間に利用者が窓の外の要素へ移したなら、そこに置いたまま）
//   - 戻す時点（閉じた後の microtask）で、ほかの要素が焦点を取っていない・窓（覆い）が増えていない（閉じるのと同時に開いた別の窓 =
//     取込 → 参照エディタ・救出 → 競合 など。その窓の焦点を奪わない）・前面のタブが開いたときと同じ
// covered() だけでは決めない: 下に別の窓（参照エディタ等）が開いたままのときも、その窓の欄へ戻したい（覆いの数で「新しく開いた」を見る）。
// CodeMirror へは view.focus()（focusPreventScroll + 選択を状態から描き直す = カーソル・選択・スクロールが開く前のまま）。ほかは focus({ preventScroll })。
// CodeMirror は import しない（静的に書くと起動時の chunk に入る = 約 240 KB）: エディタの側（cm.ts）が登録した戻し方を focusTargets の表から引く
// = 同期のまま戻せる（動的 import の待ちの間に打った字の行き先を気にしなくてよい）

import { editorFocus } from "./focusTargets";
import { app } from "./state.svelte";

/** 窓（覆い）の要素 = 本体 / OpenNote / チャットの窓の .overlay・引用の窓・画像を大きく見る覆い */
const WINDOWS = ".overlay, .wk-cite-back, .wk-lb";

function shown(el: HTMLElement): boolean {
  const cv = (el as HTMLElement & { checkVisibility?: () => boolean }).checkVisibility;
  if (typeof cv === "function") return cv.call(el);
  for (let e: HTMLElement | null = el; e; e = e.parentElement) {
    const cs = getComputedStyle(e);
    if (cs.display === "none" || cs.visibility === "hidden") return false;
  }
  return true;
}

/** frame = 開いた窓の枠。戻り値 = 窓が閉じたとき（枠が DOM から外れるとき）に呼ぶ関数 */
export function holdFocus(frame: HTMLElement): () => void {
  const a0 = document.activeElement;
  const from = a0 instanceof HTMLElement && a0 !== document.body && !frame.contains(a0) ? a0 : null;
  const tab = app.tab;
  // エディタ（.cm-editor）の中の DOM の選択範囲を写して覚える。覆いの上の押下はブラウザが DOM の選択範囲を覆いへ移す → 戻すときに
  // CodeMirror は画面の外の選択を DOM に書けず、描かれていない区間（cm-gap）に残った DOM の選択範囲を読んで選択を 0 にしていた
  // = 戻す直前に元の DOM の選択範囲を置き直してから CodeMirror の焦点を戻す（Esc / キャンセルで閉じたときと同じ状態にする）
  const ed0 = from?.closest(".cm-editor") ?? null;
  const s0 = window.getSelection();
  const r0 = ed0 && s0 && s0.rangeCount && ed0.contains(s0.getRangeAt(0).startContainer) ? s0.getRangeAt(0).cloneRange() : null;
  const own = frame.closest(WINDOWS);
  const others = () => [...document.querySelectorAll(WINDOWS)].filter((w) => w !== own).length;
  const base = others();
  return () => {
    if (!from) return;
    const a = document.activeElement;
    if (a && a !== document.body && !frame.contains(a)) return; // 開いている間に窓の外へ移した
    queueMicrotask(() => {
      const cur = document.activeElement;
      if (cur && cur !== document.body && !frame.contains(cur)) return; // 閉じる間にほかが焦点を取った
      if (others() > base) return; // 閉じるのと同時に別の窓が開いた
      if (app.tab !== tab) return;
      if (!from.isConnected || !shown(from) || (from as HTMLInputElement).disabled || from.closest("[inert]")) return;
      const ed = from.closest(".cm-editor");
      const viaEditor = ed ? editorFocus.get(ed) : undefined;
      if (viaEditor) {
        const s = window.getSelection();
        if (r0 && s && r0.startContainer.isConnected && r0.endContainer.isConnected && ed?.contains(r0.startContainer)) {
          s.removeAllRanges();
          s.addRange(r0);
        }
        viaEditor();
      } else from.focus({ preventScroll: true });
    });
  };
}
