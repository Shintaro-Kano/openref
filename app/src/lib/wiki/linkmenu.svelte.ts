// OpenNote: ページの中のリンクの右クリックのメニュー（WebView の既定のメニュー = 押しても何も起きない「Open Link in New Window」
// 「Download Linked File」「Share…」等の代わり）。項目はリンクの種類ごとに、効く物だけ（linkItems）。
// 項目を組むのは描画部品（ShadowPage = ページ・分割のプレビュー・チャットの応答）、描くのは WikiLinkMenu（App に 1 つ）。
// 状態はここ（wk には入れない = ワークスペースの写しの対象外・チャットは OpenNote のタブの外でも出る）

import { t } from "../i18n.svelte";
import { toast } from "../state.svelte";
import type { WikiNav } from "./types";
import type { Nav } from "./render/sanitize";
import { absOf } from "./render/asset";
import { opensInNewTab } from "./wiki.svelte";

/** メニューの 1 行（null = 区切り） */
export interface LinkMenuItem {
  label: string;
  run: () => unknown;
}

export interface LinkMenu {
  /** 右クリックの位置（viewport） */
  x: number;
  y: number;
  /** 1 行目の淡い見出し = リンク先（書かれたまま） */
  head: string;
  items: (LinkMenuItem | null)[];
  /** 右クリックした描画部品（shadow の host）。これを動かすスクロールでだけ閉じる（チャットの応答の自動追従など別の箱では閉じない） */
  origin?: Element;
}

export const linkMenu = $state({ cur: null as LinkMenu | null });

export function openLinkMenu(m: LinkMenu) {
  linkMenu.cur = m;
}

export function closeLinkMenu() {
  linkMenu.cur = null;
}

/** 描画の分類（navOf）→ 遷移の要求（WikiNav）。同じページの見出し・未解決・分類なしは null（描画部品が自分で扱う） */
export function toWikiNav(n: Nav): WikiNav | null {
  switch (n.t) {
    case "page":
      return n.heading ? { kind: "page", path: n.path, heading: n.heading } : { kind: "page", path: n.path };
    case "item": {
      const v: WikiNav = { kind: "item", key: n.key };
      if (n.page != null) v.page = n.page;
      if (n.att) v.att = n.att;
      return v;
    }
    case "ext":
      return { kind: "external", url: n.url };
    case "open":
      return { kind: "open", abs: n.abs };
    case "ref":
      return { kind: "ref", abs: n.abs };
    default:
      return null;
  }
}

export interface LinkCtx {
  /** 描画部品の onNavigate（newTab = 新しいタブで開く） */
  go: (nav: WikiNav, newTab?: boolean) => unknown;
  /** 同じページの見出しへスクロール */
  anchor: (id: string) => void;
  /** 描画の root（ページ = ワークスペース・参照フォルダのページ = そのフォルダ）= root 相対の path を絶対パスにする起点 */
  root: string;
}

/** リンクの右クリックのメニューの項目。n = リンクの分類（navOf）・written = 書かれたままのリンク（data-wk-href = `[[名前]]` / URL / 相対パス）。
    並び = 開く系 → 区切り → リンクをコピー（書かれたまま = `[[名前]]` と URL はどのページに貼っても同じリンク・相対パスは書いたページからの相対）。
    md のページ = 開く / 新しいタブで開く。md 以外（PDF・html・画像等 = いつも自分のタブで開く）と参照フォルダのファイル = 開く / 外部のアプリで開く。
    内蔵で開けないファイル = 外部のアプリで開く。文献 = MY LIBRARY に表示（いつも新しいタブ）。URL = ブラウザで開く。同じページの見出し = 移動 */
export function linkItems(n: Nav, written: string, c: LinkCtx): (LinkMenuItem | null)[] {
  const copy: LinkMenuItem = { label: t("リンクをコピー"), run: () => copyText(written) };
  const tail = written ? [null, copy] : [];
  const w = toWikiNav(n);
  const open = (label: string): LinkMenuItem => ({ label, run: () => w && c.go(w) });
  const external = (abs: string): LinkMenuItem => ({ label: t("外部のアプリで開く"), run: () => c.go({ kind: "open", abs }) });
  switch (n.t) {
    case "page":
      if (opensInNewTab(n.path)) return [open(t("開く")), external(absOf(c.root, n.path)), ...tail];
      return [open(t("開く")), { label: t("新しいタブで開く"), run: () => w && c.go(w, true) }, ...tail];
    case "ref":
      return [open(t("開く")), external(n.abs), ...tail];
    case "open":
      return [external(n.abs), ...tail];
    case "item":
      return [open(t("MY LIBRARY に表示")), ...tail];
    case "ext":
      return [open(t("ブラウザで開く")), ...tail];
    case "anchor":
      return [{ label: t("この見出しへ移動"), run: () => c.anchor(n.id) }, ...tail];
    default:
      // 未解決・分類なし = 開く先が無い。コピーだけ
      return written ? [copy] : [];
  }
}

/** 選択の「コピー」（WebView の既定のメニューの代わり）を項目の頭に足す。selected = 選択している文字（無ければ足さない） */
export function withSelection(items: (LinkMenuItem | null)[], selected: string): (LinkMenuItem | null)[] {
  if (!selected || !items.length) return items;
  return [{ label: t("コピー"), run: () => copyText(selected) }, null, ...items];
}

export async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    toast(t("コピーしました"));
  } catch (e) {
    toast(`✗ ${t("コピーできませんでした")}: ${String(e).slice(0, 160)}`);
  }
}
