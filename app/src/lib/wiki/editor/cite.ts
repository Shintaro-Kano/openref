// OpenNote のエディタ: 文献の引用（md だけ）。リンクの形の正準は標準の md リンクの `openref://open?key=KEY`
// （Obsidian でも外部 URL として開ける。`[[openref:KEY]]` は Obsidian がノート名と読み `:` も使えないので採らない）。
// - 入れる形: 選択があれば `[選択](openref://open?key=KEY)`・無ければ `[](openref://open?key=KEY)`。題名は入れない
//   （リンクの文字は書き手の本文 = 公開物の側でその文字の後ろに番号が付く。題名を入れると論文名が本文に混ざる）
// - DOI / PubMed の URL だけを選択なしで貼った: URL はそのまま入り、その後ろに小さなメニュー [文献リンクにする] [URL のまま]。
//   照合（ネットワークなし）で蔵書にあれば置き換え、無ければ [LIBRARY に登録して文献リンクに]（押したときだけ resolver = ネットワーク）。
//   待つ間の編集には位置を写して追随し、URL の文字が変わった・メニューの段で打ち続けた（= URL のまま）なら何もしない。
//   照合と登録の呼び口（Tauri）は呼び出し側（WikiEditor → edit.svelte / cite.svelte）= この部品は CodeMirror だけを知る

import { syntaxTree } from "@codemirror/language";
import { EditorSelection, StateEffect, StateField, type EditorState, type Extension } from "@codemirror/state";
import { EditorView, keymap, showTooltip, type Tooltip } from "@codemirror/view";
import { t } from "../../i18n.svelte";
import { scanWikiText } from "../render/md.ts";

export const CITE_PREFIX = "openref://open?key=";
/** item の key（core の keys::new_key = 英数字）。公開物のビルドの正規表現 `^openref://open\?key=([A-Za-z0-9]+)$` と同じ字の集合 */
const KEY = /^[A-Za-z0-9]+$/;

/** リンクの文字の `\` `[` `]` を escape（それ以外の記法はそのまま = 選んだ文字の書式を残す） */
export function escapeLinkText(s: string): string {
  return s.replace(/[\\[\]]/g, "\\$&");
}

/** 文献リンク。key が英数字でなければ投げる（壊れたリンクを本文に入れない） */
export function citeLink(key: string, text = ""): string {
  if (!KEY.test(key)) throw new Error(`bad item key: ${key}`);
  return `[${escapeLinkText(text)}](${CITE_PREFIX}${key})`;
}

/** 文献リンクを置けない所の判定（citeBlocked）。中に入れると地の文に `](openref://…)` が残る・描画されない・周りの構文が壊れる:
    - 塊の構文（コード・HTML・frontmatter）= その塊の**行**（頭と終わりを含む）に端があれば置けない。閉じの ``` / `---` の直後・
      字下げのコードの最終行の末尾・frontmatter の前（caret 0）に入れると、fence が閉じない・frontmatter が壊れる
      （先頭の `---` を要求する静的サイト生成器では、その記事が黙って外れる）
    - 行の中の構文（インラインのコード・リンク / 画像・自動リンク・素の URL・参照の定義・`[[…]]` / `![[…]]`）= 端がその**中**に
      あれば置けない（直後・直前は置ける）
    - 選択は 1 行の中だけ。選択の中にリンク・画像・自動リンク・`[[…]]` があれば包めない（escape で消える・リンクの入れ子）。
      インラインのコードと素の URL は包める（コードの中は escape しない）
    - 閉じていない `](` の直後（リンク先を打っている途中）・閉じていない自動リンク `<https:…` の中。貼り付け（paste）は `<` の直後も
    - 表（GFM）: 行の最初と最後の `|` の外・区切りの行・`|` を含む選択（セルをまたぐ・`\|`）。列の数を超えたセルは markdown-it も
      ほかの markdown-it 系の処理系も捨てる = 引用が黙って消える・見出しの行なら表が段落に崩れる。セルの中は置ける
      （caret をセルへ寄せずに拒んで知らせる = どこに入るかを利用者が決める）
    - 脚注の定義（`[^1]: 本文`）は名前の中だけ拒む。本文は lezer が参照の定義の URL として読むことがある（空白の無い和文）が置ける */
