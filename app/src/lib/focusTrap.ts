// 窓（覆いの窓）の中に Tab の焦点を留める。窓の骨格 Modal.svelte が開いている間に holdTab(枠) を呼び、骨格に載っていない窓
// （WikiConflict・WikiRescue・WikiCite・WikiLightbox）は同じ holdTab を action（trapTab）で、または受け手から moveTab を呼ぶ = 窓ごとに別の実装を書かない。
// 決まり:
// - Tab / Shift+Tab だけ（⌘ / Ctrl / ⌥ との組・IME の変換中・既に誰かが扱った Tab = CodeMirror の字下げ等は素通し）
// - 窓が開いている間の Tab / Shift+Tab は、囲いが全部自分で動かす（押せる物の並びの次 / 前へ focus() して既定を止める）。ブラウザの Tab の順に
//   頼らない = WKWebView は「フルキーボードアクセス」が切（macOS の既定）だとボタンとリンクに Tab が止まらない（実 WebKit で確認）。
//   Chrome と WebKit で同じ順になる。入力欄・<select> から出る Tab も次の物へ送る（欄の中身は変えない）
// - 囲うのは画面でいちばん上の窓（z-index が大きい物・同じなら DOM で後ろの物 = 後から描かれて上に見える）。開いた順ではない
//   （参照エディタの上に PDF 取込が DOM の前に開くと、取込は下に隠れる = 隠れた窓を囲わない）
// - 押せる物 = button・a[href]・input・select・textarea・[tabindex]・contenteditable のうち、:disabled（<fieldset disabled> の中も = .disabled は
//   false のまま。<legend> の中は除く）・tabindex < 0・hidden / inert の中・見えない物を除く。radio の組は 1 つ。窓の中の menu / listbox も、
//   窓の Tab の順の一部（中に閉じ込めない）。focus() が効かなかった物（判定の漏れ）は飛ばして次へ（1 周まで）= Tab が窓の中で止まらない
// - 扱わない物（今の窓に無い）: shadow DOM の中・正の tabindex の順（文書の順で数える）・iframe の中
// - 押せる物が 0 = 枠に留まる
// 覆いの外のクリック・閉じた後の焦点の戻り（focusReturn）・覆いでないポップオーバーやメニューは触らない（holdTab を呼ばない = 今の Tab のまま）

const stack: HTMLElement[] = [];
let listening = false;

const FOCUSABLE = 'a[href], area[href], button, input, select, textarea, iframe, summary, [tabindex], [contenteditable=""], [contenteditable="true"]';

function visible(el: HTMLElement): boolean {
  const cv = (el as HTMLElement & { checkVisibility?: (o?: object) => boolean }).checkVisibility;
  if (typeof cv === "function") return cv.call(el, { visibilityProperty: true });
  for (let n: HTMLElement | null = el; n; n = n.parentElement) {
    const s = getComputedStyle(n);
    if (s.display === "none" || s.visibility === "hidden") return false;
  }
  return true;
}

/** root の中の押せる物（文書の順）。同じ name の radio は 1 つ（選ばれた物・無ければ最初）= ブラウザの Tab と同じ止まり方 */
export function tabbables(root: HTMLElement): HTMLElement[] {
  const seen = new Set<string>();
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => {
    if (el.matches(":disabled")) return false;
    const ti = el.getAttribute("tabindex");
    if (ti !== null && Number(ti) < 0) return false;
    if (el instanceof HTMLInputElement && el.type === "hidden") return false;
    if (el.closest("[hidden], [inert]")) return false;
    if (el instanceof HTMLInputElement && el.type === "radio" && el.name) {
      const group = [...root.querySelectorAll<HTMLInputElement>('input[type="radio"]')].filter((r) => r.name === el.name);
      const pick = group.find((r) => r.checked) ?? group[0];
      if (pick !== el || seen.has(el.name)) return false;
      seen.add(el.name);
    }
    return visible(el);
  });
}

