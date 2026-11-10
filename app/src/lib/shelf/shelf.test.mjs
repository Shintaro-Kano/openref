/* 本棚の組立ての試験（node app/src/lib/shelf/shelf.test.mjs・実部品 App.svelte + jsdom・invoke は差し替え）。
   見るのは分岐: 見出しのメニュー・タブの永続と復元・⌘N / ⌘A / Esc / `/` / ドロップが見えない一覧に効かないこと・
   表紙の差し替えと温め・MY NOTE との排他。読み手（pdf.js）だけ偽物 = 実機で見る。土台は wiki/test-harness.mjs */
import assert from "node:assert/strict";
import { setupHarness } from "../wiki/test-harness.mjs";

const { calls, sleep, waitFor } = setupHarness({
  fakes: {
    "@tauri-apps/api/webview": "export const getCurrentWebview = () => ({ onDragDropEvent: async (f) => { globalThis.__drop = f; return () => {}; } });",
    "@tauri-apps/plugin-dialog": "export const open = async () => null; export const save = async () => null; export const confirm = async () => false; export const message = async () => {};",
    "./lib/PdfViewer.svelte": "export default function PdfViewer() {}",
    "./lib/shelf/ShelfViewer.svelte": "export default function ShelfViewer(a, p) { globalThis.__books.push(p.book.path); }",
  },
});
globalThis.__books = [];
let tests = 0;

const ROOT = "/r/教科書";
const OTHER = "/r/別の棚";
const GONE = "/r/外付け";
const mkBook = (root, dir, name, mtime = 1, size = 1) => {
  const path = `${root}/${dir ? dir + "/" : ""}${name}.pdf`;
  return { id: `id-${path}`, title: name, path, size, mtime, folder: dir || root.slice(root.lastIndexOf("/") + 1) };
};
const fold = (root, dir, books, children = []) => ({
  id: `f-${root}/${dir}`,
  name: dir ? dir.slice(dir.lastIndexOf("/") + 1) : root.slice(root.lastIndexOf("/") + 1),
  path: dir ? `${root}/${dir}` : root,
  books,
  children,
  totalBooks: books.length + children.reduce((n, c) => n + c.totalBooks, 0),
});
const treeOf = (root) => {
  if (root === OTHER) return fold(root, "", [mkBook(root, "", "別の本")]);
  const sub = fold(root, "腹部/肝臓", [mkBook(root, "腹部/肝臓", "肝臓の画像")]);
  const abd = fold(root, "腹部", [mkBook(root, "腹部", "腹部 CT", 9, 5)], [sub]);
  const neu = fold(root, "神経", [mkBook(root, "神経", "ＭＲＩ 入門", 3, 9)]);
  return fold(root, "", [mkBook(root, "", "総論", 5, 1)], [abd, neu]);
};

const prefs = {};
const items = ["K1", "K2"].map((key, i) => ({ key, title: `題名 ${i + 1}`, firstAuthor: "A", tags: [], year: 2020, starred: false, hasPdf: false }));
const detail = (key) => ({ key, csl: { title: `題名 ${key}` }, attachments: key === "K1" ? [{ id: 7, filename: "a.pdf", role: "main", exists: true }] : [], collections: [], tags: [], notes: [], dateAdded: "2026-01-01", deleted: false });
let wikiOn = false;
let shelves = [
  { root: ROOT, name: "教科書", last_used: 2, exists: true, collapse_single: false, issue_titles: false, aliases: {} },
  { root: OTHER, name: "別の棚", last_used: 1, exists: true, collapse_single: false, issue_titles: false, aliases: {} },
  { root: GONE, name: "外付け", last_used: 0, exists: false, collapse_single: false, issue_titles: false, aliases: {} },
];
let thumbGate = null; // Promise = shelf_thumb の応答を止める
let thumbStep = null; // 配列 = shelf_thumb を 1 本ずつ流す（試験が resolve を呼ぶ）
let switchGate = null; // Promise = shelf_switch_root の応答を止める
let statusNull = false; // shelf_status が null を返す
let thumbFail = new Set();
let pickRoot = null;
let statusFails = false;
globalThis.__invoke = async (name, payload) => {
  const a = payload?.args ?? payload ?? {};
  calls.push({ name, args: a });
  switch (name) {
    case "get_ui_pref":
      return prefs[a.key] ?? null;
    case "set_ui_pref":
      prefs[a.key] = a.value;
      return null;
    case "list_items":
      return items;
    case "get_item":
      return detail(a.key);
    case "wiki_settings":
    case "wiki_status":
      return { enabled: wikiOn, root: "/v" };
    case "data_version":
      return 1;
    case "cite_prefs":
      return { style: "vancouver", locale: "", doi_always: false };
    case "wiki_chat_status":
      return { enabled: false, cli: "/c", version: "1", logged_in: true, mcp: "/m", state: "off", session_id: null };
    case "shelf_status":
      if (statusFails) throw new Error("読めない");
      return statusNull ? null : { shelves };
    case "shelf_tree":
      return { root: a.root, dir: null, tree: treeOf(a.root), unreadable: 0, used_find: false, thumbs_dir: "/thumbs" };
    case "shelf_thumb":
      await thumbGate;
      if (thumbStep) await new Promise((r) => thumbStep.push(r));
      if (thumbFail.has(a.id)) throw new Error("作れない");
      return { path: `/thumbs/${a.id}.png` };
    case "shelf_open_root":
      return { root: pickRoot };
    case "shelf_switch_root":
      await switchGate;
      return { root: a.root };
    case "shelf_front_root":
      return { root: a.root };
    case "shelf_forget_root":
      shelves = shelves.filter((s) => s.root !== a.root);
      return { thumbs: 3, unknown: false };
    case "shelf_set_opts":
      shelves = shelves.map((s) => (s.root === a.root ? { ...s, collapse_single: a.collapse_single } : s));
      return {};
    default:
      if (name.startsWith("wiki_")) return { pages: [], total: 0, files: [], items: [], hits: [], entries: [], tree: [], tags: [], root: "/v", workspaces: [] };
      return /collections|tags/.test(name) || name.startsWith("list") || name.startsWith("cite_") ? [] : /counts/.test(name) ? {} : null;
  }
};

