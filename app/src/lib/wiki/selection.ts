// OpenNote のチャットの文脈に添える選択の読み取り（DOM だけ = 状態を持たない）。中央のページの表示（shadow DOM）とエディタ（CodeMirror）。
// chat.svelte.ts から移しただけ（B-34）。選択を chat.sel に入れる判断（readSelection）は chat.svelte.ts

interface EditorLike {
  view: { dom: HTMLElement; state: { selection: { main: { from: number; to: number } }; sliceDoc(a: number, b: number): string } };
}
let editor: EditorLike | null = null;
/** WikiPage のエディタ（CodeMirror の選択は DOM の選択より正確 = 長い選択で行が描かれていない部分も取れる）。閉じたら null */
export function setChatEditor(h: EditorLike | null) {
  editor = h;
}

/** 影の木を越えて祖先の .wk-page を探す */
export function inPage(n: Node | null): boolean {
  let el: Node | null = n;
  while (el) {
    const e = el instanceof Element ? el : el.parentElement;
    if (e?.closest(".wk-page")) return true;
    const root = el.getRootNode?.();
    el = root && root !== el && (root as ShadowRoot).host ? (root as ShadowRoot).host : null;
  }
  return false;
}

/** 選択の文字列。表示（shadow DOM）は getComposedRanges（WebKit）→ shadowRoot.getSelection（Chromium）→ 素の toString の順 */
export function selectionText(s: Selection): { text: string; node: Node | null } {
  const node = s.anchorNode;
  if (editor && node && editor.view.dom.isConnected && editor.view.dom.contains(node)) {
    const m = editor.view.state.selection.main;
    return { text: editor.view.state.sliceDoc(m.from, m.to), node };
  }
  const roots = [...document.querySelectorAll(".wk-page .wk-md")].map((h) => h.shadowRoot).filter((r): r is ShadowRoot => !!r);
  const sx = s as Selection & { getComposedRanges?: (o: unknown) => StaticRange[] };
  if (roots.length && typeof sx.getComposedRanges === "function") {
    try {
      const [r] = sx.getComposedRanges({ shadowRoots: roots });
      if (r && !r.collapsed) {
        const range = document.createRange();
        range.setStart(r.startContainer, r.startOffset);
        range.setEnd(r.endContainer, r.endOffset);
        const text = range.toString();
        if (text) return { text, node: r.startContainer };
      }
    } catch {
      /* 次の方法へ */
    }
  }
  for (const r of roots) {
    const rs = (r as ShadowRoot & { getSelection?: () => Selection | null }).getSelection?.();
    if (rs && !rs.isCollapsed && rs.toString()) return { text: rs.toString(), node: rs.anchorNode };
  }
  return { text: s.toString(), node };
}
