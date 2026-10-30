import { DEFAULT_LABEL_COLOR } from "./labels";
import {
  api,
  type CollectionRow,
  type ItemDetail,
  type ImportStatus,
  type ItemSummary,
  type StyleEntry,
  type TagRow,
  type ViewCounts,
} from "./api";
import { formatBibliographyMany } from "./cite";

export type ImportFileStatus = "pending" | "running" | "canceled" | ImportStatus;
export interface ImportFile {
  path: string;
  name: string;
  status: ImportFileStatus;
  message: string;
  key: string | null;
}
/** PDF 一括取込ジョブ（モーダルの状態そのもの） */
export interface ImportJob {
  files: ImportFile[];
  running: boolean;
  cancel: boolean;
}
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { tick } from "svelte";
import { SvelteSet } from "svelte/reactivity";
import { i18n, setLang, t, tn } from "./i18n.svelte";

export type View =
  | { kind: "all" }
  | { kind: "starred" }
  | { kind: "trash" }
  | { kind: "collection"; id: number; name: string }
  | { kind: "tag"; id: number; name: string };

/** 検索ボックス内のチップ（Paperpile の author / journal フィルタ）。検索語・ビューと AND */
export interface Filters {
  author: null | { family: string; initial: string | null; label: string };
  journal: null | string;
}

/** PDF タブのビューア。attachmentId で同じ PDF のタブを見分ける */
export interface TabViewer {
  itemKey: string;
  attachmentId: number;
  title: string;
}

/** タブ 1 枚 = 一覧の状態一式。不活性のタブはここに snapshot（値の複製）で眠る。
    前面のタブの中身は app.view / search / filters / items / checked / checkAnchor / selected / detail が正で、
    ここは切替の時に書く（前面の view は tabView() で読む）。
    viewer（PDF タブ = 一覧の上にビューアを重ねる）だけは前面も含めて常にここが正（snapshot / restore しない。
    ビューアのインスタンスがタブに付いて生きている = 切り替えてもページ位置・ズーム・検索語を失わない） */
export interface Tab {
  id: number;
  view: View;
  search: string;
  filters: Filters;
  items: ItemSummary[];
  checked: string[];
  checkAnchor: string | null;
  selected: string | null;
  detail: ItemDetail | null;
  scrollTop: number;
  viewer: TabViewer | null;
}

let nextTabId = 1;
function blankTab(view: View = { kind: "all" }, search = "", filters: Filters = { author: null, journal: null }): Tab {
  return { id: nextTabId++, view, search, filters, items: [], checked: [], checkAnchor: null, selected: null, detail: null, scrollTop: 0, viewer: null };
}
const firstTab = blankTab();

export const app = $state({
  view: { kind: "all" } as View,
  search: "",
  /** reveal が検索語・チップを外した / タブを切り替えた回数（App の検索欄が、入力中の文字と待ち中の検索を捨てて app.search を写す合図） */
  searchReset: 0,
  filters: { author: null, journal: null } as Filters,
  /** タブ（ヘッダーの上に 1 段）。前面 = id が tab のもの */
  tabs: [firstTab] as Tab[],
  tab: firstTab.id,
  /** ui_tabs の復元が済んだ（済むまで保存しない = マウント直後の既定の 1 枚で保存済みを上書きしない） */
  tabsRestored: false,
  items: [] as ItemSummary[],
  /** 詳細パネルに出している 1 件（クリック選択） */
  selected: null as string | null,
  /** チェックボックスで選んだ複数件（一括操作の対象。Paperpile の select references） */
  checked: new SvelteSet<string>(),
  /** shift クリック範囲選択の起点 */
  checkAnchor: null as string | null,
  /** 一括ツールバーで開いているメニュー（Esc の優先順位制御のため state に置く） */
  listMenu: null as null | "folder" | "label" | "cite",
  detail: null as ItemDetail | null,
  collections: [] as CollectionRow[],
  /** サイドバーのフォルダの並び（Sort folders）。ui_folder_sort に永続 */
  folderSort: "alpha" as "alpha" | "count",
  /** 折り畳んだフォルダの key（ui_folders_collapsed に永続。id は削除後に再利用されうるので key で持つ） */
  foldersCollapsed: new SvelteSet<string>(),
  tags: [] as TagRow[],
  tagSort: "alpha" as "alpha" | "count",
  counts: { all: 0, starred: 0, trash: 0 } as ViewCounts,
  toast: "",
  dropActive: false,
  busy: false,
  /** 起動時の新版確認で分かった、動いている版より新しい版番号（ヘッダー下の帯に出す。× でこの起動の間だけ null に） */
  updateLatest: null as string | null,
  modal: null as
    | null
    | "newLabel"
    | "manageLabels"
    | "purge"
    | "citeStyle"
    | "newFolder"
    | "renameFolder"
    | "deleteFolder"
    | "manageFolders"
    | "migration",
  /** フォルダのモーダルの対象（Rename / Delete = id、New folder = parentId（null = root）） */
  folderTarget: { id: null as number | null, parentId: null as number | null },
  /** フォルダ行の「…」/ 右クリックのメニュー（fixed で出す位置）。Esc の優先順位制御のため state に置く */
  folderMenu: null as null | { id: number; top: number; left: number },
  /** 完全削除の確認対象（modal === "purge" の間だけ意味を持つ。一覧順の key） */
  purgeKeys: [] as string[],
  /** 行下のインラインメモ編集。複数メモは noteKey で選択。saved = 保存済み本文・text = 編集中 */
  noteEditor: null as null | { key: string; noteKey: string | null; text: string; saved: string },
  /** 「＋ 新規 ▾」メニューの開閉 */
  newMenu: false,
  /** サイドバー見出しの「…」メニュー（LABELS = Sort labels / Manage labels…、MY LIBRARY = Sort folders / Manage folders…）。
      Esc の優先順位制御のため state に置く */
  labelMenu: false as false | "labels" | "folders",
  /** PDF 一括取込（non-null の間モーダル表示） */
  importJob: null as null | ImportJob,
  editorOpen: false,
  editorKey: null as string | null,
  editorCsl: null as any,
  /** 引用スタイル（Paperpile の Change citation style…）。style/locale/doiAlways は DB settings に永続 */
  cite: {
    style: "vancouver",
    /** "" = スタイルの既定言語 */
    locale: "",
    /** DOI（無ければ URL）を常に含める（Paperpile の Always include DOI and URL） */
    doiAlways: false,
    /** 同梱 4 種 + 導入済み（表示名の解決・スタイル設定の一覧） */
    installed: [] as StyleEntry[],
  },
  // pointer ベースの内部 D&D（Tauri dragDropEnabled と HTML5 dnd が競合するため自作）
  // kind: rows = 行 → サイドバーのフォルダ/ラベル / label = サイドバーのラベル → 行（Paperpile の「ラベルを論文へドラッグ」）
  drag: null as null | {
    kind: "rows" | "label";
    label: string;
    x: number;
    y: number;
    over: string | null;
  },
});

/** 処理中（app.busy）の数。重なった処理の片方が終わっても、残りが終わるまで busy を下ろさない
    （busy はタブの切替を止める根拠 = 後から始めた処理の finally が先に下ろすと、残った処理の後処理が別のタブに効く） */
let busyN = 0;
function busyBegin() {
  busyN++;
  app.busy = true;
}
function busyEnd() {
  busyN = Math.max(0, busyN - 1);
  app.busy = busyN > 0;
}

let toastTimer: ReturnType<typeof setTimeout> | undefined;
export function toast(msg: string) {
  // 英語は断片を日本語の語順で並べるので、文頭（✓ ✗ ⚠ の後）が小文字の訳になりうる → 大文字に
  app.toast = i18n.lang === "en" ? msg.replace(/^([✓✗⚠]?\s*)([a-z])/, (_, p, c) => p + c.toUpperCase()) : msg;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (app.toast = ""), 5000);
}

export async function refreshSidebar() {
  const [collections, counts, tags] = await Promise.all([
    api.collectionsTree(),
    api.viewCounts(),
    api.listTags(app.tagSort === "count"),
  ]);
  app.collections = collections;
  app.counts = counts;
  app.tags = tags;
}

/** list_items は非同期（本文検索が重いことがある）。連打で古い応答が後から届いても新しい一覧を上書きしない */
let listSeq = 0;

export async function refreshList() {
  const v = app.view;
  const seq = ++listSeq;
  const items = await api.listItems({
    search: app.search || null,
    collectionId: v.kind === "collection" ? v.id : null,
    tagId: v.kind === "tag" ? v.id : null,
    starredOnly: v.kind === "starred",
    trashed: v.kind === "trash",
    authorFamily: app.filters.author?.family ?? null,
    authorInitial: app.filters.author?.initial ?? null,
    journal: app.filters.journal,
    limit: 500,
  });
  if (seq !== listSeq) return;
  app.items = items;
  if (app.selected && !app.items.some((i) => i.key === app.selected)) closeDetail();
  // 一覧から消えた item（ゴミ箱移動・絞込変更）はチェックからも外す
  const visible = new Set(app.items.map((i) => i.key));
  for (const k of [...app.checked]) if (!visible.has(k)) app.checked.delete(k);
  if (app.checkAnchor && !visible.has(app.checkAnchor)) app.checkAnchor = null;
}

export async function refreshAll() {
  await Promise.all([refreshSidebar(), refreshList()]);
}

// 不変条件: 詳細に出る item は常にチェック中（詳細に出ている item にはツールバーの操作が常に効く、という前提）。
// 開くのは select だけで、開くときにその行だけをチェックする。チェックが外れたら closeDetailIfUnchecked が閉じる

/** 詳細に出す = その行だけをチェック + 詳細表示（2 回目でも外さない）。今の一覧に無い item はチェックできないので開かない
    （新しく作った item は reveal が一覧に出してから開く） */
export async function select(key: string) {
  if (!app.items.some((i) => i.key === key)) return;
  app.checked.clear();
  app.checked.add(key);
  app.checkAnchor = key;
  app.selected = key;
  await loadDetail(key);
}

export async function reloadDetail() {
  if (app.selected) await loadDetail(app.selected);
}

/** key の詳細を読んで出す。応答待ちの間に閉じた・別の行を開いた場合は、古い応答で上書きしない。
    読めなければ（失敗・item が無い）詳細を閉じて知らせる（前の item の詳細を出したまま固まらない） */
async function loadDetail(key: string) {
  let d: ItemDetail | null = null;
  let err = "";
  try {
    d = await api.getItem(key);
  } catch (e) {
    err = String(e);
  }
  if (app.selected !== key) return;
  if (d) {
    app.detail = d;
    return;
  }
  closeDetail();
  toast(`✗ ${t("詳細を読めません")}: ${err.slice(0, 200) || t("item が見つかりません")}`);
}