const S = await import("../state.svelte.ts");
const H = await import("./shelf.svelte.ts");
const W = await import("../wiki/wiki.svelte.ts");
const { mount, flushSync } = await import("svelte");
const { default: App } = await import("../../App.svelte");

const key = (o, el = window) => el.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...o }));
const $ = (q) => document.querySelector(q);
const $$ = (q) => [...document.querySelectorAll(q)];
const settle = async () => {
  flushSync();
  await sleep(15);
  flushSync();
};
const called = (name) => calls.filter((c) => c.name === name);
const front = () => S.app.tabs.find((t) => t.id === S.app.tab);
/** タブを 1 枚の一覧に戻す */
const reset = async () => {
  while (S.app.tabs.length > 1) await S.closeTab(S.app.tabs.at(-1).id);
  await S.closeTab(S.app.tab);
  await settle();
};
const menuRow = (label) => $$(".sh-menu .lmenu-item").find((b) => b.textContent.includes(label));
const openMenu = async () => {
  $(".sh-head").click();
  await settle();
};

const target = document.createElement("div");
document.body.append(target);
mount(App, { target });
await waitFor(() => $$(".row").length === 2);
await settle();

// 文献のとき: 見出しは MY LIBRARY・＋ / … は今までどおり・メニュー = ✓ 文献 / 履歴（無いフォルダは押せない）/ 別のフォルダ…・本棚の設定は出ない
{
  assert.equal($(".sh-head-name").textContent, "MY LIBRARY");
  assert.ok($(".head-actions"), "文献の ＋ / … が消えた");
  await openMenu();
  assert.ok(menuRow("文献").textContent.includes("✓"), "文献に ✓ が無い");
  assert.ok(menuRow("教科書") && !menuRow("教科書").textContent.includes("✓"));
  assert.equal(menuRow("外付け").disabled, true, "無いフォルダが押せる");
  assert.ok(menuRow("別のフォルダを本棚として開く…"));
  assert.equal(menuRow("Finder で表示"), undefined, "文献のときに本棚の設定が出た");
  key({ key: "Escape" });
  await settle();
  assert.equal($(".sh-menu"), null, "Esc でメニューが閉じない");
  tests++;
}

// 選んでいる文献があっても本棚へ入ると外す（文脈のチップが見えない文献を指さない）・本棚の中身に切り替わる
{
  $$(".row")[1].click();
  await settle();
  assert.equal(S.app.selected, "K2");
  await openMenu();
  menuRow("教科書").click();
  await settle();
  assert.deepEqual(called("shelf_switch_root").at(-1).args, { root: ROOT });
  assert.equal(front().shelf?.root, ROOT, "前面のタブが本棚にならない");
  assert.equal(front().shelf.dir, null);
  assert.deepEqual([S.app.selected, S.app.checked.size], [null, 0], "一覧の選択が残った");
  assert.equal($(".sh-head-name").textContent, "📚 教科書");
  assert.equal($(".sh-head").title, ROOT);
  // LABELS・ゴミ箱・＋・… は描かない。QuickAdd・＋新規・一覧も無い
  assert.equal($(".head-actions"), null, "本棚に ＋ / … が出た");
  assert.equal($("nav.sidebar").textContent.includes("LABELS"), false);
  assert.equal($("nav.sidebar").textContent.includes("ゴミ箱"), false);
  assert.deepEqual([$("#quickadd"), $(".newref"), $(".rows")], [null, null, null], "本棚に文献の部品が残った");
  assert.ok($("#sh-search"), "本棚の検索欄が無い");
  assert.equal($$(".sh-book").length, 4, "すべての本が全部出ない");
  assert.equal($$(".sh-folder").length, 2, "直下のフォルダが出ない");
  assert.equal($("nav.sidebar > button .n").textContent, "4", "すべての本の冊数");
  tests++;
}

