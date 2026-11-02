/* OpenNote: 表示モードのページ内検索（WKWebView / Tauri に検索 UI は無い = 自前）。
   DOM は書き換えず CSS Custom Highlight API（::highlight(wk-find) / wk-find-cur・shadow の stylesheet に定義）で強調する。
   API が無い環境は現在の 1 件を selection で示すだけ。大小は無視（toLowerCase で比べる） */

const MAX_HITS = 5000;
const SKIP = new Set(["STYLE", "SCRIPT", "TEMPLATE"]);

type HL = { set(name: string, h: unknown): void; delete(name: string): void };
function registry(): { hl: HL; make: (r: Range[]) => unknown } | null {
  const css = (globalThis as unknown as { CSS?: { highlights?: HL } }).CSS;
  const H = (globalThis as unknown as { Highlight?: new (...r: Range[]) => unknown }).Highlight;
  return css?.highlights && H ? { hl: css.highlights, make: (r) => new H(...r) } : null;
}

/** CSS.highlights の登録表は文書で 1 つ。いま wk-find / wk-find-cur を登録している Finder（= 最後に paint した物）。
    ShadowPage は中央のページのほかチャットの応答・埋め込みにもあり、描き直しや破棄のたびに reset / clear を呼ぶので、
    持ち主でない Finder の clear が中央のページの強調を消さないようにする */
let owner: Finder | null = null;

export class Finder {
  hits: Range[] = [];
  cur = -1;
  private texts: Text[] | null = null;
  private root: ShadowRoot | Element;
  /** root = 探す範囲（ShadowPage の shadow root）。parameter property を使わない = node の型の削除だけで読める（render.test） */
  constructor(root: ShadowRoot | Element) {
    this.root = root;
  }

  /** 描き直した後に呼ぶ（テキストの一覧を作り直す） */
  reset() {
    this.texts = null;
    this.clear();
  }

  private collect(): Text[] {
    if (this.texts) return this.texts;
    const out: Text[] = [];
    const doc = (this.root as Node).ownerDocument ?? document;
    const w = doc.createTreeWalker(this.root, NodeFilter.SHOW_TEXT);
    for (let n = w.nextNode(); n; n = w.nextNode()) {
      const p = n.parentElement;
      if (p && SKIP.has(p.nodeName.toUpperCase())) continue;
      if ((n as Text).data.trim()) out.push(n as Text);
    }
    this.texts = out;
    return out;
  }

  search(q: string): number {
    this.hits = [];
    this.cur = -1;
    const needle = q.toLowerCase();
    if (needle) {
      const doc = (this.root as Node).ownerDocument ?? document;
      outer: for (const t of this.collect()) {
        const hay = t.data.toLowerCase();
        for (let i = hay.indexOf(needle); i >= 0; i = hay.indexOf(needle, i + needle.length)) {
          const r = doc.createRange();
          r.setStart(t, i);
          r.setEnd(t, i + needle.length);
          this.hits.push(r);
          if (this.hits.length >= MAX_HITS) break outer;
        }
      }
    }
    if (this.hits.length) this.cur = 0;
    this.paint();
    return this.hits.length;
  }

  step(d: 1 | -1) {
    if (!this.hits.length) return;
    this.cur = (this.cur + d + this.hits.length) % this.hits.length;
    this.paint();
  }

  private paint() {
    const reg = registry();
    if (reg) {
      reg.hl.set("wk-find", reg.make(this.hits));
      reg.hl.set("wk-find-cur", reg.make(this.cur >= 0 ? [this.hits[this.cur]] : []));
      owner = this;
    }
    const r = this.hits[this.cur];
    if (!r) return;
    // 閉じた <details> の中なら開く
    for (let p: Node | null = r.startContainer; p; p = p.parentNode) {
      if (p.nodeName === "DETAILS" && !(p as HTMLDetailsElement).open) (p as HTMLDetailsElement).open = true;
    }
    const el = r.startContainer.parentElement;
    el?.scrollIntoView({ block: "center" });
    if (!reg) {
      const sel = (this.root as Node).ownerDocument?.getSelection();
      sel?.removeAllRanges();
      sel?.addRange(r);
    }
  }

  /** 強調を消す（登録表を消すのは自分が持ち主のときだけ。他の ShadowPage の reset / 破棄では中央のページの強調が残る） */
  clear() {
    const reg = registry();
    if (reg && owner === this) {
      reg.hl.delete("wk-find");
      reg.hl.delete("wk-find-cur");
      owner = null;
    }
    this.hits = [];
    this.cur = -1;
  }
}
