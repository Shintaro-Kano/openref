// 表示設定（settings の ui_* = 画面の開閉・並び・折畳・言語）の読み書き。state.svelte.ts から出した。値は app（state）に持ったまま。
// キー・既定値・読む時機・書く時機・書く値の形は state にあった時と同じ（DB の settings の行と互換）。読む時機は 3 つに分かれている:
//   main.ts が mount の前に待つ（ui_lang・ui_detail・ui_list_sort・ui_viewer_side = 最初の描画から正しい形）/
//   App の onMount（loadFolderPrefs = フォルダ・ラベルの並びと折畳・loadCitePrefs と同じ場所）/ 左の列の幅は sidebarw.svelte.ts。
// 書くのは変えた時だけ（失敗は toast = 画面の値はそのまま）。
// state ⇄ prefs は互いに import する = module の評価時には app / toast に触らない（関数の中だけ）

import { api, type ListSortKey } from "./api";
import { setLang, t } from "./i18n.svelte";
import { cmpFor } from "./sort";
import { app, toast } from "./state.svelte";
import { errText } from "./err";

export function saveUiPref(key: string, value: string) {
  api.setUiPref(key, value).catch((e) => toast(`✗ ${t("表示設定を保存できません")}: ${errText(e, 160)}`));
}

export function saveCollapsed() {
  saveUiPref("ui_folders_collapsed", JSON.stringify([...app.foldersCollapsed]));
}

/** 起動時: フォルダ・ラベルの並び順とフォルダの折畳を読む（App.svelte の onMount・loadCitePrefs と同じ場所） */
export async function loadFolderPrefs() {
  try {
    const [sort, tagSort, collapsed] = await Promise.all([
      api.getUiPref("ui_folder_sort"),
      api.getUiPref("ui_tag_sort"),
      api.getUiPref("ui_folders_collapsed"),
    ]);
    if (sort === "alpha" || sort === "count") app.folderSort = sort;
    if (tagSort === "alpha" || tagSort === "count") {
      app.tagSort = tagSort;
      app.tags = app.tags.slice().sort(cmpFor(tagSort)); // refreshSidebar が先に終わっていた場合の並べ直し
    }
    if (collapsed) {
      const keys: unknown = JSON.parse(collapsed);
      if (Array.isArray(keys)) for (const k of keys) if (typeof k === "string") app.foldersCollapsed.add(k);
    }
  } catch (e) {
    toast(t("✗ フォルダ・ラベルの表示設定の読込に失敗: ") + errText(e, 160));
  }
}

/** 起動時: 表記の言語（Settings › Language = ui_lang）。main.ts が mount の前に待つ。切替は Rust の emit "ui-lang" を App.svelte が受けて setLang */
export async function loadLangPref() {
  try {
    setLang(await api.getUiPref("ui_lang"));
  } catch (e) {
    toast(t("✗ 言語の設定の読込に失敗: ") + errText(e, 160));
  }
}

/** 起動時: 一覧の右ペインの開閉（ui_detail）。main.ts が mount の前に待つ（mount の後に閉じると最初の 1 フレームに出て消える）。読めなければ既定（開く） */
export async function loadDetailPref() {
  try {
    app.detailOpen = (await api.getUiPref("ui_detail")) !== "0";
  } catch (e) {
    toast(`✗ ${t("表示設定を読めません")}: ${errText(e, 160)}`);
  }
}

/** 一覧の右ペインの開閉（ListToolbar のボタン・⌘⌥I） */
export function toggleDetail() {
  app.detailOpen = !app.detailOpen;
  saveUiPref("ui_detail", app.detailOpen ? "1" : "0");
}

/** 起動時: PDF ビューアの右バーの開閉（ui_viewer_side）。main.ts が mount の前に待つ（ui_detail と同じ = 最初の描画で出て消えない）。
    読めなければ既定（開く） */
export async function loadViewerSidePref() {
  try {
    app.viewerSideOpen = (await api.getUiPref("ui_viewer_side")) !== "0";
  } catch (e) {
    toast(`✗ ${t("表示設定を読めません")}: ${errText(e, 160)}`);
  }
}

/** PDF ビューアの右バーの開閉（ツールバーの右端のボタン・PDF のタブが前面のときの ⌘⌥I） */
export function toggleViewerSide() {
  app.viewerSideOpen = !app.viewerSideOpen;
  saveUiPref("ui_viewer_side", app.viewerSideOpen ? "1" : "0");
}

/** 右バーの中身が目的地の操作（⌘F・ツールバーの ⌕）の前に: 閉じていれば開く（開いていれば何もしない = 書かない） */
export function showViewerSide() {
  if (!app.viewerSideOpen) toggleViewerSide();
}

/** 起動時: 一覧の並び（ui_list_sort = `added:desc` の形）。main.ts が mount の前に待つ（最初の一覧から選んだ並び）。読めない・形が違えば既定（追加日・降順） */
export async function loadListSortPref() {
  try {
    const m = /^(added|published|journal):(asc|desc)$/.exec((await api.getUiPref("ui_list_sort")) ?? "");
    if (m) app.listSort = { key: m[1] as ListSortKey, desc: m[2] === "desc" };
  } catch (e) {
    toast(`✗ ${t("表示設定を読めません")}: ${errText(e, 160)}`);
  }
}

/** ListToolbar の並びのメニューの設定の側: 選択中の項目 = 向きを反転・別の項目 = その項目の自然な向き（誌名 = A→Z・日付 = 新しい順）。
    登録表の list_items の desc の省略と同じ。一覧の取り直しは state の pickListSort */
export function setListSort(key: ListSortKey) {
  const cur = app.listSort;
  const desc = cur.key === key ? !cur.desc : key !== "journal";
  app.listSort = { key, desc };
  saveUiPref("ui_list_sort", `${key}:${desc ? "desc" : "asc"}`);
}

export function setFolderSort(sort: "alpha" | "count") {
  app.folderSort = sort;
  saveUiPref("ui_folder_sort", sort);
}

export function toggleFolderCollapsed(key: string) {
  if (app.foldersCollapsed.has(key)) app.foldersCollapsed.delete(key);
  else app.foldersCollapsed.add(key);
  saveCollapsed();
}

/** ラベルの並びを変える。永続して（ui_tag_sort）app.tags をフロントで並べ直すだけ（IPC で取り直さない） */
export function setTagSort(sort: "alpha" | "count") {
  app.tagSort = sort;
  saveUiPref("ui_tag_sort", sort);
  app.tags = app.tags.slice().sort(cmpFor(sort));
}