// フォルダの行・パンくず・カード・検索 = タブの dir / query（語は NFKC + AND・フォルダへ移ると語は外れる）
{
  const row = () => $$(".sh-node").find((r) => r.textContent.includes("腹部"));
  row().click();
  await settle();
  assert.equal(front().shelf.dir, "腹部");
  assert.equal($$(".sh-book").length, 1);
  assert.deepEqual($$(".sh-folder").map((f) => f.querySelector(".sh-folder-name").textContent), ["肝臓"]);
  assert.ok(row().classList.contains("active"));
  $(".sh-folder").click();
  await settle();
  assert.equal(front().shelf.dir, "腹部/肝臓");
  assert.deepEqual($$(".sh-crumbs > *").map((e) => e.textContent), ["教科書", "›", "腹部", "›", "肝臓"]);
  $$(".sh-crumb")[0].click(); // 本棚名 = すべての本
  await settle();
  assert.equal(front().shelf.dir, null);
  const input = $("#sh-search");
  input.value = "ｍｒｉ";
  input.dispatchEvent(new Event("input", { bubbles: true }));
  await sleep(170);
  await settle();
  assert.deepEqual($$(".sh-book-title").map((e) => e.textContent), ["ＭＲＩ 入門"], "NFKC で当たらない");
  assert.equal($$(".sh-folder").length, 0, "検索中にフォルダが出た");
  input.value = "ｍｒｉ 腹部";
  input.dispatchEvent(new Event("input", { bubbles: true }));
  await sleep(170);
  await settle();
  assert.equal($$(".sh-book").length, 0, "AND にならない");
  assert.ok($(".sh-state").textContent.includes("一致する本はありません"));
  key({ key: "Escape" }, input);
  await settle();
  assert.equal(front().shelf.query, "", "Esc で語が外れない");
  tests++;
}

// 並び = 名前 / 更新 / サイズ（ui_shelf_sort）
{
  $(".lb-sort").click();
  await settle();
  $$(".lb-sort-pop button")[2].click();
  await settle();
  assert.equal(prefs.ui_shelf_sort, "size");
  assert.equal(S.app.shelfSort, "size");
  assert.equal($(".sh-book-title").textContent, "ＭＲＩ 入門", "サイズの大きい順でない");
  tests++;
}

// 本棚の前面で ⌘N / ⌘A / Esc / `/` / PDF のドロップが見えない一覧に効かない
{
  S.app.checked.add("K1");
  const n0 = S.app.checked.size;
  key({ metaKey: true, key: "a" });
  await settle();
  assert.equal(S.app.checked.size, n0, "⌘A が見えない一覧をチェックした");
  key({ key: "Escape" });
  await settle();
  assert.equal(S.app.checked.size, n0, "Esc が見えない一覧のチェックを外した");
  S.app.checked.clear();
  key({ metaKey: true, key: "n" });
  await settle();
  assert.equal(document.activeElement?.id === "quickadd", false);
  key({ key: "/" });
  await settle();
  assert.equal(document.activeElement?.id, "sh-search", "/ が本棚の検索欄へ行かない");
  document.activeElement.blur();
  const imports = calls.length;
  globalThis.__drop({ payload: { type: "enter", paths: [] } });
  assert.equal(S.app.dropActive, false, "ドロップの枠が出た");
  globalThis.__drop({ payload: { type: "drop", paths: ["/tmp/x.pdf"] } });
  await settle();
  assert.equal(S.app.importJob, null, "本棚の前面でドロップが取込に流れた");
  assert.equal(calls.slice(imports).filter((c) => /import/.test(c.name)).length, 0);
  tests++;
}

// ⌘⌥I = 右ペインの開閉（app.detailOpen を共用）・グリッドの右クリック = 本の情報
{
  assert.ok($("main > aside.sh-right"), "右ペインが無い");
  assert.ok($(".sh-info-none"), "選んでいないのに本の情報が出た");
  $(".sh-book").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
  await settle();
  assert.equal($(".sh-info-title").textContent, "ＭＲＩ 入門");
  assert.ok($(".sh-info").textContent.includes(`${ROOT}/神経/ＭＲＩ 入門.pdf`));
  $$(".sh-info-acts button")[0].click();
  await settle();
  assert.deepEqual(called("shelf_reveal").at(-1).args, { root: ROOT, id: `id-${ROOT}/神経/ＭＲＩ 入門.pdf` });
  $$(".sh-info-acts button")[1].click();
  await settle();
  assert.equal(called("shelf_open_external").at(-1).args.root, ROOT);
  key({ metaKey: true, altKey: true, code: "KeyI", key: "ˆ" });
  await settle();
  assert.equal($("aside.sh-right"), null, "⌘⌥I で閉じない");
  assert.equal($("main").className, "lib-nodetail", "2 列にならない");
  key({ metaKey: true, altKey: true, code: "KeyI", key: "ˆ" });
  await settle();
  assert.ok($("aside.sh-right"));
  tests++;
}