/** 新しく作った / 取り込んだ item を開く（quickAdd・手動作成・取込結果の行）。今の一覧に無ければ、ゴミ箱内の item
    （取込の「重複」がゴミ箱の item と md5 一致など）はゴミ箱、それ以外は「すべて」に移り、検索語と著者 / 誌名チップも外して
    一覧を取り直してから開く（一覧に無い item は select が開かないため）。それでも出せなければ黙らずに知らせる。
    待つ間に別のタブへ切り替わったら（取込結果の行は取込モーダルを閉じてから走る）そこでやめる（切替先の検索語・チップ・ビューを変えない）。
    前面が PDF タブ（⌘⇧N の作成・ビューアの上の取込）なら、その下の一覧（見えない）は触らず、新しい一覧のタブ（すべて）を前面に
    開いてそこで出す（呼び出し側はエディタ・取込モーダルを閉じてから呼ぶ = タブを開ける）。開けなければ（処理中・取込の実行中）知らせる。
    一覧の取り直しの失敗（IPC）も toast に出して reject しない（呼び出し側のエディタ・取込モーダルは閉じた後 = そこには出せない） */
export async function reveal(key: string) {
  try {
    await revealNow(key);
  } catch (e) {
    toast(`✗ ${t("一覧に表示できませんでした")}: ${String(e).slice(0, 200)}`);
  }
}

async function revealNow(key: string) {
  if (frontViewer()) {
    await openTab();
    if (frontViewer()) {
      toast(t("✗ 一覧に表示できませんでした（PDF のタブから新しいタブを開けません。処理中・取込の実行中は後で開いてください）"));
      return;
    }
  }
  const tab = app.tab;
  const listed = () => app.items.some((i) => i.key === key);
  if (!listed()) {
    const d = await api.getItem(key);
    if (app.tab !== tab) return;
    if (!d) {
      toast(t("✗ 一覧に表示できませんでした（item が見つかりません）"));
      return;
    }
    app.search = "";
    app.filters.author = null;
    app.filters.journal = null;
    app.searchReset++;
    await setView({ kind: d.deleted ? "trash" : "all" });
    if (app.tab !== tab) return;
    // 一覧の取得が別の refreshList に追い越された（取込の終了と重なった等）ときは、もう 1 回だけ取り直す
    if (!listed()) await refreshList();
    if (app.tab !== tab) return;
  }
  if (!listed()) {
    toast(t("✗ 一覧に表示できませんでした（表示件数の上限の外など）"));
    return;
  }
  await select(key);
}

function closeDetail() {
  app.selected = null;
  app.detail = null;
}

/** 詳細の item のチェックが外れたら詳細を閉じる（☐・⌘クリック・Space・Esc・☑ で解除・行の 2 回目のクリック）。
    チェックを外しうる関数（toggleCheck / clearChecks）の最後に呼ぶ */
function closeDetailIfUnchecked() {
  if (app.selected && !app.checked.has(app.selected)) closeDetail();
}

/** v のフォルダ / ラベルが今のサイドバーにあるか（すべて・★・ゴミ箱は常にある） */
function viewLive(v: View): boolean {
  if (v.kind === "collection") return app.collections.some((c) => c.id === v.id);
  if (v.kind === "tag") return app.tags.some((t) => t.id === v.id);
  return true;
}

export async function setView(v: View) {
  const tab = app.tab;
  const live = viewLive(v);
  await autoSaveAndCloseNote();
  // メモの保存を待つ間に別のタブへ切り替わった（前のタブでのクリックを切替先に効かせない）・
  // 開こうとしたフォルダ / ラベルがその間に消えた（削除と行き違い）なら何もしない
  if (app.tab !== tab || (live && !viewLive(v))) return;
  app.view = v;
  keepOnlyDetailChecked();
  await refreshList();
}

/** ビューを設定するとき（setView・patchViews）: 詳細の item のチェックだけは残す（新しい一覧に無ければ refreshList がチェックからも詳細からも外す） */
function keepOnlyDetailChecked() {
  for (const k of [...app.checked]) if (k !== app.selected) app.checked.delete(k);
  app.checkAnchor = app.selected;
}

// ---------- タブ（ブラウザ型・ヘッダーの上に 1 段） ----------
// 入替方式: app.view / search / filters / items / selected / checked / checkAnchor / detail は常に前面のタブを指し
// （コンポーネントはタブを知らない）、不活性のタブは app.tabs[i] の snapshot に眠る。切り替えるときに前面を snapshot へ
// 複製し、行き先の snapshot を app.* へ複製して戻す（参照を共有すると、その場で書き換える app.filters 等が眠っているタブを書き換える）

/** タブのビュー（前面は app.view が正） */
export function tabView(t: Tab): View {
  return t.id === app.tab ? app.view : t.view;
}

/** ビューの題名。フォルダ / ラベルは id から引く（MCP / CLI 経由の改名にも追随）。見つからなければ持っている名前 */
export function viewTitle(v: View): string {
  switch (v.kind) {
    case "all":
      return t("すべて");
    case "starred":
      return "★ Starred";
    case "trash":
      return t("ゴミ箱");
    case "collection":
      return app.collections.find((c) => c.id === v.id)?.name ?? v.name;
    case "tag":
      return app.tags.find((t) => t.id === v.id)?.name ?? v.name;
  }
}

/** 前面のタブの PDF ビューア（無ければ null = 一覧が見えている） */
export function frontViewer(): TabViewer | null {
  return app.tabs.find((t) => t.id === app.tab)?.viewer ?? null;
}

/** タブの操作（切替・開く・閉じる）を受け付けるか。処理中（busy）・エディタ・モーダル・取込中は受け付けない
    （一括操作などの後処理 = clearChecks / closeDetail / reloadDetail が別のタブに効く事故を 1 つの規則で防ぐ。
    purge モーダル中に切り替わると PurgeModal が新しいタブの一覧から題名を引けない等）。PDF ビューアの中では受け付ける */
export function tabOpsAllowed(): boolean {
  return !app.busy && !app.editorOpen && !app.modal && !app.importJob;
}

/** 切替の世代。各 await の後で比べ、追い越された（連打・切替の途中の切替）切替はそこで降りる */
let tabGen = 0;

function rowsEl(): HTMLElement | null {
  return document.querySelector<HTMLElement>(".list > .rows");
}

/** 前面の状態を t へ複製する。詳細は選択中の item のものだけ（選んだ行の読込を待つ間に眠ると、前の行の詳細と新しい行の
    チェックが食い違ったまま戻る）。スクロール位置はこの瞬間の DOM（まだ前面のタブの一覧）から読む */
function snapshotInto(t: Tab) {
  t.view = $state.snapshot(app.view) as View;
  t.search = app.search;
  t.filters = $state.snapshot(app.filters) as Filters;
  t.items = $state.snapshot(app.items) as ItemSummary[];
  t.checked = [...app.checked];
  t.checkAnchor = app.checkAnchor;
  t.selected = app.selected;
  t.detail = app.detail && app.detail.key === app.selected ? ($state.snapshot(app.detail) as ItemDetail) : null;
  t.scrollTop = rowsEl()?.scrollTop ?? 0;
}

/** t の snapshot を app.* へ複製して戻す。前面の間は t の一覧と詳細を使わないので空にする（二重に持たない） */
function restoreFrom(t: Tab) {
  app.view = $state.snapshot(t.view) as View;
  app.search = t.search;
  app.filters = $state.snapshot(t.filters) as Filters;
  app.items = $state.snapshot(t.items) as ItemSummary[];
  app.checked.clear();
  for (const k of t.checked) app.checked.add(k);
  app.checkAnchor = t.checkAnchor;
  app.selected = t.selected;
  app.detail = $state.snapshot(t.detail) as ItemDetail | null;
  t.items = [];
  t.detail = null;
}

/** id のタブを前面にする（クリック・⌘1〜9・巡回）。前面を snapshot に眠らせて id の snapshot を戻し、一覧と詳細を
    取り直す（他のタブでゴミ箱へ移した item・付けた PDF・改名を反映。一覧から消えた item は refreshList がチェックと
    詳細から外す = 詳細の item は常にチェック中）。スクロール位置は戻した一覧の描画の後に戻す */
export async function activateTab(id: number) {
  await switchTab(id, false);
}

/** 切替の本体。closeFront = 前面のタブを snapshot にせず除く（前面を閉じる）。切り替えたら true */
async function switchTab(id: number, closeFront: boolean): Promise<boolean> {
  if (id === app.tab || !app.tabs.some((t) => t.id === id) || !tabOpsAllowed()) return false;
  const gen = ++tabGen;
  // 開いているメモは前のタブで保存して閉じる（保存の後処理 = 一覧と詳細の取り直しも前のタブに効かせてから切り替える）。
  // 待つ間に追い越された・保存に失敗して開いたまま（未保存 = 前のタブに残して失わせない）・行き先が閉じられた・
  // 保存の後に一括操作やモーダルが始まった（busy・modal）なら切り替えない
  await autoSaveAndCloseNote();
  if (gen !== tabGen || app.noteEditor || !app.tabs.some((t) => t.id === id) || !tabOpsAllowed()) return false;
  // メニューは閉じる（ツールバーのメニューは前のタブのチェックが対象）
  app.listMenu = null;
  app.folderMenu = null;
  app.labelMenu = false;
  app.newMenu = false;
  const fi = app.tabs.findIndex((t) => t.id === app.tab);
  if (fi >= 0) {
    if (closeFront) app.tabs.splice(fi, 1);
    else snapshotInto(app.tabs[fi]);
  }
  listSeq++; // 進行中の list_items（前のタブのもの）の応答を捨てる
  const target = app.tabs.find((t) => t.id === id)!;
  const top = target.scrollTop;
  restoreFrom(target);
  app.tab = id;
  app.searchReset++; // 検索欄の文字を戻したタブの検索語に（待ち中の検索は捨てる）
  // 戻した一覧の描画を待ってスクロール位置を戻す（tick は microtask なので、ここに別の切替は割り込めない）
  await tick();
  const el = rowsEl();
  if (el && gen === tabGen) el.scrollTop = top;
  await refreshList();
  if (gen !== tabGen) return true;
  // refreshList は一覧とチェックしか取り直さない。詳細は snapshot のまま古い（消えたフォルダのチップ・付けた PDF）ので読み直す
  if (app.selected) await reloadDetail();
  return true;
}

/** 新しいタブを末尾に足して前面で開く（⌘T・＋・⌘クリック / 中クリック・フォルダの「新しいタブで開く」）。
    中身は v（既定 = すべて）だけ = 検索語・チップ・チェックなし */
export async function openTab(v: View = { kind: "all" }) {
  if (!tabOpsAllowed()) return;
  const live = viewLive(v);
  // 開いているメモは先に保存して閉じる（保存に失敗したらタブを作らない = 切り替えられない空のタブを残さない）。
  // 待つ間に開こうとしたフォルダ / ラベルが消えた（削除と行き違い）ら作らない（消えたビューのタブを残さない）
  await autoSaveAndCloseNote();
  if (app.noteEditor || !tabOpsAllowed() || (live && !viewLive(v))) return;
  const t = blankTab($state.snapshot(v) as View);
  app.tabs.push(t);
  await switchTab(t.id, false);
}

