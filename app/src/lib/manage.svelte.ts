/* Manage labels / Manage folders の表の状態と操作（TagModals / FolderModals で共用）:
   検索・選択・並び・押している間と改名中の並びの凍結・行内改名の確定・行の削除確認・Find and replace の dry run と置換。
   ラベルとフォルダの差（一覧・並び・名前の検査・呼ぶ api・名詞）は opts で渡す。モーダルの開閉・Esc の順・操作欄の組み立て・
   ラベルの色と統合は各コンポーネントに残す。表は ManageTable・Find and replace の欄は FindReplacePanel */
import { SvelteMap } from "svelte/reactivity";
import type { RenameOutcome } from "./api";
import { toast } from "./state.svelte";
import { t } from "./i18n.svelte";

export interface Row<T> {
  item: T;
  /** 木の深さ（字下げ。ラベルは 0・検索中は 0） */
  depth: number;
}

export interface ManageOpts<T extends { id: number; name: string }> {
  /** 名詞（ラベル / フォルダ）を含む表示の文。言語の切替で描き直るよう関数（t() はテンプレートの中で呼ばれる）。
      empty = 一覧が空・noMatch = 検索語 q の一致なし・remove = 行の削除ボタン・noFind = Find の一致なし */
  text: { empty: () => string; noMatch: (q: string) => string; remove: () => string; noFind: () => string };
  /** 一覧（app.tags / app.collections） */
  all: () => T[];
  /** 並べた全行（検索と凍結の前）。新しい配列を返す（並べ替えて使う）。フォルダは木の前順 + 字下げ */
  rows: (sortKey: "name" | "count", asc: boolean) => Row<T>[];
  /** 行内改名の名前の問題（null = 確定できる）。行に出す短い文 */
  nameError: (name: string, item: T) => string | null;
  rename: (id: number, name: string) => Promise<boolean>;
  remove: (ids: number[]) => Promise<unknown>;
  dryRun: (find: string, replace: string) => Promise<RenameOutcome[]>;
  replace: (find: string, replace: string) => Promise<unknown>;
}

/** 表の上に開く操作欄（1 つずつ。color / merge はラベルだけ） */
export type Panel = null | "new" | "color" | "merge" | "find" | "delete";

export class Manage<T extends { id: number; name: string }> {
  readonly opts: ManageOpts<T>;

  q = $state("");
  sel = $state<Set<number>>(new Set());
  sortKey = $state<"name" | "count">("name");
  asc = $state(true);
  panel = $state<Panel>(null);
  newName = $state("");
  findText = $state("");
  replaceText = $state("");
  preview = $state<RenameOutcome[]>([]);
  previewErr = $state("");
  /** dry run の応答待ち（その間の Replace は押させない = 一覧が変わった後の古いプレビューのまま置換しない） */
  previewing = $state(false);
  renaming = $state<number | null>(null);
  renameText = $state("");
  confirmRow = $state<number | null>(null);
  busy = $state(false);

  /** 改名欄を開いている間の行の並び（id と、止めたときの検索語）。確定・置換で並べ替わると、開いている欄の行が DOM 上で動いて
      フォーカスを失う（WebKit）・blur で途中の値のまま確定する（Chrome）ので、欄を閉じるまで並びを止める。止めている間は、
      止めたときにあった行を検索の絞り込みでも残す（確定で名前が検索に合わなくなっても行が抜けない）。マウスを押している間と
      確定中も止めたまま（mousedown の blur・別の操作で投げた確定が click より先に終わっても、押した行が動いて click が外れない）。
      検索語が変わったら止めない（フォーカスは検索欄にある・木の字下げを崩さない） */
  frozen = $state.raw<{ q: string; ids: number[] } | null>(null);
  pressing = $state(false);
  /** 押下の世代（前の押下を離したときの setTimeout が、次の押下の最中に pressing を外さない） */
  private press = 0;

  /** 確定中の改名（id → 送った名前）。同じ改名を 2 回送らない（blur・Enter・flushRename が重なっても 1 回）・行には送った名前を出す・
      確定中の欄は打てなくする（打っても確定する名前は変わらない） */
  readonly sending = new SvelteMap<number, string>();
  /** 改名欄を開くたびに進む（確定が終わったとき、その後に開いた欄は閉じない） */
  private edit = 0;
  /** 改名欄で打ったか。打っていなければ送らない（開いている間に置換・別経路で変わった名前を、開いたときの古い名前で戻さない）。
      確定中の行を送った名前で開き直したときは打った扱い（その確定が失敗しても Enter で送り直せる） */
  typed = false;
  /** 送った改名の数と、Replace を押し始めたときのその値。押し始めてから改名を送っていたら置換しない（押している間の blur で
      改名が届き、見ていたプレビューに無い名前まで置換する） */
  private sent = 0;
  private armed: number | null = null;
  private fseq = 0;