// 本を開く = 読み手のタブ（同じ本は同じタブ）・最近開いた本・タブの題名・保存の形・読み手の下に一覧を作らない
{
  const n = S.app.tabs.length;
  const shelfTab = S.app.tab;
  $(".sh-book").click();
  await settle();
  assert.equal(S.app.tabs.length, n + 1, "新しいタブにならない");
  assert.ok(S.frontShelfBook(), "読み手のタブが前面でない");
  assert.deepEqual(globalThis.__books, [`${ROOT}/神経/ＭＲＩ 入門.pdf`], "読み手が作られない");
  assert.equal($(".tabs-item.front .tabs-title").textContent, "ＭＲＩ 入門");
  assert.equal($(".rows"), null, "読み手の下に一覧を作った");
  assert.deepEqual(JSON.parse(prefs.ui_shelf_recent), [{ root: ROOT, path: `${ROOT}/神経/ＭＲＩ 入門.pdf` }]);
  await S.activateTab(shelfTab);
  await settle();
  assert.ok($(".sh-recents"), "最近開いた本が出ない");
  assert.equal($(".tabs-item.front .tabs-title").textContent, "教科書", "本棚のタブの題名");
  assert.ok($(".tabs-item.front .tabs-tab").title.includes(ROOT), "tooltip に root が無い");
  $(".sh-grid .sh-book").click();
  await settle();
  assert.equal(S.app.tabs.length, n + 1, "同じ本で 2 枚目のタブが出た");
  const saved = JSON.parse(S.tabsPref());
  assert.deepEqual(saved.tabs[0].shelf, { root: ROOT, dir: null });
  assert.equal(saved.tabs[1].shelfBook.path, `${ROOT}/神経/ＭＲＩ 入門.pdf`);
  assert.equal(saved.tabs[1].shelfBook.root, ROOT);
  tests++;
}

// ⌘T = 前面と同じ本棚の「すべての本」（読み手からも）・⌘クリックのフォルダ = 新しいタブ
{
  key({ metaKey: true, key: "t" });
  await settle();
  assert.deepEqual([front().shelf?.root, front().shelf?.dir, front().shelfBook], [ROOT, null, null], "⌘T が本棚を継がない");
  $$(".sh-node")[0].dispatchEvent(new MouseEvent("click", { bubbles: true, metaKey: true }));
  await settle();
  assert.equal(front().shelf.dir, "腹部", "⌘クリックが新しいタブにならない");
  assert.equal($(".tabs-item.front .tabs-title").textContent, "腹部", "フォルダ名の題名");
  await S.closeTab(S.app.tab);
  await S.closeTab(S.app.tab);
  await settle();
  tests++;
}

// 復元: 走査なしで戻す（読み手だけの root に shelf_tree を出さない）・asset scope が先・履歴に無い root は「すべて」へ（toast 1 回）・壊れた値は一覧のタブ
{
  S.app.tabsRestored = false;
  prefs.ui_tabs = JSON.stringify({
    active: 1,
    tabs: [
      { view: { kind: "all" }, search: "", filters: {}, shelf: { root: ROOT, dir: "腹部" } },
      { view: { kind: "all" }, search: "", filters: {}, shelfBook: { root: OTHER, id: "x", path: `${OTHER}/別の本.pdf`, title: "別の本", size: 1, mtime: 1 } },
      { view: { kind: "all" }, search: "", filters: {}, shelf: { root: "/r/履歴に無い", dir: null } },
      { view: { kind: "starred" }, search: "", filters: {}, shelfBook: { root: "/r/履歴に無い", id: "y", path: "/r/履歴に無い/y.pdf", title: "y" } },
      { view: { kind: "all" }, search: "", filters: {}, shelfBook: { root: OTHER, id: "x", path: `${OTHER}/別の本.pdf`, title: "別の本" } },
      { view: { kind: "trash" }, search: "", filters: {}, shelf: { dir: null } },
    ],
  });
  const before = calls.length;
  await S.restoreTabs();
  await settle();
  const after = calls.slice(before);
  assert.deepEqual(after.filter((c) => c.name === "shelf_front_root").map((c) => c.args.root).sort(), [OTHER, ROOT].sort(), "scope を足す root が違う");
  const fronted = after.findIndex((c) => c.name === "shelf_front_root");
  const firstTree = after.findIndex((c) => c.name === "shelf_tree" && c.args.root === OTHER);
  assert.equal(firstTree, -1, "読み手だけの root を走査した");
  assert.ok(fronted >= 0);
  assert.equal(S.app.tabs.length, 5, "同じ本のタブを捨てない / 壊れた値のタブを消した");
  assert.deepEqual(S.app.tabs.map((t) => [!!t.shelf, !!t.shelfBook, t.view.kind]), [
    [true, false, "all"],
    [false, true, "all"],
    [false, false, "all"],
    [false, false, "all"],
    [false, false, "trash"],
  ], "復元の形が違う");
  assert.equal(S.app.tabs[0].shelf.dir, "腹部");
  assert.ok(S.frontShelfBook(), "前面の読み手が戻らない");
  assert.match(S.app.toast, /本棚のタブ 3 枚/, "履歴に無い root・壊れた値を知らせない");
  // 履歴を読めない = 見えない本棚のタブを消さない
  statusFails = true;
  prefs.ui_tabs = JSON.stringify({ active: 0, tabs: [{ view: { kind: "all" }, search: "", filters: {}, shelf: { root: "/r/履歴に無い", dir: null } }] });
  await S.restoreTabs();
  await settle();
  statusFails = false;
  assert.equal(S.app.tabs[0].shelf?.root, "/r/履歴に無い", "読めないときに本棚のタブを消した");
  tests++;
}