/** タブを閉じる（× / 中クリック / ⌘W・PDF タブはビューアの × / Esc も）。最後の 1 枚は閉じない（PDF タブならビューアだけ
    閉じて下の一覧を見せる）。前面なら、開いているメモを先に保存してから（失敗したら閉じない）その時点の右隣（無ければ左）を
    前面にして除く（保存を待つ間に隣が閉じられても、残っている隣へ移る） */
export async function closeTab(id: number) {
  if (!tabOpsAllowed()) return;
  const i = app.tabs.findIndex((t) => t.id === id);
  if (i < 0) return;
  if (app.tabs.length <= 1) {
    app.tabs[i].viewer = null;
    return;
  }
  if (id !== app.tab) {
    app.tabs.splice(i, 1);
    return;
  }
  await autoSaveAndCloseNote();
  if (app.noteEditor || app.tab !== id || app.tabs.length <= 1 || !tabOpsAllowed()) return;
  const j = app.tabs.findIndex((t) => t.id === id);
  const next = app.tabs[j + 1] ?? app.tabs[j - 1];
  await switchTab(next.id, true);
}

/** 隣のタブへ（⌘⇧] ⌘⇧[ ⌃Tab ⌃⇧Tab。端からは反対の端へ）。step = +1 / -1 */
export function cycleTab(step: number) {
  const n = app.tabs.length;
  const i = app.tabs.findIndex((t) => t.id === app.tab);
  if (n < 2 || i < 0) return;
  void activateTab(app.tabs[(i + step + n) % n].id);
}

/** ⌘1〜8 = n 番目・⌘9 = 末尾（無ければ何もしない） */
export function activateTabAt(n: number) {
  const t = n >= 9 ? app.tabs[app.tabs.length - 1] : app.tabs[n - 1];
  if (t) void activateTab(t.id);
}

/** id のタブを to 番目へ動かす（TabStrip のドラッグ並べ替え）。配列の順そのものを変える = ⌘1〜9・巡回・⌘W の行き先・
    ui_tabs の保存（App の effect が見張る）はこの順に従う。前面の中身は app.* に居て配列の位置に依らないので切替は要らない。
    to は端に丸める。無い id・同じ位置なら何もしない。処理中（busy）でも受け付ける（並びは一覧の状態に触れない）。
    動く範囲 [lo, hi] を 1 回の splice で書き換える（抜いて挿す 2 回だと length が n → n-1 → n と揺れ、length を見る effect
    = TabStrip の前面タブの scrollIntoView が走って溢れた列が飛ぶ） */
export function moveTab(id: number, to: number) {
  const from = app.tabs.findIndex((t) => t.id === id);
  if (from < 0) return;
  to = Math.max(0, Math.min(app.tabs.length - 1, to));
  if (to === from) return;
  const lo = Math.min(from, to);
  const seg = app.tabs.slice(lo, Math.max(from, to) + 1);
  const [t] = seg.splice(from - lo, 1);
  seg.splice(to - lo, 0, t);
  app.tabs.splice(lo, seg.length, ...seg);
}

const sameTarget = (a: View, b: View) =>
  a.kind === b.kind && (a.kind === "collection" || a.kind === "tag" ? a.id === (b as { id: number }).id : true);

/** 全タブのビューを f で写す（フォルダ / ラベルの改名・削除・統合の追随）。f が null = 「すべて」へ・同じ値 = そのまま。
    行き先が変わったタブは setView と同じく詳細の item のチェックだけ残す。不活性のタブは snapshot を書き換え（旧ビューの一覧と
    スクロール位置は捨てる = 新しい題名の下に旧ビューの一覧を出さない。一覧と詳細は切替の時に取り直す）、前面は一覧を取り直す。
    全タブを await の前に同期で書く（メモの保存などを待つ間に切り替わっても、書き漏らすタブ・別のタブに効く書込みを作らない）。
    戻り値 = 前面の一覧を取り直した（呼び出し側が二重に取らないため） */
export async function patchViews(f: (v: View) => View | null): Promise<boolean> {
  for (const t of app.tabs) {
    if (t.id === app.tab) continue;
    const nv = f(t.view) ?? { kind: "all" };
    if (nv === t.view) continue;
    if (!sameTarget(nv, t.view)) {
      t.checked = t.selected ? t.checked.filter((k) => k === t.selected) : [];
      t.checkAnchor = t.selected;
      t.items = [];
      t.scrollTop = 0;
    }
    t.view = nv;
  }
  const cur = app.view;
  const nv = f(cur) ?? { kind: "all" };
  if (nv === cur) return false;
  const moved = !sameTarget(nv, cur);
  if (moved) keepOnlyDetailChecked();
  app.view = nv;
  if (moved) await refreshList();
  return moved;
}

// ---- 外の書込への追随（MCP・CLI・connectd が DB を書いたら読み直す） ----

let dataVersion: number | null = null;
let dataVersionPolling = false;
let dataVersionFailed = false;

/** 外の変更を見る間合い（処理中・モーダル・エディタ・メモの編集中・取込中・ドラッグ中は見送り = 書きかけを消さない） */
function externalRefreshAllowed(): boolean {
  return tabOpsAllowed() && !app.noteEditor && !app.drag;
}

/** 外で消えたフォルダ・ラベルのビューは「すべて」へ、改名は題名に追随（id で照合） */
function followExternal(v: View): View | null {
  if (v.kind === "collection") {
    const c = app.collections.find((c) => c.id === v.id);
    return !c ? null : c.name === v.name ? v : { ...v, name: c.name };
  }
  if (v.kind === "tag") {
    const t = app.tags.find((t) => t.id === v.id);
    return !t ? null : t.name === v.name ? v : { ...v, name: t.name };
  }
  return v;
}

/** App が 2 秒ごとに呼ぶ。PRAGMA data_version（他の接続のコミットで増える）が前回と違えばサイドバー・一覧・詳細を読み直す。
    見送った回・読み直しに失敗した回は前回の値を据え置く（次の回にもう一度）。失敗の toast は続く間 1 回だけ */
export async function pollDataVersion() {
  if (dataVersionPolling || !externalRefreshAllowed()) return;
  dataVersionPolling = true;
  try {
    const v = await api.dataVersion();
    if (dataVersion === null) dataVersion = v;
    if (v === dataVersion) {
      dataVersionFailed = false;
      return;
    }
    if (!externalRefreshAllowed()) return;
    await refreshSidebar();
    if (!(await patchViews(followExternal))) await refreshList();
    await reloadDetail();
    dataVersion = v;
    dataVersionFailed = false;
  } catch (e) {
    if (!dataVersionFailed) toast(`✗ ${t("外部の変更を読み込めません")}: ${String(e).slice(0, 160)}`);
    dataVersionFailed = true;
  } finally {
    dataVersionPolling = false;
  }
}

// ---- タブの永続（ui_tabs = ビュー・検索語・チップ・PDF・前面の番号だけ。チェック・詳細・スクロールは戻さない） ----

type ViewPref =
  | { kind: "all" | "starred" | "trash" }
  | { kind: "collection"; key: string }
  | { kind: "tag"; id: number; name: string };

/** フォルダは key で持つ（id は削除後に再利用されうる）。ラベルは key 列が無いので id + 名前（両方合うときだけ戻す） */
function viewPref(v: View): ViewPref {
  if (v.kind === "collection") {
    const c = app.collections.find((c) => c.id === v.id);
    return c ? { kind: "collection", key: c.key } : { kind: "all" };
  }
  if (v.kind === "tag") {
    const t = app.tags.find((t) => t.id === v.id);
    return t ? { kind: "tag", id: t.id, name: t.name } : { kind: "all" };
  }
  return { kind: v.kind };
}

/** 保存する ui_tabs の JSON（App の $effect が見張って 300ms 後に保存する） */
export function tabsPref(): string {
  return JSON.stringify({
    active: Math.max(0, app.tabs.findIndex((t) => t.id === app.tab)),
    tabs: app.tabs.map((t) => {
      const front = t.id === app.tab;
      return {
        view: viewPref(tabView(t)),
        search: front ? app.search : t.search,
        filters: front ? app.filters : t.filters,
        // PDF タブ = item key + 添付 id（照合用）+ 題名（ビューアの見出し・タブの題名）
        ...(t.viewer ? { viewer: { key: t.viewer.itemKey, attachment: t.viewer.attachmentId, title: t.viewer.title } } : {}),
      };
    }),
  });
}

/** ui_tabs を保存する。保存できたら true（失敗は toast。呼び出し側は次の変化で保存し直す） */
export async function saveTabsPref(json: string): Promise<boolean> {
  try {
    await api.setUiPref("ui_tabs", json);
    return true;
  } catch (e) {
    toast(`✗ ${t("タブを保存できません")}: ${String(e).slice(0, 160)}`);
    return false;
  }
}

function parseView(v: any): View | null {
  switch (v?.kind) {
    case "all":
    case "starred":
    case "trash":
      return { kind: v.kind };
    case "collection": {
      const c = app.collections.find((c) => c.key === v.key);
      return c ? { kind: "collection", id: c.id, name: c.name } : null;
    }
    case "tag": {
      const name = typeof v.name === "string" ? v.name.toLowerCase() : null;
      const t = app.tags.find((t) => t.id === v.id && t.name.toLowerCase() === name);
      return t ? { kind: "tag", id: t.id, name: t.name } : null;
    }
  }
  return null;
}

function parseFilters(f: any): Filters {
  const a = f?.author;
  const author =
    a && typeof a.family === "string" && typeof a.label === "string" && (a.initial === null || typeof a.initial === "string")
      ? { family: a.family, initial: a.initial as string | null, label: a.label }
      : null;
  return { author, journal: typeof f?.journal === "string" && f.journal ? f.journal : null };
}

/** ui_tabs の PDF タブの値（item key・添付 id の型が違えば null = 一覧のタブとして戻す。題名は無くてよい = checkViewer が今の題名にする） */
function parseViewer(v: any): TabViewer | null {
  return v && typeof v.key === "string" && v.key && Number.isInteger(v.attachment)
    ? { itemKey: v.key, attachmentId: v.attachment, title: typeof v.title === "string" ? v.title : "" }
    : null;
}

/** 戻す PDF タブの item を引き、添付 id がその item の添付にあるか確かめる（id は削除後に再利用されうる・item が消えていることも
    ある = MCP / CLI）。あれば今の題名で返し、無ければ viewer = null（一覧のタブとして戻す）。引けない（IPC の失敗）は確かめずに
    そのまま戻して err を返す（保存済みの PDF タブを消さない。添付が本当に無ければビューアの読込のエラーが見える） */
async function checkViewer(v: TabViewer): Promise<{ viewer: TabViewer | null; err: string | null }> {
  let d: ItemDetail | null;
  try {
    d = await api.getItem(v.itemKey);
  } catch (e) {
    return { viewer: v, err: String(e) };
  }
  if (!d || !d.attachments.some((a) => a.id === v.attachmentId)) return { viewer: null, err: null };
  const title = typeof d.csl?.title === "string" && d.csl.title ? d.csl.title : v.title;
  return { viewer: { ...v, title }, err: null };
}

