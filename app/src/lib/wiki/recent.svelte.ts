// OpenNote: 最近開いたページ。検索欄に入って何も打っていないとき、検索欄の真下に重ねる小さな一覧（WikiSearch）に出す
// = 本文・エディタは隠さない（作り直さない）。一覧を出す旗（focus）は欄の blur に加えて、欄が消えた（onDestroy）・前面のタブが変わった・Esc でも下ろす
// （欄にフォーカスしたまま OpenNote 以外のタブへ移ると blur が来ない）。IME の変換中は出し入れを変えない（frozen）。
// 記録 = 設定 ui_wiki_recent = JSON `{root: [path, …]}`（新しい順・ワークスペースごとに RECENT_MAX 件・set_ui_pref で書ける = Rust は変えない）。
// 記録するのは前面の OpenNote タブで読めたワークスペースの md / html ページ（参照フォルダのファイル・画像等のメディアは入れない）。
// 設定を読めなかった間は記録しない（空の記録で全ワークスペースの分を上書きしない）。
// 書くときに、ワークスペースの履歴（wiki_status.workspaces）から外した root の分を捨てる。出すときは今の木（wk.pages）に無いページを出さない
// （消えた・改名した・未接続 = 記録からは消さない）。壊れた JSON・形の違う値は空として読む（落ちない・次の記録で正しい形に書き直す）

import { untrack } from "svelte";
import { api } from "../api";
import { t } from "../i18n.svelte";
import { app, frontWiki, modalOpen, toast } from "../state.svelte";
import { baseName } from "./render/asset";
import { hasQuery, openWikiPage, titleIndex, wk } from "./wiki.svelte";
import { errText } from "../err";

export const RECENT_KEY = "ui_wiki_recent";
/** ワークスペースごとに覚える数 */
export const RECENT_MAX = 50;
/** 出す数 */
export const RECENT_SHOWN = 10;

export const recent = $state({
  /** root → path（新しい順）。raw = 置き換えだけ */
  map: {} as Record<string, string[]>,
  /** 検索欄にフォーカスがある（WikiSearch が立てる） */
  focus: false,
  /** 一覧の選択（検索欄の ↑↓ / Enter） */
  sel: 0,
  /** IME の変換中に止めた出し入れ（変換を始めたときの値・変換していなければ null） */
  frozen: null as boolean | null,
  /** 設定を読めなかった（一覧に「読めませんでした」と出す・次に欄へ入ったとき読み直す） */
  readError: null as string | null,
  /** 設定を読めた（ホームが「0 件」と言ってよいかを見る = 読む前の空の記録で 0 件と言わない） */
  loaded: false,
});

/** 一覧の旗を下ろす（欄の blur / onDestroy・前面のタブの変化・Esc） */
export function leaveRecent() {
  recent.focus = false;
  recent.frozen = null;
}

/** 設定の値 → 記録（形の違う物は捨てる = 落ちない） */
export function parseRecent(raw: string | null | undefined): Record<string, string[]> {
  if (!raw) return {};
  let v: unknown;
  try {
    v = JSON.parse(raw);
  } catch {
    return {};
  }
  if (!v || typeof v !== "object" || Array.isArray(v)) return {};
  const out: Record<string, string[]> = {};
  for (const [root, list] of Object.entries(v as Record<string, unknown>)) {
    if (!root || !Array.isArray(list)) continue;
    const paths = [...new Set(list.filter((p): p is string => typeof p === "string" && p.length > 0))].slice(0, RECENT_MAX);
    if (paths.length) out[root] = paths;
  }
  return out;
}

let loading: Promise<boolean> | null = null;
/** 設定を 1 回だけ読む。読めたら true（読めなければ false・次に呼ばれたら読み直す = その間は記録も外すもしない） */
export function loadRecent(): Promise<boolean> {
  loading ??= api.getUiPref(RECENT_KEY).then(
    (raw) => {
      recent.map = parseRecent(raw); // 記録・外すは読み終えてから（recordVisit / removeRecent が待つ）
      recent.readError = null;
      recent.loaded = true;
      return true;
    },
    (e) => {
      loading = null;
      recent.readError = String(e);
      console.warn("ui_wiki_recent:", e);
      return false;
    }
  );
  return loading;
}

