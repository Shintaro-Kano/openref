// 本棚の画面の状態と操作。タブの中身は app.tabs[i].shelf / shelfBook（state.svelte.ts）が正で、ここは
// root ごとの走査結果の写し・履歴・表紙の順番待ち・読書位置を持つ。正はディスクのフォルダ = ここは何も書かない（書くのは settings の ui_shelf_* だけ）
import { convertFileSrc } from "@tauri-apps/api/core";
import { api } from "../api";
import { t } from "../i18n.svelte";
import { app, clearChecks, frontShelf, openShelfBookTab, openTab, setView, shelfHome, toast, type Tab } from "../state.svelte";
import { shelfIpc } from "./ipc";
import {
  breadcrumbOf,
  collectBooks,
  dirOf,
  hideRootParents,
  indexDirs,
  indexFolders,
  indexParents,
  pushPos,
  pushRecent,
  type ShelfBook,
  type ShelfBookTab,
  type ShelfFolder,
  type ShelfRoot,
  type ShelfSortKey,
  type ShelfTab,
} from "./tree";
import { errText } from "../err";

/** root の走査結果 + 索引。木は大きい（数千冊の本棚もある）= 読むだけの生の値で持つ（$state の proxy を通さない） */
export interface ShelfIndex {
  tree: ShelfFolder;
  thumbsDir: string | null;
  unreadable: number;
  usedFind: boolean;
  byId: Map<string, ShelfFolder>;
  byDir: Map<string, ShelfFolder>;
  parentOf: Map<string, string>;
  allBooks: ShelfBook[];
  byPath: Map<string, ShelfBook>;
}
export type ShelfEntry = ShelfIndex | { error: string };

export interface ShelfRecent {
  root: string;
  path: string;
}

class ShelfStore {
  /** 履歴（shelf_status・新しい順） */
  shelves = $state.raw<ShelfRoot[]>([]);
  /** root → 走査結果（開いた本棚だけ）。無い = まだ読んでいない */
  trees = $state.raw<Record<string, ShelfEntry>>({});
  /** 見出しのメニュー（x, y = fixed の位置） */
  menu = $state<{ x: number; y: number } | null>(null);
  /** サイドバーの木の開閉（フォルダ id） */
  expanded = $state<Record<string, boolean>>({});
  /** 背景の温めの進捗（slow = 実際に作っている = サイドバーに 1 行出す） */
  warm = $state<{ done: number; total: number; slow: boolean } | null>(null);
  /** 最近開いた本（ui_shelf_recent・上限 12） */
  recent = $state.raw<ShelfRecent[]>([]);
  /** グリッドの右ペインの本（開いた・右クリックした本） */
  info = $state.raw<ShelfBookTab | null>(null);
}
export const sh = new ShelfStore();

/** 読書位置（ui_shelf_pos = {"<id>": page}・上限 500）。読み手が書く間は画面に出ない = 反応させない */
let pos: Record<string, number> = {};

const fail = (e: unknown) => toast(`✗ ${errText(e, 200)}`);
/** root の本棚の操作の失敗 = Rust の文の root をフォルダ名に（失敗の案内にフルパスを出さない。Rust の殻も畳む = 二重でも同じ字）。
    切る前に畳む（先に 200 字で切ると root の途中で切れて畳めない） */
const failAt = (root: string) => (e: unknown) => fail(hideRootParents(String(e), root));
const parse = (raw: string | null): unknown => {
  try {
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null; // 壊れた値は捨てる（次の保存で上書き）
  }
};

/** 履歴を読む。履歴から消えた root のタブ（[外す]・上限 10 の押し出し）は文献の「すべて」へ。
    形が違う応答は「分からない」= タブを落とさない（Rust の契約は常に {shelves}。分からないまま全部の本棚のタブを消さない） */
export async function loadShelves() {
  const r = await shelfIpc.status();
  if (!Array.isArray(r?.shelves)) return;
  sh.shelves = r.shelves;
  const known = new Set(r.shelves.map((s) => s.root));
  await dropTabs((x) => !known.has(x));
}