/** 起動時: ui_tabs からタブを戻す（refreshSidebar の後 = フォルダ / ラベルの実在を確かめてから。一覧は呼び出し側が取る）。
    無いフォルダ・ラベルのタブは捨て（前面だったら手前のタブを前面に）、0 枚なら既定の「すべて」1 枚のまま。
    PDF タブは item と添付を確かめて戻し（下の一覧のフォルダ・ラベルが消えていたら一覧は「すべて」・その item をチェックして
    詳細に出す = openViewer と同じ）、item や添付が無い・値が壊れていれば一覧のタブとして戻して（一覧のビューも無ければ捨てて）
    知らせる。確かめる IPC が失敗したらそのまま戻して知らせる。同じ PDF のタブが 2 枚あれば後の方は捨てる。
    読めなければ何も戻さず、保存もしない（tabsRestored を立てない = 保存済みを既定で上書きしない） */
export async function restoreTabs() {
  // Settings › 起動時に前回のタブを開く（ui_restore_tabs = "0" で ②次回は初期状態）。保存は続ける = ①に戻せば直前の状態が戻る。
  // 読めなければ従来どおり復元へ
  try {
    if ((await api.getUiPref("ui_restore_tabs")) === "0") {
      app.tabsRestored = true; // 既定の「すべて」1 枚のまま。以後の変化は保存する
      return;
    }
  } catch {
    /* ui_tabs の読込と同じ扱いに任せる */
  }
  let raw: string | null;
  try {
    raw = await api.getUiPref("ui_tabs");
  } catch (e) {
    toast(t("✗ タブの読込に失敗: ") + String(e).slice(0, 160));
    return;
  }
  let saved: any = null;
  try {
    saved = raw ? JSON.parse(raw) : null;
  } catch {
    saved = null; // 壊れた値は捨てる（次の保存で上書き）
  }
  const list: any[] = Array.isArray(saved?.tabs) ? saved.tabs : [];
  const activeAt = Number.isInteger(saved?.active) ? (saved.active as number) : 0;
  const found: { at: number; tab: Tab; viewer: TabViewer | null; orphan: boolean }[] = [];
  let lost = 0; // 開けない PDF タブ（値が壊れている・item / 添付が無い）
  list.forEach((s, at) => {
    const view = parseView(s?.view);
    const viewer = parseViewer(s?.viewer);
    if (s?.viewer != null && !viewer) lost++;
    if (!view && !viewer) return;
    const tab = blankTab(view ?? { kind: "all" }, typeof s.search === "string" ? s.search : "", parseFilters(s.filters));
    found.push({ at, tab, viewer, orphan: !view });
  });
  const checked = await Promise.all(found.map((f) => (f.viewer ? checkViewer(f.viewer) : null)));
  const tabs: Tab[] = [];
  const seen = new Set<string>(); // 確かめた PDF（key + 添付 id）。同じ PDF のタブは 1 枚 = openViewer と同じ（後の方を捨てる）
  let active = 0;
  let failed = 0;
  let err = "";
  found.forEach((f, i) => {
    const c = checked[i];
    if (c?.err) {
      failed++;
      err = c.err;
    }
    const v = c?.viewer ?? null;
    if (f.viewer && !v) lost++;
    if (f.orphan && !v) return;
    if (v) {
      const id = `${v.itemKey}\u0000${v.attachmentId}`;
      if (seen.has(id)) return;
      seen.add(id);
      f.tab.viewer = v;
      f.tab.checked = [v.itemKey];
      f.tab.checkAnchor = v.itemKey;
      f.tab.selected = v.itemKey; // 一覧に無ければ切替の refreshList がチェックと詳細から外す
    }
    if (f.at <= activeAt) active = tabs.length;
    tabs.push(f.tab);
  });
  const notes: string[] = [];
  if (lost) notes.push(`${t("PDF タブ")} ${lost} ${t("枚を開けません（item か PDF が見つからない・保存値が壊れている）")}`);
  if (failed) notes.push(`${t("PDF タブ")} ${failed} ${t("枚を確かめられません（そのまま開きます）")}: ${err.slice(0, 120)}`);
  if (notes.length) toast("✗ " + notes.join(" / "));
  if (tabs.length) {
    app.tabs = tabs;
    const front = app.tabs[active];
    app.tab = front.id;
    restoreFrom(front);
  }
  app.searchReset++; // 検索欄に戻した検索語を写す
  app.tabsRestored = true;
}

// ---------- 複数選択（チェックボックス） ----------

/** チェック済み key を一覧の表示順で返す */
export function checkedKeys(): string[] {
  return app.items.filter((i) => app.checked.has(i.key)).map((i) => i.key);
}

/** 行本体の素クリック: その行だけをチェック（他は解除）+ 詳細表示。その行が詳細に出ていて、チェックもその行だけなら
    （2 回目のクリック）外して詳細を閉じる。☐・⌘・Space でチェックしただけの行（詳細は未表示）は開く。
    蓄積型（クリックするたび増える）は煩わしいので採らない。☐ 自体のクリックは toggleCheck で蓄積 */
export function checkOnly(key: string) {
  if (app.selected === key && app.checked.size === 1) clearChecks();
  else void select(key);
}

/** 1 件のチェックを反転。range=true（shift）なら起点〜対象の範囲をまとめてチェック */
export function toggleCheck(key: string, range = false) {
  if (range && app.checkAnchor) {
    const a = app.items.findIndex((i) => i.key === app.checkAnchor);
    const b = app.items.findIndex((i) => i.key === key);
    if (a >= 0 && b >= 0) {
      const [s, e] = a < b ? [a, b] : [b, a];
      for (let i = s; i <= e; i++) app.checked.add(app.items[i].key);
      return;
    }
  }
  if (app.checked.has(key)) app.checked.delete(key);
  else app.checked.add(key);
  app.checkAnchor = key;
  closeDetailIfUnchecked();
}

export function checkAll() {
  for (const i of app.items) app.checked.add(i.key);
}

export function clearChecks() {
  app.checked.clear();
  app.checkAnchor = null;
  closeDetailIfUnchecked();
}

/** チェック全件に逐次適用。失敗は数えて続行（1 件の失敗で全体を止めない） */
async function forChecked(fn: (key: string) => Promise<unknown>): Promise<{ ok: number; fail: number; keys: string[] }> {
  return forKeys(checkedKeys(), fn);
}

/** 指定 key 群に逐次適用（forChecked の実体。詳細パネル起点の 1 件操作とも共用） */
async function forKeys(keys: string[], fn: (key: string) => Promise<unknown>): Promise<{ ok: number; fail: number; keys: string[] }> {
  let ok = 0;
  let fail = 0;
  busyBegin();
  try {
    for (const k of keys) {
      try {
        await fn(k);
        ok++;
      } catch (e) {
        fail++;
        console.error("bulk op failed", k, e);
      }
    }
  } finally {
    busyEnd();
  }
  return { ok, fail, keys };
}

function bulkToast(verb: string, r: { ok: number; fail: number }) {
  toast(r.fail ? `${verb}: ${r.ok} ${t("件成功・")}${r.fail} ${t("件失敗")}` : `✓ ${tn(r.ok, "件を")}${verb}`);
}

/** 詳細パネルの item が keys（操作した item）に含まれていたら詳細も更新 */
async function reloadDetailIfAmong(keys: string[]) {
  if (app.selected && keys.includes(app.selected)) await reloadDetail();
}

/** keys（既定 = 今のチェック）をフォルダへ追加 */
export async function bulkAddToCollection(collectionId: number, keys: string[] = checkedKeys()) {
  const name = app.collections.find((c) => c.id === collectionId)?.name ?? t("フォルダ::fallback");
  const r = await forKeys(keys, (k) => api.addToCollection(k, collectionId));
  bulkToast(`${t("「::added-to")}${name}${t("」へ追加")}`, r);
  await Promise.all([refreshSidebar(), reloadDetailIfAmong(r.keys)]);
  if (app.view.kind === "collection") await refreshList();
}

export async function bulkRemoveFromCollection(collectionId: number) {
  const name = app.collections.find((c) => c.id === collectionId)?.name ?? t("フォルダ::fallback");
  const r = await forChecked((k) => api.removeFromCollection(k, collectionId));
  bulkToast(`${t("「::removed-from")}${name}${t("」から外す")}`, r);
  await Promise.all([refreshSidebar(), reloadDetailIfAmong(r.keys)]);
  if (app.view.kind === "collection") await refreshList();
}

/** ツールバーの フォルダ ▾「＋「q」を作成して追加」。追加するのは押したときのチェック（作成を待つ間にタブが切り替わっても、
    切替先のチェックを入れない） */
export async function createCollectionAndAdd(name: string) {
  if (!name.trim()) return;
  const keys = checkedKeys();
  try {
    const id = await api.createCollection(name.trim());
    await refreshSidebar();
    await bulkAddToCollection(id, keys);
  } catch (e) {
    toast(`✗ ${String(e).slice(0, 200)}`);
  }
}

/** ツールバーの「ラベル ▾」の Apply changes（Paperpile 準拠: ピッカーで選んだ変更をまとめて反映）。
    add = チェック全件に付ける / remove = チェック全件から外す */
export async function applyLabelChanges(add: number[], remove: number[]) {
  const keys = checkedKeys();
  if (keys.length === 0 || (add.length === 0 && remove.length === 0)) return;
  busyBegin();
  let fail = 0;
  try {
    for (const k of keys) {
      for (const id of add) await api.attachTag(k, id).catch(() => fail++);
      for (const id of remove) await api.detachTag(k, id).catch(() => fail++);
    }
    const nm = (id: number) => `${t("「")}${app.tags.find((t) => t.id === id)?.name ?? t("ラベル::fallback")}${t("」")}`;
    const parts = [...add.map((id) => `+${nm(id)}`), ...remove.map((id) => `−${nm(id)}`)];
    toast(`${fail ? "⚠" : "✓"} ${tn(keys.length, "件のラベルを更新")}: ${parts.join(" ")}${fail ? `${t("（失敗")} ${fail}${t("）")}` : ""}`);
  } finally {
    busyEnd();
  }
  await Promise.all([refreshSidebar(), refreshList(), reloadDetailIfAmong(keys)]);
}

export async function bulkTrash() {
  const r = await forChecked((k) => api.trashItem(k));
  bulkToast(t("ゴミ箱へ移動（復元可）"), r);
  clearChecks();
  await refreshAll();
}

export async function bulkRestore() {
  const r = await forChecked((k) => api.restoreItem(k));
  bulkToast(t("復元"), r);
  clearChecks();
  await refreshAll();
}

// ---------- 完全削除（ゴミ箱のみ。Paperpile の Delete permanently） ----------

/** 確認モーダルを開く。keys 空なら何もしない */
export function askPurge(keys: string[]) {
  if (keys.length === 0) return;
  app.purgeKeys = keys;
  app.modal = "purge";
  app.listMenu = null;
}

/** チェック全件を完全削除する前の確認 */
export function askPurgeChecked() {
  askPurge(checkedKeys());
}

