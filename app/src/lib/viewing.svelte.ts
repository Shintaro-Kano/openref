// 「今、人が見ている物」の層。前面のタブから決まる元（ページ / 文献 / なし）と、その元で選んだ範囲を 1 か所に持つ。
// - ホスト（PDF viewer）は選択を followItemSelection に渡すだけ = チャットを知らない。ページ / エディタの選択はここが selectionchange で拾う
//   （wiki/selection.ts を読む = wiki → viewing の import は作らない）
// - 読み手（チャット）は viewingSource() / viewing.sel を読み、選択を使った / 外したときだけ clearViewingSelection / setViewingSelection を呼ぶ = ホストを知らない
// - 読み手がいるか（wanted）は関数で登録する（setViewingWanted）。拾う前に同期で呼ぶ = 読み手が有効になった直後の最初の選択も拾う。登録が無ければ何も拾わない
// - 選択の寿命: 外れるのは × / 送信 / 別の元へ移ったとき（PDF の selInfo が消えても残す）。「元が変わったら捨てる」は initViewing の effect
// 依存の向き: viewing は state.svelte / wiki.svelte / wiki/selection.ts を読むが、チャットとホストの部品は import しない。
// chat → viewing → wiki.svelte → chat の環があるので、module の評価時には wk / app に触らない（関数の中だけ）

import { untrack } from "svelte";
import { app, frontShelf, frontShelfBook, frontViewer, frontWiki } from "./state.svelte";
import { inPage, selectionText } from "./wiki/selection";
import { wk } from "./wiki/wiki.svelte";

/** 見ている物: OpenNote のタブなら表示中のページ・PDF のタブなら開いている文献・一覧のタブなら選んでいる文献 */
export type ViewingSource = { kind: "page"; path: string } | { kind: "item"; key: string; title: string };

export const viewing = $state({
  /** 見ている物で選んだ範囲（中央のページ / エディタ / PDF viewer で最後に選んだもの）。key = その元の viewingKey */
  sel: null as { key: string; text: string } | null,
});

/** 読み手がいるか（チャットが () => chatShown() を登録する）。登録が無い = 拾わない */
let wanted: () => boolean = () => false;

/** 読み手を登録する（null = 外す）。拾う前に同期で呼ぶ = 状態の写しにしない（反映が遅れて最初の選択を取りこぼさない） */
export function setViewingWanted(fn: (() => boolean) | null) {
  wanted = fn ?? (() => false);
}

/** 今見ている物（前面のタブで決まる）。本棚のタブ・本のタブは null（一覧で選んでいた文献を返さない = 取り違えない） */
export function viewingSource(): ViewingSource | null {
  if (frontShelf() || frontShelfBook()) return null;
  const v = frontViewer();
  if (v) return { kind: "item", key: v.itemKey, title: v.title };
  // 一覧のタブ: 選んでいる文献。見えていない Wiki のページは元にしない。
  // 別の行を読み込む間（app.detail がまだ前の行）は元なし = 選んでいない文献を添えない
  if (app.tabs.length && !frontWiki()) {
    const d = app.detail;
    return d && d.key === app.selected ? { kind: "item", key: d.key, title: d.csl?.title ?? "" } : null;
  }
  const p = wk.page?.path ?? null;
  return p ? { kind: "page", path: p } : null;
}

/** 元の鍵（viewing.sel.key・チャットの ctxOff と比べる）: ページ = path・文献 = `item:<key>` */
export function viewingKey(s: ViewingSource | null): string | null {
  return !s ? null : s.kind === "page" ? s.path : `item:${s.key}`;
}

/** 選択を書く（key = 元の viewingKey）。text が空 = その元の選択だけ消す（別の元の選択は触らない）。読み手がいなければ何もしない。
    何も追わせない（呼び手の effect が chat.status や viewing.sel を追うと、state event のたびに書き戻す）。ホストの effect は followItemSelection */
export function setViewingSelection(key: string, text: string | null) {
  untrack(() => {
    if (!wanted()) return;
    if (text && text.trim()) viewing.sel = { key, text };
    else if (viewing.sel?.key === key) viewing.sel = null;
  });
}

/** ホスト（PDF viewer）の選択を見ている物に書く effect を張る（ホストの部品の初期化の中で呼ぶ）。前面で選択があるときだけ
    `item:<itemKey>` へ書く（鍵の書式は viewing が持つ = ホストは知らない）。読み手の有無は真偽値の derived で追う =
    値が変わったとき（有効化）だけ選び直し、子の state event で chat.status が置き換わっても書き戻さない（× / 送信で消した選択を蘇らせない）。
    書くのは untrack の中 = 選択の状態は追わない */
export function followItemSelection(itemKey: string, front: () => boolean, text: () => string | null) {
  const want = $derived(wanted());
  $effect(() => {
    const t = text();
    if (front() && want && t) untrack(() => setViewingSelection(`item:${itemKey}`, t));
  });
}

/** 選択を消す（× / 送信） */
export function clearViewingSelection() {
  viewing.sel = null;
}

/** 元（viewingKey）が変わったら別の元の選択を捨てる（initViewing の effect が呼ぶ） */
export function viewingChanged(key: string | null) {
  if (viewing.sel && viewing.sel.key !== key) viewing.sel = null;
}

// ---- ページ / エディタの選択（中央のページ = 表示の shadow DOM / エディタ） ----

/** 選択の読み取り（selectionchange の後の次のタスク = CodeMirror が自分の selectionchange で選択を反映した後に読む。
    listener ごとにマイクロタスクが処理されるのでマイクロタスクでは足りない） */
export function readViewingSelection() {
  if (!wanted()) return; // 読み手がいなければ何もしない
  const p = wk.page?.path;
  const s = document.getSelection();
  if (!p || !s) return;
  const { text, node } = selectionText(s);
  // チャットの入力欄など中央のページの外の選択 = 前の選択を残す（入力欄を押すと選択が移る）
  if (!inPage(node)) return;
  if (!text.trim()) {
    if (s.isCollapsed) viewing.sel = null;
    return;
  }
  viewing.sel = { key: p, text };
}

function onSelection() {
  if (!wanted()) return;
  setTimeout(readViewingSelection, 0);
}

/** selectionchange の登録と「元が変わったら選択を捨てる」effect（App の onMount から initChat と並べて呼ぶ。戻り値 = 解除） */
export function initViewing(): () => void {
  document.addEventListener("selectionchange", onSelection);
  const stopRoot = $effect.root(() => {
    $effect(() => {
      const k = viewingKey(viewingSource());
      untrack(() => viewingChanged(k));
    });
  });
  return () => {
    stopRoot();
    document.removeEventListener("selectionchange", onSelection);
  };
}