/** 操作の後の読み直し。読めなくても知らせるだけ（切替・外すは Rust で済んでいる = 呼び手の続きを止めない） */
async function reloadShelves() {
  try {
    await loadShelves();
  } catch (e) {
    toast(`✗ ${t("本棚の履歴を読めません")}: ${errText(e, 160)}`);
  }
}

/** 起動時: 履歴と ui_shelf_*（タブの復元の前 = 新しいタブの並び）。読めなければ空のまま（SHELF はフォルダの選択へ進む） */
export async function loadShelf() {
  try {
    sh.shelves = (await shelfIpc.status())?.shelves ?? [];
    const [sort, recent, savedPos] = await Promise.all([api.getUiPref("ui_shelf_sort"), api.getUiPref("ui_shelf_recent"), api.getUiPref("ui_shelf_pos")]);
    if (sort === "title" || sort === "mtime" || sort === "size") app.shelfSort = sort;
    const r = parse(recent);
    if (Array.isArray(r)) sh.recent = r.filter((x) => typeof x?.root === "string" && typeof x?.path === "string").map((x) => ({ root: x.root, path: x.path }));
    const p = parse(savedPos);
    if (p && typeof p === "object" && !Array.isArray(p)) pos = Object.fromEntries(Object.entries(p).filter(([, v]) => typeof v === "number")) as Record<string, number>;
  } catch (e) {
    toast(`✗ ${t("本棚の履歴を読めません")}: ${errText(e, 160)}`);
  }
}

/** root の表示名（履歴の name・無ければフォルダ名） */
export function shelfName(root: string): string {
  return sh.shelves.find((s) => s.root === root)?.name ?? root.slice(root.lastIndexOf("/") + 1);
}

export function shelfIndex(root: string | undefined | null): ShelfIndex | null {
  const e = root ? sh.trees[root] : undefined;
  return e && "tree" in e ? e : null;
}

function makeIndex(r: Awaited<ReturnType<typeof shelfIpc.tree>>): ShelfIndex {
  const byId = new Map<string, ShelfFolder>();
  const byDir = new Map<string, ShelfFolder>();
  const parentOf = new Map<string, string>();
  const allBooks: ShelfBook[] = [];
  indexFolders(r.tree, byId);
  indexDirs(r.tree, byDir);
  indexParents(r.tree, parentOf);
  collectBooks(r.tree, allBooks);
  return { tree: r.tree, thumbsDir: r.thumbs_dir, unreadable: r.unreadable, usedFind: r.used_find, byId, byDir, parentOf, allBooks, byPath: new Map(allBooks.map((b) => [b.path, b])) };
}

const treeLoads = new Map<string, Promise<void>>();

/** root の木を読む（表があれば走査なし）。同じ root の読みは重ねない */
export function loadTree(root: string): Promise<void> {
  let p = treeLoads.get(root);
  if (!p) {
    p = shelfIpc
      .tree(root)
      .then(
        (r) => {
          sh.trees = { ...sh.trees, [root]: makeIndex(r) };
        },
        (e) => {
          sh.trees = { ...sh.trees, [root]: { error: String(e) } };
        },
      )
      .finally(() => treeLoads.delete(root));
    treeLoads.set(root, p);
  }
  return p;
}

function frontTab(): Tab | undefined {
  return app.tabs.find((x) => x.id === app.tab);
}

/** root の本棚を開く（見出しのメニュー = 本棚のタブから）。中身は常に 1 つ: 前面が一覧 / 本棚のタブならそのタブを本棚に（一覧の選択を外す =
    文脈のチップが見えない文献を指さない）、PDF・OpenNote・読み手のタブなら新しいタブで（走査を待つ間に NOTE を押した・別のタブへ移った =
    PDF のタブからセグメントを押したときと同じ） */