const BLOCK = /^(?:FencedCode|CodeBlock|HTMLBlock|CommentBlock|ProcessingInstructionBlock|Frontmatter)$/;
const INLINE_LINK = /^(?:Link|Image|Autolink|URL|LinkReference)$/;

type NodeRef = { name: string; from: number; to: number };
type TreeNode = NodeRef & { firstChild: TreeNode | null; nextSibling: TreeNode | null; parent: TreeNode | null };

/** 表の行に端がある・`|` を含む選択 = 置けない理由（無ければ null） */
function tableIssue(state: EditorState, table: TreeNode, from: number, to: number): string | null {
  const doc = state.doc;
  const ends = from === to ? [from] : [from, to];
  for (let c = table.firstChild; c; c = c.nextSibling) {
    const line = doc.lineAt(c.from);
    const on = ends.filter((p) => line.from <= p && p <= line.to);
    if (!on.length) continue;
    // 表の直下の TableDelimiter = 区切りの行（`| --- |`）
    if (c.name === "TableDelimiter") return t("表のセルの外には文献リンクを入れられません（セルの中にカーソルを置いてください）");
    if (c.name !== "TableHeader" && c.name !== "TableRow") continue;
    if (from !== to && state.sliceDoc(from, to).includes("|")) return t("セルをまたぐ選択・「|」を含む選択はリンクの文字にできません");
    const pipes: NodeRef[] = [];
    for (let d = c.firstChild; d; d = d.nextSibling) if (d.name === "TableDelimiter") pipes.push(d);
    const lo = pipes.length && pipes[0].from === c.from ? pipes[0].to : c.from;
    const last = pipes[pipes.length - 1];
    const hi = last && last.to === c.to ? last.from : c.to;
    if (on.some((p) => p < lo || p > hi)) return t("表のセルの外には文献リンクを入れられません（セルの中にカーソルを置いてください）");
  }
  return null;
}

/** 地の文として描かれる `[文字]`（リンク先も参照の名前も無い・定義も無い・脚注 `[^…]` / callout `[!…]` / `[[…]]` / `![…]` でない）。
    lezer はどれも「リンク先の無い Link」として出す */