/** 詳細パネルの 1 件を完全削除する前の確認 */
export function askPurgeSelected() {
  if (app.detail) askPurge([app.detail.key]);
}

/** 確認済み: purgeKeys を完全削除。ファイルの扱い（managed 削除 / linked 残置）は core の報告を toast に出す */
export async function confirmPurge() {
  const keys = app.purgeKeys.slice();
  app.modal = null;
  app.purgeKeys = [];
  if (keys.length === 0) return;
  let removed = 0;
  let kept = 0;
  let failed = 0;
  const gone = new Set<string>();
  const r = await forKeys(keys, async (k) => {
    const p = await api.purgeItem(k);
    gone.add(k);
    removed += p.filesRemoved;
    kept += p.filesKept;
    failed += p.filesFailed;
  });
  // 消えた item の PDF を開いていたタブはビューアを閉じる（タブは一覧のタブとして残す。ゴミ箱へ移すだけなら読める）
  for (const t of app.tabs) if (t.viewer && gone.has(t.viewer.itemKey)) t.viewer = null;
  const files: string[] = [];
  // 英語は「2 PDFs deleted」の語順（先頭の「PDF」「リンク済み PDF」は訳が空 = trim で詰める）
  if (removed) files.push(`${t("PDF::count")} ${tn(removed, "件を削除")}`.trim());
  if (kept) files.push(`${t("リンク済み PDF")} ${tn(kept, "件は残置")}`.trim());
  if (failed) files.push(`${t("PDF::count")} ${tn(failed, "件の削除に失敗")}`.trim());
  const suffix = files.length ? `${t("（")}${files.join(t("・"))}${t("）")}` : "";
  toast(
    r.fail
      ? `${t("完全削除")}: ${r.ok} ${t("件成功・")}${r.fail} ${t("件失敗")}${suffix}`
      : `✓ ${tn(r.ok, "件を完全に削除しました")}${suffix}`
  );
  clearChecks(); // 詳細も閉じる（チェックが空になるので closeDetailIfUnchecked が閉じる）
  await refreshAll();
}

export function cancelPurge() {
  app.modal = null;
  app.purgeKeys = [];
}

// ---------- メモ（Paperpile の note。行の下にインライン編集） ----------

/** 指定 item のメモ編集を開く。user 著のメモがあれば前置きして編集、無ければ新規（空）。
    MCP 経由で Claude が残したメモ（author≠user）は編集対象にしない（詳細パネルに著者付きで表示するだけ） */
let noteEditorRequest = 0;
export async function openNoteEditor(key: string, noteKey?: string | null) {
  const request = ++noteEditorRequest;
  app.listMenu = null;
  if (app.noteEditor?.key === key && (noteKey === undefined || app.noteEditor.noteKey === noteKey)) return;
  const tab = app.tab;
  // 別の item のメモ欄が開いていたら、先に自動保存して閉じる（行のクリック・ビューの変更・タブの切替と同じ。行のメモバッジと
  // 詳細の「メモを編集…」は行のクリックを通らないので、ここで保存しないと未保存の追記を黙って捨てる）。保存に失敗して開いたままなら開かない
  if (app.noteEditor) {
    await autoSaveAndCloseNote();
    if (app.noteEditor || app.tab !== tab || request !== noteEditorRequest) return;
  }
  try {
    const notes = await api.listNotes(key);
    // 読む間に別のタブへ切り替わった（前のタブの item のメモ欄を切替先で開かない）・別のメモ欄が開いて書き始めていたら置き換えない
    if (app.tab !== tab || noteDirty() || request !== noteEditorRequest) return;
    const mine = noteKey === null ? null : notes.find((n) => n.author === "user" && (noteKey === undefined || n.key === noteKey)) ?? null;
    if (noteKey && !mine) throw new Error(t("指定したメモが見つかりません"));
    app.noteEditor = { key, noteKey: mine?.key ?? null, text: mine?.contentMd ?? "", saved: mine?.contentMd ?? "" };
  } catch (e) {
    toast(`✗ ${t("メモの読込に失敗")}: ${String(e).slice(0, 200)}`);
  }
}

export function noteDirty(): boolean {
  const n = app.noteEditor;
  return !!n && n.text !== n.saved;
}

/** 保存中の saveNote。保存の途中にもう一度呼ばれたら（タブの切替・ビューの変更・行のクリックの自動保存が重なる）、
    終わるのを待ってから続ける（新規のメモを 2 件 add しない）。対象は呼ばれたときのメモ欄で、待つ間に閉じた・別のメモ欄に
    置き換わったなら何もしない（打ち始めたばかりの別の欄を保存して閉じない） */
let noteSaving: Promise<void> | null = null;

/** 保存: 本文あり → update か add / 本文を空にした → 既存メモを削除（Paperpile と同じ「空 = 消す」） */
export async function saveNote(close: boolean) {
  const target = app.noteEditor;
  const waited = !!noteSaving;
  while (noteSaving) await noteSaving.catch(() => {});
  if (!target || app.noteEditor !== target) return;
  // 先の保存が今の本文まで保存し終えていれば、同じ本文をもう一度保存しない（閉じるだけ）
  if (waited && !noteDirty()) {
    if (close) app.noteEditor = null;
    return;
  }
  const p = saveNoteNow(close);
  noteSaving = p;
  try {
    await p;
  } finally {
    if (noteSaving === p) noteSaving = null;
  }
}

async function saveNoteNow(close: boolean) {
  const n = app.noteEditor;
  if (!n) return;
  const text = n.text;
  const trimmed = text.trim();
  busyBegin();
  try {
    if (trimmed.length === 0) {
      if (n.noteKey) {
        await api.deleteNote(n.noteKey);
        n.noteKey = null;
        toast(t("メモを削除しました"));
      }
      n.saved = "";
      n.text = "";
    } else if (n.noteKey) {
      await api.updateNote(n.noteKey, text);
      n.saved = text;
      toast(t("✓ メモを保存しました"));
    } else {
      n.noteKey = await api.addNote(n.key, text);
      n.saved = text;
      toast(t("✓ メモを保存しました"));
    }
  } catch (e) {
    toast(`✗ ${t("メモの保存に失敗")}: ${String(e).slice(0, 200)}`);
    return;
  } finally {
    busyEnd();
  }
  if (close && app.noteEditor === n) app.noteEditor = null; // 保存の間に開き直した別のメモ欄は閉じない
  await Promise.all([refreshList(), reloadDetailIfAmong([n.key])]);
}

/** 別の行をクリック / ビュー切替のとき: 未保存なら保存して閉じ、未編集ならただ閉じる（自動保存） */
export async function autoSaveAndCloseNote() {
  if (!app.noteEditor) return;
  if (noteDirty()) await saveNote(true);
  else app.noteEditor = null;
}

/** 閉じる。未保存の編集があるときは force でなければ閉じない（Esc の誤爆で失わせない） */
export function closeNoteEditor(force = false) {
  if (!app.noteEditor) return;
  if (noteDirty() && !force) return;
  app.noteEditor = null;
}

export async function bulkStar(starred: boolean) {
  const r = await forChecked((k) => api.setStar(k, starred));
  bulkToast(starred ? t("★ Star") : t("★ 解除"), r);
  await Promise.all([refreshList(), refreshSidebar(), reloadDetailIfAmong(r.keys)]);
}

/** 引用メニューのコピー種別（Paperpile の Cite ▾: Citation / BibTeX / Citation key / LaTeX citation / DOI） */
export type CopyKind = "citation" | "bibtex" | "citekey" | "latex" | "doi";

/** チェック全件（一覧順）を kind の形式でクリップボードへ（一覧ツールバーの 引用 ▾）。citation は既定スタイル・言語
    （番号付きスタイルは一覧の順で採番） */
export async function bulkCopy(kind: CopyKind) {
  const keys = checkedKeys();
  if (keys.length === 0) return;
  busyBegin();
  try {
    let text = "";
    let note = "";
    let n = 0; // 実際にコピーした件数（見つからない item・DOI なしは数えない）
    if (kind === "citation") {
      const items: { key: string; csl: any }[] = [];
      for (const k of keys) {
        const d = await api.getItem(k);
        if (d) items.push({ key: k, csl: d.csl });
      }
      text = await formatBibliographyMany(items, app.cite.style, app.cite.locale, app.cite.doiAlways);
      note = styleTitle(app.cite.style);
      n = items.length;
    } else {
      const es = await api.bibtexEntries(keys);
      n = es.length;
      if (kind === "bibtex") text = es.map((e) => e.bibtex.trim()).join("\n\n");
      else if (kind === "citekey") text = es.map((e) => e.citekey).join(", ");
      else if (kind === "latex") text = es.length ? `\\cite{${es.map((e) => e.citekey).join(",")}}` : "";
      else {
        const dois = es.filter((e) => e.doi);
        text = dois.map((e) => e.doi).join("\n");
        n = dois.length;
        if (dois.length < es.length) note = tn(es.length - dois.length, "件は DOI なし");
      }
    }
    if (!text) {
      toast(kind === "doi" ? t("DOI のある item がありません") : t("引用を生成できませんでした"));
      return;
    }
    await navigator.clipboard.writeText(text);
    const what = { citation: t("引用::copied"), bibtex: "BibTeX", citekey: t("引用キー::copied"), latex: t("LaTeX 引用::copied"), doi: "DOI" }[kind];
    toast(`✓ ${tn(n, "件の")}${what}${t("をコピー")}${note ? `${t("（")}${note}${t("）")}` : ""}`);
  } catch (e) {
    toast("✗ " + String(e).slice(0, 200));
  } finally {
    busyEnd();
  }
}

export async function doSearch(q: string) {
  app.search = q;
  await refreshList();
}

/** given → 頭文字列（"Shiro" → "S"、"Jean-Pierre" → "JP"、"S." → "S"） */
function initialsOf(given: string | null | undefined): string {
  if (!given) return "";
  return given
    .split(/[\s\-.]+/)
    .filter(Boolean)
    .map((p) => p[0].toUpperCase())
    .join("");
}

/** 行の筆頭著者クリック → 著者チップを置いて絞り込む（同じ著者なら何もしない） */
export async function filterByAuthor(family: string, given: string | null) {
  if (!family || family === "?") return;
  const ini = initialsOf(given);
  const label = ini ? `${family} ${ini}` : family;
  if (app.filters.author?.label === label) return;
  app.filters.author = { family, initial: ini ? ini[0] : null, label };
  await refreshList();
}

/** 行の誌名クリック → 誌名チップを置いて絞り込む */
export async function filterByJournal(name: string) {
  const n = name.trim();
  if (!n || app.filters.journal === n) return;
  app.filters.journal = n;
  await refreshList();
}

/** チップの × */
export async function clearFilter(kind: "author" | "journal") {
  if (kind === "author") app.filters.author = null;
  else app.filters.journal = null;
  await refreshList();
}