async function enter(root: string) {
  const tab = frontTab();
  if (!tab) return;
  if (tab.viewer || tab.wiki || tab.shelfBook) {
    await openTab({ kind: "all" }, null, shelfHome(root));
    return;
  }
  tab.shelf = shelfHome(root);
  tab.shelfAway = null; // 戻る先は本棚が無い間だけ（mode.svelte の switchMode と同じ）
  clearChecks();
}

/** 履歴の本棚へ切り替える（走査し直す = Rust が last_used を進める）→ 木と履歴を読み直す。
    切り替えられない（Drive の readdir 拒否・走査の失敗 = 失敗の toast は Rust）ときも、呼び手はその本棚に入る:
    木は必ず読み直す（loadTree は前に読めた木を今の結果で置き換える）= 読めなければ中央に理由と案内（ShelfGrid）が出て、本棚の左バーの
    メニュー（ほかの本棚・別のフォルダ…・この本棚を履歴から外す）に届く。入らないと、本棚のメニューは本棚の左バーにしか無い = 行き止まりになる */
async function useRoot(root: string) {
  warmed.delete(root);
  await shelfIpc.switchRoot(root).catch(() => {}); // 失敗しても入る（上）
  await loadTree(root);
  await reloadShelves();
}

/** 「別のフォルダを本棚として開く…」の native の選択 → 木と履歴を読む。取り消し・失敗 = null（失敗の toast は Rust） */
async function pickRoot(): Promise<string | null> {
  let root: string | null;
  try {
    root = (await shelfIpc.openRoot()).root;
  } catch {
    return null;
  }
  if (!root) return null;
  warmed.delete(root);
  await loadTree(root);
  await reloadShelves();
  return root;
}

/** メニューの本棚の行: 履歴の本棚へ（走査し直す）。今の本棚なら読み直しだけ（開いているフォルダはそのまま） */
export async function switchShelf(root: string) {
  sh.menu = null;
  const tab = app.tab;
  await useRoot(root);
  if (app.tab === tab && frontShelf()?.root !== root) await enter(root);
}

/** 別のフォルダを本棚として開く…（native の選択）。取り消しは何もしない */
export async function pickShelf() {
  sh.menu = null;
  const tab = app.tab;
  const root = await pickRoot();
  if (root && app.tab === tab) await enter(root);
}

/** セグメントの SHELF の行き先（入るのは mode.svelte の switchMode）: タブ tab が覚えた本棚（tab.shelfAway・履歴にある物 =
    無いフォルダでも = 無いフォルダのタブは開ける）はそのまま（走査しない = 開いていたフォルダ・並び・位置）→ 履歴の新しい順で最初の
    実在する本棚（走査し直す = 見出しのメニューの行と同じ・開けなくても入る = useRoot）→ 本棚が 1 つも無ければ「別のフォルダを本棚として開く…」。
    フォルダの選択の取り消し・失敗 = null（呼び手は何も変えない）。履歴は先に読み直す（見出しのメニューを開くときと同じ・読めなければ持っている物で） */
export async function shelfTarget(tab: Tab | null): Promise<ShelfTab | null> {
  await loadShelves().catch(() => {});
  const away = tab?.shelfAway; // 読み直しで履歴から消えた本棚は dropTabs が外している
  if (away && sh.shelves.some((s) => s.root === away.root)) return away;
  const r = sh.shelves.find((s) => s.exists);
  if (r) {
    await useRoot(r.root); // 開けなくても入る（中央に理由・左バーのメニューで逃げられる）
    return shelfHome(r.root);
  }
  const root = await pickRoot();
  return root ? shelfHome(root) : null;
}

