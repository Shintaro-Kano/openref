/* App の組立ての試験（node app/src/lib/app.test.mjs・実部品 App.svelte + jsdom・invoke は差し替え）。部品ごとの試験（chat/chat.test.mjs の柱 ChatDock）が
   見ない配線 = main の 2 列・{#if app.detailOpen}・ListToolbar のボタンと並びのメニュー・App の keydown（⌘⌥I / ⌘⌥C / Esc）・チャットの柱の門と置き場
   （.tabs-page の外・タブの種類を切り替えても作り直さない）・reveal の手応え・行の「PDF」ボタン・main.ts の読む順序を、本物の App を通して確かめる。
   PdfViewer と ShelfViewer だけ偽物（pdf.js は jsdom で動かない）。土台は wiki/test-harness.mjs */
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { setupHarness } from "./wiki/test-harness.mjs";

const { calls, emit, sleep, waitFor } = setupHarness({
  fakes: {
    "@tauri-apps/api/webview": "export const getCurrentWebview = () => ({ onDragDropEvent: async () => () => {} });",
    "@tauri-apps/plugin-dialog": "export const open = async () => null; export const save = async () => null; export const confirm = async () => false; export const message = async () => {};",
    "./lib/PdfViewer.svelte": "export default function PdfViewer() {}",
    "./lib/shelf/ShelfViewer.svelte": "export default function ShelfViewer() {}",
  },
});
let tests = 0;

// 蔵書 3 件・OpenNote とチャットは最初 OFF（下で ON）。無い command は名前で既定を返す = App の起動に要る読みの全部をここに並べない
const prefs = {};
const items = ["K1", "K2", "K3"].map((key, i) => ({ key, title: `題名 ${i + 1}`, firstAuthor: "A", tags: [], year: 2020, starred: false, hasPdf: key !== "K3" }));
const files = {}; // key → 添付（行の「PDF」の節が入れる）
const trash = new Set();
const detail = (key) => ({ key, csl: { title: `題名 ${key}` }, attachments: files[key] ?? [], collections: [], tags: [], notes: [], dateAdded: "2026-01-01", deleted: trash.has(key) });
let failItem = false;
let holdItem = null; // Promise = get_item の応答を止める
let holdList = null; // Promise = list_items の応答を止める
let dockGate = null; // Promise = ui_chat_dock の読みを止める（読みの間に開いた柱を、読んだ値で上書きしない = 門が開く節）
let failPrefKey = null; // この鍵の get_ui_pref を失敗させる（ui_viewer_side の読みの節）
let wikiOn = false;
let chatOn = false;
// ライブラリ移行の job（MigrationModal は App と一緒に mount され、起動時に 1 度読み、進行中なら 600ms ごとに読み直す）。
// 「バックグラウンドで続行」して閉じた後に Settings の項目で戻る節のため、最初から進行中の job を返す
let migJob = { id: "J1", phase: "running", progress: { phase: "copy", completed: 3, total: 10, bytes: 0 }, config: null, plan: null, report: null, error: null };
globalThis.__invoke = async (name, payload) => {
  const a = payload?.args ?? payload ?? {};
  calls.push({ name, args: a });
  switch (name) {
    case "get_ui_pref": {
      if (a.key === failPrefKey) throw new Error("読めない");
      const v = prefs[a.key] ?? null; // 頼まれた時点の値（止めている間に書かれた値を返さない）
      if (a.key === "ui_chat_dock") await dockGate;
      return v;
    }
    case "set_ui_pref":
      prefs[a.key] = a.value;
      return null;
    case "list_items":
      await holdList;
      return items;
    case "get_item":
      if (failItem) throw new Error("読めない");
      await holdItem;
      return detail(a.key);
    case "wiki_chat_status":
      return { enabled: chatOn, cli: "/c", version: "1", logged_in: true, mcp: "/m", state: "off", session_id: null };
    case "wiki_settings":
      return { enabled: wikiOn, root: "/v" };
    case "wiki_status":
      return { enabled: wikiOn, root: "/v" };
    case "data_version":
      return 1;
    case "migration_status":
      return migJob;
    case "migration_history":
      return [];
    case "cite_prefs":
      return { style: "vancouver", locale: "", doi_always: false };
    // 参照エディタ（⌘⌥C がその窓の裏で効かないことを見る節）= 型の一覧は空でよい
    case "item_types":
      return [];
    // 本棚のタブ（柱がタブの種類を切り替えても作り直されないことを見る節）= 本 1 冊の棚
    case "shelf_status":
      return { shelves: [{ root: "/books", name: "books", last_used: 1, exists: true, collapse_single: false, issue_titles: false, aliases: {} }] };
    case "shelf_tree":
      return { root: a.root, dir: null, tree: { id: "f-/books/", name: "books", path: "/books", books: [{ id: "b1", title: "本 1", path: "/books/b1.pdf", size: 1, mtime: 0, folder: "books" }], children: [], totalBooks: 1 }, unreadable: 0, used_find: false, thumbs_dir: "/thumbs" };
    case "shelf_front_root":
      return { root: a.root };
    default:
      if (name.startsWith("wiki_")) return { pages: [], total: 0, files: [], items: [], hits: [], entries: [], tree: [], tags: [], root: "/v", workspaces: [] };
      return /collections|tags/.test(name) || name.startsWith("list") || name.startsWith("cite_") ? [] : /counts/.test(name) ? {} : null;
  }
};

const S = await import("./state.svelte.ts");
const W = await import("./wiki/wiki.svelte.ts");
const CH = await import("./chat/chat.svelte.ts");
const V = await import("./viewing.svelte.ts");
const SH = await import("./shelf/shelf.svelte.ts");
const { mount, flushSync } = await import("svelte");
const { default: App } = await import("../App.svelte");