export async function quickAdd(input: string) {
  if (!input.trim()) return;
  busyBegin();
  try {
    const item = await api.quickAdd(input.trim());
    toast(`✓ ${t("登録")}: ${item.firstAuthor} (${item.year ?? "n.d."}) ${item.title.slice(0, 60)}`);
    await refreshAll();
    await reveal(item.key);
  } catch (e) {
    toast(`✗ ${String(e).slice(0, 300)}`);
  } finally {
    busyEnd();
  }
}

export async function handleDrop(paths: string[]) {
  const pdfs = paths.filter((p) => p.toLowerCase().endsWith(".pdf"));
  if (pdfs.length === 0) {
    toast(t("PDF ファイルをドロップしてください"));
    return;
  }
  await startImport(pdfs);
}

// ---------- PDF 一括取込（ドロップ / ＋新規メニュー） ----------

/** PDF ファイルの選択ダイアログ（複数選択）から取込を開始 */
export async function openImportDialog() {
  const p = await openDialog({ multiple: true, filters: [{ name: "PDF", extensions: ["pdf"] }] });
  const paths = Array.isArray(p) ? p : typeof p === "string" ? [p] : [];
  if (paths.length) await startImport(paths);
}

/** 取込を開始（実行中なら末尾に追加してループに拾わせる） */
export async function startImport(paths: string[]) {
  const pdfs = paths.filter((p) => p.toLowerCase().endsWith(".pdf"));
  if (pdfs.length === 0) {
    toast(t("PDF ファイルがありません"));
    return;
  }
  const files: ImportFile[] = pdfs.map((p) => ({
    path: p,
    name: p.split("/").pop() ?? p,
    status: "pending",
    message: "",
    key: null,
  }));
  if (app.importJob?.running) {
    app.importJob.files.push(...files);
    return;
  }
  app.importJob = { files, running: false, cancel: false };
  await runImport();
}

async function runImport() {
  const job = app.importJob;
  if (!job || job.running) return;
  job.running = true;
  job.cancel = false;
  busyBegin();
  try {
    // 1 件ずつ await して直列に回す（core は抽出・照会の間 DB ロックを離すので並列にもできるが、Crossref の匿名枠は
    // 同時 1 本・1 req/s = 並べても core の門で待つだけ。進捗の順序もファイル順のまま）。中止はファイル境界で効く。
    // NCBI 3 req/s・Crossref の間隔は core（resolver/pubmed.rs・crossref.rs）が要求ごとに守る
    for (let i = 0; i < job.files.length; i++) {
      const f = job.files[i];
      if (f.status !== "pending") continue;
      if (job.cancel) {
        f.status = "canceled";
        continue;
      }
      f.status = "running";
      try {
        const r = await api.importPdf(f.path);
        f.status = r.status;
        f.message = r.message;
        f.key = r.key;
      } catch (e) {
        f.status = "error";
        f.message = String(e).slice(0, 300);
      }
    }
  } finally {
    job.running = false;
    busyEnd();
  }
  await refreshAll();
}

export function cancelImport() {
  if (app.importJob) app.importJob.cancel = true;
}

/** 失敗・中止分を再キューして再実行 */
export async function retryImportFailed() {
  const job = app.importJob;
  if (!job || job.running) return;
  for (const f of job.files) {
    if (f.status === "error" || f.status === "canceled") {
      f.status = "pending";
      f.message = "";
    }
  }
  await runImport();
}

export function closeImport() {
  if (app.importJob?.running) return;
  app.importJob = null;
}

// ---------- detail 操作 ----------

/** 詳細の banner の ↩ 復元。復元した item はゴミ箱の一覧から消えるので、詳細も閉じる（refreshList。ツールバーの 復元 と同じ） */
export async function restoreSelected() {
  const d = app.detail;
  if (!d) return;
  try {
    await api.restoreItem(d.key);
  } catch (e) {
    toast(`✗ ${t("復元できません")}: ${String(e).slice(0, 200)}`);
    return;
  }
  toast(t("復元しました"));
  await refreshAll();
}

// ---------- 参照エディタ / viewer ----------

export function openEditorNew() {
  app.editorKey = null;
  app.editorCsl = null;
  app.editorOpen = true;
}

export function openEditorFor(key: string, csl: any) {
  app.editorKey = key;
  app.editorCsl = csl;
  app.editorOpen = true;
}

export function closeEditor() {
  app.editorOpen = false;
  app.editorKey = null;
  app.editorCsl = null;
}

/** 「PDF を読む」= 新しいタブ（前面）で開く（ブラウザのタブと同じ）。同じ PDF のタブがあればそれを前面にする。
    下に持つ一覧の状態は前面のタブの写し（ビュー・検索語・チップ・一覧・スクロール）で、その item だけをチェックして詳細に出す
    （詳細に出る item は常にチェック中という不変条件。最後の 1 枚でビューアを閉じるとこの一覧が見える）。閉じる = closeTab */
export async function openViewer(itemKey: string, attachmentId: number, title: string) {
  if (!tabOpsAllowed()) return;
  const same = (t: Tab) => t.viewer?.itemKey === itemKey && t.viewer.attachmentId === attachmentId;
  const open = app.tabs.find(same);
  if (open) {
    await activateTab(open.id);
    return;
  }
  // 開いているメモは先に保存して閉じる（openTab と同じ = 失敗したらタブを作らない）。待つ間に別のタブへ切り替わった
  // （下に写す一覧が別のタブのものになる）・同じ PDF のタブが開かれたなら作らない
  const tab = app.tab;
  await autoSaveAndCloseNote();
  if (app.noteEditor || app.tab !== tab || !tabOpsAllowed() || app.tabs.some(same)) return;
  const t = blankTab($state.snapshot(app.view) as View, app.search, $state.snapshot(app.filters) as Filters);
  t.items = $state.snapshot(app.items) as ItemSummary[];
  t.scrollTop = rowsEl()?.scrollTop ?? 0;
  if (t.items.some((i) => i.key === itemKey)) {
    t.checked = [itemKey];
    t.checkAnchor = itemKey;
    t.selected = itemKey;
    t.detail = app.detail?.key === itemKey ? ($state.snapshot(app.detail) as ItemDetail) : null;
  }
  t.viewer = { itemKey, attachmentId, title };
  app.tabs.push(t);
  await switchTab(t.id, false);
}

// ---------- 引用スタイル ----------

/** 表示名（導入済み一覧から。未知の id はそのまま） */
export function styleTitle(id: string): string {
  return app.cite.installed.find((s) => s.id === id)?.title ?? id;
}

/** 起動時: 既定スタイル・言語・導入済みを読む（失敗しても同梱 Vancouver で動く） */
export async function loadCitePrefs() {
  try {
    const [p, installed] = await Promise.all([api.citePrefs(), api.citeInstalled()]);
    app.cite.style = p.style;
    app.cite.locale = p.locale;
    app.cite.doiAlways = p.doi_always;
    app.cite.installed = installed;
  } catch (e) {
    toast(t("✗ 引用スタイル設定の読込に失敗: ") + String(e).slice(0, 160));
  }
}

/** 既定スタイル・言語・DOI / URL を保存（未導入ならここで取得・導入） */
export async function saveCitePrefs(style: string, locale: string, doiAlways: boolean) {
  const p = await api.citeSetPrefs(style, locale, doiAlways);
  app.cite.style = p.style;
  app.cite.locale = p.locale;
  app.cite.doiAlways = p.doi_always;
  app.cite.installed = await api.citeInstalled();
  toast(
    t("✓ 引用スタイル: ") + styleTitle(p.style) + (p.locale ? `${t("（")}${p.locale}${t("）")}` : "") + (p.doi_always ? t(" · DOI / URL を常に含める") : "")
  );
}

export function openStyleModal() {
  app.listMenu = null;
  app.modal = "citeStyle";
}

// ---------- フォルダ（Paperpile の folders。MY LIBRARY） ----------

/** core の COLLATE NOCASE と同じ畳み方（ASCII だけ大小無視）。兄弟の同名判定と名前順に使う */
const nocase = (s: string) => s.replace(/[A-Z]/g, (c) => c.toLowerCase());
function byFolderName(a: CollectionRow, b: CollectionRow): number {
  const x = nocase(a.name);
  const y = nocase(b.name);
  return x < y ? -1 : x > y ? 1 : a.id - b.id;
}

export interface FolderRow {
  c: CollectionRow;
  depth: number;
}

/** フォルダの木を前順に平らにする（サイドバー・ツールバーのピッカー・Manage folders の表で共用）。
    兄弟の並び = sort（alpha: 名前・大小無視 = core の既定順 / count: 件数の多い順・同数は名前）、reverse = 兄弟の並びを逆に
    （Manage の ↕）、respectCollapse = 折り畳んだフォルダの子孫を出さない（サイドバー）。親が見えないフォルダは root に出す */
export function flatFolders(
  opts: { sort?: "alpha" | "count"; reverse?: boolean; respectCollapse?: boolean } = {}
): FolderRow[] {
  const sort = opts.sort ?? app.folderSort;
  const ids = new Set(app.collections.map((c) => c.id));
  const kids = new Map<number | null, CollectionRow[]>();
  for (const c of app.collections) {
    const p = c.parentId !== null && ids.has(c.parentId) ? c.parentId : null;
    const arr = kids.get(p);
    if (arr) arr.push(c);
    else kids.set(p, [c]);
  }
  const cmp = (a: CollectionRow, b: CollectionRow) => {
    const r = sort === "count" ? b.nItems - a.nItems || byFolderName(a, b) : byFolderName(a, b);
    return opts.reverse ? -r : r;
  };
  const out: FolderRow[] = [];
  const walk = (parent: number | null, depth: number) => {
    for (const c of (kids.get(parent) ?? []).sort(cmp)) {
      out.push({ c, depth });
      if (!(opts.respectCollapse && app.foldersCollapsed.has(c.key))) walk(c.id, depth + 1);
    }
  };
  walk(null, 0);
  return out;
}

/** 子孫（子・孫…）の id。自分は含まない */
export function descendants(id: number): number[] {
  const out: number[] = [];
  const seen = new Set([id]);
  const stack = [id];
  while (stack.length) {
    const p = stack.pop()!;
    for (const c of app.collections) {
      if (c.parentId === p && !seen.has(c.id)) {
        seen.add(c.id);
        out.push(c.id);
        stack.push(c.id);
      }
    }
  }
  return out;
}

/** 祖先の id（親・祖父…の順） */
function ancestors(id: number): number[] {
  const out: number[] = [];
  const seen = new Set([id]);
  let cur = app.collections.find((c) => c.id === id)?.parentId ?? null;
  while (cur !== null && !seen.has(cur)) {
    seen.add(cur);
    out.push(cur);
    cur = app.collections.find((c) => c.id === cur)?.parentId ?? null;
  }
  return out;
}

/** 祖先の名前を root から並べたもの（"親 › 子"。root なら ""）。同名のフォルダを見分ける表示に使う */
export function folderPath(id: number): string {
  return ancestors(id)
    .reverse()
    .map((a) => app.collections.find((c) => c.id === a)?.name ?? "?")
    .join(" › ");
}