/** pred の root の本棚 / 読み手のタブを全部文献の「すべて」へ（履歴から外した・押し出された） */
async function dropTabs(pred: (root: string) => boolean) {
  let front = false;
  for (const tab of app.tabs) {
    // OpenNote のタブが覚えている「戻る先の本棚」も外す（戻すと「すべて」）
    if (tab.shelfAway && pred(tab.shelfAway.root)) tab.shelfAway = null;
    const r = tab.shelf?.root ?? tab.shelfBook?.root;
    if (!r || !pred(r)) continue;
    tab.shelf = null;
    tab.shelfBook = null;
    if (tab.id === app.tab) front = true;
    else {
      tab.view = { kind: "all" };
      tab.items = [];
    }
  }
  if (front) await setView({ kind: "all" });
}

/** この本棚を履歴から外す（表と表紙も消す・結果の toast は Rust）。開いているタブは文献の「すべて」へ */
export async function forgetShelf(root: string) {
  sh.menu = null;
  try {
    await shelfIpc.forgetRoot(root);
  } catch {
    return;
  }
  warmed.delete(root);
  await dropTabs((r) => r === root); // タブを先に外す（木を先に消すと、開いているタブが履歴に無い root を読みに行く）
  sh.shelves = sh.shelves.filter((s) => s.root !== root);
  const { [root]: _, ...rest } = sh.trees;
  sh.trees = rest;
  await reloadShelves();
}

/** PDF が 1 つだけのフォルダを 1 冊として表示（次の shelf_tree が設定の差を見て読み直す） */
export async function setCollapse(root: string, on: boolean) {
  sh.menu = null;
  try {
    await shelfIpc.setOpts(root, { collapse_single: on });
    warmed.delete(root);
    await loadTree(root);
    await loadShelves();
  } catch (e) {
    failAt(root)(e);
  }
}

/** フォルダへ（dir = root 相対・null = すべての本）。newTab = 新しいタブで。祖先を開く */
export async function gotoDir(dir: string | null, newTab = false) {
  const s = frontShelf();
  if (!s) return;
  if (newTab) {
    await openTab({ kind: "all" }, null, shelfHome(s.root, dir));
    return;
  }
  s.dir = dir;
  s.query = "";
  s.scrollTop = 0;
  const ix = shelfIndex(s.root);
  let f = dir !== null ? ix?.byDir.get(dir) : undefined;
  while (f && ix) {
    sh.expanded[f.id] = true;
    const pid = ix.parentOf.get(f.id);
    f = pid ? ix.byId.get(pid) : undefined;
  }
}

export function setSort(sort: ShelfSortKey) {
  const s = frontShelf();
  if (s) {
    s.sort = sort;
    s.scrollTop = 0;
  }
  app.shelfSort = sort;
  savePref("ui_shelf_sort", sort);
}

function savePref(key: string, value: string) {
  api.setUiPref(key, value).catch((e) => toast(`✗ ${t("表示設定を保存できません")}: ${errText(e, 160)}`));
}

/** タブの題名 = 本棚名（すべての本）/ フォルダ名 */
export function shelfTabTitle(s: ShelfTab): string {
  if (s.dir === null) return shelfName(s.root);
  return shelfIndex(s.root)?.byDir.get(s.dir)?.name ?? s.dir.slice(s.dir.lastIndexOf("/") + 1);
}

/** 今のフォルダ（dir が木に無ければ = 畳まれた・消えた = null = すべての本） */
export function currentFolder(s: ShelfTab, ix: ShelfIndex): ShelfFolder | null {
  return s.dir === null ? null : (ix.byDir.get(s.dir) ?? null);
}

export function crumbs(s: ShelfTab, ix: ShelfIndex): ShelfFolder[] {
  return breadcrumbOf(currentFolder(s, ix) ?? ix.tree, ix.byId, ix.parentOf);
}

export const folderDir = (ix: ShelfIndex, f: ShelfFolder) => dirOf(ix.tree, f);

export function bookTab(root: string, b: ShelfBook): ShelfBookTab {
  return { root, id: b.id, path: b.path, title: b.title, size: b.size, mtime: b.mtime };
}