  constructor(opts: ManageOpts<T>) {
    this.opts = opts;
    $effect(() => {
      if (this.renaming === null && !this.pressing && this.sending.size === 0) this.frozen = null;
    });
    // 開いている欄・削除の確認の行が一覧から消えたら（検索で隠れた・削除・統合）閉じる。確定できなかった入力は捨てる（行に理由を
    // 出していた）。戻ったときに欄が検索欄のフォーカスを奪ってその打鍵で確定しない・見えない確認で Esc が空振りしない
    $effect(() => {
      if (this.renaming !== null && !this.rows.some((r) => r.item.id === this.renaming)) this.renaming = null;
      if (this.confirmRow !== null && !this.rows.some((r) => r.item.id === this.confirmRow)) this.confirmRow = null;
    });
    // Find and replace: 入力ごと（と一覧が変わるたび = 行内改名・色の変更などの後）に dry run でプレビュー（書き込まない）
    $effect(() => {
      const f = this.findText;
      const r = this.replaceText;
      void this.opts.all();
      if (this.panel !== "find" || !f) {
        this.fseq++; // 走っている dry run の応答は捨てる（空欄に戻した後に古い結果で Replace を有効にしない）
        this.preview = [];
        this.previewErr = "";
        this.previewing = false;
        return;
      }
      const my = ++this.fseq;
      this.previewing = true;
      void this.opts
        .dryRun(f, r)
        .then((p) => {
          if (my !== this.fseq) return;
          this.preview = p;
          this.previewErr = "";
          this.previewing = false;
        })
        .catch((e) => {
          if (my !== this.fseq) return;
          this.preview = [];
          this.previewErr = String(e).slice(0, 200);
          this.previewing = false;
        });
    });
  }

  find(id: number | null): T | null {
    return id === null ? null : (this.opts.all().find((x) => x.id === id) ?? null);
  }

  /** 開いたときの初期化 */
  reset() {
    this.q = "";
    this.sel = new Set();
    this.panel = null;
    this.renaming = null;
    this.confirmRow = null;
    this.frozen = null;
    this.pressing = false;
    this.busy = false;
  }

  /** busy を立てて fn を待つ（例外でも busy を戻す） */
  async run<R>(fn: () => Promise<R>): Promise<R> {
    this.busy = true;
    try {
      return await fn();
    } finally {
      this.busy = false;
    }
  }

  // ---- 表 ----
  rows = $derived.by((): Row<T>[] => {
    const needle = this.q.trim().toLowerCase();
    const fz = this.frozen && this.frozen.q === this.q ? this.frozen : null;
    const keep = fz ? new Set(fz.ids) : null;
    const all = this.opts.rows(this.sortKey, this.asc);
    // 検索中は名前の部分一致で平らに絞る（字下げせず。フォルダは親の名前を名前の列に添える）
    const list = needle
      ? all.filter((r) => r.item.name.toLowerCase().includes(needle) || !!keep?.has(r.item.id)).map((r) => ({ item: r.item, depth: 0 }))
      : all;
    if (!fz) return list;
    const at = new Map(fz.ids.map((id, i) => [id, i]));
    return list.sort((a, b) => (at.get(a.item.id) ?? at.size) - (at.get(b.item.id) ?? at.size)); // 後から増えた行は末尾
  });
  /** 検索に合う行（並びを止めている間に絞り込みでも残している行を除く）。すべて選択はこれだけを対象にする（確定で名前が
      検索に合わなくなった行を、選んだまま隠さない） */
  selectable = $derived.by(() => {
    const needle = this.q.trim().toLowerCase();
    return needle ? this.rows.filter((r) => (this.sending.get(r.item.id) ?? r.item.name).toLowerCase().includes(needle)) : this.rows;
  });
  allOn = $derived(this.selectable.length > 0 && this.selectable.every((r) => this.sel.has(r.item.id)));
  someOn = $derived(this.selectable.some((r) => this.sel.has(r.item.id)));
  selected = $derived.by(() => this.opts.all().filter((x) => this.sel.has(x.id)));
  /** 検索で隠れている選択（消す対象に入っていることを確認欄で知らせる） */
  hiddenSel = $derived([...this.sel].filter((id) => this.find(id) && !this.selectable.some((r) => r.item.id === id)).length);
  willRename = $derived(this.preview.filter((p) => p.status === "renamed" && p.old !== p.new).length);