/** フォルダ名を確定できない理由（確定できるなら null・空なら "" = 理由は出さずにボタンだけ無効）。
    core と同じ規則: `/` は不可・同じ親の下で大小無視一意（別の親の下の同名は可）。except = 改名時の自分 */
export function folderNameError(name: string, parentId: number | null, except?: number): string | null {
  const n = name.trim();
  if (!n) return "";
  if (n.includes("/")) return t("「/」は使えません（フォルダの区切りに使うため）");
  if (app.collections.some((c) => c.id !== except && c.parentId === parentId && nocase(c.name) === nocase(n)))
    return `${t("同じ階層に「")}${n}${t("」が既にあります")}`;
  return null;
}

function saveUiPref(key: string, value: string) {
  api.setUiPref(key, value).catch((e) => toast(`✗ ${t("表示設定を保存できません")}: ${String(e).slice(0, 160)}`));
}

function saveCollapsed() {
  saveUiPref("ui_folders_collapsed", JSON.stringify([...app.foldersCollapsed]));
}

/** 起動時: フォルダの並び順と折畳を読む（App.svelte の onMount・loadCitePrefs と同じ場所） */
export async function loadFolderPrefs() {
  try {
    const [sort, collapsed] = await Promise.all([api.getUiPref("ui_folder_sort"), api.getUiPref("ui_folders_collapsed")]);
    if (sort === "alpha" || sort === "count") app.folderSort = sort;
    if (collapsed) {
      const keys: unknown = JSON.parse(collapsed);
      if (Array.isArray(keys)) for (const k of keys) if (typeof k === "string") app.foldersCollapsed.add(k);
    }
  } catch (e) {
    toast(t("✗ フォルダの表示設定の読込に失敗: ") + String(e).slice(0, 160));
  }
}

/** 起動時: 表記の言語（Settings › Language = ui_lang）。main.ts が mount の前に待つ。切替は Rust の emit "ui-lang" を App.svelte が受けて setLang */
export async function loadLangPref() {
  try {
    setLang(await api.getUiPref("ui_lang"));
  } catch (e) {
    toast(t("✗ 言語の設定の読込に失敗: ") + String(e).slice(0, 160));
  }
}

/** 起動時の新版確認（Settings › 起動時に新版を確認・core update.rs）。新しい版があれば App がヘッダー下の帯に出す。
    失敗は console だけ（起動直後に toast で邪魔しない。取りに行けなかった理由は Rust が stderr に出す） */
export async function checkUpdate() {
  try {
    app.updateLatest = await api.checkUpdate();
  } catch (e) {
    console.warn("check_update failed:", e);
  }
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

/** id とその祖先を展開する（作ったサブフォルダを見せる） */
function expandFolder(id: number | null) {
  if (id === null) return;
  let changed = false;
  for (const f of [id, ...ancestors(id)]) {
    const key = app.collections.find((c) => c.id === f)?.key;
    if (key && app.foldersCollapsed.delete(key)) changed = true;
  }
  if (changed) saveCollapsed();
}

/** フォルダのモーダルを開く（見出し・行のメニューを閉じてから）。id = Rename / Delete の対象、parentId = New folder の親 */
export function openFolderModal(
  modal: "newFolder" | "renameFolder" | "deleteFolder" | "manageFolders",
  target: { id?: number; parentId?: number | null } = {}
) {
  app.labelMenu = false;
  app.folderMenu = null;
  app.folderTarget = { id: target.id ?? null, parentId: target.parentId ?? null };
  app.modal = modal;
}

/** New folder / Create subfolder。作ったら親を展開して知らせる（ビューは動かさない = Paperpile）。戻り値 = id（失敗は toast して null） */
export async function createFolder(name: string, parentId: number | null): Promise<number | null> {
  const n = name.trim();
  if (!n) return null;
  try {
    const id = await api.createCollectionIn(n, parentId);
    expandFolder(parentId);
    await refreshSidebar();
    const parent = parentId === null ? undefined : app.collections.find((c) => c.id === parentId);
    toast(parent ? `✓ ${t("「::create-in")}${parent.name}${t("」にフォルダ「")}${n}${t("」を作成::in")}` : `✓ ${t("フォルダ「")}${n}${t("」を作成")}`);
    return id;
  } catch (e) {
    toast(`✗ ${String(e).slice(0, 200)}`);
    return null;
  }
}

export async function renameFolder(id: number, name: string): Promise<boolean> {
  const n = name.trim();
  if (!n) return false;
  try {
    await api.renameCollection(id, n);
    await patchViews((v) => (v.kind === "collection" && v.id === id ? { ...v, name: n } : v));
    await Promise.all([refreshSidebar(), reloadDetail()]); // 詳細のフォルダチップも新しい名前に
    return true;
  } catch (e) {
    toast(`✗ ${String(e).slice(0, 200)}`);
    return false;
  }
}

/** 祖先も選ばれている id を除いたもの（親の削除で一緒に消えるので、先に子を消すと数え方が崩れる）= 実際に削除を呼ぶ根 */
export function folderDeleteRoots(ids: number[]): number[] {
  const picked = new Set(ids);
  return ids.filter((id) => app.collections.some((c) => c.id === id) && !ancestors(id).some((a) => picked.has(a)));
}

/** フォルダをサブフォルダごと削除（即時・ゴミ箱なし・論文は残る）。削除の後に一覧を取り直し、開いていたフォルダ
    （子孫・別の経路で先に消えていたものも）が無くなっていたタブは「すべて」へ移る（子フォルダを開いたまま親を消しても
    空の一覧に残らない。不活性のタブも patchViews で同じ）。取り直しに失敗したときは、消したはずの木（削除の前に集めた
    子孫）で判定する。結果は 1 本の toast（失敗は最初の理由つき） */
export async function deleteFolders(ids: number[]) {
  const roots = folderDeleteRoots(ids);
  if (roots.length === 0) return;
  const names = new Map(roots.map((id) => [id, app.collections.find((c) => c.id === id)?.name ?? t("フォルダ::fallback")]));
  const gone = new Set<number>();
  let removed = 0;
  let ok = 0;
  let firstErr = "";
  for (const id of roots) {
    const subtree = [id, ...descendants(id)];
    try {
      removed += await api.deleteCollection(id);
      ok++;
      for (const f of subtree) gone.add(f);
    } catch (e) {
      firstErr ||= `${t("「")}${names.get(id)}${t("」")}: ${String(e).slice(0, 160)}`;
      console.error("delete folder failed", id, e);
    }
  }
  const fail = roots.length - ok;
  let msg: string;
  if (fail) msg = `${t("⚠ フォルダ")} ${removed} ${t("個を削除・")}${fail} ${t("個は失敗")} — ${firstErr}`;
  else if (removed === 1) msg = `✓ ${t("フォルダ「")}${names.get(roots[0])}${t("」を削除（論文はそのまま）")}`;
  else if (ok === 1) msg = `✓ ${t("フォルダ「")}${names.get(roots[0])}${t("」とサブフォルダ")} ${removed - 1} ${t("個を削除（論文はそのまま）")}`;
  else msg = `${t("✓ フォルダ")} ${removed} ${t("個を削除（サブフォルダを含む・論文はそのまま）")}`;
  let reloaded = false;
  try {
    await refreshSidebar();
    reloaded = true;
    const live = new Set(app.collections.map((c) => c.key));
    let pruned = false;
    for (const k of [...app.foldersCollapsed]) if (!live.has(k) && app.foldersCollapsed.delete(k)) pruned = true;
    if (pruned) saveCollapsed();
  } catch (e) {
    msg += ` — ✗ ${t("一覧の再読込に失敗")}: ${String(e).slice(0, 160)}`;
  }
  toast(msg);
  const isGone = (id: number) => (reloaded ? !app.collections.some((c) => c.id === id) : gone.has(id));
  try {
    await patchViews((v) => (v.kind === "collection" && isGone(v.id) ? null : v));
    await reloadDetail(); // 詳細のフォルダチップから消したフォルダを外す
  } catch (e) {
    toast(`${msg} — ✗ ${t("再読込に失敗")}: ${String(e).slice(0, 160)}`);
  }
}

/** Find and replace（フォルダ名の検索置換）。兄弟に同名・空・「/」を含む結果は飛ばす */
export async function findReplaceFolders(find: string, replace: string) {
  try {
    const out = await api.findReplaceCollections(find, replace, false);
    const done = out.filter((o) => o.status === "renamed" && o.old !== o.new).length;
    const skipped = out.length - done;
    const renamed = new Map(out.filter((o) => o.status === "renamed").map((o) => [o.id, o.new]));
    await patchViews((v) => (v.kind === "collection" && renamed.has(v.id) ? { ...v, name: renamed.get(v.id)! } : v));
    toast(`✓ ${done} ${t("個を改名")}${skipped ? `${t("・")}${skipped} ${t("個は対象外（同じ階層に同名・「/」を含む等）")}` : ""}`);
  } catch (e) {
    toast(`✗ ${String(e).slice(0, 200)}`);
  }
  await Promise.all([refreshSidebar(), reloadDetail()]).catch((e) => toast(`✗ ${t("再読込に失敗")}: ${String(e).slice(0, 200)}`));
}

// ---------- ラベル（Paperpile の LABELS） ----------

/** ラベル名の重複（大小無視。DB の UNIQUE COLLATE NOCASE と同じ判定）。except = 改名時の自分 */
export function labelNameTaken(name: string, except?: number): boolean {
  const n = name.trim().toLowerCase();
  return !!n && app.tags.some((t) => t.id !== except && t.name.toLowerCase() === n);
}

export async function setTagSort(sort: "alpha" | "count") {
  app.tagSort = sort;
  await refreshSidebar();
}

/** 新規ラベル（既定色 = 灰）。戻り値 = id（失敗は toast して null） */
export async function createLabel(name: string, color: string | null = DEFAULT_LABEL_COLOR): Promise<number | null> {
  const n = name.trim();
  if (!n) return null;
  if (labelNameTaken(n)) {
    toast(`${t("同名のラベル「")}${n}${t("」があります")}`);
    return null;
  }
  try {
    const id = await api.createTag(n, color);
    await refreshSidebar();
    toast(`✓ ${t("ラベル「")}${n}${t("」を作成")}`);
    return id;
  } catch (e) {
    toast(`✗ ${String(e).slice(0, 200)}`);
    return null;
  }
}

export async function renameLabel(id: number, name: string): Promise<boolean> {
  try {
    await api.updateTag(id, name.trim(), null);
    await patchViews((v) => (v.kind === "tag" && v.id === id ? { ...v, name: name.trim() } : v));
    await Promise.all([refreshSidebar(), refreshList(), reloadDetail()]);
    return true;
  } catch (e) {
    toast(`✗ ${String(e).slice(0, 200)}`);
    return false;
  }
}

/** 色の変更（1 件でも複数でも） */
export async function recolorLabels(ids: number[], color: string) {
  try {
    for (const id of ids) await api.updateTag(id, null, color);
  } catch (e) {
    toast(`✗ ${String(e).slice(0, 200)}`);
  }
  await Promise.all([refreshSidebar(), refreshList(), reloadDetail()]);
}

/** ラベルの削除（論文は消えない・付与が外れるだけ） */
export async function deleteLabels(ids: number[]) {
  let fail = 0;
  const done = new Set<number>();
  for (const id of ids) await api.deleteTag(id).then(() => done.add(id), () => fail++);
  let msg = fail ? `⚠ ${ids.length - fail} ${t("個を削除・")}${fail} ${t("個は失敗")}` : `✓ ${ids.length} ${t("個のラベルを削除（論文はそのまま）")}`;
  // 消えたラベルを開いていたタブは「すべて」へ。取り直したラベルの一覧で判定する（削除に失敗して残ったラベルのタブは動かさない・
  // 別の経路で先に消えていたものは移す）。取り直しに失敗したら削除できた id で判定する（deleteFolders と同じ）
  let reloaded = false;
  try {
    await refreshSidebar();
    reloaded = true;
  } catch (e) {
    msg += ` — ✗ ${t("ラベルの再読込に失敗")}: ${String(e).slice(0, 160)}`;
  }
  toast(msg);
  const isGone = (id: number) => (reloaded ? !app.tags.some((t) => t.id === id) : done.has(id));
  if (!(await patchViews((v) => (v.kind === "tag" && ids.includes(v.id) && isGone(v.id) ? null : v)))) await refreshList();
  await reloadDetail();
}

/** Merge: sources を target 1 つにまとめる */
export async function mergeLabels(sources: number[], target: number) {
  const nameOf = (id: number) => app.tags.find((t) => t.id === id)?.name ?? t("ラベル::fallback");
  const name = nameOf(target);
  let merged = false;
  let msg: string;
  try {
    const absorbed = sources.filter((s) => s !== target).map(nameOf);
    const n = await api.mergeTags(sources, target);
    merged = true;
    msg = `✓ ${absorbed.map((a) => `${t("「")}${a}${t("」")}`).join("")}${t("を「")}${name}${t("」に統合（")}${tn(n, "件）")}`;
  } catch (e) {
    msg = `✗ ${String(e).slice(0, 200)}`;
  }
  // 吸収されたラベルを開いていたタブは残る方のラベルへ。サイドバーを取り直してから写す（取り直しの間に、まだ出ている
  // 吸収されたラベルを開いたタブも後で移る = deleteLabels と同じ順序）。写し先は id で決まるので、取り直しに失敗しても写す
  try {
    await refreshSidebar();
  } catch (e) {
    msg += ` — ✗ ${t("ラベルの再読込に失敗")}: ${String(e).slice(0, 160)}`;
  }
  toast(msg);
  const patched =
    merged && (await patchViews((v) => (v.kind === "tag" && sources.includes(v.id) && v.id !== target ? { kind: "tag", id: target, name } : v)));
  if (!patched) await refreshList();
  await reloadDetail();
}

/** Find and replace（ラベル名の検索置換）。同名が既にあるものは飛ばす */
export async function findReplaceLabels(find: string, replace: string) {
  try {
    const out = await api.findReplaceTags(find, replace, false);
    const done = out.filter((o) => o.status === "renamed" && o.old !== o.new).length;
    const skipped = out.length - done;
    const renamed = new Map(out.filter((o) => o.status === "renamed").map((o) => [o.id, o.new]));
    await patchViews((v) => (v.kind === "tag" && renamed.has(v.id) ? { ...v, name: renamed.get(v.id)! } : v));
    toast(`✓ ${done} ${t("個を改名")}${skipped ? `${t("・")}${skipped} ${t("個は対象外（同名あり等）")}` : ""}`);
  } catch (e) {
    toast(`✗ ${String(e).slice(0, 200)}`);
  }
  await Promise.all([refreshSidebar(), refreshList(), reloadDetail()]);
}

/** 行・サイドバーのラベルクリック → そのラベルのビュー（一覧の上の検索欄にチップ） */
export async function filterByLabel(id: number) {
  const t = app.tags.find((t) => t.id === id);
  if (!t) return;
  if (app.view.kind === "tag" && app.view.id === id) return;
  await setView({ kind: "tag", id, name: t.name });
}

// ---------- pointer ベース D&D（行 → サイドバーのラベル/フォルダ・ラベル → 行） ----------

/** 追っている最中のドラッグを落とさずに終える（次の pointerdown で前のが残っていたら先に解く） */
let endDrag: (() => void) | null = null;
let dragConsumed = false;

/** 行・ラベルの pointerdown の後を window で追う。6px 動いたらドラッグを開始し（それまではクリック扱い）、move ごとに幽霊
    （app.drag = ghost + ポインタの位置 + 直下の落とし先のうち kinds の種類のもの）を書く。左ボタンを離したら幽霊を消し、
    ドラッグになっていれば直後の click（選択）を抑止して、落とし先があれば drop(落とし先) を呼ぶ。
    pointerup が来ない経路は落とさずに終える: pointercancel・window の blur（⌘Tab で他のアプリへ）・contextmenu（掴んだまま右クリック
    = OS のメニューが mouseup を飲む）・左ボタンが離れている move（ev.buttons = pointerup の取りこぼし。これが無いと離した後も hover で
    幽霊が付いて回り、次にどこかで離すと最後に重なった落とし先へ落ちる）。前の追跡が残っていれば先に落とさずに終えて解く */
function trackDrag(e: PointerEvent, ghost: { kind: "rows" | "label"; label: string }, kinds: string[], drop: (over: string) => Promise<void>) {
  if (e.button !== 0 || !e.isPrimary) return;
  endDrag?.();
  const sx = e.clientX;
  const sy = e.clientY;
  let started = false;
  dragConsumed = false;
  const move = (ev: PointerEvent) => {
    if (!(ev.buttons & 1)) return cancel();
    if (!started && Math.hypot(ev.clientX - sx, ev.clientY - sy) < 6) return;
    if (!started) {
      started = true;
      document.body.classList.add("dragging");
    }
    app.drag = { ...ghost, x: ev.clientX, y: ev.clientY, over: dropTarget(ev, kinds) };
  };
  /** リスナーを外して幽霊を消す。ドラッグになっていなければ undefined・なっていれば落とし先（null = 落とし先の外） */
  const stop = (): string | null | undefined => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    window.removeEventListener("pointercancel", cancel);
    window.removeEventListener("blur", cancel);
    window.removeEventListener("contextmenu", cancel, true);
    if (endDrag === cancel) endDrag = null;
    if (!started) return undefined;
    const over = app.drag?.over ?? null;
    app.drag = null;
    document.body.classList.remove("dragging");
    return over;
  };
  const up = () => {
    const over = stop();
    if (over === undefined) return;
    dragConsumed = true; // 直後の click（選択）を抑止
    if (over) void drop(over);
  };
  /** cancel の経路では click が続かないので抑止しない */
  const cancel = () => void stop();
  endDrag = cancel;
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
  window.addEventListener("pointercancel", cancel);
  window.addEventListener("blur", cancel);
  window.addEventListener("contextmenu", cancel, true); // capture = フォルダ行の oncontextmenu が stopPropagation しても届く
}