/** 本を開く（読み手を新しいタブで）。最近開いた本へ */
export async function openBook(root: string, b: ShelfBook) {
  const book = bookTab(root, b);
  sh.info = book;
  sh.recent = pushRecent(sh.recent, { root, path: b.path });
  savePref("ui_shelf_recent", JSON.stringify(sh.recent));
  await openShelfBookTab(book);
}

export function revealBook(root: string, id?: string) {
  sh.menu = null;
  shelfIpc.reveal(root, id).catch(failAt(root));
}

export function openExternal(root: string, id: string) {
  shelfIpc.openExternal(root, id).catch(failAt(root));
}

// ---- 表紙: 見えたカードの分（asset の 404 → 作る）を先に、残りは温めで。shelf_thumb は Rust の鍵で直列 = 待ちを 1 本にする ----

type CoverJob = { root: string; id: string; done: (ok: boolean) => void };
let urgent: CoverJob[] = [];
let warmJobs: { root: string; id: string }[] = [];
let warmAt = 0;
let pumping = false;
/** 温め終えた root（読み直すと外す）。2 回目のタブの切替で 1 冊ずつ問い直さない */
const warmed = new Set<string>();
/** 温めの 1 冊がこれより遅ければ実際に作っている = 進捗を出す（既にある物は 0.3 ms） */
const SLOW_MS = 30;

export function coverUrl(dir: string, b: ShelfBook): string {
  return convertFileSrc(`${dir}/${b.id}-${b.mtime}.png`);
}

/** カードの <img> が 404 = 作る。作れたら true */
export function needCover(root: string, id: string): Promise<boolean> {
  return new Promise((done) => {
    urgent.push({ root, id, done });
    void pump();
  });
}

async function pump() {
  if (pumping) return;
  pumping = true;
  try {
    for (;;) {
      const job = urgent.shift();
      if (job) {
        const ok = await shelfIpc.thumb(job.root, job.id).then(
          () => true,
          () => false, // 作れない本（暗号化等）= カードは代わりの表紙
        );
        job.done(ok);
        continue;
      }
      if (warmAt >= warmJobs.length) break;
      const w = warmJobs[warmAt++];
      const t0 = performance.now();
      await shelfIpc.thumb(w.root, w.id).catch(() => null);
      if (warmJobs.length && sh.warm) sh.warm = { done: warmAt, total: warmJobs.length, slow: sh.warm.slow || performance.now() - t0 > SLOW_MS };
      if (warmJobs.length && warmAt >= warmJobs.length) {
        warmed.add(w.root);
        warmJobs = [];
        warmAt = 0;
        sh.warm = null;
      }
    }
  } finally {
    pumping = false;
  }
}

/** 開いた本棚の背景の温め（直列）。止める = stopWarm（本棚を離れた） */
export function startWarm(root: string) {
  stopWarm();
  const ix = shelfIndex(root);
  if (!ix || warmed.has(root) || !ix.thumbsDir || !ix.allBooks.length) return;
  warmJobs = ix.allBooks.map((b) => ({ root, id: b.id }));
  warmAt = 0;
  sh.warm = { done: 0, total: warmJobs.length, slow: false };
  void pump();
}

export function stopWarm() {
  warmJobs = [];
  warmAt = 0;
  sh.warm = null;
}

/** 本棚を離れた = 見えていたカードの待ちも捨てる（木の読み直しでは捨てない = カードは残って待っている） */
export function dropCovers() {
  for (const j of urgent) j.done(false);
  urgent = [];
}

// ---- 読書位置 ----

let posTimer: ReturnType<typeof setTimeout> | undefined;

export function bookPos(id: string): number {
  return pos[id] ?? 1;
}

/** 読書位置を覚える（書くのはまとめて 1 秒後） */
export function saveBookPos(id: string, page: number) {
  if (pos[id] === page) return;
  pos = pushPos(pos, id, page);
  clearTimeout(posTimer);
  posTimer = setTimeout(() => savePref("ui_shelf_pos", JSON.stringify(pos)), 1000);
}