// 履歴から外す = 開いているタブは全部「すべて」へ（読み手も）・上限 10 の押し出しも同じ（loadShelves）
{
  prefs.ui_tabs = JSON.stringify({
    active: 0,
    tabs: [
      { view: { kind: "all" }, search: "", filters: {}, shelf: { root: ROOT, dir: null } },
      { view: { kind: "all" }, search: "", filters: {}, shelfBook: { root: ROOT, id: "a", path: `${ROOT}/a.pdf`, title: "a" } },
      { view: { kind: "all" }, search: "", filters: {}, shelf: { root: OTHER, dir: null } },
    ],
  });
  await S.restoreTabs();
  await settle();
  await openMenu();
  menuRow("この本棚を履歴から外す").click();
  await settle();
  assert.deepEqual(called("shelf_forget_root").at(-1).args, { root: ROOT });
  assert.deepEqual(S.app.tabs.map((t) => [t.shelf?.root ?? null, t.shelfBook?.root ?? null]), [[null, null], [null, null], [OTHER, null]], "外した root のタブが残った");
  assert.equal(H.sh.trees[ROOT], undefined, "外した root の木が残った");
  assert.ok($(".rows"), "前面が一覧に戻らない");
  // 押し出し: 履歴から消えた root のタブも同じ
  await S.activateTab(S.app.tabs[2].id);
  await settle();
  shelves = shelves.filter((s) => s.root !== OTHER);
  await H.loadShelves();
  await settle();
  assert.equal(S.app.tabs[2].shelf, null, "押し出された root のタブが残った");
  shelves = [
    { root: ROOT, name: "教科書", last_used: 2, exists: true, collapse_single: false, issue_titles: false, aliases: {} },
    { root: OTHER, name: "別の棚", last_used: 1, exists: true, collapse_single: false, issue_titles: false, aliases: {} },
  ];
  tests++;
}

// 別のフォルダ…: 取り消し = 何もしない・選んだら前面のタブへ。メニューの collapse / Finder / 文献
{
  await H.loadShelves();
  await openMenu();
  pickRoot = null;
  menuRow("別のフォルダを本棚として開く…").click();
  await settle();
  assert.equal(front().shelf, null, "取り消しで本棚になった");
  await openMenu();
  pickRoot = OTHER;
  menuRow("別のフォルダを本棚として開く…").click();
  await settle();
  assert.equal(front().shelf?.root, OTHER);
  await openMenu();
  menuRow("PDF が 1 つだけのフォルダ").click();
  await settle();
  assert.deepEqual(called("shelf_set_opts").at(-1).args, { root: OTHER, collapse_single: true });
  await openMenu();
  assert.ok(menuRow("PDF が 1 つだけのフォルダ").textContent.includes("✓"), "チェックが付かない");
  menuRow("Finder で表示").click();
  await settle();
  assert.deepEqual(called("shelf_reveal").at(-1).args, { root: OTHER }, "本棚の Finder に id が付いた");
  await openMenu();
  menuRow("文献").click();
  await settle();
  assert.equal(front().shelf, null);
  assert.ok($(".rows"), "文献に戻らない");
  assert.equal($(".sh-head-name").textContent, "MY LIBRARY");
  tests++;
}