  /** window の pointerdown / pointerup（capture）。ManageTable が表を出している間だけ付ける */
  onPointerDown(e: PointerEvent) {
    // 主ボタンだけ（右クリック・ctrl+クリックのメニューは pointerup を食いうる）
    if (e.button === 0 && !e.ctrlKey) {
      this.press++;
      this.pressing = true;
    }
  }
  onPointerUp() {
    if (!this.pressing) return;
    const my = this.press;
    setTimeout(() => {
      if (my === this.press) this.pressing = false; // 同じ押下の click の後で
    }, 0);
  }

  toggleSel(id: number) {
    const s = new Set(this.sel);
    if (s.has(id)) s.delete(id);
    else s.add(id);
    this.sel = s;
  }
  toggleAll() {
    const s = new Set(this.sel);
    if (this.allOn) this.selectable.forEach((r) => s.delete(r.item.id));
    else this.selectable.forEach((r) => s.add(r.item.id));
    this.sel = s;
  }
  sortBy(k: "name" | "count") {
    this.flushRename(); // 並びを止めたままにしない
    if (this.sortKey === k) this.asc = !this.asc;
    else {
      this.sortKey = k;
      this.asc = k === "name";
    }
  }
  /** 削除・統合で消えた id を選択から外す */
  pruneSel() {
    const live = new Set(this.opts.all().map((x) => x.id));
    this.sel = new Set([...this.sel].filter((id) => live.has(id)));
  }

  /** 行内改名を確定してから他の操作へ（WKWebView はボタンを押しても入力欄から blur しないことがあるので、×・背景・別の操作で
      黙って捨てない）。確定は待たない（欄はすぐ閉じ、行には送った名前を出す = 確定待ちの間の操作・打鍵が食い違わない）。
      確定できない入力（同名あり・「/」・空）は捨てる（行に理由を出している）。Esc は取消 */
  flushRename() {
    const x = this.find(this.renaming);
    if (x) void this.commitRename(x);
    this.renaming = null;
  }

  openPanel(p: Panel) {
    this.flushRename();
    this.confirmRow = null;
    this.panel = this.panel === p ? null : p;
    if (this.panel === "new") this.newName = "";
    else if (this.panel === "find") {
      this.findText = "";
      this.replaceText = "";
      this.preview = [];
    }
  }

  armReplace() {
    this.armed = this.sent;
  }
  async doFindReplace() {
    this.flushRename();
    const from = this.armed;
    this.armed = null;
    // 改名の確定中・押し始めてから改名を送ったなら置換しない（プレビューは改名前の一覧で取ったもの = 取り直したのを見て押し直す）
    if (this.sending.size > 0 || (from !== null && from !== this.sent)) {
      toast(t("改名を保存したのでプレビューを取り直しました。確かめてからもう一度 Replace を押してください"));
      return;
    }
    if (this.previewing || this.busy || !this.findText || this.willRename === 0) return;
    await this.run(() => this.opts.replace(this.findText, this.replaceText));
    this.panel = null;
  }

  async doDelete(ids: number[]) {
    this.flushRename();
    await this.run(() => this.opts.remove(ids));
    this.pruneSel();
    this.panel = null;
    this.confirmRow = null;
  }

  startRename(x: T) {
    if (this.renaming !== null && this.renaming !== x.id) this.flushRename();
    if (this.frozen?.q !== this.q) this.frozen = { q: this.q, ids: this.rows.map((r) => r.item.id) }; // 欄を閉じるまで並びを止める（乗り換えは止めたまま）
    this.confirmRow = null;
    this.edit++;
    this.renaming = x.id;
    this.renameText = this.sending.get(x.id) ?? x.name; // 確定中なら送った名前で開く
    this.typed = this.sending.has(x.id);
  }

  askDeleteRow(x: T) {
    this.flushRename();
    this.confirmRow = x.id;
  }

  async commitRename(x: T) {
    if (this.renaming !== x.id || this.sending.has(x.id)) return; // Enter で確定済み・Esc で取消済み（欄が消えるときの blur）・確定中
    const n = this.renameText.trim();
    if (!n || !this.typed || n === x.name) {
      this.renaming = null;
      return;
    }
    if (this.opts.nameError(n, x) !== null) return; // 確定させない（行に理由を出す）
    const my = this.edit;
    this.sent++;
    this.sending.set(x.id, n);
    try {
      // 確定したら欄を閉じる（失敗なら残す = 直して Enter し直せる）
      if ((await this.opts.rename(x.id, n)) && this.edit === my) this.renaming = null;
    } finally {
      this.sending.delete(x.id);
    }
  }
}