function plainBrackets(state: EditorState, n: NodeRef): boolean {
  if (n.name !== "Link") return false;
  const s = state.sliceDoc(n.from, n.to);
  if (/\]\s*[([]/.test(s) || /^\[[\^!]/.test(s) || s.startsWith("[[") || s.endsWith("]]")) return false;
  const before = state.sliceDoc(Math.max(0, n.from - 1), n.from);
  const after = state.sliceDoc(n.to, n.to + 1);
  if (before === "[" || before === "!" || after === "]") return false;
  const label = s.slice(1, -1).trim().replace(/\s+/g, " ").replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return !new RegExp(`^ {0,3}\\[${label}\\]:`, "im").test(state.doc.toString());
}

/** 行の中の `[[…]]` / `![[…]]` の範囲（文書の位置） */
function wikiSpans(state: EditorState, from: number, to: number): { from: number; to: number }[] {
  const out: { from: number; to: number }[] = [];
  const a = state.doc.lineAt(from);
  const b = state.doc.lineAt(to);
  for (let i = a.number; i <= b.number; i++) {
    const line = state.doc.line(i);
    for (const w of scanWikiText(line.text)) out.push({ from: line.from + w.from, to: line.from + w.to });
  }
  return out;
}

/** from〜to（caret なら from = to）に文献リンクを置けるか。置けなければ理由（画面に出す文）・置ければ null。paste = 貼り付けのメニュー */
export function citeBlocked(state: EditorState, from: number, to: number, paste = false): string | null {
  const doc = state.doc;
  if (from !== to && doc.lineAt(from).number !== doc.lineAt(to).number) return t("行をまたぐ選択はリンクの文字にできません（1 行の中で選んでください）");
  const tree = syntaxTree(state);
  const lineFrom = doc.lineAt(from).from;
  const lineTo = doc.lineAt(to).to;
  let code = false;
  let link = false;
  let table: string | null = null;
  tree.iterate({
    from: lineFrom,
    to: lineTo,
    enter: (n) => {
      if (BLOCK.test(n.name)) {
        // 塊の行（頭と終わりを含む）に端がある
        const a = doc.lineAt(n.from).from;
        const b = doc.lineAt(n.to).to;
        if ((a <= from && from <= b) || (a <= to && to <= b)) code = true;
        return;
      }
      if (n.name === "Table") {
        table ??= tableIssue(state, n.node as unknown as TreeNode, from, to);
        return; // 中（セルの中のリンク等）も見る
      }
      // 脚注の定義の本文（参照の定義の URL として読まれたもの）は置ける
      if (n.name === "URL" && n.node.parent?.name === "LinkReference" && state.sliceDoc(n.node.parent.from, n.node.parent.from + 2) === "[^") return;
      const strictly = (p: number) => n.from < p && p < n.to;
      if (n.name === "InlineCode") {
        if (strictly(from) || strictly(to)) code = true;
      } else if (INLINE_LINK.test(n.name)) {
        if (plainBrackets(state, n)) return;
        // 脚注の定義（`[^1]: 本文`）は名前の中だけ（本文には置ける）
        const end = n.name === "LinkReference" && state.sliceDoc(n.from, n.from + 2) === "[^" ? state.sliceDoc(n.from, n.to).indexOf("]") + n.from + 1 : n.to;
        const inside = (p: number) => n.from < p && p < end;
        if (inside(from) || inside(to)) link = true;
        // 選択がリンク・画像・自動リンク・参照の定義を含む（素の URL は包める）
        else if (from !== to && n.name !== "URL" && n.from >= from && n.to <= to) link = true;
      }
    },
  });
  if (code) return t("コード・frontmatter の中には文献リンクを入れられません");
  if (table) return table;
  for (const w of wikiSpans(state, from, to)) {
    if ((w.from < from && from < w.to) || (w.from < to && to < w.to) || (from !== to && w.from >= from && w.to <= to)) link = true;
  }
  const before = state.sliceDoc(lineFrom, from);
  if (/\]\(\s*$/.test(before) || /<[A-Za-z][A-Za-z0-9+.-]{1,31}:[^\s<>]*$/.test(before) || (paste && /<$/.test(before))) link = true;
  if (link) return t("リンクの中には文献リンクを入れられません");
  return null;
}

/** 選択の文字をリンクの文字に（`\` `[` `]` を escape。インラインのコードの中は escape しない = コードは escape を解かない） */
function linkText(state: EditorState, from: number, to: number): string {
  let out = "";
  let pos = from;
  syntaxTree(state).iterate({
    from,
    to,
    enter: (n) => {
      if (n.name !== "InlineCode" || n.from < from || n.to > to) return;
      out += escapeLinkText(state.sliceDoc(pos, n.from)) + state.sliceDoc(n.from, n.to);
      pos = n.to;
      return false;
    },
  });
  return out + escapeLinkText(state.sliceDoc(pos, to));
}

/** 主の選択（無ければ caret）を文献リンクにして、その後ろへ caret を置く。置けない場所なら入れずに理由を返す */
export function insertCite(view: EditorView, key: string): string | null {
  const r = view.state.selection.main;
  const why = citeBlocked(view.state, r.from, r.to);
  if (why) return why;
  if (!KEY.test(key)) throw new Error(`bad item key: ${key}`);
  const insert = `[${linkText(view.state, r.from, r.to)}](${CITE_PREFIX}${key})`;
  view.dispatch({
    changes: { from: r.from, to: r.to, insert },
    selection: EditorSelection.cursor(r.from + insert.length),
    scrollIntoView: true,
    userEvent: "input",
  });
  return null;
}

/** 例外の文（Error の "Error: " の前置きは画面に出さない） */
const errText = (e: unknown) => (e instanceof Error ? e.message : String(e)).slice(0, 200);

/** DOI（doi.org）と PubMed の論文の URL（空白なし・1 本だけ）。それ以外の URL は対象外 = 今どおり貼るだけ */
const CITE_URL = /^https?:\/\/(?:(?:www\.|dx\.)?doi\.org\/10\.\d+\/\S+|pubmed\.ncbi\.nlm\.nih\.gov\/\d+\/?(?:[?#]\S*)?)$/i;
export function citeUrlOf(text: string | null | undefined): string | null {
  const s = (text ?? "").trim();
  return CITE_URL.test(s) ? s : null;
}

/** 照合・登録の結果（key と撤回の印）。照合で蔵書に無ければ null。登録で null = 呼び出し側が諦めた（知らせ済み） */
export interface CiteHit {
  key: string;
  retracted: boolean;
}

export interface CitePasteOptions {
  /** 蔵書と照合（ネットワークなし）。無ければ null・識別子として読めなければ投げる */
  lookup(url: string): Promise<CiteHit | null>;
  /** resolver で解いて登録（重複は既存の key）。null = 入れない（呼び出し側が知らせた） */
  register(url: string): Promise<CiteHit | null>;
  /** 入れた文献に撤回の印がある・失敗の知らせ（toast） */
  notice?(msg: string): void;
}

type Phase = "ask" | "busy" | "missing" | "adding" | "error";
interface Pending {
  id: number;
  from: number;
  to: number;
  url: string;
  phase: Phase;
  msg?: string;
}

const setPending = StateEffect.define<Pending | null>();
let seq = 0;

/** 貼った URL のメニューの拡張（cm.ts が md のときに足す）。戻り値の paste(view, url) = caret に URL を入れてメニューを出す */
export function citePaste(o: CitePasteOptions): { extension: Extension; paste(view: EditorView, url: string): void } {
  const field: StateField<Pending | null> = StateField.define<Pending | null>({
    create: () => null,
    update(v, tr) {
      for (const e of tr.effects) if (e.is(setPending)) return e.value;
      if (!v || !tr.docChanged) return v;
      // 尋ねている段で打ち続けた = URL のまま（メニューを残して本文の上に浮かせない）
      if (v.phase === "ask" || v.phase === "missing" || v.phase === "error") return null;
      // 待つ間の編集: 位置を写す。URL の文字そのものが変わったら諦める
      const from = tr.changes.mapPos(v.from, 1);
      const to = tr.changes.mapPos(v.to, -1);
      if (tr.newDoc.sliceString(from, to) !== v.url) return null;
      return { ...v, from, to };
    },
    provide: (f) => showTooltip.from(f, (v) => (v ? tooltip(v) : null)),
  });

  const destroyed = (view: EditorView) => !!(view as unknown as { destroyed?: boolean }).destroyed;
  const current = (view: EditorView, id: number): Pending | null => {
    if (destroyed(view)) return null;
    const v = view.state.field(field, false);
    return v && v.id === id && view.state.sliceDoc(v.from, v.to) === v.url ? v : null;
  };
  const phase = (view: EditorView, id: number, p: Phase, msg?: string) => {
    const v = current(view, id);
    if (v) view.dispatch({ effects: setPending.of({ ...v, phase: p, msg }) });
  };
  // focus は動かさない（ボタンは mousedown を止めている = 押してもエディタの focus のまま。待つ間に別の所へ移った focus を奪わない）
  const close = (view: EditorView) => {
    if (!destroyed(view) && view.state.field(field, false)) view.dispatch({ effects: setPending.of(null) });
  };
  /** URL を文献リンクに置き換える（まだ同じメニュー・同じ URL のときだけ）。caret は URL の直後にあったときだけリンクの後ろへ
      （待つ間に別の所で打っていれば、その caret は CodeMirror が変更に合わせて写す = 飛ばさない） */
  const replace = (view: EditorView, id: number, hit: CiteHit): boolean => {
    const v = current(view, id);
    if (!v) return false;
    const insert = citeLink(hit.key);
    const m = view.state.selection.main;
    const follow = view.state.selection.ranges.length === 1 && m.empty && m.head === v.to;
    view.dispatch({
      changes: { from: v.from, to: v.to, insert },
      effects: setPending.of(null),
      ...(follow ? { selection: EditorSelection.cursor(v.from + insert.length) } : {}),
      userEvent: "input",
    });
    if (hit.retracted) o.notice?.(`⚠ ${t("撤回の印がある文献です")}`);
    return true;
  };
  const toLink = async (view: EditorView, id: number) => {
    const v = current(view, id);
    if (!v) return;
    phase(view, id, "busy");
    try {
      const hit = await o.lookup(v.url);
      if (hit) replace(view, id, hit);
      else phase(view, id, "missing");
    } catch (e) {
      phase(view, id, "error", errText(e));
    }
  };
  const addAndLink = async (view: EditorView, id: number) => {
    const v = current(view, id);
    if (!v) return;
    phase(view, id, "adding");
    try {
      const hit = await o.register(v.url);
      if (!hit) close(view);
      // 登録はできたが、待つ間に URL を書き換えた・エディタを作り直した = 置き換えない（蔵書には入った）
      else if (!replace(view, id, hit)) o.notice?.(`✗ ${t("登録しましたが、貼った URL が変わったので文献リンクに置き換えませんでした")}`);
    } catch (e) {
      phase(view, id, "error", errText(e));
    }
  };

  function tooltip(v: Pending): Tooltip {
    return {
      pos: v.to,
      above: false,
      strictSide: false,
      create(view) {
        const dom = document.createElement("div");
        dom.className = "wk-cite-pop";
        const note = (s: string) => {
          const p = document.createElement("span");
          p.className = "wk-cite-pop-note";
          p.textContent = s;
          dom.append(p);
        };
        const button = (label: string, run: () => void, primary = false) => {
          const b = document.createElement("button");
          b.type = "button";
          b.textContent = label;
          if (primary) b.className = "wk-cite-pop-main";
          // mousedown で選択と focus を動かさない（押してもエディタの caret のまま）
          b.addEventListener("mousedown", (e) => e.preventDefault());
          b.addEventListener("click", run);
          dom.append(b);
        };
        if (v.phase === "ask") {
          button(t("文献リンクにする"), () => void toLink(view, v.id), true);
          button(t("URL のまま"), () => close(view));
        } else if (v.phase === "missing") {
          note(t("蔵書にありません"));
          button(t("LIBRARY に登録して文献リンクに"), () => void addAndLink(view, v.id), true);
          button(t("URL のまま"), () => close(view));
        } else if (v.phase === "busy" || v.phase === "adding") {
          note(v.phase === "busy" ? t("蔵書と照合中…") : t("登録中…"));
        } else {
          note(`✗ ${v.msg ?? ""}`);
          button(t("閉じる"), () => close(view));
        }
        return { dom };
      },
    };
  }

  const extension: Extension = [
    field,
    // Esc = URL のまま（メニューが出ているときだけ取る）
    keymap.of([
      {
        key: "Escape",
        run: (view) => {
          if (!view.state.field(field, false)) return false;
          view.dispatch({ effects: setPending.of(null) });
          return true;
        },
      },
    ]),
  ];

  return {
    extension,
    paste(view, url) {
      const r = view.state.selection.main;
      view.dispatch({
        changes: { from: r.from, to: r.to, insert: url },
        selection: EditorSelection.cursor(r.from + url.length),
        effects: setPending.of({ id: ++seq, from: r.from, to: r.from + url.length, url, phase: "ask" }),
        scrollIntoView: true,
        userEvent: "input.paste",
      });
    },
  };
}