const key = (o, el = window) => el.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...o }));
const optI = (el) => key({ metaKey: true, altKey: true, code: "KeyI", key: "ˆ" }, el); // ⌥I は配列により dead key = e.code で見る
/** ⌘⌥C（⌥C は配列により ç 等 = e.code で見る）。戻り値 = 送った event（defaultPrevented を見る） */
const optC = (el = window, o = {}) => {
  const ev = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, metaKey: true, altKey: true, code: "KeyC", key: "ç", ...o });
  el.dispatchEvent(ev);
  return ev;
};
const $ = (q) => document.querySelector(q);
const rows = () => [...document.querySelectorAll(".row")];
const settle = async () => {
  flushSync();
  await sleep(10);
  flushSync();
};

// main.ts / ハーネスは ui_detail・ui_list_sort・ui_viewer_side を mount の前に読む（mount の後に閉じると右ペイン / PDF の右バーが出て消える・既定の並びで 1 度取ってしまう）
for (const f of ["../main.ts", "../../harness/list-main.ts"]) {
  if (f.includes("harness") && !existsSync(new URL(f, import.meta.url))) continue; // ハーネスはリポジトリに同梱しないことがある = 無ければ飛ばす
  assert.match(readFileSync(new URL(f, import.meta.url), "utf8"), /Promise\.all\(\[loadLangPref\(\), loadDetailPref\(\), loadListSortPref\(\), loadViewerSidePref\(\)\]\)\.finally\(\(\) => mount\(App/, `${f}: mount の前に ui_detail / ui_list_sort / ui_viewer_side を読まない`);
}
tests++;

// 並びの pref: 既定 = 追加日・降順（= 今の並び）・形の違う値は既定のまま
assert.deepEqual({ ...S.app.listSort }, { key: "added", desc: true }, "既定が追加日・降順でない");
for (const bad of ["title:asc", "added", "added:up", "published:asc:x", "xpublished:asc", "published:ASC", ""]) {
  prefs.ui_list_sort = bad;
  await S.loadListSortPref();
  assert.deepEqual({ ...S.app.listSort }, { key: "added", desc: true }, `形の違う値を受けた: ${bad}`);
}
prefs.ui_list_sort = "published:asc";
await S.loadListSortPref();
assert.deepEqual({ ...S.app.listSort }, { key: "published", desc: false }, "保存した並びを読まない");
tests++;

// 閉じたまま起動 = 最初の描画から 2 列（本物の起動順: loadDetailPref → mount）
prefs.ui_detail = "0";
await S.loadDetailPref();
const target = document.createElement("div");
document.body.append(target);
mount(App, { target });
flushSync();
assert.equal($("aside.detail"), null, "ui_detail = 0 なのに最初の描画に右ペインが出た");
assert.equal($("main").className, "lib-nodetail", "ui_detail = 0 で 2 列にならない");
assert.ok(await waitFor(() => rows().length === 3), "一覧が出ない");
await settle();
assert.equal($(".lb-pane").getAttribute("aria-pressed"), "false");
// 再起動で残る = 最初の一覧から保存した並び（既定の並びで 1 度も取らない）
{
  const asked = calls.filter((c) => c.name === "list_items").map((c) => [c.args.filter.sort, c.args.filter.desc]);
  assert.ok(asked.length > 0 && asked.every(([k, d]) => k === "published" && d === false), `起動の一覧が保存した並びでない: ${JSON.stringify(asked)}`);
  assert.equal($(".lb-sort").textContent.replace(/\s+/g, " ").trim(), "↑ 出版日 ▾");
}
tests++;

// ⌘⌥I・ListToolbar のボタン = 開閉して覚える・キーリピートは 1 回分
optI();
await settle();
assert.ok($("main > aside.detail"), "⌘⌥I で開かない（門が閉じていれば ItemDetail そのまま）");
assert.deepEqual([$("main").className, prefs.ui_detail], ["", "1"], "⌘⌥I で 3 列に戻らない");
key({ metaKey: true, altKey: true, code: "KeyI", key: "ˆ", repeat: true });
await settle();
assert.ok($("main > aside.detail"), "キーリピートで閉じた");
$(".lb-pane").click();
await settle();
assert.deepEqual([$("aside.detail"), $("main").className, prefs.ui_detail], [null, "lib-nodetail", "0"], "ボタンで閉じない");
assert.equal($(".lb-pane").getAttribute("aria-pressed"), "false");
assert.ok($(".lb-pane").classList.contains("lb-pane-off"), "閉じた絵にならない");
tests++;

// 閉じたまま行を選ぶ = 開かない・詳細は読む（開くと出る）
rows()[1].click();
await settle();
assert.deepEqual([S.app.selected, S.app.detail?.key, $("aside.detail")], ["K2", "K2", null], "行の選択で開いた / 詳細を読まない");
$(".lb-pane").click();
await settle();
assert.ok($("aside.detail")?.textContent.includes("題名 K2"), "開くと選んでいる文献の詳細が出ない");
tests++;

// モーダルの下・エディタの下・取込の下では ⌘⌥I が効かない
for (const [label, on, off] of [
  ["モーダル", () => (S.app.modal = "citeStyle"), () => (S.app.modal = null)],
  ["取込", () => (S.app.importJob = { files: [], running: true, cancel: false }), () => (S.app.importJob = null)],
]) {
  on();
  await settle();
  optI();
  await settle();
  assert.equal(S.app.detailOpen, true, `${label}の下で ⌘⌥I が効いた`);
  off();
  await settle();
}
tests++;

// 門が閉じている = タブ列にボタンなし・柱なし・⌘⌥C は素通り（preventDefault しない・開閉の状態も変えない）・右ペインは ItemDetail だけ・チャットの invoke なし
{
  assert.deepEqual([$(".tabs-chat"), $(".cd"), $(".wk-seg")], [null, null, null], "門が閉じているのに痕跡が残った");
  const ev = optC();
  await settle();
  assert.deepEqual([ev.defaultPrevented, CH.chat.dock.open, $(".cd")], [false, false, null], "チャット OFF で ⌘⌥C を奪った / 柱を開いた");
  assert.ok($("main > aside.detail"), "門が閉じているのに右ペインが ItemDetail でない");
  assert.deepEqual(calls.filter((c) => c.name.startsWith("wiki_chat")).map((c) => c.name), [], "OpenNote OFF で invoke した");
  tests++;
}

// 門が開く = タブ列の右端にボタン・⌘⌥C で柱が .tabs-page の外（右）に開いて入力欄へ focus・覚える（ui_chat_dock）・キーリピートは 1 回分・
// 右ペイン（ItemDetail）はそのまま（チャットと無関係）・チャットの入力欄の Esc はチェックを外さない（外すと選択が閉じて文脈のチップが消える）・
// 入力欄の ⌘⌥I は右ペインだけを閉じる（柱は残る）・入力欄の外の Esc は今までどおり・モーダル中の ⌘⌥C は素通り
// 柱の記憶（ui_chat_dock）の読みが返る前に ⌘⌥C で開いた = 開閉は読んだ値（閉）で上書きしない・幅は覚えていた 444 を戻して書き直す
let releaseDock;
dockGate = new Promise((r) => (releaseDock = r));
prefs.ui_chat_dock = JSON.stringify({ open: false, w: 444 });
wikiOn = true;
chatOn = true;
S.app.wikiOn = true;
assert.ok(await waitFor(() => (flushSync(), !!$(".tabs-chat"))), "門が開いてもタブ列にボタンが出ない");
assert.ok(await waitFor(() => calls.some((c) => c.name === "get_ui_pref" && c.args.key === "ui_chat_dock")), "チャットが有効になっても柱の記憶を読まない");
assert.equal($(".cd"), null, "開いていないのに柱が出た");
{
  const ev = optC();
  await settle();
  releaseDock();
  dockGate = null;
  await settle();
  assert.deepEqual([CH.chat.dock.open, CH.chat.dock.w, !!$(".cd")], [true, 444, true], "読みの間に開いた柱を読んだ値で上書きした / 覚えていた幅を捨てた");
  assert.equal($(".cd").style.width, "444px", "覚えていた幅で柱を描かない");
  assert.equal(ev.defaultPrevented, true, "⌘⌥C を奪わない");
  assert.ok($(".dock-row > aside.cd .wk-ch"), "⌘⌥C で柱が開かない");
  assert.equal($(".tabs-page .cd"), null, "柱が .tabs-page の中にある（PDF のビューアに覆われる）");
  assert.ok($("main > aside.detail"), "柱を開いたら右ペインが変わった");
  assert.equal(document.activeElement, $(".cd textarea"), "柱を開いても入力欄へ focus しない");
  assert.deepEqual(JSON.parse(prefs.ui_chat_dock), { open: true, w: 444 }, "開いたことを覚えない / 覚えていた幅を書き直さない");
  optC(window, { repeat: true });
  await settle();
  assert.ok($(".cd"), "キーリピートで閉じた");
}
const chip = () => $(".wk-ch-ctx .wk-ch-chip-text")?.textContent;
assert.equal(chip(), "題名 K2", "選んでいる文献がチップに出ない");
const ta = $(".wk-ch textarea");
key({ key: "Escape", code: "Escape" }, ta);
await settle();
assert.deepEqual([S.app.selected, chip()], ["K2", "題名 K2"], "入力欄の Esc で選択が閉じた");
optI(ta); // 入力欄の中からでも効く = 前面のモード（一覧）の右ペインの開閉
await settle();
assert.deepEqual([$("aside.detail"), $(".cd textarea")], [null, ta], "入力欄の ⌘⌥I で右ペインが閉じない / 柱まで閉じた・作り直した");
optI();
await settle();
assert.deepEqual([!!$("main > aside.detail"), $(".cd textarea")], [true, ta], "開き直すと右ペインが戻らない / 柱が作り直された");
for (const [label, on, off] of [
  ["モーダル", () => (S.app.modal = "citeStyle"), () => (S.app.modal = null)],
  ["参照エディタ", () => (S.app.editorOpen = true), () => (S.app.editorOpen = false)],
  ["PDF 取込", () => (S.app.importJob = { files: [], running: true, cancel: false }), () => (S.app.importJob = null)],
]) {
  on();
  await settle();
  const before = document.activeElement;
  const ev = optC();
  await settle();
  assert.deepEqual([ev.defaultPrevented, !!$(".cd"), document.activeElement === before], [false, true, true], `${label}の下で ⌘⌥C が効いた / focus が動いた`);
  off();
  await settle();
}
await settle();
key({ key: "Escape", code: "Escape" }, document.body);
await settle();
assert.deepEqual([S.app.selected, $(".wk-ch-ctx")], [null, null], "入力欄の外の Esc でチェックが外れない");
tests++;

// reveal: 柱を開いている・右ペインを開いている → 行を見せる。reveal は柱も右ペインも変えない。右ペインを閉じたまま reveal しても行を見せ、右ペインは開かない
{
  const seen = [];
  const scroll = HTMLElement.prototype.scrollIntoView;
  HTMLElement.prototype.scrollIntoView = function (o) {
    if (this.classList.contains("row")) seen.push([this.classList.contains("selected"), o?.block]);
  };
  await S.reveal("K3");
  await settle();
  assert.deepEqual(seen, [[true, "nearest"]], "柱を開いたまま reveal が行を見せない");
  assert.deepEqual([S.app.selected, $(".cd textarea") === ta, S.app.detailOpen], ["K3", true, true], "reveal が柱か右ペインを変えた");
  optI();
  await settle();
  seen.length = 0;
  await S.reveal("K1");
  await settle();
  assert.deepEqual(seen, [[true, "nearest"]], "閉じたまま reveal が行を見せない");
  assert.deepEqual([S.app.selected, S.app.detailOpen, prefs.ui_detail], ["K1", false, "0"], "reveal が右ペインを開いた");
  optI();
  await settle();
  HTMLElement.prototype.scrollIntoView = scroll;
  tests++;
}

// PDF のタブ: 柱は同じ 1 つ（入力欄の DOM が同じ = 作り直さない）・チップはその文献・⌘⌥I = PDF の右バーの開閉（ui_viewer_side・一覧の右ペインは変えない・
// モーダル / エディタ / 取込の下では効かない）・⌘⌥C は効く・閉じると元のタブのまま（PdfViewer は偽物 = 右バーの DOM は viewer/side.test）
{
  const front = S.app.tab;
  await S.openViewer("K2", 1, "T");
  await settle();
  assert.ok(S.frontViewer(), "PDF のタブが前面でない");
  assert.ok($("main > aside.detail"), "PDF のタブの下の詳細が消えた（今までどおり = 右ペインの枝の条件は変えない）");
  assert.equal($(".cd textarea"), ta, "PDF のタブへ移ると柱を作り直した");
  assert.equal(chip(), "T", "PDF のタブで柱のチップがその文献でない");
  assert.equal(S.app.viewerSideOpen, true, "既定で PDF の右バーが閉じている");
  optI();
  await settle();
  assert.deepEqual([S.app.viewerSideOpen, prefs.ui_viewer_side, S.app.detailOpen, prefs.ui_detail], [false, "0", true, "1"], "PDF のタブの ⌘⌥I が右バーを閉じない / 一覧の右ペインを変えた");
  key({ metaKey: true, altKey: true, code: "KeyI", key: "ˆ", repeat: true });
  await settle();
  assert.equal(S.app.viewerSideOpen, false, "キーリピートで右バーが開いた");
  for (const [label, on, off] of [
    ["モーダル", () => (S.app.modal = "citeStyle"), () => (S.app.modal = null)],
    ["参照エディタ", () => (S.app.editorOpen = true), () => (S.app.editorOpen = false)],
    ["PDF 取込", () => (S.app.importJob = { files: [], running: true, cancel: false }), () => (S.app.importJob = null)],
  ]) {
    on();
    await settle();
    optI();
    await settle();
    assert.equal(S.app.viewerSideOpen, false, `${label}の下の PDF のタブで ⌘⌥I が効いた`);
    off();
    await settle();
  }
  optI();
  await settle();
  assert.deepEqual([S.app.viewerSideOpen, prefs.ui_viewer_side, S.app.detailOpen], [true, "1", true], "PDF のタブの ⌘⌥I で右バーが開き直さない");
  optC();
  await settle();
  assert.equal($(".cd"), null, "PDF のタブで ⌘⌥C が効かない");
  optC();
  await settle();
  assert.ok($(".cd textarea"), "PDF のタブで ⌘⌥C で開き直せない");
  await S.closeTab(S.app.tab);
  await settle();
  assert.equal(S.app.tab, front);
  assert.ok($(".cd .wk-ch"), "一覧へ戻ると柱が消えた");
  tests++;
}

// タブの種類を切り替えても（一覧 → PDF → OpenNote → 本棚 → 本 → 一覧）柱は作り直されない（入力欄の DOM 要素が同一）・
// 文脈のチップは前面の物（一覧 = 選んだ文献 / PDF = その文献 / OpenNote = ページ / 本棚・本 = なし）・前面が変わるたびに状態を読み直す
{
  const list = S.app.tab;
  const box = $(".cd textarea");
  const base = S.app.tabs.find((t) => t.id === list);
  rows()[1].click(); // K2 を選ぶ
  await settle();
  W.wk.page = { path: "notes/a.md", abs_path: "/v/notes/a.md", kind: "md", title: "A", size: 1, mtime: 0, content: "本文", sha256: "s", truncated: false };
  const kinds = [
    ["一覧", null, "題名 K2"],
    ["PDF", { viewer: { itemKey: "K1", attachmentId: 1, title: "PDF の題名" } }, "PDF の題名"],
    ["OpenNote", { wiki: { path: "notes/a.md", scrollTop: 0, editing: false } }, "notes/a.md"],
    ["本棚", { shelf: { root: "/books", dir: null, query: "", sort: "name", scrollTop: 0 } }, undefined],
    ["本", { shelfBook: { root: "/books", id: "b1", path: "/books/b1.pdf", title: "本 1", size: 1, mtime: 0 } }, undefined],
  ];
  let id = 9500;
  const reads = () => calls.filter((c) => c.name === "wiki_chat_status").length;
  for (const [label, over, want] of kinds) {
    const r0 = reads();
    if (over) {
      S.app.tabs.push({ ...base, id: ++id, wiki: null, viewer: null, shelf: null, shelfBook: null, ...over });
      S.app.tab = id;
    }
    await settle();
    if (over) W.wk.page = { path: "notes/a.md", abs_path: "/v/notes/a.md", kind: "md", title: "A", size: 1, mtime: 0, content: "本文", sha256: "s", truncated: false };
    await settle();
    assert.equal($(".cd textarea"), box, `${label}のタブで柱を作り直した`);
    assert.equal(chip(), want, `${label}のタブで柱のチップが前面の物でない`);
    // 前面の元が変わったら前の元の選択を捨てる（App の onMount の initViewing の配線）
    if (over) assert.equal(V.viewing.sel, null, `${label}のタブへ移っても前の元の選択が残った`);
    V.viewing.sel = { key: "item:前の元", text: "前の元の選択" };
    if (over) assert.ok(await waitFor(() => reads() > r0), `${label}のタブへ移っても状態を読み直さない`);
    if (label === "OpenNote") {
      W.wk.infoOpen = true;
      W.wk.prefsReady = true;
      await settle();
      assert.ok($("main > aside.wk-info"), "OpenNote の右ペイン（WikiInfo）が出ない");
    }
  }
  S.app.tab = list;
  S.app.tabs.splice(S.app.tabs.findIndex((t) => t.id === 9501), 5);
  await settle();
  assert.equal(V.viewing.sel, null, "一覧へ戻っても前の元の選択が残った");
  assert.equal($(".cd textarea"), box, "一覧へ戻ると柱を作り直した");
  tests++;
}

// 柱を開いたまま Settings でチャットを OFF（Rust の wiki-chat {enabled: false}）= 柱もボタンも消え ⌘⌥C は素通り・開閉の記憶は残る → ON で戻る
{
  chatOn = false;
  emit("wiki-chat", { type: "enabled", enabled: false });
  await settle();
  assert.deepEqual([$(".cd"), $(".tabs-chat"), CH.chat.dock.open], [null, null, true], "チャット OFF で柱かボタンが残った / 開閉の記憶を消した");
  const ev = optC();
  await settle();
  assert.deepEqual([ev.defaultPrevented, CH.chat.dock.open], [false, true], "チャット OFF で ⌘⌥C を奪った");
  chatOn = true;
  emit("wiki-chat", { type: "enabled", enabled: true });
  assert.ok(await waitFor(() => (flushSync(), !!$(".cd .wk-ch") && !!$(".tabs-chat"))), "チャット ON で柱とボタンが戻らない");
  tests++;
}

// 柱の置き場の CSS（jsdom に配置は無い = 規則の文を見る。実機の幅は実機で確認）: 縮むのは .tabs-page（min-width: 0・はみ出しは clip）・
// 柱は縮まない（flex: none）・柱の z-index は読み手（.viewer-overlay）より上でモーダルの覆い（.overlay）より下
{
  const css = readFileSync(new URL("../app.css", import.meta.url), "utf8");
  const dock = readFileSync(new URL("./chat/ChatDock.svelte", import.meta.url), "utf8");
  const rule = (src, sel) => src.match(new RegExp(`(?:^|\\n)\\s*${sel.replace(/[.]/g, "\\.")}\\s*\\{([^}]*)\\}`))?.[1] ?? "";
  const z = (body) => Number(body.match(/z-index:\s*(\d+)/)?.[1]);
  assert.match(rule(css, ".dock-row"), /display:\s*flex/, ".dock-row が横の flex でない");
  assert.match(rule(css, ".tabs-page"), /min-width:\s*0/, ".tabs-page に min-width: 0 が無い（最小幅 900 で柱が押し出される）");
  assert.match(rule(css, ".tabs-page"), /overflow:\s*clip/, ".tabs-page のはみ出しを clip で切らない");
  assert.match(rule(dock, ".cd"), /flex:\s*none/, "柱が縮む");
  assert.ok(z(rule(dock, ".cd")) > z(rule(css, ".viewer-overlay")) && z(rule(dock, ".cd")) < z(rule(css, ".overlay")), "柱の z-index が読み手より上・モーダルより下でない");
  tests++;
}

// 柱を閉じる = ⌘⌥C・覚える（起動し直すと戻る = 起動時の読みは chat.test）
{
  optC();
  await settle();
  assert.deepEqual([$(".cd"), JSON.parse(prefs.ui_chat_dock).open], [null, false], "⌘⌥C で閉じない / 閉じたことを覚えない");
  tests++;
}

// 起動時の読み（main.ts が mount の前に待つ = ui_detail と同じ）: "0" だけが閉じる・無ければ開く・読めなければ既定（開く）のまま知らせる
{
  for (const [v, open] of [["0", false], ["1", true], [undefined, true]]) {
    if (v === undefined) delete prefs.ui_viewer_side;
    else prefs.ui_viewer_side = v;
    S.app.viewerSideOpen = !open;
    await S.loadViewerSidePref();
    assert.equal(S.app.viewerSideOpen, open, `ui_viewer_side = ${v}`);
  }
  failPrefKey = "ui_viewer_side";
  S.app.toast = null;
  await S.loadViewerSidePref();
  failPrefKey = null;
  assert.equal(S.app.viewerSideOpen, true, "読めないのに既定（開く）でない");
  assert.match(S.app.toast ?? "", /表示設定を読めません/, "読めないことを知らせない");
  // 開いているときに開く（⌘F の前の showViewerSide）= 書かない・閉じていれば開いて覚える
  delete prefs.ui_viewer_side;
  S.showViewerSide();
  await settle();
  assert.equal(prefs.ui_viewer_side, undefined, "開いているのに右バーの開閉を書いた");
  S.app.viewerSideOpen = false;
  S.showViewerSide();
  await settle();
  assert.deepEqual([S.app.viewerSideOpen, prefs.ui_viewer_side], [true, "1"], "閉じた右バーを開かない / 覚えない");
  tests++;
}

// 行の「PDF」: 主 PDF（実在する最初の添付）のタブを開く・行の選択は変えない・ドラッグを始めない・開いていれば既存のタブへ・実体なしは知らせる
{
  const list = S.app.tab;
  const badge = (key) => document.querySelector(`.row[data-drop="row:${key}"] button.pdf`);
  const nGet = () => calls.filter((c) => c.name === "get_item").length;
  const tabsN = () => S.app.tabs.length;
  files.K1 = [{ id: 11, exists: true, path: "/p/k1.pdf" }];
  files.K2 = [{ id: 21, exists: false, path: "/p/gone.pdf" }, { id: 22, exists: true, path: "/p/k2.pdf" }, { id: 23, exists: true, path: "/p/k2b.pdf" }];
  await S.reveal("K1");
  await settle();
  assert.equal(badge("K3"), null, "添付の無い行がボタンになった");
  assert.equal(document.querySelector('.row[data-drop="row:K3"] span.pdf')?.textContent, "", "添付の無い行の空の span が無い");
  assert.equal(badge("K2").title, "PDF を読む");

  // 掴んでも行のドラッグを始めない（pointerdown を行へ上げない）。jsdom に elementFromPoint が無いと trackDrag の move が投げて app.drag に届かず、表明が空振りする
  document.elementFromPoint = () => null;
  badge("K2").dispatchEvent(new window.PointerEvent("pointerdown", { bubbles: true, button: 0, buttons: 1, clientX: 10, clientY: 10, pointerId: 1, isPrimary: true }));
  window.dispatchEvent(new window.PointerEvent("pointermove", { bubbles: true, buttons: 1, clientX: 200, clientY: 200, pointerId: 1 }));
  await settle();
  assert.equal(S.app.drag, null, "PDF のボタンから行のドラッグが始まった");
  window.dispatchEvent(new window.PointerEvent("pointerup", { bubbles: true, button: 0, clientX: 200, clientY: 200, pointerId: 1 }));

  // 選んでいない行: get_item を読み、実在する最初の添付を開く・元のタブの選択は変わらない
  const g = nGet();
  const n = tabsN();
  badge("K2").click();
  await settle();
  assert.equal(nGet() > g, true, "選んでいない行で get_item を読まない");
  assert.deepEqual({ ...S.frontViewer() }, { itemKey: "K2", attachmentId: 22, title: "題名 K2" }, "実在する最初の添付を開かない");
  assert.equal(tabsN(), n + 1, "新しいタブにならない");
  const orig = S.app.tabs.find((t) => t.id === list);
  assert.deepEqual([orig.selected, [...orig.checked]], ["K1", ["K1"]], "元のタブの行の選択が変わった");

  // 同じ PDF が開いていれば既存のタブへ
  const pdfTab = S.app.tab;
  await S.activateTab(list);
  await settle();
  assert.deepEqual([S.app.selected, [...S.app.checked]], ["K1", ["K1"]], "戻ると選択が変わっていた");
  badge("K2").click();
  await settle();
  assert.deepEqual([S.app.tab, tabsN()], [pdfTab, n + 1], "既存のタブへ行かない");
  await S.closeTab(pdfTab);
  await settle();

  // 詳細に出している行: 読み直さない・右ペインを閉じていても開く
  optI();
  await settle();
  assert.equal(S.app.detailOpen, false);
  const g2 = nGet();
  badge("K1").click();
  await settle();
  assert.deepEqual([S.frontViewer()?.itemKey, S.frontViewer()?.attachmentId, S.app.detailOpen], ["K1", 11, false], "閉じたまま開かない");
  assert.equal(nGet(), g2 + 1, "詳細に出している行を読み直した（+1 は新しいタブの reloadDetail）");
  await S.closeTab(S.app.tab);
  await settle();
  optI();
  await settle();

  // 実体が無い = 開かずに知らせる（hasPdf は添付の行があるだけ）
  files.K2 = [{ id: 21, exists: false, path: "/p/gone.pdf" }];
  const n2 = tabsN();
  badge("K2").click();
  await settle();
  assert.equal(S.frontViewer(), null, "実体が無いのに開いた");
  assert.equal(tabsN(), n2, "実体が無いのにタブが増えた");
  assert.match(S.app.toast, /PDF の実体が見つかりません/, "実体が無いことを知らせない");

  // 読めない = 知らせる・タブを作らない
  files.K2 = [{ id: 22, exists: true, path: "/p/k2.pdf" }];
  failItem = true;
  badge("K2").click();
  await settle();
  failItem = false;
  assert.equal(tabsN(), n2, "読めないのにタブが増えた");
  assert.match(S.app.toast, /詳細を読めません/, "読めないことを知らせない");

  // get_item を待つ間に別のタブへ移った = やめる（切替先の下に開かない）
  let release;
  holdItem = new Promise((r) => (release = r));
  badge("K2").click();
  await sleep(0);
  await S.openTab({ kind: "all" });
  await settle();
  const other = S.app.tab;
  assert.notEqual(other, list, "別のタブへ移れていない");
  release();
  holdItem = null;
  await settle();
  assert.deepEqual([S.app.tab, S.frontViewer(), tabsN()], [other, null, n2 + 1], "切替の後に PDF を開いた");
  await S.closeTab(other);
  await settle();

  // 処理中は開かない（ItemDetail の「PDF を読む」と同じ = openViewer の tabOpsAllowed）・開いているメモにも触らない
  S.app.noteEditor = { key: "K1", noteKey: null, text: "未保存", saved: "" };
  S.app.busy = true;
  badge("K2").click();
  await settle();
  S.app.busy = false;
  assert.deepEqual([S.frontViewer(), tabsN()], [null, n2], "処理中に開いた");
  assert.equal(S.app.noteEditor?.text, "未保存", "処理中にメモを保存して閉じた");
  S.app.noteEditor = null;
  await settle();

  // ボタンの上のキーは行へ効かない（Enter = 行の select・Space = チェックの反転にならない）
  const sel = S.app.selected;
  const checked = [...S.app.checked];
  key({ key: "Enter", code: "Enter" }, badge("K2"));
  await settle();
  assert.equal(S.app.selected, sel, "ボタンの上の Enter で行を選んだ");
  key({ key: " ", code: "Space" }, badge("K2"));
  await settle();
  assert.deepEqual([...S.app.checked], checked, "ボタンの上の Space でチェックが変わった");

  // ゴミ箱の文献でも開く（ItemDetail と同じ）
  trash.add("K2");
  badge("K2").click();
  await settle();
  assert.equal(S.frontViewer()?.itemKey, "K2", "ゴミ箱の文献が開かない");
  trash.clear();
  await S.closeTab(S.app.tab);
  await settle();
  tests++;
}

// 並びのメニュー: 件数の左・3 項目・選択中をもう一度押すと向きが反転・別の項目は自然な向き・ui_list_sort に覚える・検索中も・どのビューでも・タブには持たない
{
  const last = () => calls.filter((c) => c.name === "list_items").at(-1).args.filter;
  const sortOf = () => [last().sort, last().desc];
  const label = () => $(".lb-sort").textContent.replace(/\s+/g, " ").trim();
  const menu = () => [...document.querySelectorAll(".lb-sort-pop button")];
  const open = async () => {
    $(".lb-sort").click();
    await settle();
    return menu();
  };
  const pick = async (i) => {
    (await open())[i].click();
    await settle();
  };

  // 置き場: 件数の左（右端の開閉ボタンの位置は変えない）・ツールバーの最後の塊
  assert.equal($(".listbar").lastElementChild, $(".lb-tail"), "右の塊が最後でない");
  assert.ok($(".lb-tail .lb-sort"), "並びが右の塊の中にない");
  assert.deepEqual([...$(".lb-tail").children].map((e) => e.matches(".lb-wrap") ? "sort" : e.className.split(" ")[0]), ["sort", "lb-count", "lb-pane"], "件数の左にない");

  // 3 項目・選択中に印と矢印
  assert.equal(menu().length, 0, "開く前に出ている");
  const rowsMenu = await open();
  assert.deepEqual(rowsMenu.map((b) => b.textContent.trim()), ["追加日", "出版日↑", "誌名"], "項目が違う");
  assert.deepEqual(rowsMenu.map((b) => b.getAttribute("aria-checked")), ["false", "true", "false"], "選択中の印が違う");
  $(".lb-sort").click(); // もう一度押すと閉じる
  await settle();
  assert.equal(menu().length, 0, "ボタンで閉じない");

  // 選択中をもう一度 = 反転（既定の向きに関わらず）・閉じる・覚える
  await pick(1);
  assert.deepEqual([sortOf(), label(), prefs.ui_list_sort, S.app.listMenu], [["published", true], "↓ 出版日 ▾", "published:desc", null], "出版日の反転");
  await pick(1);
  assert.deepEqual([sortOf(), label(), prefs.ui_list_sort], [["published", false], "↑ 出版日 ▾", "published:asc"], "出版日の反転の反転");

  // 別の項目 = その項目の自然な向き（誌名 = A→Z・日付 = 新しい順）。今の向きを引き継がない
  await pick(2);
  assert.deepEqual([sortOf(), label(), prefs.ui_list_sort], [["journal", false], "↑ 誌名 ▾", "journal:asc"], "誌名の最初は昇順");
  await pick(2);
  assert.deepEqual([sortOf(), prefs.ui_list_sort], [["journal", true], "journal:desc"], "誌名の反転");
  await pick(0);
  assert.deepEqual([sortOf(), label(), prefs.ui_list_sort], [["added", true], "↓ 追加日 ▾", "added:desc"], "追加日の最初は降順");
  await pick(0);
  assert.deepEqual(sortOf(), ["added", false], "追加日の反転");
  await pick(1);
  assert.deepEqual(sortOf(), ["published", true], "出版日の最初は降順（昇順のあとでも）");

  // 並べ替えたら先頭から
  const rowsBox = $(".list > .rows");
  rowsBox.scrollTop = 120;
  assert.equal(rowsBox.scrollTop, 120, "試験の前提: jsdom が scrollTop を覚える");
  await pick(2);
  assert.equal(rowsBox.scrollTop, 0, "並べ替えの後に先頭へ戻らない");

  // 応答を待つ間に別のタブへ移った = 移った先のスクロールは動かさない
  {
    let release;
    holdList = new Promise((r) => (release = r));
    const from = S.app.tab;
    menu().length || (await open());
    menu()[2].click();
    await sleep(0);
    const moved = S.openTab({ kind: "all" });
    await sleep(0);
    await sleep(0);
    assert.notEqual(S.app.tab, from, "別のタブへ移れていない");
    $(".list > .rows").scrollTop = 77;
    release();
    holdList = null;
    await moved;
    await settle();
    assert.equal($(".list > .rows").scrollTop, 77, "別のタブのスクロールを先頭へ戻した");
    await S.closeTab(S.app.tab);
    await settle();
    await pick(2); // journal:desc → journal:asc へ戻す（下の節の前提）
  }

  // Esc・外のクリックで閉じる（並びは変えない）
  await open();
  key({ key: "Escape", code: "Escape" }, document.body);
  await settle();
  assert.deepEqual([menu().length, S.app.listSort.key], [0, "journal"], "Esc で閉じない");
  await open();
  document.body.click();
  await settle();
  assert.equal(menu().length, 0, "外のクリックで閉じない");
  await open();
  $(".lb-count").click(); // ツールバーの中でもメニューの外
  await settle();
  assert.equal(menu().length, 0, "件数のクリックで閉じない");
  await open();
  $(".row .fchip").click(); // 行のチップは stopPropagation = window には capture でしか届かない
  await settle();
  assert.deepEqual([menu().length, S.app.filters.author?.family], [0, "A"], "行のチップを押してもメニューが閉じない");
  await S.clearFilter("author");
  await settle();
  await open();
  S.app.modal = "citeStyle";
  await settle();
  assert.equal(menu().length, 0, "モーダルが開いても閉じない");
  S.app.modal = null;
  await settle();
  await open();
  const elsewhere = S.app.tab;
  await S.openTab({ kind: "all" });
  await settle();
  assert.equal(menu().length, 0, "タブの切替で閉じない");
  await S.closeTab(S.app.tab);
  await settle();
  assert.equal(S.app.tab, elsewhere);

  // 検索中も選んだ並び（本物の検索欄 → 250ms 後の doSearch）。ボタンは灰色にしない
  const box = $("#searchbox");
  box.value = "liver";
  box.dispatchEvent(new window.Event("input", { bubbles: true }));
  assert.ok(await waitFor(() => last().search === "liver"), "検索が走らない");
  assert.deepEqual(sortOf(), ["journal", false], "検索中に並びが外れた");
  assert.equal($(".lb-sort").disabled, false, "検索中に並びを選べない");
  await pick(1);
  assert.deepEqual([last().search, ...sortOf()], ["liver", "published", true], "検索中に選んだ並びが効かない");
  box.value = "";
  box.dispatchEvent(new window.Event("input", { bubbles: true }));
  assert.ok(await waitFor(() => last().search === null), "検索が消えない");

  // ビュー（フォルダ / ラベル / ★ / ゴミ箱 / すべて）を問わず同じ並び・ゴミ箱のツールバーにも出る
  for (const [view, field, want] of [
    [{ kind: "collection", id: 7 }, "collectionId", 7],
    [{ kind: "tag", id: 8 }, "tagId", 8],
    [{ kind: "starred" }, "starredOnly", true],
    [{ kind: "trash" }, "trashed", true],
    [{ kind: "all" }, "trashed", false],
  ]) {
    await S.setView(view);
    await settle();
    assert.deepEqual([last()[field], ...sortOf(), label()], [want, "published", true, "↓ 出版日 ▾"], `${view.kind} で並びが違う`);
  }

  // 全タブ共通: 新しいタブも同じ並び・ui_tabs に入れない
  await S.openTab({ kind: "all" });
  await settle();
  assert.deepEqual(sortOf(), ["published", true], "新しいタブで並びが違う");
  const made = S.app.tab;
  await pick(0);
  await sleep(350); // ui_tabs の保存（300ms 後）
  assert.ok(prefs.ui_tabs && !/sort|desc/i.test(prefs.ui_tabs), `ui_tabs に並びが入った: ${prefs.ui_tabs}`);
  await S.closeTab(made);
  await settle();
  assert.deepEqual([sortOf(), label()], [["added", true], "↓ 追加日 ▾"], "元のタブへ戻ると並びが違う");

  // en の表示（辞書の網羅は i18n.test.mjs）
  prefs.ui_lang = "en";
  await S.loadLangPref();
  await settle();
  assert.equal(label(), "↓ Date added ▾");
  assert.deepEqual((await open()).map((b) => b.textContent.trim()), ["Date added↓", "Date published", "Journal"]);
  assert.match($(".lb-sort").title, /^Sort order/);
  $(".lb-sort").click();
  prefs.ui_lang = "ja";
  await S.loadLangPref();
  await settle();
  tests++;
}

// ライブラリ移行の入口は Settings › 他の文献管理ソフトから移行…（Rust の event "migration-open"）。「＋ 新規 ▾」は 2 行
{
  assert.ok(!S.frontWiki() && !S.frontShelf() && !S.frontViewer(), "一覧のタブが前面でない");
  $(".newref").click();
  await settle();
  assert.deepEqual([...document.querySelectorAll(".nm-menu button span:first-child")].map((s) => s.textContent.trim()), ["手動で作成…", "PDF を取り込む…"], "「＋ 新規 ▾」の行が 2 つでない");
  $(".newref").click();
  await settle();
  assert.equal($(".nm-menu"), null);
  // 開かない間: モーダル・Wiki のダイアログ（modalOpen の guard）・参照エディタ・PDF 取込
  for (const [name, on, off, modal] of [
    ["モーダル", () => (S.app.modal = "citeStyle"), () => (S.app.modal = null), "citeStyle"],
    ["Wiki のダイアログ", () => (CH.chat.consent = true), () => (CH.chat.consent = false), null],
    ["参照エディタ", () => (S.app.editorOpen = true), () => (S.app.editorOpen = false), null],
    ["PDF 取込", () => (S.app.importJob = { files: [], running: true, cancel: false }), () => (S.app.importJob = null), null],
  ]) {
    on();
    await settle();
    emit("migration-open");
    await settle();
    assert.equal(S.app.modal, modal, `${name}の間に移行の窓が開いた`);
    assert.equal($(".mg-modal"), null, `${name}の間に移行の窓が出た`);
    off();
    await settle();
  }
  // 開く = 起動時に読んだ進行中の job を出す。メニューバーのクリックは DOM のクリックにならない = 開いていたメニュー
  // （「＋ 新規 ▾」・OpenNote の木 / 本棚の見出しの浮きメニュー）とフォーカスは openMigration / 受け口が片付ける
  $(".newref").click();
  W.wk.menu = { kind: "file", path: "a.md", x: 1, y: 1 };
  SH.sh.menu = { x: 1, y: 1 };
  $("#searchbox").focus();
  await settle();
  assert.ok(S.app.newMenu && document.activeElement === $("#searchbox"), "前提: メニューが開き検索欄にフォーカス");
  emit("migration-open");
  await settle();
  assert.equal(S.app.modal, "migration", "Settings の項目で移行の窓が開かない");
  assert.ok($(".mg-modal"), "移行の窓が出ない");
  assert.match($(".mg-modal [role=status]")?.textContent ?? "", /3 \/ 10/, "進行中の job が出ない");
  assert.deepEqual([S.app.newMenu, W.wk.menu, SH.sh.menu], [false, null, null], "開いていたメニューが移行の窓の上に残った");
  assert.notEqual(document.activeElement, $("#searchbox"), "フォーカスが裏の検索欄に残った");
  // バックグラウンドで続行 → 閉じる（job は動き続け、窓の部品は 600ms ごとに読み直す）
  const bg = [...document.querySelectorAll(".mg-modal .modal-foot button")].find((b) => b.textContent.trim() === "バックグラウンドで続行");
  assert.ok(bg, "バックグラウンドで続行 のボタンが無い");
  bg.click();
  await settle();
  assert.equal(S.app.modal, null);
  assert.equal($(".mg-modal"), null);
  migJob = { ...migJob, progress: { ...migJob.progress, completed: 7 } };
  assert.ok(await waitFor(() => calls.filter((c) => c.name === "migration_status").length > 1, 2000), "閉じた後に job を読み直さない");
  await sleep(700);
  // もう一度押す = 進行中の窓に戻る（最新の進み具合）
  emit("migration-open");
  await settle();
  assert.equal(S.app.modal, "migration", "バックグラウンドで続行の後に戻れない");
  assert.match($(".mg-modal [role=status]")?.textContent ?? "", /7 \/ 10/, "戻った窓が進行中の job を出さない");
  // 開いている間に押し直しても何も変えない
  emit("migration-open");
  await settle();
  assert.equal(S.app.modal, "migration");
  // 終わらせる（以後の節で読み直さない）
  migJob = { ...migJob, phase: "done", progress: null };
  await sleep(700);
  await settle();
  S.app.modal = null;
  await settle();
  tests++;
}

// OpenNote のタブ: ⌘⌥I は OpenNote の右ペインだけ（一覧の ui_detail を変えない）・main は wk-main-noinfo
{
  await S.openTab({ kind: "all" }, { path: null, root: "/v", scrollTop: 0, editing: false });
  await settle();
  assert.ok(S.frontWiki(), "OpenNote のタブが前面でない");
  const info = W.wk.infoOpen;
  const d = prefs.ui_detail;
  optI();
  await settle();
  assert.deepEqual([W.wk.infoOpen, S.app.detailOpen, prefs.ui_detail], [!info, true, d], "OpenNote で ⌘⌥I が一覧の右ペインを変えた");
  assert.equal($("main").className, info ? "wk-main-noinfo" : "", "OpenNote の 2 列にならない");
  tests++;
}

console.log(`app shell: ok (${tests} tests)`);
process.exit(0);