/** ItemList の行 pointerdown から呼ぶ。6px 動いたらドラッグ開始 */
export function beginRowDrag(e: PointerEvent, key: string, label: string) {
  // チェック済みの行を掴んだらチェック全件をまとめて運ぶ（Paperpile と同じ）
  const keys = app.checked.has(key) && app.checked.size > 1 ? checkedKeys() : [key];
  const ghost = keys.length > 1 ? tn(keys.length, "件") : label;
  trackDrag(e, { kind: "rows", label: ghost }, ["col", "tag"], async (over) => {
    const [type, idStr] = over.split(":");
    const id = Number(idStr);
    try {
      const n = keys.length;
      const pfx = n > 1 ? tn(n, "件に") : "";
      if (type === "tag") {
        for (const k of keys) await api.attachTag(k, id);
        const tag = app.tags.find((x) => x.id === id);
        toast(`✓ ${pfx}${t("「::labeled")}${tag?.name ?? t("ラベル::fallback")}${t("」を付与")}`);
      } else if (type === "col") {
        for (const k of keys) await api.addToCollection(k, id);
        const c = app.collections.find((c) => c.id === id);
        toast(`✓ ${pfx}${t("「::added-to")}${c?.name ?? t("フォルダ::fallback")}${t("」へ追加")}`);
      }
      await Promise.all([refreshSidebar(), refreshList()]); // 行のラベルバッジも更新
      await reloadDetailIfAmong(keys);
    } catch (err) {
      toast(`✗ ${String(err).slice(0, 200)}`);
    }
  });
}

/** ポインタ直下の [data-drop]（"col:1" / "tag:2" / "row:KEY"）のうち kinds に含まれる種類だけ */
function dropTarget(ev: PointerEvent, kinds: string[]): string | null {
  const el = document.elementFromPoint(ev.clientX, ev.clientY);
  const t = el?.closest?.("[data-drop]")?.getAttribute("data-drop") ?? null;
  return t && kinds.includes(t.split(":")[0]) ? t : null;
}

/** サイドバーのラベル → 一覧の行へドラッグして付与（Paperpile: "drag it onto one or more references"）。
    チェック済みの行へ落とせばチェック全件に、それ以外はその行だけに付ける。6px 動くまではクリック扱い */
export function beginLabelDrag(e: PointerEvent, tagId: number, name: string) {
  trackDrag(e, { kind: "label", label: name }, ["row"], async (over) => {
    const key = over.slice("row:".length);
    const keys = app.checked.has(key) && app.checked.size > 1 ? checkedKeys() : [key];
    try {
      for (const k of keys) await api.attachTag(k, tagId);
      toast(`✓ ${keys.length > 1 ? tn(keys.length, "件に") : ""}${t("「::labeled")}${name}${t("」を付与")}`);
      await Promise.all([refreshSidebar(), refreshList()]);
      await reloadDetailIfAmong(keys);
    } catch (err) {
      toast(`✗ ${String(err).slice(0, 200)}`);
    }
  });
}

/** ドラッグ直後の click を 1 回だけ握りつぶす（行選択の誤発火防止） */
export function consumeDragClick(): boolean {
  if (dragConsumed) {
    dragConsumed = false;
    return true;
  }
  return false;
}

export async function fetchPdfSelected() {
  const d = app.detail;
  if (!d) return;
  busyBegin();
  try {
    toast(await api.fetchPdf(d.key));
    await Promise.all([reloadDetail(), refreshList()]);
  } catch (e) {
    toast(`✗ ${String(e).slice(0, 300)}`);
  } finally {
    busyEnd();
  }
}

export async function attachViaDialog() {
  const d = app.detail;
  if (!d) return;
  const p = await openDialog({
    multiple: false,
    filters: [{ name: "PDF", extensions: ["pdf"] }],
  });
  if (typeof p === "string") {
    try {
      await api.attachPdf(d.key, p);
      toast(t("✓ 添付しました"));
      await Promise.all([reloadDetail(), refreshList()]);
    } catch (e) {
      toast(`✗ ${String(e).slice(0, 300)}`);
    }
  }
}