// 表紙: 404 → shelf_thumb → 差し替え。作れない本は代わりの表紙
{
  await H.switchShelf(ROOT);
  await settle();
  await waitFor(() => !H.sh.warm);
  const img = $(".sh-grid img");
  assert.ok(img, "表紙の img が無い");
  assert.match(img.src, /^asset:\/\/localhost\/.*%2Fthumbs%2F/, "thumbs_dir の URL でない");
  const before = img.src;
  img.dispatchEvent(new Event("error"));
  await settle();
  await waitFor(() => $(".sh-grid img")?.src !== before);
  assert.ok($(".sh-grid img").src.endsWith("?r=1"), "作り直した URL にならない");
  const id = S.app.tabs.length && H.shelfIndex(ROOT).allBooks[1].id;
  thumbFail.add(id);
  const bad = $$(".sh-grid .sh-book").find((c) => c.title === H.shelfIndex(ROOT).allBooks[1].title).querySelector("img");
  bad.dispatchEvent(new Event("error"));
  await settle();
  await waitFor(() => $$(".sh-fallback").length > 0);
  assert.ok($(".sh-fallback").textContent.includes(H.shelfIndex(ROOT).allBooks[1].title), "代わりの表紙が出ない");
  tests++;
}

// 温め = 直列で全冊・一度終えた本棚は問い直さない・離れたら止まる・開いている間だけ
{
  await H.switchShelf(ROOT); // 読み直し = もう 1 度温める
  await settle();
  await waitFor(() => !H.sh.warm);
  const done = called("shelf_thumb").length;
  await S.openTab();
  await settle();
  await S.activateTab(S.app.tabs.find((t) => t.shelf?.root === ROOT).id);
  await settle();
  assert.equal(called("shelf_thumb").length, done, "終えた本棚で問い直した");

  let open;
  thumbGate = new Promise((r) => (open = r));
  await H.switchShelf(ROOT);
  await settle();
  assert.ok(H.sh.warm, "温めが始まらない");
  const started = called("shelf_thumb").length;
  await S.openTab(); // 本棚を離れる
  await settle();
  assert.equal(H.sh.warm, null, "離れても進捗が残った");
  open();
  thumbGate = null;
  await sleep(20);
  assert.ok(called("shelf_thumb").length - started <= 1, "離れた後も作り続けた");
  await S.closeTab(S.app.tab);
  await settle();
  tests++;
}

// MY NOTE: 本棚のタブから = shelf を外して wiki（中身は常に 1 つ）・読み手のタブから = 新しいタブ・MY LIBRARY は本棚のまま
{
  wikiOn = true;
  await W.loadWikiStatus();
  await settle();
  await S.activateTab(S.app.tabs.find((t) => t.shelf).id);
  await settle();
  assert.ok(S.frontShelf(), "本棚のタブが前面でない");
  await W.switchMode("library");
  await settle();
  assert.ok(S.frontShelf(), "MY LIBRARY で本棚が外れた");
  const tab = S.app.tab;
  await W.switchMode("wiki");
  await settle();
  assert.equal(S.app.tab, tab, "同じタブのまま切り替わらない");
  assert.deepEqual([front().shelf, !!front().wiki], [null, true], "本棚と Wiki が同居した");
  const saved = JSON.parse(S.tabsPref()).tabs.find((x) => x.wiki);
  assert.equal(saved.shelf, undefined);
  wikiOn = false;
  await W.loadWikiStatus();
  await settle();
  tests++;
}

// 中身は常に 1 つ: 走査を待つ間に MY NOTE を押す = 本棚は新しいタブで・同じタブに同居しない
{
  await reset();
  wikiOn = true;
  await W.loadWikiStatus();
  await settle();
  let open;
  switchGate = new Promise((r) => (open = r));
  const tab = S.app.tab;
  const p = H.switchShelf(ROOT);
  await settle();
  await W.switchMode("wiki");
  await settle();
  open();
  switchGate = null;
  await p;
  await settle();
  const first = S.app.tabs.find((t) => t.id === tab);
  assert.deepEqual([!!first.wiki, first.shelf], [true, null], "OpenNote のタブに本棚が足された");
  assert.equal(front().shelf?.root, ROOT, "本棚が新しいタブで開かない");
  assert.equal(front().wiki, null);
  assert.ok(JSON.parse(S.tabsPref()).tabs.every((x) => !(x.wiki && x.shelf)), "ui_tabs に同居した");
  await reset();
  wikiOn = false;
  await W.loadWikiStatus();
  await settle();
  tests++;
}