let saving: Promise<unknown> = Promise.resolve();
/** 書く（履歴から外したワークスペースの分を捨ててから）。書けなければ知らせる（記録は手元に残る） */
function save() {
  const known = wk.status?.workspaces ? new Set(wk.status.workspaces.map((w) => w?.root).filter(Boolean)) : null;
  const shown = wk.status?.root;
  const out: Record<string, string[]> = {};
  for (const [root, list] of Object.entries(recent.map)) {
    if (known && !known.has(root) && root !== shown) continue;
    if (list.length) out[root] = list;
  }
  recent.map = out;
  const text = JSON.stringify(out);
  saving = saving.then(() =>
    api.setUiPref(RECENT_KEY, text).catch((e) => toast(`✗ ${t("最近開いたページを保存できません")}: ${errText(e, 160)}`))
  );
}

/** 開いたページを頭へ（既に頭なら書かない = 保存のたびの wk.page の置き換えで書き続けない） */
export async function recordVisit(root: string, path: string) {
  if (!(await loadRecent())) return;
  const list = recent.map[root] ?? [];
  if (list[0] === path) return;
  recent.map = { ...recent.map, [root]: [path, ...list.filter((p) => p !== path)].slice(0, RECENT_MAX) };
  save();
}

/** × = 記録から外す */
export async function removeRecent(root: string, path: string) {
  if (!(await loadRecent())) return;
  const list = recent.map[root];
  if (!list?.includes(path)) return;
  recent.map = { ...recent.map, [root]: list.filter((p) => p !== path) };
  save();
}

/** 一覧を出すか = 検索欄にいて、語も tag も無い（IME の変換中は変換を始めたときのまま）。モーダル・参照エディタ・取込の覆いの下では出さない
    （欄にフォーカスを残したまま覆いが開くことがある = 参照エディタは初期フォーカスを取らない → ↑↓ / Enter も効かない） */
export function recentShown(): boolean {
  if (modalOpen() || app.editorOpen || app.importJob) return false;
  if (recent.frozen !== null) return recent.frozen;
  return recent.focus && !hasQuery();
}

/** 出す一覧（画面のワークスペース・今の木に在る物だけ・前面のタブの今のページは除く・RECENT_SHOWN 件） */
export function recentList(): { path: string; title: string }[] {
  const root = wk.status?.root;
  if (!root) return [];
  const titles = titleIndex(wk.pages);
  const here = frontWiki()?.path ?? null;
  const out: { path: string; title: string }[] = [];
  for (const p of recent.map[root] ?? []) {
    if (p === here) continue;
    const title = titles.get(p);
    if (title === undefined) continue;
    out.push({ path: p, title: title || baseName(p) });
    if (out.length >= RECENT_SHOWN) break;
  }
  return out;
}

/** 一覧のページを開く（検索欄から出る = 一覧を畳む） */
export async function openRecent(path: string, newTab = false) {
  if (!newTab) {
    leaveRecent();
    (document.getElementById("wk-search") as HTMLInputElement | null)?.blur();
  }
  await openWikiPage(path, newTab);
}

/** 前面の OpenNote タブで読めたページを記録する・前面のタブが変わったら一覧の旗を下ろす（App の onMount で 1 回）。戻り値 = 止める */
export function initRecent(): () => void {
  return $effect.root(() => {
    let seenTab = untrack(() => app.tab);
    $effect(() => {
      const tab = app.tab;
      if (tab === seenTab) return;
      seenTab = tab;
      untrack(() => {
        const shown = recentShown();
        leaveRecent();
        // OpenNote のタブ同士の切替では欄が残る = 一覧を出していた（空の欄にいた）ならフォーカスも外す（残ると次に欄を押しても focus が来ず
        // 一覧が出ない）。語を打っている最中（一覧なし）は外さない
        const el = document.activeElement as HTMLElement | null;
        if (shown && el?.id === "wk-search") el.blur();
      });
    });
    $effect(() => {
      const pg = wk.page;
      const w = frontWiki();
      if (!pg || pg.external || (pg.kind !== "md" && pg.kind !== "html") || !w || w.path !== pg.path) return;
      const root = w.root ?? wk.status?.root;
      if (!root) return;
      untrack(() => void recordVisit(root, pg.path));
    });
  });
}