function focusFrame(frame: HTMLElement) {
  if (!frame.hasAttribute("tabindex")) frame.setAttribute("tabindex", "-1");
  frame.focus({ preventScroll: true });
}

/** 1 回の Tab を窓の中で動かす（既定の Tab は止める = 常に true）。WikiLightbox・WikiCite の受け手と試験からも呼ぶ */
export function moveTab(e: KeyboardEvent, frame: HTMLElement): boolean {
  e.preventDefault();
  const act = document.activeElement as HTMLElement | null;
  const list = tabbables(frame);
  if (!list.length) {
    if (!act || !frame.contains(act)) focusFrame(frame);
    return true;
  }
  const n = list.length;
  const d = e.shiftKey ? -1 : 1;
  let i: number; // 最初に試す物
  if (!act || !frame.contains(act)) i = e.shiftKey ? n - 1 : 0;
  else {
    const k = list.indexOf(act);
    if (k >= 0) i = k + d;
    else {
      // 枠そのもの・押せない物（tabindex="-1" の枠・中の文・roving の行）= 文書の順で次 / 前の押せる物へ（無ければ反対の端）
      const after = list.findIndex((el) => act.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING);
      let before = -1;
      for (let j = n - 1; j >= 0; j--) if (act.compareDocumentPosition(list[j]) & Node.DOCUMENT_POSITION_PRECEDING) { before = j; break; }
      i = e.shiftKey ? (before >= 0 ? before : n - 1) : after >= 0 ? after : 0;
    }
  }
  // focus() が効かなかった物（押せる物の判定の漏れ）は飛ばして次へ。1 周まで（無限に回らない）
  for (let t = 0; t < n; t++) {
    const el = list[(((i + d * t) % n) + n) % n];
    el.focus();
    if (document.activeElement === el) return true;
  }
  if (!act || !frame.contains(act)) focusFrame(frame);
  return true;
}

/** 画面での重なりの順（body の直下に近い側の z-index = 重なりの文脈の根の値） */
function zOf(el: HTMLElement): number {
  let z = 0;
  for (let n: HTMLElement | null = el; n && n !== document.body; n = n.parentElement) {
    const s = getComputedStyle(n);
    if (s.position !== "static" && s.zIndex !== "auto" && s.zIndex !== "") z = Number(s.zIndex) || 0;
  }
  return z;
}

/** 画面でいちばん上の窓（z が大きい物・同じなら DOM で後ろの物）。文書から外れた物は捨てる */
function topFrame(): HTMLElement | undefined {
  for (let i = stack.length - 1; i >= 0; i--) if (!stack[i].isConnected) stack.splice(i, 1);
  let top: HTMLElement | undefined;
  let topZ = 0;
  for (const f of stack) {
    const z = zOf(f);
    if (!top || z > topZ || (z === topZ && top.compareDocumentPosition(f) & Node.DOCUMENT_POSITION_FOLLOWING)) {
      top = f;
      topZ = z;
    }
  }
  return top;
}

function onKey(e: KeyboardEvent) {
  if (e.key !== "Tab" || e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey || e.isComposing || e.keyCode === 229) return;
  const frame = topFrame();
  if (frame) moveTab(e, frame);
}

/** 窓が開いている間、Tab の焦点をその枠の中に留める。戻り値 = 外す（閉じたとき） */
export function holdTab(frame: HTMLElement): () => void {
  stack.push(frame);
  if (!listening) {
    window.addEventListener("keydown", onKey); // 泡の段 = 中の部品（CodeMirror の Tab 等）が先に扱う
    listening = true;
  }
  return () => {
    const i = stack.lastIndexOf(frame);
    if (i >= 0) stack.splice(i, 1);
  };
}

/** svelte の action（骨格に載っていない窓の枠に use:trapTab） */
export function trapTab(node: HTMLElement) {
  const off = holdTab(node);
  return { destroy: off };
}