// 同じ: タブが変わったらメニューは閉じる。PDF のタブの下の見出しからメニューを開いても（Tab キーで届く）PDF のタブには足さない
{
  await reset();
  const lib = S.app.tab;
  await S.openTab();
  await settle();
  const pdf = front();
  pdf.viewer = { itemKey: "K1", attachmentId: 7, title: "PDF" };
  await settle();
  await S.activateTab(lib);
  await settle();
  await openMenu();
  assert.ok($(".sh-menu"), "メニューが開かない");
  key({ metaKey: true, key: "2" });
  await waitFor(() => S.frontViewer());
  await settle();
  assert.deepEqual([H.sh.menu, $(".sh-menu")], [null, null], "タブが変わってもメニューが残った");
  await openMenu(); // 前面は PDF のタブ（見出しはビューアの下に生きている）
  menuRow("教科書").click();
  await waitFor(() => S.frontShelf());
  await settle();
  assert.deepEqual([pdf.viewer?.itemKey, pdf.shelf], ["K1", null], "PDF のタブに本棚が足された");
  assert.equal(front().shelf?.root, ROOT, "本棚が新しいタブで開かない");
  assert.ok(JSON.parse(S.tabsPref()).tabs.every((x) => !(x.viewer && x.shelf)), "ui_tabs に同居した");
  await reset();
  tests++;
}

// 読み手のタブから MY NOTE / MY LIBRARY = 新しいタブ（読み手のタブの中身は変えない）・読み手の前面で ⌘N は下の QuickAdd へ行かない
{
  await H.switchShelf(ROOT);
  await settle();
  $(".sh-grid .sh-book").click();
  await settle();
  const reader = front();
  assert.ok(reader.shelfBook, "読み手のタブでない");
  assert.ok($("#quickadd"), "読み手の下のヘッダーに QuickAdd がある前提が崩れた");
  key({ metaKey: true, key: "n" });
  await settle();
  assert.notEqual(document.activeElement?.id, "quickadd", "⌘N が読み手の下の QuickAdd へ行った");
  wikiOn = true;
  await W.loadWikiStatus();
  await settle();
  await W.switchMode("wiki");
  await settle();
  assert.deepEqual([reader.wiki, !!reader.shelfBook], [null, true], "読み手のタブに Wiki が入った");
  assert.ok(front().wiki && front().id !== reader.id, "新しいタブにならない");
  await S.activateTab(reader.id);
  await settle();
  const n = S.app.tabs.length;
  await W.switchMode("library");
  await settle();
  assert.equal(S.app.tabs.length, n + 1, "MY LIBRARY が新しいタブにならない");
  assert.ok(reader.shelfBook, "読み手のタブの中身が変わった");
  await reset();
  wikiOn = false;
  await W.loadWikiStatus();
  await settle();
  tests++;
}

// 復元の排他: 壊れた値で 2 つ持っていても中身は 1 つ（viewer > wiki > 読み手 > 本棚）
{
  wikiOn = true;
  await W.loadWikiStatus();
  await settle();
  const book = { root: ROOT, id: "a", path: `${ROOT}/a.pdf`, title: "a" };
  const base = { view: { kind: "all" }, search: "", filters: {} };
  prefs.ui_tabs = JSON.stringify({
    active: 0,
    tabs: [
      { ...base, viewer: { key: "K1", attachment: 7, title: "P" }, shelfBook: book, shelf: { root: ROOT, dir: null } },
      { ...base, wiki: { path: null, root: "/v" }, shelfBook: book },
      { ...base, wiki: { path: null, root: "/v" }, shelf: { root: ROOT, dir: null } },
      { ...base, shelfBook: book, shelf: { root: ROOT, dir: null } },
    ],
  });
  await S.restoreTabs();
  await settle();
  const kinds = S.app.tabs.map((t) => ["viewer", "wiki", "shelf", "shelfBook"].filter((k) => t[k]));
  assert.deepEqual(kinds, [["viewer"], ["wiki"], ["wiki"], ["shelfBook"]], "2 つ持ったタブが戻った");
  await reset();
  wikiOn = false;
  await W.loadWikiStatus();
  await settle();
  tests++;
}

// shelf_status が形の違う値 = 分からない = 本棚のタブを落とさない（読み直し・復元とも）
{
  await H.switchShelf(ROOT);
  await settle();
  statusNull = true;
  await H.loadShelves();
  await settle();
  assert.equal(front().shelf?.root, ROOT, "null で本棚のタブが落ちた");
  prefs.ui_tabs = JSON.stringify({ active: 0, tabs: [{ view: { kind: "all" }, search: "", filters: {}, shelf: { root: ROOT, dir: null } }] });
  await S.restoreTabs();
  await settle();
  statusNull = false;
  assert.equal(front().shelf?.root, ROOT, "null で復元のタブが落ちた");
  await reset();
  tests++;
}

// 操作の後の読み直しが失敗 = 切替は済んでいるので続ける・知らせる（未処理の reject にしない）
{
  S.app.toast = "";
  statusFails = true;
  await H.switchShelf(ROOT); // reject しない
  await settle();
  assert.equal(front().shelf?.root, ROOT, "読み直しの失敗で本棚にならない");
  assert.match(S.app.toast, /本棚の履歴を読めません/, "読み直しの失敗を知らせない");
  await H.forgetShelf(ROOT);
  await settle();
  statusFails = false;
  assert.equal(front().shelf, null, "読み直しの失敗で外したタブが残った");
  assert.equal(H.sh.trees[ROOT], undefined, "読み直しの失敗で木が残った");
  shelves = [
    { root: ROOT, name: "教科書", last_used: 2, exists: true, collapse_single: false, issue_titles: false, aliases: {} },
    { root: OTHER, name: "別の棚", last_used: 1, exists: true, collapse_single: false, issue_titles: false, aliases: {} },
  ];
  await H.loadShelves();
  await reset();
  tests++;
}

// 表紙の順番待ち: 見えたカードの分が温めより先・作り直しは 1 回だけ（2 度目の 404 は代わりの表紙 = 無限に作らない）
{
  thumbStep = [];
  await H.switchShelf(ROOT);
  await settle();
  const books = H.shelfIndex(ROOT).allBooks;
  assert.ok(thumbStep.length === 1, "温めの 1 冊目が始まらない");
  assert.equal(called("shelf_thumb").at(-1).args.id, books[0].id);
  const lastBook = books.at(-1);
  const card = () => $$(".sh-grid .sh-book").find((c) => c.title === lastBook.title);
  card().querySelector("img").dispatchEvent(new Event("error"));
  await settle();
  thumbStep.shift()();
  await waitFor(() => thumbStep.length === 1);
  assert.equal(called("shelf_thumb").at(-1).args.id, lastBook.id, "見えたカードの分が温めより後になった");
  const asked = () => called("shelf_thumb").filter((c) => c.args.id === lastBook.id).length;
  const n = asked();
  const step = thumbStep;
  thumbStep = null;
  step.splice(0).forEach((r) => r());
  await waitFor(() => !H.sh.warm && card().querySelector("img")?.src.endsWith("?r=1"));
  await settle();
  const m = asked();
  card().querySelector("img").dispatchEvent(new Event("error"));
  await settle();
  await sleep(20);
  assert.equal(asked(), m, "2 度目の 404 でまた作った");
  assert.ok(n >= 1 && card().querySelector(".sh-fallback"), "2 度目の 404 で代わりの表紙にならない");
  await reset();
  tests++;
}

// カードの中のボタンの Enter は読み手を開かない（ボタン自身の操作だけ）・カードの Enter は開く
{
  await H.switchShelf(ROOT);
  await settle();
  const card = $(".sh-grid .sh-book");
  const n = S.app.tabs.length;
  key({ key: "Enter" }, card.querySelector(".sh-book-acts button"));
  await settle();
  assert.equal(S.app.tabs.length, n, "中のボタンの Enter で読み手が開いた");
  key({ key: "Enter" }, card);
  await settle();
  assert.equal(S.app.tabs.length, n + 1, "カードの Enter で開かない");
  await reset();
  tests++;
}

// 最後の 1 枚は中身を外す（× が出る）
{
  while (S.app.tabs.length > 1) await S.closeTab(S.app.tabs.at(-1).id);
  await settle();
  await H.switchShelf(ROOT);
  await settle();
  assert.ok($(".tabs-x"), "最後の本棚のタブに × が無い");
  await S.closeTab(S.app.tab);
  await settle();
  assert.deepEqual([S.app.tabs.length, front().shelf], [1, null], "中身が外れない");
  assert.ok($(".rows"), "一覧に戻らない");
  // 読み手も同じ
  await H.switchShelf(ROOT);
  await settle();
  $(".sh-grid .sh-book").click();
  await settle();
  await S.closeTab(S.app.tabs[0].id);
  await settle();
  assert.deepEqual([S.app.tabs.length, !!S.frontShelfBook()], [1, true], "本棚のタブを閉じられない");
  await S.closeTab(S.app.tab);
  await settle();
  assert.deepEqual([S.app.tabs.length, front().shelfBook], [1, null], "最後の読み手のタブの中身が外れない");
  assert.ok($(".rows"), "一覧に戻らない");
  tests++;
}

// reveal（⌘⇧N の作成・取込の結果の行）: 本棚の前面からは新しいタブ = 見えない一覧のビューを変えない・本棚のタブはそのまま
{
  await H.switchShelf(ROOT);
  await settle();
  const shelfTab = S.app.tab;
  await S.reveal("K2");
  await settle();
  assert.notEqual(S.app.tab, shelfTab, "本棚のタブの下の一覧で reveal した");
  assert.equal(S.app.selected, "K2", "新しいタブで選ばれない");
  assert.equal(S.app.tabs.find((t) => t.id === shelfTab).shelf?.root, ROOT, "本棚のタブが変わった");
  tests++;
}

console.log(`shelf: ok (${tests} tests)`);
process.exit(0);
