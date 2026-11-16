/* App の組立ての試験（node app/src/lib/app.test.mjs・実部品 App.svelte + jsdom・invoke は差し替え）。部品ごとの試験（chat/chat.test.mjs の柱 ChatDock）が
   見ない配線 = main の 2 列・{#if app.detailOpen}・ListToolbar のボタンと並びのメニュー・App の keydown（⌘⌥I / ⌘⌥C / Esc）・チャットの柱の門と置き場
   （.tabs-page の外・タブの種類を切り替えても作り直さない）・reveal の手応え・行の「PDF」ボタン・main.ts の読む順序を、本物の App を通して確かめる。
   PdfViewer と ShelfViewer だけ偽物（pdf.js は jsdom で動かない）。土台は wiki/test-harness.mjs */
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { setupHarness } from "./wiki/test-harness.mjs";
import { fakeInvoke } from "./test-invoke.mjs";

const { calls, emit, sleep, waitFor } = setupHarness({
  fakes: {
    "@tauri-apps/api/webview": "export const getCurrentWebview = () => ({ onDragDropEvent: async (f) => { globalThis.__drop = f; return () => {}; } });",
    "@tauri-apps/plugin-dialog": "export const open = async (o) => { (globalThis.__dialogs ??= []).push(o ?? null); return null; }; export const save = async () => null; export const confirm = async () => false; export const message = async () => {};",
    // App の中の PdfViewer / ShelfViewer は偽物 = 作った・外したを globalThis.__viewerLife に書くだけ（作り直さない節）
    "./lib/PdfViewer.svelte":
      "import { onMount } from 'svelte'; export default function PdfViewer(_a, p) { const id = 'pdf' + p.viewer.attachmentId; (globalThis.__viewerLife ??= []).push('+' + id); onMount(() => () => globalThis.__viewerLife.push('-' + id)); }",
    "./lib/shelf/ShelfViewer.svelte":
      "import { onMount } from 'svelte'; export default function ShelfViewer(_a, p) { const id = 'book:' + p.book.path; (globalThis.__viewerLife ??= []).push('+' + id); onMount(() => () => globalThis.__viewerLife.push('-' + id)); }",
    // 覆いの下のキーの節だけ、実部品の PdfViewer を App の外に mount する（pdf.js は偽物 = viewer/side.test と同じ）
    "pdfjs-dist": "export const GlobalWorkerOptions = {}; export const getDocument = (o) => globalThis.__pdf(o); export class TextLayer {} export class AnnotationLayer {}",
    "pdfjs-dist/build/pdf.worker.mjs?url": "export default 'worker.js';",
    "pdfjs-dist/web/images/annotation-note.svg?url": "export default 'note.svg';",
    "pdfjs-dist/web/pdf_viewer.css": "export default '';",
  },
});
globalThis.__BUILD_ID__ = "test"; // vite の define（実部品の PdfViewer が読む）
globalThis.ResizeObserver ??= class {
  observe() {}
  disconnect() {}
};
globalThis.IntersectionObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
// 2 ページの文書（寸法だけ・テキスト層なし・注釈なし）
const pdfPage = {
  rotate: 0,
  getViewport: ({ scale }) => ({ width: 600 * scale, height: 800 * scale, clone() { return this; } }),
  getAnnotations: async () => [],
  streamTextContent: () => new ReadableStream({ start(c) { c.enqueue({ items: [], styles: {} }); c.close(); } }),
  getTextContent: async () => ({ items: [], styles: {} }),
  cleanup() {},
};
globalThis.__pdf = () => ({ promise: Promise.resolve({ numPages: 2, getPage: async () => pdfPage, getOutline: async () => null, destroy: async () => {} }), destroy: async () => {} });
let tests = 0;

// 蔵書 3 件・OpenNote とチャットは最初 OFF（下で ON）。App の起動に要る読みの既定は test-invoke.mjs（ここには場面の値だけ）
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
let failSetPrefKey = null; // この鍵の set_ui_pref を失敗させる（表示設定の読み書きの列の節）
let wikiOn = false;
let importGate = null; // Promise = import_pdf の応答を止める（取込の窓が開いている間のドロップの節）
let chatOn = false;
// ライブラリ移行の job（MigrationModal は App と一緒に mount され、起動時に 1 度読み、進行中なら 600ms ごとに読み直す）。
// 「バックグラウンドで続行」して閉じた後に Settings の項目で戻る節のため、最初から進行中の job を返す
let shelfKids = []; // 本棚の木のサブフォルダ（パンくずの場面 = フォルダ名はデータ）
let qaGate = null; // Promise = quick_add の応答を止める（登録中）
let qaFail = null; // quick_add が投げる文（null = 登録できる）
let migJob = { id: "J1", phase: "running", progress: { phase: "copy", completed: 3, total: 10, bytes: 0 }, config: null, plan: null, report: null, error: null };
// 場面の応答だけを上書き（ほかは test-invoke.mjs の既定 = 本物と同じ形の空。表に無い command は投げ、最後の inv.done() で落ちる）
const inv = fakeInvoke(calls, {
  get_ui_pref: async (a) => {
    if (a.key === failPrefKey) throw new Error("読めない");
    const v = prefs[a.key] ?? null; // 頼まれた時点の値（止めている間に書かれた値を返さない）
    if (a.key === "ui_chat_dock") await dockGate;
    return v;
  },
  set_ui_pref: (a) => {
    if (a.key === failSetPrefKey) throw new Error("書けない");
    prefs[a.key] = a.value;
    return null;
  },
  list_items: async () => {
    await holdList;
    return items;
  },
  get_item: async (a) => {
    if (failItem) throw new Error("読めない");
    await holdItem;
    return detail(a.key);
  },
  wiki_chat_status: () => ({ enabled: chatOn, cli: "/c", version: "1", logged_in: true, mcp: "/m", state: "off", session_id: null }),
  wiki_settings: () => ({ enabled: wikiOn, root: "/v" }),
  wiki_status: () => ({ enabled: wikiOn, root: "/v" }),
  migration_status: () => migJob,
  // PDF の取込（ドロップの節）= 止めておける・頼まれたパスを返す
  import_pdf: async (a) => {
    await importGate;
    return { status: "created", key: "K1", message: a.path };
  },
  // 識別子で登録（「＋ 追加 ▾」）= 一覧にある K1 を返す（reveal が一覧の行を選べる）。失敗は Rust の文の形
  quick_add: async () => {
    await qaGate;
    if (qaFail) throw qaFail;
    return { ...items[0], warnings: [] };
  },
  // 本棚のタブ（柱がタブの種類を切り替えても作り直されないことを見る節）= 本 1 冊の棚
  shelf_status: () => ({ shelves: [{ root: "/books", name: "books", last_used: 1, exists: true, collapse_single: false, issue_titles: false, aliases: {} }] }),
  shelf_tree: (a) => ({ root: a.root, dir: null, tree: { id: "f-/books/", name: "books", path: "/books", books: [{ id: "b1", title: "本 1", path: "/books/b1.pdf", size: 1, mtime: 0, folder: "books" }], children: shelfKids, totalBooks: 1 }, unreadable: 0, used_find: false, thumbs_dir: "/thumbs" }),
});
globalThis.__invoke = inv.invoke;

const S = await import("./state.svelte.ts");
const W = await import("./wiki/wiki.svelte.ts");
const CH = await import("./chat/chat.svelte.ts");
const V = await import("./viewing.svelte.ts");
const SH = await import("./shelf/shelf.svelte.ts");
const ED = await import("./wiki/edit.svelte.ts");
// 表示設定の読み書き（prefs.svelte.ts にある）。F = state と prefs を合わせた入口（pickListSort は state・ほかは prefs）
const PR = await import("./prefs.svelte.ts");
const F = { ...S, ...PR };
const I18N = await import("./i18n.svelte.ts");
const { mount, unmount, flushSync } = await import("svelte");
const { default: App } = await import("../App.svelte");
const { default: RealPdfViewer } = await import("./PdfViewer.svelte"); // App が読む "./lib/PdfViewer.svelte" は偽物・この指定は実部品

const key = (o, el = window) => el.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...o }));
const optI = (el) => key({ metaKey: true, altKey: true, code: "KeyI", key: "ˆ" }, el); // ⌥I は配列により dead key = e.code で見る
/** ⌘⌥C（⌥C は配列により ç 等 = e.code で見る）。戻り値 = 送った event（defaultPrevented を見る） */
const optC = (el = window, o = {}) => {
  const ev = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, metaKey: true, altKey: true, code: "KeyC", key: "ç", ...o });
  el.dispatchEvent(ev);
  return ev;
};
const $ = (q) => document.querySelector(q);
// main の列の配置の class（左の列の幅の印 sbw = sidebarw.test が見る = ここでは除く）
const mainCls = () => [...$("main").classList].filter((c) => c !== "sbw").join(" ");
const rows = () => [...document.querySelectorAll(".row")];
const settle = async () => {
  flushSync();
  await sleep(10);
  flushSync();
};

// main.ts（と一覧の駆動用の list-main.ts）は ui_detail・ui_list_sort・ui_viewer_side を mount の前に読む（mount の後に閉じると右ペイン / PDF の右バーが出て消える・既定の並びで 1 度取ってしまう）
for (const f of ["../main.ts", "../../harness/list-main.ts"]) {
  if (f.includes("harness") && !existsSync(new URL(f, import.meta.url))) continue; // list-main.ts は同梱しないことがある = 無ければ飛ばす
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
assert.equal(mainCls(), "lib-nodetail", "ui_detail = 0 で 2 列にならない");
assert.ok(await waitFor(() => rows().length === 3), "一覧が出ない");
await settle();
assert.equal($("header .pn-toggle").getAttribute("aria-pressed"), "false");
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
assert.deepEqual([mainCls(), prefs.ui_detail], ["", "1"], "⌘⌥I で 3 列に戻らない");
key({ metaKey: true, altKey: true, code: "KeyI", key: "ˆ", repeat: true });
await settle();
assert.ok($("main > aside.detail"), "キーリピートで閉じた");
$("header .pn-toggle").click();
await settle();
assert.deepEqual([$("aside.detail"), mainCls(), prefs.ui_detail], [null, "lib-nodetail", "0"], "ボタンで閉じない");
assert.equal($("header .pn-toggle").getAttribute("aria-pressed"), "false");
assert.ok($("header .pn-toggle").classList.contains("pn-off"), "閉じた絵にならない");
tests++;

// 閉じたまま行を選ぶ = 開かない・詳細は読む（開くと出る）
rows()[1].click();
await settle();
assert.deepEqual([S.app.selected, S.app.detail?.key, $("aside.detail")], ["K2", "K2", null], "行の選択で開いた / 詳細を読まない");
$("header .pn-toggle").click();
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
  assert.deepEqual([$(".tabs-chat"), $(".cd")], [null, null], "門が閉じているのに痕跡が残った");
  // 最上段のセグメントは残る = NOTE だけが無い
  assert.deepEqual([...document.querySelectorAll(".mode-seg-btn")].map((b) => b.textContent), ["LIBRARY", "SHELF"], "OpenNote OFF のセグメントに NOTE が出た");
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

// 柱を開いたまま Settings でチャットを OFF（Rust の wiki-chat {enabled: false}）= 柱は消え ⌘⌥C は素通り・開閉の記憶は残る・ボタンは薄い絵で残る
// （OpenNote が有効ならボタンは常に出す。押すと有効にする説明 = chat.test）→ ON で戻る
{
  chatOn = false;
  emit("wiki-chat", { type: "enabled", enabled: false });
  await settle();
  assert.deepEqual([$(".cd"), CH.chat.dock.open], [null, true], "チャット OFF で柱が残った / 開閉の記憶を消した");
  assert.deepEqual([!!$(".tabs-chat"), $(".tabs-chat")?.classList.contains("tabs-chat-off"), $(".tabs-chat")?.getAttribute("aria-pressed")], [true, true, "false"], "チャット OFF でボタンが消えた / 無効の絵でない");
  const ev = optC();
  await settle();
  assert.deepEqual([ev.defaultPrevented, CH.chat.dock.open], [false, true], "チャット OFF で ⌘⌥C を奪った");
  chatOn = true;
  emit("wiki-chat", { type: "enabled", enabled: true });
  assert.ok(await waitFor(() => (flushSync(), !!$(".cd .wk-ch") && !!$(".tabs-chat") && !$(".tabs-chat-off"))), "チャット ON で柱とボタン（有効の絵）が戻らない");
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
  F.showViewerSide();
  await settle();
  assert.equal(prefs.ui_viewer_side, undefined, "開いているのに右バーの開閉を書いた");
  S.app.viewerSideOpen = false;
  F.showViewerSide();
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

  // 置き場: 件数の左・ツールバーの最後の塊（右ペインの開閉 ▣ はアプリのヘッダーの右端にある = ツールバーには無い）
  assert.equal($(".listbar").lastElementChild, $(".lb-tail"), "右の塊が最後でない");
  assert.ok($(".lb-tail .lb-sort"), "並びが右の塊の中にない");
  assert.deepEqual([...$(".lb-tail").children].map((e) => e.matches(".lb-wrap") ? "sort" : e.className.split(" ")[0]), ["sort", "lb-count"], "件数の左にない / ▣ がツールバーに残った");

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

// 「＋ 追加 ▾」: ヘッダーの入力欄は検索の 1 つだけ。開くと最上段が識別子の欄（焦点が入る）・その下に PDF を取り込む… / 手動で作成…
{
  assert.ok(!S.frontWiki() && !S.frontShelf() && !S.frontViewer(), "一覧のタブが前面でない");
  assert.deepEqual([...document.querySelectorAll("header input")].map((i) => i.id), ["searchbox"], "閉じている間のヘッダーの入力欄が検索の 1 つでない");
  assert.equal($(".newref").textContent.replace(/\s+/g, " ").trim(), "＋ 追加 ▾");
  assert.equal($(".newref").getAttribute("aria-expanded"), "false");
  $(".newref").click();
  await settle();
  assert.deepEqual([...$(".nm-menu").children].map((c) => c.id || c.className || c.tagName), ["quickadd", "nm-sep", "BUTTON", "BUTTON"], "開いた形が 識別子の欄 → 区切り → 2 項目 でない");
  assert.deepEqual([...document.querySelectorAll(".nm-menu button span:first-child")].map((s) => s.textContent.trim()), ["PDF を取り込む…", "手動で作成…"], "「＋ 追加 ▾」の項目");
  assert.equal(document.activeElement, $("#quickadd"), "開いても識別子の欄に焦点が入らない");
  assert.equal($(".newref").getAttribute("aria-expanded"), "true");
  $(".newref").click();
  await settle();
  assert.equal($(".nm-menu"), null);
  tests++;
}

// ⌘N = 「＋ 追加 ▾」を開いて識別子の欄へ（手癖 = 押す → 貼る → Enter）。Enter で登録 1 回・閉じる・欄は空・焦点は開く前へ /
// IME の確定の Enter・空の Enter は登録しない（閉じない）/ 登録中はヘッダーのスピナー / 失敗は toast / Esc・外側のクリックで閉じる /
// 開いている間の ⌘N は欄へ焦点を戻すだけ / 覆い（窓・参照エディタ・取込）の下では開かない / ⌘⇧N は今までどおり / 2 項目は今までと同じ窓
{
  const qa = () => $("#quickadd");
  const cmdN = () => key({ metaKey: true, key: "n" }, document.body);
  const adds = () => calls.filter((c) => c.name === "quick_add");
  const type = (v) => {
    qa().value = v;
    qa().dispatchEvent(new Event("input", { bubbles: true }));
  };
  const enter = (o = {}) => qa().dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true, ...o }));
  $("#searchbox").focus();
  cmdN();
  await settle();
  assert.deepEqual([S.app.newMenu, document.activeElement?.id], [true, "quickadd"], "⌘N で開いて識別子の欄に焦点が入らない");
  const n0 = adds().length;
  type("10.1000/xyz");
  enter({ isComposing: true });
  enter({ keyCode: 229 });
  await settle();
  assert.deepEqual([adds().length, S.app.newMenu, qa()?.value], [n0, true, "10.1000/xyz"], "IME の確定の Enter で登録した / 閉じた / 欄を消した");
  // 登録中 = ヘッダーのスピナー（欄は閉じる）・済んだら toast
  let release;
  qaGate = new Promise((r) => (release = r));
  enter();
  await settle();
  assert.equal(adds().length, n0 + 1, "Enter で登録しない");
  assert.deepEqual(adds().at(-1).args, { input: "10.1000/xyz" });
  assert.deepEqual([S.app.newMenu, !!$("header .spinner")], [false, true], "登録中に欄が閉じない / スピナーが無い");
  assert.equal(document.activeElement, $("#searchbox"), "閉じた後に焦点が開く前（検索欄）へ戻らない");
  release();
  qaGate = null;
  await waitFor(() => !S.app.busy);
  await settle();
  assert.match(S.app.toast, /^✓ 登録/, `登録の結果の toast: ${S.app.toast}`);
  assert.equal(adds().length, n0 + 1, "登録が 2 回");
  // もう一度 ⌘N = 欄は空・空の Enter は登録しない
  cmdN();
  await settle();
  assert.equal(qa().value, "", "登録した後も欄に文字が残った");
  enter();
  await settle();
  assert.deepEqual([adds().length, S.app.newMenu], [n0 + 1, true], "空の Enter で登録した / 閉じた");
  // 失敗 = toast（Rust の文）
  qaFail = "入力を解釈できません: abc";
  type("abc");
  enter();
  await waitFor(() => !S.app.busy);
  await settle();
  assert.match(S.app.toast, /^✗ 入力を解釈できません: abc/, `失敗の toast: ${S.app.toast}`);
  qaFail = null;
  // Esc = 閉じる・焦点は開く前へ（体 = body から開いた = どこへも移さない）・欄の文字は持ったまま
  document.activeElement?.blur?.();
  cmdN();
  await settle();
  type("PMID:123");
  key({ key: "Escape" }, qa());
  await settle();
  assert.equal(S.app.newMenu, false, "Esc で閉じない");
  cmdN();
  await settle();
  assert.equal(qa().value, "PMID:123", "閉じて開き直すと貼りかけが消えた");
  // 開いている間の ⌘N = 焦点を欄へ戻すだけ（閉じない）
  $("#searchbox").focus();
  cmdN();
  await settle();
  assert.deepEqual([S.app.newMenu, document.activeElement?.id], [true, "quickadd"], "開いている間の ⌘N で欄へ焦点が戻らない");
  // 外側のクリック = 閉じる（欄の中のクリックは閉じない）
  qa().click();
  await settle();
  assert.equal(S.app.newMenu, true, "欄の中のクリックで閉じた");
  $("main").click();
  await settle();
  assert.equal(S.app.newMenu, false, "外側のクリックで閉じない");
  // 覆いの下では開かない
  for (const [label, on, off] of [
    ["窓", () => (S.app.modal = "citeStyle"), () => (S.app.modal = null)],
    ["参照エディタ", () => (S.app.editorOpen = true), () => (S.app.editorOpen = false)],
    ["取込", () => (S.app.importJob = { files: [], running: true, cancel: false }), () => (S.app.importJob = null)],
  ]) {
    on();
    await settle();
    cmdN();
    await settle();
    assert.equal(S.app.newMenu, false, `${label}の下で ⌘N が「＋ 追加」を開いた`);
    off();
    await settle();
  }
  // 登録中: ⌘N で開いた欄は無効にしない = 焦点が入り、貼った物は欄に残る（検索欄に落ちない）・登録中の Enter は何もしない
  // （貼った物を消さない・欄の横にスピナー）・前の登録が済んでも欄を閉じない / 貼りかけを消さない・済んだら Enter で 1 回だけ登録
  {
    let release;
    const searches = () => calls.filter((c) => c.name === "list_items" && c.args?.filter?.search).length;
    const searches0 = searches();
    qaGate = new Promise((r) => (release = r));
    $("#searchbox").focus();
    cmdN();
    await settle();
    type("10.1000/one");
    enter();
    await settle();
    const n1 = adds().length;
    assert.ok(S.app.busy, "前提: 1 件目の登録中");
    cmdN();
    await settle();
    assert.deepEqual([S.app.newMenu, document.activeElement?.id, qa().disabled], [true, "quickadd", false], "登録中に ⌘N で開いた欄に焦点が入らない / 無効");
    type("10.1000/two");
    enter();
    await settle();
    assert.deepEqual([adds().length, qa().value, S.app.newMenu, $("#searchbox").value], [n1, "10.1000/two", true, ""], "登録中の Enter で登録した / 貼った物を消した / 閉じた / 検索欄に落ちた");
    assert.ok($(".nm-menu .spinner"), "登録中であることが欄の近くで分からない");
    release();
    qaGate = null;
    await waitFor(() => !S.app.busy);
    await settle();
    assert.deepEqual([S.app.newMenu, qa()?.value, document.activeElement?.id, $(".nm-menu .spinner")], [true, "10.1000/two", "quickadd", null], "1 件目が済んだら欄を閉じた / 貼りかけを消した / 焦点を奪った");
    enter();
    await waitFor(() => !S.app.busy);
    await settle();
    assert.equal(adds().length, n1 + 1, "済んだ後の Enter で登録しない / 二重に登録した");
    assert.deepEqual(adds().at(-1).args, { input: "10.1000/two" });
    assert.equal(searches(), searches0, "識別子で検索が走った");
  }
  // 覆いが出たら閉じる（⌘⇧N・ドロップの取込・メニューバーの項目の窓の下に開いたまま残った）
  for (const [label, on, off] of [
    ["⌘⇧N の参照エディタ", () => key({ metaKey: true, shiftKey: true, key: "N" }, qa()), () => S.closeEditor()],
    ["取込", () => (S.app.importJob = { files: [], running: true, cancel: false }), () => (S.app.importJob = null)],
    ["メニューバーの窓", () => emit("migration-open"), () => (S.app.modal = null)],
  ]) {
    cmdN();
    await settle();
    assert.equal(S.app.newMenu, true);
    on();
    await settle();
    assert.equal(S.app.newMenu, false, `${label}が出ても「＋ 追加」が閉じない`);
    assert.notEqual(document.activeElement?.id, "quickadd", `${label}の下の識別子の欄に焦点が残った`);
    off();
    await settle();
  }
  // ほかのメニューと同時に開かない: 開くとほかを閉じる / ほかが開くと閉じる
  for (const [label, open, isOpen] of [
    ["左バーの …", () => (S.app.labelMenu = "folders"), () => S.app.labelMenu === "folders"],
    ["フォルダの行のメニュー", () => (S.app.folderMenu = { id: 1, top: 1, left: 1 }), () => !!S.app.folderMenu],
    ["一覧のツールバー", () => (S.app.listMenu = "cite"), () => !!S.app.listMenu],
  ]) {
    open();
    await settle();
    cmdN();
    await settle();
    assert.deepEqual([S.app.newMenu, isOpen()], [true, false], `「＋ 追加」を開いても${label}が閉じない`);
    open();
    await settle();
    assert.deepEqual([S.app.newMenu, isOpen()], [false, true], `${label}を開いても「＋ 追加」が閉じない`);
    S.app.labelMenu = false;
    S.app.folderMenu = null;
    S.app.listMenu = null;
    await settle();
  }
  // ⌘⇧N = 手動で作成（今までどおり）
  key({ metaKey: true, shiftKey: true, key: "N" }, document.body);
  await settle();
  assert.equal(S.app.editorOpen, true, "⌘⇧N で参照エディタが開かない");
  S.closeEditor();
  await settle();
  // 2 項目 = 今までと同じ窓（PDF のファイルの選択・参照エディタ）
  $(".newref").click();
  await settle();
  const dialogs = (globalThis.__dialogs ?? []).length;
  [...document.querySelectorAll(".nm-menu button")].find((b) => b.textContent.includes("PDF を取り込む…")).click();
  await settle();
  assert.deepEqual([S.app.newMenu, (globalThis.__dialogs ?? []).length, globalThis.__dialogs?.at(-1)?.filters?.[0]?.extensions], [false, dialogs + 1, ["pdf"]], "PDF を取り込む… がファイルの選択を開かない / 閉じない");
  $(".newref").click();
  await settle();
  [...document.querySelectorAll(".nm-menu button")].find((b) => b.textContent.includes("手動で作成…")).click();
  await settle();
  assert.deepEqual([S.app.newMenu, S.app.editorOpen], [false, true], "手動で作成… が参照エディタを開かない / 閉じない");
  S.closeEditor();
  await settle();
  tests++;
}

// ライブラリ移行の入口は Settings › 他の文献管理ソフトから移行…（Rust の event "migration-open"）
{
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
  // （「＋ 追加 ▾」・OpenNote の木 / 本棚の見出しの浮きメニュー）とフォーカスは openMigration / 受け口が片付ける
  $(".newref").click();
  W.wk.menu = { kind: "file", path: "a.md", x: 1, y: 1 };
  SH.sh.menu = { x: 1, y: 1 };
  await settle();
  $("#searchbox").focus(); // 開いた「＋ 追加」は識別子の欄へ焦点を入れる = その後で裏の検索欄へ移す
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

// PDF のタブのビューア・本のタブの読み手は、そのタブが初めて前面になったときに作り、切り替えても作り直さず（前面でなくても生かす）、
// タブを閉じる（中身を外す）と外す。前面にしていないタブ（起動時に戻した物）は作らない
{
  const life = (globalThis.__viewerLife = []);
  const list = S.app.tab;
  const base = S.app.tabs.find((t) => t.id === list);
  const blank = { ...base, wiki: null, viewer: null, shelf: null, shelfBook: null };
  S.app.tabs.push({ ...blank, id: 9700, viewer: { itemKey: "K1", attachmentId: 71, title: "P" } });
  S.app.tabs.push({ ...blank, id: 9701, shelfBook: { root: "/books", id: "b1", path: "/books/b1.pdf", title: "本 1", size: 1, mtime: 0 } });
  S.app.tabs.push({ ...blank, id: 9702, viewer: { itemKey: "K2", attachmentId: 72, title: "Q" } });
  await settle();
  assert.deepEqual(life, [], "前面にしていない PDF / 本のタブで作った");
  for (const [to, want] of [
    [9700, ["+pdf71"]],
    [9701, ["+pdf71", "+book:/books/b1.pdf"]],
    [list, ["+pdf71", "+book:/books/b1.pdf"]],
    [9700, ["+pdf71", "+book:/books/b1.pdf"]],
    [9701, ["+pdf71", "+book:/books/b1.pdf"]],
    [list, ["+pdf71", "+book:/books/b1.pdf"]],
  ]) {
    S.app.tab = to;
    await settle();
    assert.deepEqual(life, want, `タブ ${to} へ切り替えて作り直した / 外した / 作らない`);
  }
  // 中身を外す（最後の 1 枚の PDF を閉じる = closeTab の only.viewer = null の型）= 外す。前面でない間でも
  S.app.tabs.find((t) => t.id === 9700).viewer = null;
  await settle();
  assert.deepEqual(life.slice(2), ["-pdf71"], "PDF を外したタブのビューアを外さない");
  // タブを閉じる = 外す
  S.app.tabs.splice(S.app.tabs.findIndex((t) => t.id === 9701), 1);
  await settle();
  assert.deepEqual(life.slice(3), ["-book:/books/b1.pdf"], "閉じた本のタブの読み手を外さない");
  S.app.tabs.splice(S.app.tabs.findIndex((t) => t.id === 9700), 1);
  S.app.tabs.splice(S.app.tabs.findIndex((t) => t.id === 9702), 1);
  await settle();
  assert.deepEqual(life.length, 4, "前面にしなかった PDF のタブで作った / 外した");
  tests++;
}

// 前面のタブの種類ごとの出し分け（実装を置き換えても同じ = 表は置き換えの前のコードで採った）: ヘッダー・main の列・左の列の幅・
// ⌘N（QuickAdd へ）・⌘A（全部チェック）・`/`（どの検索欄へ）・Esc（チェックを外す）・PDF のドラッグ（取込の枠）・frontKind()
{
  const list = S.app.tab;
  const base = S.app.tabs.find((t) => t.id === list);
  const blank = { ...base, wiki: null, viewer: null, shelf: null, shelfBook: null };
  const row = async () => {
    const m = $("main");
    const out = {
      brand: $(".brand")?.textContent,
      header: ["searchbox", "quickadd", "sh-search", "wk-search"].filter((id) => document.getElementById(id)),
      main: [...m.classList].join(" "),
      sbw: m.style.getPropertyValue("--sb-w"),
      kids: [...m.children].map((c) => c.tagName.toLowerCase() + (c.classList[0] ? "." + c.classList[0] : "")),
    };
    document.activeElement?.blur?.();
    key({ metaKey: true, key: "n" }, document.body);
    await settle();
    out.cmdN = document.activeElement?.id || null;
    S.app.newMenu = false; // ⌘N は「＋ 追加 ▾」を開く = 次の Esc がメニューを閉じる側へ行かないよう閉じる
    await settle();
    document.activeElement?.blur?.();
    S.app.checked.clear();
    key({ metaKey: true, key: "a" }, document.body);
    await settle();
    out.cmdA = S.app.checked.size;
    S.app.checked.add("K1");
    key({ key: "Escape", code: "Escape" }, document.body);
    await settle();
    out.esc = S.app.checked.size;
    S.app.checked.clear();
    key({ key: "/" }, document.body);
    await settle();
    out.slash = document.activeElement?.id || null;
    document.activeElement?.blur?.();
    globalThis.__drop({ payload: { type: "enter", paths: [] } });
    out.drop = S.app.dropActive;
    globalThis.__drop({ payload: { type: "leave", paths: [] } });
    await settle();
    return out;
  };
  const kinds = [
    ["list", null],
    ["viewer", { viewer: { itemKey: "K1", attachmentId: 81, title: "P" } }],
    ["wiki", { wiki: { path: null, root: "/v", scrollTop: 0, editing: false } }],
    ["shelf", { shelf: { root: "/books", dir: null, query: "", sort: "name", scrollTop: 0 } }],
    ["book", { shelfBook: { root: "/books", id: "b1", path: "/books/b1.pdf", title: "本 1", size: 1, mtime: 0 } }],
  ];
  // 置き換えの前のコードで採った表（KINDS_DUMP=1 で今の表を出す）。ヘッダーの識別子の欄（quickadd）は
  // 「＋ 追加 ▾」の中へ移った = 閉じている間のヘッダーは検索欄だけ（⌘N の後の焦点は今までどおり quickadd）。本棚にも左バーの幅（sbw・つまみ）
  const K1_TABLE = {
    list: { brand: "OpenRef", header: ["searchbox"], main: "sbw", sbw: "230px", kids: ["nav.ls-nav", "section.list", "aside.detail", "div.sbw-grip"], cmdN: "quickadd", cmdA: 3, esc: 0, slash: "searchbox", drop: true },
    viewer: { brand: "OpenRef", header: ["searchbox"], main: "sbw", sbw: "230px", kids: ["nav.ls-nav", "section.list", "aside.detail", "div.sbw-grip"], cmdN: null, cmdA: 0, esc: 1, slash: null, drop: true },
    wiki: { brand: "OpenNote", header: ["wk-search"], main: "sbw", sbw: "230px", kids: ["nav.wk-sidebar", "section.wk-page", "aside.wk-info", "div.sbw-grip"], cmdN: null, cmdA: 0, esc: 1, slash: "wk-search", drop: false },
    shelf: { brand: "OpenRef", header: ["sh-search"], main: "sbw", sbw: "230px", kids: ["nav.ls-nav", "section.list", "aside.sh-right", "div.sbw-grip"], cmdN: null, cmdA: 0, esc: 1, slash: "sh-search", drop: false },
    book: { brand: "OpenRef", header: ["searchbox"], main: "", sbw: "", kids: [], cmdN: null, cmdA: 0, esc: 1, slash: null, drop: false },
  };
  const got = {};
  for (const [label, over] of kinds) {
    if (over) {
      S.app.tabs.push({ ...blank, id: 9800, ...over });
      S.app.tab = 9800;
    }
    await settle();
    await waitFor(() => (flushSync(), label !== "wiki" || !!document.getElementById("wk-search")));
    got[label] = await row();
    if (typeof S.frontKind === "function") assert.equal(S.frontKind(), label, `frontKind が ${label} でない`);
    if (over) {
      S.app.tab = list;
      S.app.tabs.splice(S.app.tabs.findIndex((t) => t.id === 9800), 1);
      await settle();
    }
  }
  if (process.env.KINDS_DUMP) console.log(JSON.stringify(got));
  else assert.deepEqual(got, K1_TABLE, "前面のタブの種類ごとの出し分けが置き換えの前と違う");
  tests++;
}

// ▣（.pn-toggle）は既定で右端へ押す（margin-left: auto）= 同じ段に右へ押す物がほかにあると余りを半分ずつ取り、
// その物が真ん中へ寄る（OpenNote の [＋ 新規] が 289px 動いた）。jsdom は配置を計らない = 字面で守る: 右へ押す物を持つヘッダー
// （一覧 = .nm-wrap・OpenNote = WikiHeader の margin-left: auto の規則）では ▣ の margin-left を 0 に戻す規則が app.css にある
// 限界: 字面の表明 = 同じ見た目になる別の書き方（選択子・値の書き方を変える）でも落ち、打ち消しを残したまま後ろの規則で上書きされると通る
// （配置そのものは headless の計測 = ボタンの rect を main と比べる手順で確かめる物）
{
  const { readFileSync } = await import("node:fs");
  const css = readFileSync(new URL("../app.css", import.meta.url), "utf8").replace(/\/\*[^]*?\*\//g, "");
  const wikiHead = readFileSync(new URL("./wiki/WikiHeader.svelte", import.meta.url), "utf8");
  const base = /(^|\n)\.pn-toggle \{([^}]*)\}/.exec(css)?.[2] ?? "";
  assert.match(base, /margin-left: auto/, "前提: ▣ は既定で右端へ押す（本棚のヘッダー）");
  const zero = [...css.matchAll(/([^{}]+)\{([^}]*)\}/g)].filter((m) => /margin-left: 0/.test(m[2])).map((m) => m[1]);
  const reset = (sel) => zero.some((s) => s.split(",").some((x) => x.trim() === sel));
  assert.ok(/\.nm-wrap \{[^}]*margin-left: auto/.test(css) && reset(".nm-wrap ~ .pn-toggle"), "一覧のヘッダー:「＋ 追加」と ▣ が両方右へ押す（二重の auto）");
  assert.ok(/margin-left: auto/.test(wikiHead.split("<style>")[1] ?? "") ? reset(".wk-head .pn-toggle") : true, "OpenNote のヘッダー: WikiHeader の右へ押す物と ▣ が両方右へ押す（二重の auto）");
  tests++;
}

// 右ペインの開閉（▣）= 一覧・OpenNote・本棚で同じ場所（タブ列の下の最初の段 = アプリのヘッダーの右端）・同じ部品（PaneToggle）・
// 同じ title / aria-pressed・押すと開閉して保存（一覧と本棚 = ui_detail・OpenNote = ui_wiki_info）。一覧・本棚のツールバーには無い
{
  const list = S.app.tab;
  const base = S.app.tabs.find((t) => t.id === list);
  const blank = { ...base, wiki: null, viewer: null, shelf: null, shelfBook: null };
  for (const [label, over, isOpen, key] of [
    ["一覧", null, () => S.app.detailOpen, "ui_detail"],
    ["OpenNote", { wiki: { path: null, root: "/v", scrollTop: 0, editing: false } }, () => W.wk.infoOpen, "ui_wiki_info"],
    ["本棚", { shelf: { root: "/books", dir: null, query: "", sort: "name", scrollTop: 0 } }, () => S.app.detailOpen, "ui_detail"],
  ]) {
    if (over) {
      S.app.tabs.push({ ...blank, id: 9805, ...over });
      S.app.tab = 9805;
    }
    await settle();
    await waitFor(() => (flushSync(), !!$("header .pn-toggle")));
    const all = [...document.querySelectorAll(".pn-toggle")];
    assert.equal(all.length, 1, `${label}: ▣ が 1 つでない`);
    const b = all[0];
    assert.ok(b.closest("header"), `${label}: ▣ がアプリのヘッダーに無い`);
    const last = [...b.closest("header").querySelectorAll("button")].filter((x) => !x.closest(".nm-menu")).at(-1);
    assert.equal(last, b, `${label}: ▣ がヘッダーの最後のボタンでない`);
    assert.equal(document.querySelectorAll(".listbar button[aria-pressed], .lb-tail button[aria-pressed], .lb-pane").length, 0, `${label}: ツールバーに ▣ が残った`);
    const was = isOpen();
    assert.deepEqual([b.getAttribute("aria-pressed"), b.title], [String(was), was ? "右のペインを隠す（⌘⌥I）" : "右のペインを表示（⌘⌥I）"], `${label}: aria-pressed / title`);
    const w0 = calls.filter((c) => c.name === "set_ui_pref" && c.args.key === key).length;
    b.click();
    await settle();
    assert.equal(isOpen(), !was, `${label}: 押しても開閉しない`);
    assert.equal(calls.filter((c) => c.name === "set_ui_pref" && c.args.key === key).length, w0 + 1, `${label}: 開閉を ${key} に保存しない`);
    assert.equal($("header .pn-toggle").getAttribute("aria-pressed"), String(!was));
    $("header .pn-toggle").click();
    await settle();
    assert.equal(isOpen(), was);
    if (over) {
      S.app.tab = list;
      S.app.tabs.splice(S.app.tabs.findIndex((t) => t.id === 9805), 1);
      await settle();
    }
  }
  tests++;
}

// Esc の連鎖 × 覆い × 前面の種類: ＋新規メニュー（M）・メモ欄（N）・取込の窓（I）・チェック（C）を出した状態から
// Esc を 4 回押し、毎回の残りを記録する。覆いは押すたびに置き直す（窓が自分の Esc で閉じても、次の Esc も覆いの下で押す）。
// 取込は覆いそのもの = 閉じたら置き直さない（closeImport が効いたかを見る）。表は今のコードで採った（ESC_DUMP=1 で今の表を出す）
{
  const list = S.app.tab;
  const base = S.app.tabs.find((t) => t.id === list);
  const blank = { ...base, wiki: null, viewer: null, shelf: null, shelfBook: null };
  const kinds = [
    ["list", null],
    ["viewer", { viewer: { itemKey: "K1", attachmentId: 85, title: "P" } }],
    ["wiki", { wiki: { path: null, root: "/v", scrollTop: 0, editing: false } }],
    ["shelf", { shelf: { root: "/books", dir: null, query: "", sort: "name", scrollTop: 0 } }],
    ["book", { shelfBook: { root: "/books", id: "b1", path: "/books/b1.pdf", title: "本 1", size: 1, mtime: 0 } }],
  ];
  const covers = [
    ["none", () => {}, () => {}],
    ["modal", () => (S.app.modal = "citeStyle"), () => (S.app.modal = null)],
    ["guard", () => (W.wk.dialog = { kind: "new", dir: "" }), () => (W.wk.dialog = null)],
    ["editor", () => (S.app.editorOpen = true), () => (S.app.editorOpen = false)],
    ["import", () => {}, () => (S.app.importJob = null)],
  ];
  // 一覧で覆いが無ければ 1 回に 1 つずつ畳む（M → N → I → C）。覆い（モーダル・OpenNote の窓）の下と、一覧でない前面の覆いなしは何もしない。
  // 参照エディタ・取込の下は前面の種類によらず N → I を畳む（ビューアの上のエディタ・取込は従来どおり）が、チェックは外さない。
  // M（「＋ 追加 ▾」）は覆いが出た時点で閉じる = ヘッダーに「＋ 追加」がある前面（一覧・PDF・本）の覆いのある列では
  // Esc の前から M が無い。OpenNote・本棚のヘッダーには「＋ 追加」が無い（部品が居ない = 閉じる者も居ない・タブの切替は M を閉じる）= 前のまま
  const ESC_TABLE = {};
  for (const kn of ["list", "viewer", "wiki", "shelf", "book"]) {
    const none = kn === "list" ? "-N-C ---C ---- ----" : "MN-C MN-C MN-C MN-C";
    const m = kn === "wiki" || kn === "shelf" ? "M" : "-";
    Object.assign(ESC_TABLE, {
      [`${kn}/none`]: none,
      [`${kn}/modal`]: `${m}N-C ${m}N-C ${m}N-C ${m}N-C`,
      [`${kn}/guard`]: `${m}N-C ${m}N-C ${m}N-C ${m}N-C`,
      [`${kn}/editor`]: m === "M" ? "-N-C ---C ---C ---C" : "---C ---C ---C ---C",
      [`${kn}/import`]: m === "M" ? "-NIC --IC ---C ---C" : kn === "list" ? "--IC ---C ---- ----" : "--IC ---C ---C ---C",
    });
  }
  const mark = () => (S.app.newMenu ? "M" : "-") + (S.app.noteEditor ? "N" : "-") + (S.app.importJob ? "I" : "-") + (S.app.checked.size ? "C" : "-");
  const got = {};
  for (const [kn, over] of kinds) {
    if (over) {
      S.app.tabs.push({ ...blank, id: 9850, ...over });
      S.app.tab = 9850;
    }
    await settle();
    for (const [cn, on, off] of covers) {
      S.app.newMenu = true;
      S.app.noteEditor = { key: "K1", noteKey: null, text: "", saved: "" };
      S.app.importJob = cn === "import" ? { files: [], running: false, cancel: false } : null;
      S.app.checked.clear();
      S.app.checked.add("K1");
      const seq = [];
      for (let i = 0; i < 4; i++) {
        on();
        await settle();
        document.activeElement?.blur?.();
        key({ key: "Escape", code: "Escape" }, document.body);
        await settle();
        seq.push(mark());
      }
      got[`${kn}/${cn}`] = seq.join(" ");
      off();
      S.app.newMenu = false;
      S.app.noteEditor = null;
      S.app.importJob = null;
      S.app.checked.clear();
      await settle();
    }
    if (over) {
      S.app.tab = list;
      S.app.tabs.splice(S.app.tabs.findIndex((t) => t.id === 9850), 1);
      await settle();
    }
  }
  if (process.env.ESC_DUMP) console.log(JSON.stringify(got, null, 1));
  else assert.deepEqual(got, ESC_TABLE, "Esc の連鎖の覆い × 前面の種類の出し分けが変わった");
  tests++;
}

// frontKind() / covered() は置き換えた綴りと同じ答え（front* の 4 つを並べた判定・modalOpen() || editorOpen || importJob）。
// OpenNote が無効なら Wiki のタブは一覧（frontWiki と同じ）。処理中（busy）は覆いではない（タブの操作だけが見る = tabOpsAllowed）
{
  const list = S.app.tab;
  const base = S.app.tabs.find((t) => t.id === list);
  const blank = { ...base, wiki: null, viewer: null, shelf: null, shelfBook: null };
  const legacy = () => (S.frontViewer() ? "viewer" : S.frontWiki() ? "wiki" : S.frontShelfBook() ? "book" : S.frontShelf() ? "shelf" : "list");
  const listLike = () => !S.frontViewer() && !S.frontWiki() && !S.frontShelf() && !S.frontShelfBook();
  for (const [label, over, wikiOnNow, want] of [
    ["一覧", null, true, "list"],
    ["PDF", { viewer: { itemKey: "K1", attachmentId: 91, title: "P" } }, true, "viewer"],
    ["OpenNote", { wiki: { path: null, root: "/v", scrollTop: 0, editing: false } }, true, "wiki"],
    ["OpenNote（無効）", { wiki: { path: null, root: "/v", scrollTop: 0, editing: false } }, false, "list"],
    ["本棚", { shelf: { root: "/books", dir: null, query: "", sort: "name", scrollTop: 0 } }, true, "shelf"],
    ["本", { shelfBook: { root: "/books", id: "b1", path: "/books/b1.pdf", title: "本 1", size: 1, mtime: 0 } }, true, "book"],
  ]) {
    // app.tab / app.wikiOn を描かずに読むだけ（flushSync しない = 画面は一覧のまま）
    if (over) {
      S.app.tabs.push({ ...blank, id: 9900, ...over });
      S.app.tab = 9900;
    }
    S.app.wikiOn = wikiOnNow;
    assert.deepEqual([S.frontKind(), legacy(), S.frontKind() === "list"], [want, want, listLike()], `${label}: frontKind が置き換えた判定と違う`);
    S.app.wikiOn = true;
    if (over) {
      S.app.tab = list;
      S.app.tabs.splice(S.app.tabs.findIndex((t) => t.id === 9900), 1);
    }
  }
  await settle();
  const oldCovered = () => S.modalOpen() || S.app.editorOpen || !!S.app.importJob;
  assert.deepEqual([S.covered(), oldCovered(), S.tabOpsAllowed()], [false, false, true], "覆いの無い状態");
  for (const [label, on, off, tabOps] of [
    ["モーダル", () => (S.app.modal = "citeStyle"), () => (S.app.modal = null), false],
    ["チャットの同意の窓", () => (CH.chat.consent = true), () => (CH.chat.consent = false), false],
    ["救出の窓", () => ED.ed.stranded.push({ path: "a.md", content: "本文", reason: "書けない" }), () => ED.ed.stranded.splice(0), false],
    ["OpenNote のダイアログ", () => (W.wk.dialog = { kind: "new", dir: "" }), () => (W.wk.dialog = null), false],
    ["参照エディタ", () => (S.app.editorOpen = true), () => (S.app.editorOpen = false), false],
    ["PDF 取込", () => (S.app.importJob = { files: [], running: true, cancel: false }), () => (S.app.importJob = null), false],
  ]) {
    on();
    assert.deepEqual([S.covered(), oldCovered(), S.tabOpsAllowed()], [true, true, tabOps], `${label}: covered が置き換えた判定と違う`);
    off();
  }
  S.app.busy = true;
  assert.deepEqual([S.covered(), S.tabOpsAllowed()], [false, false], "処理中: covered に busy が入った / タブの操作を受け付けた");
  S.app.busy = false;
  await settle();
  tests++;
}

// 覆いの下のキー: 覆い = modalOpen()（本体の app.modal + OpenNote / チャットの窓の guard）+ 参照エディタ + 取込。チャットの同意の窓
// （ChatConsent）と救出の窓（WikiRescue）はどのタブの上にも出る。同意の窓は自分の window keydown で Esc を受けて閉じるが、App・PdfViewer も
// 同じ window のリスナー = stopPropagation は届かない。覆いの判定が app.modal だけだと、一覧の ⌘A が裏を全部チェックし・Esc が裏のチェックを
// 外し・PDF のタブの Esc がタブ（ビューア）まで閉じ・本棚 / OpenNote の `/` が裏の検索欄へ focus を移す。
// PdfViewer は実部品（pdf.js は偽物）を App の外に mount する（App の中の PdfViewer は偽物のまま）
{
  const covers = [
    ["チャットの同意の窓", () => (CH.chat.consent = true), () => (CH.chat.consent = false), ".wk-cc-list"],
    ["救出の窓", () => ED.ed.stranded.push({ path: "a.md", content: "本文", reason: "書けない" }), () => ED.ed.stranded.splice(0), "#wk-rescue-h"],
  ];
  const target = () => document.activeElement ?? document.body; // 実機のキーは focus のある要素から（同意の窓は [キャンセル] に focus を置く）
  const esc = () => key({ key: "Escape", code: "Escape" }, target());
  // 一覧: ⌘A・Esc
  assert.ok(!S.frontWiki() && !S.frontShelf() && !S.frontViewer() && !S.frontShelfBook(), "一覧のタブが前面でない");
  for (const [label, on, off, sel] of covers) {
    S.app.checked.clear();
    on();
    await settle();
    assert.ok($(sel), `前提: ${label}が出ない`);
    key({ metaKey: true, key: "a" }, target());
    await settle();
    assert.equal(S.app.checked.size, 0, `${label}の下の ⌘A が裏の一覧をチェックした`);
    S.app.checked.add("K1");
    esc();
    await settle();
    assert.deepEqual([...S.app.checked], ["K1"], `${label}の下の Esc が裏の一覧のチェックを外した`);
    off();
    await settle();
  }
  // 対照: 覆いが無ければ ⌘A は全部チェック・Esc は外す（上が「効かない」のは試験の配線のせいでない）
  S.app.checked.clear();
  document.activeElement?.blur?.();
  key({ metaKey: true, key: "a" }, document.body);
  await settle();
  assert.equal(S.app.checked.size, 3, "覆いの無い一覧で ⌘A が効かない");
  esc();
  await settle();
  assert.equal(S.app.checked.size, 0, "覆いの無い一覧で Esc がチェックを外さない");
  tests++;

  // PDF のタブ: Esc でタブ（ビューア）を閉じない
  const front = S.app.tab;
  await S.openViewer("K2", 1, "T");
  await settle();
  assert.ok(S.frontViewer(), "PDF のタブが前面でない");
  const host = document.createElement("div");
  document.body.append(host);
  let closed = 0;
  const v = mount(RealPdfViewer, { target: host, props: { viewer: S.frontViewer(), front: true, onClose: () => closed++ } });
  assert.ok(await waitFor(() => (flushSync(), !!host.querySelector(".viewer-overlay"))), "実部品の PdfViewer が出ない");
  await settle();
  for (const [label, on, off, sel] of covers) {
    on();
    await settle();
    assert.ok($(sel), `前提: ${label}が出ない`);
    esc();
    await settle();
    assert.equal(closed, 0, `${label}の上の Esc で PDF のタブを閉じた`);
    off();
    await settle();
  }
  document.activeElement?.blur?.();
  esc(); // 対照: 覆いが無ければ Esc で閉じる
  await settle();
  assert.equal(closed, 1, "覆いの無い PDF のタブで Esc がタブを閉じない（試験の配線が効いていない）");
  unmount(v);
  host.remove();
  await S.closeTab(S.app.tab);
  await settle();
  assert.equal(S.app.tab, front, "PDF のタブを閉じて元の一覧へ戻らない");
  tests++;

  // 本棚 / OpenNote: `/` で裏の検索欄へ focus を移さない
  const base = S.app.tabs.find((t) => t.id === front);
  for (const [kind, over, box] of [
    ["本棚", { shelf: { root: "/books", dir: null, query: "", sort: "name", scrollTop: 0 } }, "sh-search"],
    ["OpenNote", { wiki: { path: null, root: "/v", scrollTop: 0, editing: false } }, "wk-search"],
  ]) {
    S.app.tabs.push({ ...base, id: 9600, wiki: null, viewer: null, shelf: null, shelfBook: null, ...over });
    S.app.tab = 9600;
    assert.ok(await waitFor(() => (flushSync(), !!document.getElementById(box))), `${kind}の検索欄が出ない`);
    for (const [label, on, off] of covers) {
      on();
      await settle();
      key({ key: "/" }, target());
      await settle();
      assert.notEqual(document.activeElement?.id, box, `${label}の下の ${kind}の \`/\` が裏の検索欄へ focus を移した`);
      off();
      await settle();
    }
    document.activeElement?.blur?.();
    key({ key: "/" }, document.body); // 対照
    await settle();
    assert.equal(document.activeElement?.id, box, `覆いの無い${kind}で \`/\` が検索欄へ行かない`);
    document.activeElement.blur();
    S.app.tab = front;
    S.app.tabs.splice(S.app.tabs.findIndex((t) => t.id === 9600), 1);
    await settle();
  }
  tests++;
}

// モーダルの骨格の前後で窓ごとの挙動を固定する（11 枚 = lib 直下の 7 ファイル）。窓ごとに: 骨格の DOM（覆いと枠の
// class・role・aria・tabindex）・開いたときの focus・IME の変換中の Esc・中の部品の Esc（パネル / パレット → 窓）・Esc・覆いのクリック・
// 中のクリック（窓は閉じない・window の bubble の click 受け手に届くか = Sidebar のメニューの閉じ方）・閉じた後の focus。
// 表は骨格（Modal.svelte）の前のコードで採った（MODAL_DUMP=1 で今の表を出す）。窓は一覧の前面で開く
{
  assert.ok(!S.frontWiki() && !S.frontShelf() && !S.frontViewer() && !S.frontShelfBook(), "一覧のタブが前面でない");
  const cols0 = S.app.collections;
  const tags0 = S.app.tags;
  S.app.collections = [{ id: 1, key: "F1", name: "フォルダ", parentId: null, nItems: 0 }];
  S.app.tags = [{ id: 1, name: "ラベル", color: null, nItems: 0 }];
  const desc = (el) => (!el || el === document.body ? "body" : el.tagName.toLowerCase() + (el.classList[0] ? "." + el.classList[0] : ""));
  // class の svelte-xxxx（部品の scoped CSS の印）は除く = 枠を骨格の部品へ移すと付かなくなる（移行の窓の CSS は :global で同じ詳細度に書き直す）
  const attrs = (el) =>
    el ? ["class", "role", "aria-modal", "aria-label", "tabindex"].map((k) => `${k}=${(el.getAttribute(k) ?? "").replace(/\s*svelte-[a-z0-9]+/g, "")}`).join(" ") : null;
  const overlay = () => [...document.querySelectorAll(".overlay")].find((o) => o.querySelector(".modal")) ?? null;
  let bubbled = 0;
  const onWin = (e) => {
    if (e.target?.closest?.(".modal")) bubbled++;
  };
  window.addEventListener("click", onWin); // 中のクリックが window の bubble に届くか（Sidebar の window onclick と同じ段）
  const WINDOWS = [
    ["新規フォルダ", () => ((S.app.folderTarget = { id: null, parentId: null }), (S.app.modal = "newFolder")), () => S.app.modal === "newFolder", null],
    ["フォルダの改名", () => ((S.app.folderTarget = { id: 1, parentId: null }), (S.app.modal = "renameFolder")), () => S.app.modal === "renameFolder", null],
    ["フォルダの削除", () => ((S.app.folderTarget = { id: 1, parentId: null }), (S.app.modal = "deleteFolder")), () => S.app.modal === "deleteFolder", null],
    ["フォルダの管理", () => (S.app.modal = "manageFolders"), () => S.app.modal === "manageFolders", ".ml-tools .ml-btn.primary"],
    ["新規ラベル", () => (S.app.modal = "newLabel"), () => S.app.modal === "newLabel", ".nl-color"],
    ["ラベルの管理", () => (S.app.modal = "manageLabels"), () => S.app.modal === "manageLabels", ".ml-tools .ml-btn.primary"],
    ["完全に削除", () => ((S.app.purgeKeys = ["K1"]), (S.app.modal = "purge")), () => S.app.modal === "purge", null],
    ["引用スタイル", () => (S.app.modal = "citeStyle"), () => S.app.modal === "citeStyle", null],
    ["移行", () => (S.app.modal = "migration"), () => S.app.modal === "migration", null],
    ["参照エディタ", () => S.openEditorNew(), () => S.app.editorOpen, null],
    ["PDF 取込", () => (S.app.importJob = { files: [], running: false, cancel: false }), () => !!S.app.importJob, null],
  ];
  const reset = async () => {
    S.app.modal = null;
    S.closeEditor();
    S.app.importJob = null;
    S.app.purgeKeys = [];
    await settle();
    document.activeElement?.blur?.();
  };
  // dragOut / dragIn / openUp: 覆いの押し始めと離しが両方とも覆い自身のときだけ閉じる。直す前は覆いのクリックで閉じる 9 枚が
  // 3 列とも「閉じた」だった（openUp の前の値は合成の click の target が覆いだったため。実機では開く操作の click は覆いに飛ばない）。
  // 骨格の前のコードで採った表。骨格（Modal.svelte）で変えたのは 2 か所だけ = 完全に削除・移行の imeEsc
  // （前は composing を見ず、IME の変換の取消の Esc で窓が閉じた → 骨格が変換中の Esc を見送る = ほかの窓と同じ）
  const MODAL_TABLE = {
    "新規フォルダ": {"overlay": "class=overlay role=presentation aria-modal= aria-label= tabindex=", "frame": "class=modal nf-modal role=dialog aria-modal=true aria-label=新規フォルダ tabindex=-1", "focus": "input.nf-name", "imeEsc": "開いたまま", "imeEsc229": "開いたまま", "esc": "閉じた", "focusAfterEsc": "body", "innerClick": "開いたまま・window へ 届く", "backdrop": "閉じた", "dragOut": "開いたまま", "dragIn": "開いたまま", "openUp": "開いたまま", "covered": true},
    "フォルダの改名": {"overlay": "class=overlay role=presentation aria-modal= aria-label= tabindex=", "frame": "class=modal nf-modal role=dialog aria-modal=true aria-label=フォルダの名前を変更 tabindex=-1", "focus": "input.nf-name", "imeEsc": "開いたまま", "imeEsc229": "開いたまま", "esc": "閉じた", "focusAfterEsc": "body", "innerClick": "開いたまま・window へ 届く", "backdrop": "閉じた", "dragOut": "開いたまま", "dragIn": "開いたまま", "openUp": "開いたまま", "covered": true},
    "フォルダの削除": {"overlay": "class=overlay role=presentation aria-modal= aria-label= tabindex=", "frame": "class=modal nf-modal role=dialog aria-modal=true aria-label=フォルダを削除 tabindex=-1", "focus": "div.modal", "imeEsc": "開いたまま", "imeEsc229": "開いたまま", "esc": "閉じた", "focusAfterEsc": "body", "innerClick": "開いたまま・window へ 届く", "backdrop": "閉じた", "dragOut": "開いたまま", "dragIn": "開いたまま", "openUp": "開いたまま", "covered": true},
    "フォルダの管理": {"overlay": "class=overlay role=presentation aria-modal= aria-label= tabindex=", "frame": "class=modal ml-modal mf-modal role=dialog aria-modal=true aria-label=フォルダを管理 tabindex=-1", "focus": "input.ml-q", "imeEsc": "開いたまま", "imeEsc229": "開いたまま", "innerEsc": "中の部品が開く → Esc: 中の部品が閉じる・窓は開いたまま", "esc": "閉じた", "focusAfterEsc": "body", "innerClick": "開いたまま・window へ 届く・中の部品が残る", "backdrop": "閉じた", "dragOut": "開いたまま", "dragIn": "開いたまま", "openUp": "開いたまま", "covered": true},
    "新規ラベル": {"overlay": "class=overlay role=presentation aria-modal= aria-label= tabindex=", "frame": "class=modal nl-modal role=dialog aria-modal=true aria-label=新規ラベル tabindex=", "focus": "input.nl-name", "imeEsc": "開いたまま", "imeEsc229": "開いたまま", "innerEsc": "中の部品が開く → Esc: 中の部品が閉じる・窓は開いたまま", "esc": "閉じた", "focusAfterEsc": "body", "innerClick": "開いたまま・window へ 届く・中の部品が閉じる", "backdrop": "閉じた", "dragOut": "開いたまま", "dragIn": "開いたまま", "openUp": "開いたまま", "covered": true},
    "ラベルの管理": {"overlay": "class=overlay role=presentation aria-modal= aria-label= tabindex=", "frame": "class=modal ml-modal role=dialog aria-modal=true aria-label=ラベルを管理 tabindex=", "focus": "input.ml-q", "imeEsc": "開いたまま", "imeEsc229": "開いたまま", "innerEsc": "中の部品が開く → Esc: 中の部品が閉じる・窓は開いたまま", "esc": "閉じた", "focusAfterEsc": "body", "innerClick": "開いたまま・window へ 届く・中の部品が残る", "backdrop": "閉じた", "dragOut": "開いたまま", "dragIn": "開いたまま", "openUp": "開いたまま", "covered": true},
    "完全に削除": {"overlay": "class=overlay role=presentation aria-modal= aria-label= tabindex=", "frame": "class=modal role=dialog aria-modal= aria-label=完全に削除 tabindex=", "focus": "body", "imeEsc": "開いたまま", "imeEsc229": "開いたまま", "esc": "閉じた", "focusAfterEsc": "body", "innerClick": "開いたまま・window へ 届かない", "backdrop": "閉じた", "dragOut": "開いたまま", "dragIn": "開いたまま", "openUp": "開いたまま", "covered": true},
    "引用スタイル": {"overlay": "class=overlay role=presentation aria-modal= aria-label= tabindex=", "frame": "class=modal sm-modal role=dialog aria-modal= aria-label=引用スタイルを設定 tabindex=", "focus": "input.sm-q", "imeEsc": "開いたまま", "imeEsc229": "開いたまま", "esc": "閉じた", "focusAfterEsc": "body", "innerClick": "開いたまま・window へ 届かない", "backdrop": "閉じた", "dragOut": "開いたまま", "dragIn": "開いたまま", "openUp": "開いたまま", "covered": true},
    "移行": {"overlay": "class=overlay role=presentation aria-modal= aria-label= tabindex=", "frame": "class=modal mg-modal role=dialog aria-modal=true aria-label=他の文献管理ソフトから移行 tabindex=-1", "focus": "body", "imeEsc": "開いたまま", "imeEsc229": "開いたまま", "esc": "閉じた", "focusAfterEsc": "body", "innerClick": "開いたまま・window へ 届く", "backdrop": "閉じない", "dragOut": "開いたまま", "dragIn": "開いたまま", "openUp": "開いたまま", "covered": true},
    "参照エディタ": {"overlay": "class=overlay role=presentation aria-modal= aria-label= tabindex=", "frame": "class=modal editor role=dialog aria-modal= aria-label=文献の編集 tabindex=", "focus": "body", "imeEsc": "開いたまま", "imeEsc229": "開いたまま", "esc": "開いたまま", "focusAfterEsc": null, "innerClick": "開いたまま・window へ 届く", "backdrop": "閉じない", "dragOut": "開いたまま", "dragIn": "開いたまま", "openUp": "開いたまま", "covered": true},
    "PDF 取込": {"overlay": "class=overlay role=presentation aria-modal= aria-label= tabindex=", "frame": "class=modal im-modal role=dialog aria-modal= aria-label=PDF 取込 tabindex=-1", "focus": "div.modal", "imeEsc": "開いたまま", "imeEsc229": "開いたまま", "esc": "閉じた", "focusAfterEsc": "body", "innerClick": "開いたまま・window へ 届く", "backdrop": "閉じた", "dragOut": "開いたまま", "dragIn": "開いたまま", "openUp": "開いたまま", "covered": true},
  };
  const got = {};
  for (const [label, open, isOpen, inner] of WINDOWS) {
    await reset();
    const row = {};
    open();
    await settle();
    await sleep(5);
    flushSync();
    const ov = overlay();
    row.overlay = attrs(ov);
    row.frame = attrs(ov?.querySelector(":scope > .modal"));
    row.focus = desc(document.activeElement);
    row.covered = S.covered(); // 覆いとして数える（app.modal・参照エディタ・取込）
    key({ key: "Escape", code: "Escape", isComposing: true }, document.activeElement ?? document.body);
    await settle();
    row.imeEsc = isOpen() ? "開いたまま" : "閉じた";
    if (!isOpen()) {
      open();
      await settle();
    }
    // WebKit の経路: 変換の取消の Esc が isComposing = false・keyCode = 229 で来る（input.ts の composing が両方を見る）
    key({ key: "Escape", code: "Escape", keyCode: 229 }, document.activeElement ?? document.body);
    await settle();
    row.imeEsc229 = isOpen() ? "開いたまま" : "閉じた";
    if (!isOpen()) {
      open();
      await settle();
    }
    if (inner) {
      document.querySelector(inner)?.click();
      await settle();
      const panel = !!document.querySelector(".ml-panel, .nl-pal");
      key({ key: "Escape", code: "Escape" }, document.body);
      await settle();
      row.innerEsc = `${panel ? "中の部品が開く" : "開かない"} → Esc: ${document.querySelector(".ml-panel, .nl-pal") ? "中の部品が残る" : "中の部品が閉じる"}・${isOpen() ? "窓は開いたまま" : "窓が閉じた"}`;
    }
    key({ key: "Escape", code: "Escape" }, document.body);
    await settle();
    row.esc = isOpen() ? "開いたまま" : "閉じた";
    row.focusAfterEsc = isOpen() ? null : desc(document.activeElement);
    if (!isOpen()) {
      open();
      await settle();
    }
    if (inner) {
      document.querySelector(inner)?.click(); // 中の部品（パネル / 色の見本）を開いてから枠の中を押す = 枠のクリックで閉じるか
      await settle();
    }
    bubbled = 0;
    overlay()?.querySelector(":scope > .modal .modal-head h3")?.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    await settle();
    row.innerClick = `${isOpen() ? "開いたまま" : "閉じた"}・window へ ${bubbled ? "届く" : "届かない"}${inner ? `・中の部品が${document.querySelector(".ml-panel, .nl-pal") ? "残る" : "閉じる"}` : ""}`;
    if (!isOpen()) {
      open();
      await settle();
    }
    // 覆いの押し方: 実機の順 = pointerdown（押した所）→ pointerup（離した所）→ click（両者の共通の祖先 = ここでは覆い）
    const h3 = () => overlay()?.querySelector(":scope > .modal .modal-head h3");
    const press = async (down, up) => {
      down?.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, cancelable: true, button: 0 }));
      up?.dispatchEvent(new PointerEvent("pointerup", { bubbles: true, cancelable: true, button: 0 }));
      overlay()?.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
      await settle();
    };
    const reopen = async () => {
      if (!isOpen()) {
        open();
        await settle();
      }
    };
    await press(overlay(), overlay());
    row.backdrop = isOpen() ? "閉じない" : "閉じた";
    await reopen();
    await press(h3(), overlay()); // 窓の中で押して外へドラッグ（欄の文字の選択）
    row.dragOut = isOpen() ? "開いたまま" : "閉じた";
    await reopen();
    await press(overlay(), h3()); // 覆いで押して窓の中で離す
    row.dragIn = isOpen() ? "開いたまま" : "閉じた";
    // 覆いで押したまま閉じた（Esc 等）→ 次に開いた直後の最初の離し（押しは覆いの出る前）= 押しの記憶が残って閉じない
    await reopen();
    overlay()?.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, cancelable: true, button: 0 }));
    await reset();
    open();
    await settle();
    await press(null, overlay());
    row.openUp = isOpen() ? "開いたまま" : "閉じた";
    got[label] = row;
  }
  await reset();
  window.removeEventListener("click", onWin);
  S.app.collections = cols0;
  S.app.tags = tags0;
  await settle();
  if (process.env.MODAL_DUMP) console.log(JSON.stringify(got, null, 1));
  else assert.deepEqual(got, MODAL_TABLE, "窓ごとの挙動が骨格の前と違う");
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
  assert.equal(mainCls(), info ? "wk-main-noinfo" : "", "OpenNote の 2 列にならない");
  tests++;
}

// ラベルの管理の Esc の連鎖の先頭 = 行の色の見本（rowPal）: 1 回目で色の見本だけが閉じ、2 回目で窓が閉じる
{
  const tags0 = S.app.tags;
  S.app.tags = [{ id: 1, name: "ラベル", color: null, nItems: 0 }];
  S.app.modal = "manageLabels";
  await settle();
  $(".ml-modal .ml-table .nl-color").click();
  await settle();
  assert.ok($(".ml-rowpal"), "前提: 行の色の見本が開かない");
  key({ key: "Escape", code: "Escape" }, document.body);
  await settle();
  assert.deepEqual([!!$(".ml-rowpal"), S.app.modal], [false, "manageLabels"], "Esc 1 回目で色の見本だけが閉じない（窓まで閉じた / 見本が残った）");
  key({ key: "Escape", code: "Escape" }, document.body);
  await settle();
  assert.equal(S.app.modal, null, "Esc 2 回目で窓が閉じない");
  S.app.tags = tags0;
  await settle();
  tests++;
}

// 覆いの下では window のショートカットが裏に効かない（`/` と ⌘N・取込の下の ⌘A と OpenNote が前面のときの参照エディタ / 取込の下の
// ⌘⌥I も止めた）: ショートカット × 覆い（窓・OpenNote の窓・チャットの窓・参照エディタ・取込）× 前面の種類（一覧・本棚・OpenNote）→ 裏の状態
// （タブ・focus の在処・チェック・右ペイン・柱・集中モード・参照エディタ・窓）が変わらない = 裏に効いた組は 0。
// 変わってよいのは覆い自身だけ: 参照エディタだけが開いているときの ⌘⇧N（参照エディタ自身を新規に戻す = 裏ではない）
{
  const FO = await import("./wiki/focus.svelte.ts");
  const orig = S.app.tab; // この時点の前面（OpenNote のタブ）= 最後に戻す
  const base = S.app.tabs.find((t) => t.id === orig);
  const blank = { ...base, wiki: null, viewer: null, shelf: null, shelfBook: null, shelfAway: null };
  S.app.tabs.push({ ...blank, id: 9950 }, { ...blank, id: 9952 }); // 一覧のタブと巡回の相手（⌘⇧] / ⌃Tab / ⌘1 が効けば前面が変わる）
  const list = 9950;
  S.app.tab = list;
  await settle();
  assert.equal(S.frontKind(), "list", "前提: 一覧のタブが前面でない");
  const snap = () => ({
    tab: S.app.tab,
    tabs: S.app.tabs.length,
    focus: document.activeElement?.id || document.activeElement?.className?.split?.(" ")[0] || "body",
    checked: S.app.checked.size,
    detail: S.app.detailOpen,
    info: W.wk.infoOpen,
    dock: CH.chat.dock.open,
    focusMode: FO.focusMode.on,
    editor: `${S.app.editorOpen}:${S.app.editorKey}`,
    newMenu: S.app.newMenu,
  });
  const keys = {
    "⌘T": { metaKey: true, key: "t" },
    "⌘W": { metaKey: true, key: "w" },
    "⌘⇧]": { metaKey: true, shiftKey: true, key: "]" },
    "⌃Tab": { ctrlKey: true, key: "Tab" },
    "⌘1": { metaKey: true, key: "1" },
    "⌘⌥S": { metaKey: true, altKey: true, code: "KeyS", key: "ß" },
    "⌘⌥I": { metaKey: true, altKey: true, code: "KeyI", key: "ˆ" },
    "⌘⌥C": { metaKey: true, altKey: true, code: "KeyC", key: "ç" },
    "⌘⇧N": { metaKey: true, shiftKey: true, key: "N" },
    "⌘N": { metaKey: true, key: "n" },
    "⌘A": { metaKey: true, key: "a" },
    "/": { key: "/" },
  };
  const covers = [
    ["窓", () => ((S.app.folderTarget = { id: null, parentId: null }), (S.app.modal = "deleteFolder")), () => (S.app.modal = null)],
    ["OpenNote の窓", () => (W.wk.dialog = { kind: "new", dir: "" }), () => (W.wk.dialog = null)],
    ["チャットの同意の窓", () => (CH.chat.consent = true), () => (CH.chat.consent = false)],
    ["参照エディタ", () => S.openEditorFor("K1", { title: "題名 K1" }), () => S.closeEditor()],
    ["取込", () => (S.app.importJob = { files: [], running: false, cancel: false }), () => (S.app.importJob = null)],
  ];
  const fronts = [
    ["一覧", null, Object.keys(keys)],
    ["本棚", { shelf: { root: "/books", dir: null, query: "", sort: "name", scrollTop: 0 } }, Object.keys(keys)],
    ["OpenNote", { wiki: { path: null, root: "/v", scrollTop: 0, editing: false } }, Object.keys(keys)],
  ];
  const effects = {};
  const prevented = new Set(); // 覆いの下で ⌘N / `/` のキーを奪ったか（⌘N = 常に奪う・`/` = 素通し）
  for (const [fn, over, ks] of fronts) {
    if (over) {
      S.app.tabs.push({ ...blank, id: 9951, ...over });
      S.app.tab = 9951;
    }
    await settle();
    for (const [cn, on, off] of covers) {
      for (const k of ks) {
        S.app.checked.clear();
        on();
        await settle();
        document.activeElement?.blur?.();
        const before = snap();
        const ev = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...keys[k] });
        document.body.dispatchEvent(ev);
        await settle();
        const after = snap();
        const changed = Object.keys(before).filter((x) => before[x] !== after[x]);
        if (changed.length) effects[`${fn} / ${cn} / ${k}`] = changed.join(",");
        if ((k === "⌘N" || k === "/") && ev.defaultPrevented) prevented.add(k);
        // 戻す（効いてしまった物も含めて）
        off();
        if (after.info !== before.info) W.wk.infoOpen = before.info;
        if (after.detail !== before.detail) S.app.detailOpen = before.detail;
        if (after.dock !== before.dock) CH.chat.dock.open = before.dock;
        if (after.focusMode !== before.focusMode) FO.focusMode.on = before.focusMode;
        S.closeEditor();
        S.app.modal = null;
        W.wk.dialog = null;
        S.app.tab = over ? 9951 : list;
        await settle();
      }
    }
    if (over) {
      S.app.tab = list;
      S.app.tabs.splice(S.app.tabs.findIndex((t) => t.id === 9951), 1);
      await settle();
    }
  }
  S.app.tab = orig;
  for (const id of [9950, 9952]) S.app.tabs.splice(S.app.tabs.findIndex((t) => t.id === id), 1);
  S.app.checked.clear();
  await settle();
  if (process.env.MODAL_DUMP) console.log("EFFECTS", JSON.stringify(effects, null, 1));
  // 覆い自身に効いた組（裏ではない）。参照エディタの ⌘⇧N = 中身を新規に戻す（今のまま）。前面の種類に依らない
  const self = (k) => /^[^/]+ \/ 参照エディタ \/ ⌘⇧N$/.test(k);
  const back = Object.fromEntries(Object.entries(effects).filter(([k]) => !self(k)));
  assert.deepEqual(back, {}, "覆いの下で window のショートカットが裏に効いた（0 であるべき）");
  assert.deepEqual(
    Object.entries(effects).filter(([k]) => self(k)),
    [["一覧 / 参照エディタ / ⌘⇧N", "editor"], ["本棚 / 参照エディタ / ⌘⇧N", "editor"], ["OpenNote / 参照エディタ / ⌘⇧N", "editor"]],
    "参照エディタだけのときの ⌘⇧N（参照エディタ自身を新規に戻す）が変わった",
  );
  // ⌘N はメニュー / ブラウザの既定へ流さない = 覆いの下でも奪う（⌘⇧N と同じ）。`/` は覆いの下では素通し（窓の中の欄以外では何も起きない）
  assert.deepEqual([...prevented].sort(), ["⌘N"], "覆いの下で ⌘N を奪わない / `/` を奪った");
  tests++;
}

// 窓の中の入力欄の ⌘A = 欄の文字の全選択（既定の動作）: App は奪わず、裏の一覧もチェックしない（⌘A の門を覆いにしても変えない）。
// 覆いの無い一覧の検索欄でも同じ（isEditable）。一覧のタブを前面にして見る（⌘A の一覧の枝に入る前面）
{
  const orig = S.app.tab;
  const base = S.app.tabs.find((t) => t.id === orig);
  S.app.tabs.push({ ...base, id: 9960, wiki: null, viewer: null, shelf: null, shelfBook: null, shelfAway: null });
  S.app.tab = 9960;
  await settle();
  assert.equal(S.frontKind(), "list", "前提: 一覧のタブが前面でない");
  const fields = [
    ["新規フォルダの名前", () => ((S.app.folderTarget = { id: null, parentId: null }), (S.app.modal = "newFolder")), ".modal input"],
    ["参照エディタの欄", () => S.openEditorFor("K1", { title: "題名 K1" }), ".modal.editor input"],
    ["覆いの無い一覧の検索欄", () => {}, "#searchbox"],
  ];
  const got = {};
  for (const [label, open, sel] of fields) {
    S.app.checked.clear();
    open();
    await settle();
    const el = $(sel);
    assert.ok(el, `前提: ${label}が無い`);
    el.focus();
    const ev = new KeyboardEvent("keydown", { metaKey: true, key: "a", bubbles: true, cancelable: true });
    el.dispatchEvent(ev);
    await settle();
    got[label] = `${ev.defaultPrevented ? "奪う" : "素通し"} チェック ${S.app.checked.size}`;
    S.app.modal = null;
    S.closeEditor();
    await settle();
  }
  assert.deepEqual(got, Object.fromEntries(fields.map(([l]) => [l, "素通し チェック 0"])), "入力欄の ⌘A を奪った / 裏の一覧をチェックした");
  S.app.tab = orig;
  S.app.tabs.splice(S.app.tabs.findIndex((t) => t.id === 9960), 1);
  await settle();
  tests++;
}

// 窓が 2 枚重ならない: 窓（モーダル・OpenNote / チャットの窓）・取込が出ている間は ⌘⇧N で参照エディタを開かない
// （前は覆いを見ずに開き、窓の上に重なった参照エディタを残して Esc が下の窓を閉じた）。Esc は今までどおりその窓を閉じる。
// 参照エディタだけが開いているときの ⌘⇧N は今のまま（openEditorNew = 編集中の参照でも editorKey を外して「新規作成」に戻す）
{
  const cols0 = S.app.collections;
  S.app.collections = [{ id: 1, key: "F1", name: "フォルダ", parentId: null, nItems: 0 }];
  const under = [
    ["新規フォルダ", () => ((S.app.folderTarget = { id: null, parentId: null }), (S.app.modal = "newFolder"))],
    ["フォルダの管理", () => (S.app.modal = "manageFolders")],
    ["新規ラベル", () => (S.app.modal = "newLabel")],
    ["完全に削除", () => ((S.app.purgeKeys = ["K1"]), (S.app.modal = "purge"))],
    ["引用スタイル", () => (S.app.modal = "citeStyle")],
    ["移行", () => (S.app.modal = "migration")],
    ["OpenNote の窓（guard）", () => (W.wk.dialog = { kind: "new", dir: "" })],
    ["チャットの同意の窓", () => (CH.chat.consent = true)],
    ["PDF 取込", () => (S.app.importJob = { files: [], running: false, cancel: false })],
  ];
  const state = () =>
    `modal=${S.app.modal ?? "-"} guard=${W.wk.dialog || CH.chat.consent ? 1 : 0} editor=${S.app.editorOpen ? 1 : 0} import=${S.app.importJob ? 1 : 0}`;
  const got = {};
  for (const [label, open] of under) {
    open();
    await settle();
    document.activeElement?.blur?.();
    const ev = new KeyboardEvent("keydown", { metaKey: true, shiftKey: true, key: "N", bubbles: true, cancelable: true });
    document.body.dispatchEvent(ev);
    await settle();
    const seq = [state() + (ev.defaultPrevented ? " 奪う" : " 素通し")];
    key({ key: "Escape", code: "Escape" }, document.body);
    await settle();
    seq.push(state());
    got[label] = seq.join(" → ");
    S.app.modal = null;
    W.wk.dialog = null;
    CH.chat.consent = false;
    S.closeEditor();
    S.app.importJob = null;
    S.app.purgeKeys = [];
    await settle();
  }
  S.app.collections = cols0;
  await settle();
  const OVERLAP = {
    新規フォルダ: "modal=newFolder guard=0 editor=0 import=0 奪う → modal=- guard=0 editor=0 import=0",
    フォルダの管理: "modal=manageFolders guard=0 editor=0 import=0 奪う → modal=- guard=0 editor=0 import=0",
    新規ラベル: "modal=newLabel guard=0 editor=0 import=0 奪う → modal=- guard=0 editor=0 import=0",
    完全に削除: "modal=purge guard=0 editor=0 import=0 奪う → modal=- guard=0 editor=0 import=0",
    引用スタイル: "modal=citeStyle guard=0 editor=0 import=0 奪う → modal=- guard=0 editor=0 import=0",
    移行: "modal=migration guard=0 editor=0 import=0 奪う → modal=- guard=0 editor=0 import=0",
    "OpenNote の窓（guard）": "modal=- guard=1 editor=0 import=0 奪う → modal=- guard=0 editor=0 import=0",
    チャットの同意の窓: "modal=- guard=1 editor=0 import=0 奪う → modal=- guard=0 editor=0 import=0",
    "PDF 取込": "modal=- guard=0 editor=0 import=1 奪う → modal=- guard=0 editor=0 import=0",
  };
  if (process.env.MODAL_DUMP) console.log(JSON.stringify(got, null, 1));
  else assert.deepEqual(got, OVERLAP, "窓の下の ⌘⇧N で参照エディタが重なった / Esc の閉じ方が変わった");
  // 参照エディタだけが開いている = 今のまま開いたまま・新規に戻す（編集中の参照の editorKey を外す）
  S.openEditorFor("K1", { title: "題名 K1" });
  await settle();
  key({ metaKey: true, shiftKey: true, key: "N" }, document.body);
  await settle();
  assert.deepEqual([S.app.editorOpen, S.app.editorKey, document.querySelectorAll(".modal.editor").length], [true, null, 1], "参照エディタだけが開いているときの ⌘⇧N が変わった");
  S.closeEditor();
  await settle();
  // 覆いの無い一覧では今までどおり開く
  key({ metaKey: true, shiftKey: true, key: "N" }, document.body);
  await settle();
  assert.equal(S.app.editorOpen, true, "覆いの無い一覧で ⌘⇧N が参照エディタを開かない");
  S.closeEditor();
  await settle();
  tests++;
}

// 表示設定の読み書きの列（実装を置き換えても同じ = 表は置き換えの前のコードで採った・PREFS_DUMP=1 で今の表を出す）:
// 起動の読み（main.ts の 4 つ・App の onMount の loadFolderPrefs）→ 読めない / 形の違う値 → 変更（開閉・並び・折畳）→ 書けない。
// 段ごとに ui_* の invoke の列（get / set・鍵・値）と、読み書きした後の画面の値（app / 言語）と toast を記録する
{
  const snap = () => ({
    lang: I18N.i18n.lang,
    detail: S.app.detailOpen,
    side: S.app.viewerSideOpen,
    listSort: `${S.app.listSort.key}:${S.app.listSort.desc}`,
    folderSort: S.app.folderSort,
    tagSort: S.app.tagSort,
    tags: S.app.tags.map((x) => x.name).join(","),
    collapsed: [...S.app.foldersCollapsed].sort().join(","),
    toast: S.app.toast?.replace(/SyntaxError: .*/, "SyntaxError: …") ?? null, // JSON.parse の文言は Node の版で違う
  });
  const ui = (from) => calls.slice(from).filter((c) => c.name === "get_ui_pref" || c.name === "set_ui_pref").map((c) => (c.name === "get_ui_pref" ? `get ${c.args.key}` : `set ${c.args.key}=${c.args.value}`));
  // 置き換えの前のコードで採った表: [段, ui_* の invoke の列, 後の画面の値]
  const PREFS_TABLE = [
    ["起動の読み（main.ts）", ["get ui_lang", "get ui_detail", "get ui_list_sort", "get ui_viewer_side"], {"lang": "en", "detail": false, "side": false, "listSort": "journal:false", "folderSort": "alpha", "tagSort": "alpha", "tags": "a,b", "collapsed": "", "toast": null}],
    ["起動の読み（onMount）", ["get ui_folder_sort", "get ui_tag_sort", "get ui_folders_collapsed"], {"lang": "en", "detail": false, "side": false, "listSort": "journal:false", "folderSort": "count", "tagSort": "alpha", "tags": "a,b", "collapsed": "F1,F2", "toast": null}],
    ["形の違う値", ["get ui_lang", "get ui_detail", "get ui_list_sort", "get ui_viewer_side", "get ui_folder_sort", "get ui_tag_sort", "get ui_folders_collapsed"], {"lang": "ja", "detail": true, "side": true, "listSort": "journal:false", "folderSort": "count", "tagSort": "count", "tags": "b,a", "collapsed": "F1,F2", "toast": null}],
    ["読めない ui_lang", ["get ui_lang"], {"lang": "ja", "detail": true, "side": true, "listSort": "journal:false", "folderSort": "count", "tagSort": "count", "tags": "b,a", "collapsed": "F1,F2", "toast": "✗ 言語の設定の読込に失敗: Error: 読めない"}],
    ["読めない ui_detail", ["get ui_detail"], {"lang": "ja", "detail": true, "side": true, "listSort": "journal:false", "folderSort": "count", "tagSort": "count", "tags": "b,a", "collapsed": "F1,F2", "toast": "✗ 表示設定を読めません: Error: 読めない"}],
    ["読めない ui_list_sort", ["get ui_list_sort"], {"lang": "ja", "detail": true, "side": true, "listSort": "journal:false", "folderSort": "count", "tagSort": "count", "tags": "b,a", "collapsed": "F1,F2", "toast": "✗ 表示設定を読めません: Error: 読めない"}],
    ["読めない ui_folder_sort", ["get ui_folder_sort", "get ui_tag_sort", "get ui_folders_collapsed"], {"lang": "ja", "detail": true, "side": true, "listSort": "journal:false", "folderSort": "count", "tagSort": "count", "tags": "b,a", "collapsed": "F1,F2", "toast": "✗ フォルダ・ラベルの表示設定の読込に失敗: Error: 読めない"}],
    ["壊れた折畳", ["get ui_folder_sort", "get ui_tag_sort", "get ui_folders_collapsed"], {"lang": "ja", "detail": true, "side": true, "listSort": "journal:false", "folderSort": "count", "tagSort": "count", "tags": "b,a", "collapsed": "F1,F2", "toast": "✗ フォルダ・ラベルの表示設定の読込に失敗: SyntaxError: …"}],
    ["右ペインの開閉 ×2", ["set ui_detail=0", "set ui_detail=1"], {"lang": "ja", "detail": true, "side": true, "listSort": "journal:false", "folderSort": "count", "tagSort": "count", "tags": "b,a", "collapsed": "F1,F2", "toast": null}],
    ["右バーを開く（閉じていれば）", ["set ui_viewer_side=1"], {"lang": "ja", "detail": true, "side": true, "listSort": "journal:false", "folderSort": "count", "tagSort": "count", "tags": "b,a", "collapsed": "F1,F2", "toast": null}],
    ["右バーの開閉", ["set ui_viewer_side=0"], {"lang": "ja", "detail": true, "side": false, "listSort": "journal:false", "folderSort": "count", "tagSort": "count", "tags": "b,a", "collapsed": "F1,F2", "toast": null}],
    ["並び（同じ項目 = 反転・別の項目 = 自然な向き）", ["set ui_list_sort=journal:desc", "set ui_list_sort=journal:asc", "set ui_list_sort=added:desc"], {"lang": "ja", "detail": true, "side": false, "listSort": "added:true", "folderSort": "count", "tagSort": "count", "tags": "b,a", "collapsed": "F1,F2", "toast": null}],
    ["フォルダの並び・ラベルの並び", ["set ui_folder_sort=alpha", "set ui_tag_sort=count", "set ui_tag_sort=alpha"], {"lang": "ja", "detail": true, "side": false, "listSort": "added:true", "folderSort": "alpha", "tagSort": "alpha", "tags": "a,b", "collapsed": "F1,F2", "toast": null}],
    ["折畳の開閉", ["set ui_folders_collapsed=[\"F1\",\"F2\",\"F9\"]", "set ui_folders_collapsed=[\"F2\",\"F9\"]"], {"lang": "ja", "detail": true, "side": false, "listSort": "added:true", "folderSort": "alpha", "tagSort": "alpha", "tags": "a,b", "collapsed": "F2,F9", "toast": null}],
    ["書けない", ["set ui_detail=0"], {"lang": "ja", "detail": false, "side": false, "listSort": "added:true", "folderSort": "alpha", "tagSort": "alpha", "tags": "a,b", "collapsed": "F2,F9", "toast": "✗ 表示設定を保存できません: Error: 書けない"}],
  ];
  const got = [];
  const step = async (label, f) => {
    S.app.toast = null;
    const from = calls.length;
    await f();
    await settle();
    got.push([label, ui(from), snap()]);
  };
  const keep = { ...prefs };
  const tags0 = S.app.tags;
  S.app.tags = [{ id: 1, name: "a", color: null, nItems: 1 }, { id: 2, name: "b", color: null, nItems: 5 }]; // 名前順 = a,b・件数順 = b,a
  for (const k of Object.keys(prefs)) delete prefs[k];
  Object.assign(prefs, { ui_lang: "en", ui_detail: "0", ui_viewer_side: "0", ui_list_sort: "journal:asc", ui_folder_sort: "count", ui_tag_sort: "alpha", ui_folders_collapsed: JSON.stringify(["F1", 3, "F2"]) });
  await step("起動の読み（main.ts）", () => Promise.all([F.loadLangPref(), F.loadDetailPref(), F.loadListSortPref(), F.loadViewerSidePref()]));
  await step("起動の読み（onMount）", () => F.loadFolderPrefs());
  Object.assign(prefs, { ui_lang: "xx", ui_detail: "1", ui_viewer_side: "", ui_list_sort: "title:asc", ui_folder_sort: "big", ui_tag_sort: "count", ui_folders_collapsed: "[]" });
  await step("形の違う値", () => Promise.all([F.loadLangPref(), F.loadDetailPref(), F.loadListSortPref(), F.loadViewerSidePref(), F.loadFolderPrefs()]));
  for (const k of ["ui_lang", "ui_detail", "ui_list_sort", "ui_folder_sort"]) {
    failPrefKey = k;
    await step(`読めない ${k}`, () => (k === "ui_lang" ? F.loadLangPref() : k === "ui_detail" ? F.loadDetailPref() : k === "ui_list_sort" ? F.loadListSortPref() : F.loadFolderPrefs()));
  }
  failPrefKey = null;
  prefs.ui_folders_collapsed = "{壊れた";
  await step("壊れた折畳", () => F.loadFolderPrefs());
  await step("右ペインの開閉 ×2", () => (F.toggleDetail(), F.toggleDetail()));
  await step("右バーを開く（閉じていれば）", () => (S.app.viewerSideOpen = false, F.showViewerSide(), F.showViewerSide()));
  await step("右バーの開閉", () => F.toggleViewerSide());
  await step("並び（同じ項目 = 反転・別の項目 = 自然な向き）", async () => {
    await F.pickListSort("journal");
    await F.pickListSort("journal");
    await F.pickListSort("added");
  });
  await step("フォルダの並び・ラベルの並び", () => (F.setFolderSort("alpha"), F.setTagSort("count"), F.setTagSort("alpha")));
  await step("折畳の開閉", () => (F.toggleFolderCollapsed("F9"), F.toggleFolderCollapsed("F1")));
  failSetPrefKey = "ui_detail";
  await step("書けない", () => F.toggleDetail());
  failSetPrefKey = null;
  if (process.env.PREFS_DUMP) console.log(JSON.stringify(got));
  else assert.deepEqual(got, PREFS_TABLE, "表示設定の読み書きの列が置き換えの前と違う");
  // 後片付け（以後の節は無い・言語は日本語へ）
  for (const k of Object.keys(prefs)) delete prefs[k];
  Object.assign(prefs, keep);
  I18N.setLang("ja");
  S.app.tags = tags0;
  await settle();
  tests++;
}

// errText（lib/err.ts）= String(e).slice(0, n) と 1 字も違わない: Error は "Error: " の前置きつき・オブジェクト / undefined / null は String のとおり・
// 改行も前後の空白も残す・… を足さない・UTF-16 の単位で切る（サロゲートの途中でも切る）
{
  const { errText } = await import("./err.ts");
  const long = "長".repeat(400);
  const table = [
    ["x", 160, "x"],
    ["wiki_save: conflict", 200, "wiki_save: conflict"],
    [new Error("x"), 200, "Error: x"],
    [new TypeError("bad"), 160, "TypeError: bad"],
    [{ message: "x" }, 200, "[object Object]"],
    [undefined, 160, "undefined"],
    [null, 160, "null"],
    [42, 200, "42"],
    [long, 300, "長".repeat(300)],
    [new Error(long), 240, ("Error: " + long).slice(0, 240)],
    ["a\nb\r\nc", 200, "a\nb\r\nc"],
    ["  padded\n ", 160, "  padded\n "],
    ["a😀😀", 2, "a\uD83D"],
    ["e\u0301e\u0301", 3, "e\u0301e"],
    ["", 200, ""],
  ];
  for (const [e, n, want] of table) assert.equal(errText(e, n), want, `errText(${String(e).slice(0, 20)}, ${n})`);
  for (const [e, n] of table) assert.equal(errText(e, n), String(e).slice(0, n), "String(e).slice(0, n) と違う");
  tests++;
}

// ラベルの色の変更・ラベル / フォルダの検索置換: 本体の操作と、その後の読み直し（サイドバー・一覧・詳細）の成否の 4 通りで、最後に残る toast の字。
// 読み直しの失敗は呼び手へ漏らさない（前は未捕捉の reject = パネルが閉じず unhandled.ts の「処理に失敗しました」）・成功の件数は消さずに
// 「— ✗ 再読込に失敗: …」を後ろに連ねる（deleteLabels / mergeLabels と同じ作法）・本体の操作の失敗は今までどおりその失敗の toast
{
  // 土台（fakeInvoke = inv.invoke）に委ね、この場面の command だけを上書きする（表に無い command は土台が投げ、最後の inv.done() が落とす）
  const base = globalThis.__invoke;
  let failOp = false;
  let failReload = false;
  let failDetail = false;
  const renamed = [{ id: 1, status: "renamed", old: "a", new: "b" }];
  const OPS = { update_tag: null, find_replace_tags: renamed, find_replace_collections: renamed };
  globalThis.__invoke = async (name, payload) => {
    if (name in OPS) {
      calls.push({ name, args: payload?.args ?? payload ?? {} });
      if (failOp) throw new Error("書けない");
      return OPS[name];
    }
    if (name === "list_tags" && failReload) {
      calls.push({ name, args: payload?.args ?? payload ?? {} });
      throw new Error("ラベルを読み直せない");
    }
    if (name === "get_item" && failDetail) {
      calls.push({ name, args: payload?.args ?? payload ?? {} });
      throw new Error("詳細を読めない");
    }
    return base(name, payload);
  };
  const RELOAD = " — ✗ 再読込に失敗: Error: ラベルを読み直せない";
  for (const [label, run, done] of [
    ["色の変更", () => S.recolorLabels([1], "#ff0000"), "✓ 1 個のラベルの色を変更"],
    ["ラベルの検索置換", () => S.findReplaceLabels("a", "b"), "✓ 1 個の名前を変更"],
    ["フォルダの検索置換", () => S.findReplaceFolders("a", "b"), "✓ 1 個の名前を変更"],
  ]) {
    for (const [op, reload, want] of [
      [true, true, label === "色の変更" ? null : done], // 色の変更は成功を知らせない（今までどおり）
      [true, false, `${done}${RELOAD}`],
      [false, true, "✗ Error: 書けない"],
      [false, false, `✗ Error: 書けない${RELOAD}`],
    ]) {
      failOp = !op;
      failReload = !reload;
      S.app.toast = null;
      await assert.doesNotReject(run, `${label}（本体 ${op ? "成功" : "失敗"}・読み直し ${reload ? "成功" : "失敗"}）: 失敗が呼び手へ漏れた`);
      assert.equal(S.app.toast, want, `${label}（本体 ${op ? "成功" : "失敗"}・読み直し ${reload ? "成功" : "失敗"}）の toast`);
    }
    // 詳細を開いた状態で、読み直しの中の詳細（get_item）だけが失敗 = loadDetail は投げずに詳細を閉じて「✗ 詳細を読めません」を出す
    // → その toast が最後に残る（本体の成功の文で上書きして、詳細が閉じた理由を消さない）
    failOp = false;
    failReload = false;
    failDetail = true;
    S.app.selected = "D1";
    S.app.toast = null;
    await assert.doesNotReject(run, `${label}（詳細だけ失敗）: 失敗が呼び手へ漏れた`);
    assert.equal(S.app.toast, "✗ 詳細を読めません: Error: 詳細を読めない", `${label}（本体 成功・詳細の読み直しだけ失敗）: 詳細が閉じた理由が残らない`);
    assert.equal(S.app.selected, null, `${label}: 詳細を閉じない`);
    // 本体が失敗 + 詳細だけ失敗 = 本体の失敗の後に詳細の知らせが残る（前と同じ順）
    failOp = true;
    S.app.selected = "D1";
    S.app.toast = null;
    await assert.doesNotReject(run, `${label}（本体 失敗・詳細だけ失敗）: 失敗が呼び手へ漏れた`);
    assert.equal(S.app.toast, "✗ 詳細を読めません: Error: 詳細を読めない", `${label}（本体 失敗・詳細の読み直しだけ失敗）: 詳細が閉じた理由が残らない`);
    failOp = false;
    failDetail = false;
  }
  // 英語: 成功の件数（1 件は単数）+ 読み直しの失敗
  I18N.setLang("en");
  failOp = false;
  failReload = true;
  for (const [run, want] of [
    [() => S.recolorLabels([1], "#ff0000"), "✓ 1 label recolored — ✗ Failed to reload: Error: ラベルを読み直せない"],
    [() => S.recolorLabels([1, 2], "#ff0000"), "✓ 2 labels recolored — ✗ Failed to reload: Error: ラベルを読み直せない"],
    [() => S.findReplaceLabels("a", "b"), "✓ 1 renamed — ✗ Failed to reload: Error: ラベルを読み直せない"],
  ]) {
    await run();
    assert.equal(S.app.toast, want, "英語の toast");
  }
  I18N.setLang("ja");
  globalThis.__invoke = base;
  S.app.toast = null;
  tests++;
}

// 日本語の表示で、LIBRARY の側の操作の語に英語の固定の文字が残らない（語の表 = 前の英語）・英語の表示は前の字のまま・固有の名前は残る。
// 場面 = 左バーの「…」（フォルダ / LABELS・並びのサブメニュー）・フォルダの行のメニュー・新規 / 名前の変更 / 削除の窓・フォルダ / ラベルを管理の窓と
// 検索と置換・新規ラベルの窓・詳細（抄録・キーワード・添付の置き方・取得元）・左バー（スター付き）
{
  const EN_WORDS = [
    "Sort folders", "Sort labels", "Alphabetically", "Number of references", "Manage folders…", "Manage labels…", "Rename…", "Create subfolder…",
    "Delete…", "★ Starred", "Search labels", "New label", "New folder", "New subfolder", "Label name", "Folder name", "Create label", "Create folder",
    "Cancel", "Merge", "Find and replace", "Delete", "Color", "Type to search", "Replace with", "Replace", "Save", "Rename folder", "Delete folder",
    "Manage labels", "Manage folders", "Abstract", "Keywords / MeSH", "source:", "linked", "labels total", "folders total", "+ New label", "+ New folder",
    "Apply changes", "Search or create new label",
  ];
  const PROPER = ["LIBRARY", "LABELS"];
  // 日本語の表示で出てよい英単語 = 固有の名前（モード・節の呼び名・アプリ名・識別子・形式）+ この試験の場面のデータ（添付 a.pdf・キーワード kw）
  const SCREEN_ALLOW = new Set(["LIBRARY", "NOTE", "SHELF", "LABELS", "OpenRef", "OpenNote", "DOI", "PMID", "PMCID", "PDF", "MeSH", "pdf", "kw"]);
  const words = () => {
    const out = [];
    const w = document.createTreeWalker(document.body, 4);
    for (let n; (n = w.nextNode()); ) if (n.data.trim()) out.push(n.data.trim());
    for (const el of document.querySelectorAll("[placeholder],[title],[aria-label]")) for (const a of ["placeholder", "title", "aria-label"]) if (el.getAttribute(a)) out.push(el.getAttribute(a));
    return out.join("\n");
  };
  const has = (text, w) => new RegExp(`(^|[^A-Za-z])${w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}($|[^A-Za-z])`).test(text);
  const keep = { collections: S.app.collections, tags: S.app.tags, detail: S.app.detail, selected: S.app.selected, detailOpen: S.app.detailOpen };
  S.app.modal = null;
  S.app.editorOpen = false;
  S.app.importJob = null;
  await S.openTab();
  await settle();
  assert.equal(S.frontKind(), "list", "前提: 一覧のタブが前面");
  S.app.collections = [{ id: 901, key: "C901", name: "フォルダ甲", parentId: null, nItems: 1 }, { id: 904, key: "C904", name: "フォルダ丁", parentId: null, nItems: 0 }];
  S.app.tags = [{ id: 902, name: "ラベル乙", color: null, nItems: 1 }, { id: 903, name: "ラベル丙", color: null, nItems: 0 }];
  await settle();
  const scenes = [
    ["左バー", async () => {}],
    ["フォルダの …", async () => { S.app.labelMenu = "folders"; await settle(); $(".lmenu-parent")?.click(); }],
    ["LABELS の …", async () => { S.app.labelMenu = "labels"; await settle(); $(".lmenu-parent")?.click(); }],
    ["フォルダの行のメニュー", async () => (S.app.folderMenu = { id: 901, top: 1, left: 1 })],
    ["新規フォルダ", async () => { S.app.folderTarget = { id: null, parentId: null }; S.app.modal = "newFolder"; }],
    ["新規サブフォルダ", async () => { S.app.folderTarget = { id: null, parentId: 901 }; S.app.modal = "newFolder"; }],
    ["名前の変更", async () => { S.app.folderTarget = { id: 901, parentId: null }; S.app.modal = "renameFolder"; }],
    ["フォルダの削除", async () => { S.app.folderTarget = { id: 901, parentId: null }; S.app.modal = "deleteFolder"; }],
    ["フォルダを管理・検索と置換・削除", async () => { S.openFolderModal("manageFolders"); await settle(); [...document.querySelectorAll(".ml-btn")][1]?.click(); await settle(); }],
    ["ラベルを管理・検索と置換", async () => { S.app.modal = "manageLabels"; await settle(); [...document.querySelectorAll(".ml-btn")].find((b) => /検索と置換|Find and replace/.test(b.textContent))?.click(); await settle(); }],
    ["新規ラベル", async () => (S.app.modal = "newLabel")],
    ["ツールバーのラベル ▾（変更あり）", async () => {
      S.app.checked.add(S.app.items[0].key);
      await settle();
      [...document.querySelectorAll(".lb-btn")].find((b) => /ラベル|Label/.test(b.textContent))?.click();
      await settle();
      document.querySelector(".lb-picker .pop-row")?.click();
    }],
    ["詳細", async () => {
      S.app.detailOpen = true;
      S.app.selected = "K1";
      S.app.detail = { key: "K1", csl: { title: "題名 K1", abstract: "要旨", keyword: "kw" }, attachments: [{ id: 1, filename: "a.pdf", path: "/x/a.pdf", mode: "linked", exists: true }], collections: [], tags: [], notes: [], dateAdded: "2026-01-01", deleted: false };
    }],
  ];
  const close = async () => {
    S.app.modal = null;
    S.app.labelMenu = false;
    S.app.folderMenu = null;
    S.app.listMenu = null;
    S.app.checked.clear();
    await settle();
  };
  for (const lang of ["ja", "en"]) {
    prefs.ui_lang = lang;
    await S.loadLangPref();
    await settle();
    const seenEn = new Set();
    for (const [label, open] of scenes) {
      await open();
      await settle();
      const text = words();
      for (const w of EN_WORDS) if (has(text, w)) seenEn.add(w);
      if (lang === "ja") {
        const left = EN_WORDS.filter((w) => has(text, w));
        assert.deepEqual(left, [], `日本語の表示の「${label}」に英語の操作の語が残った`);
        // 許した名前の表で守る（表に無い英語が新しく混ざっても落ちる）: 英単語は固有の名前・場面のデータだけ
        const extra = [...new Set(text.match(/[A-Za-z][A-Za-z-]+/g) ?? [])].filter((w) => !SCREEN_ALLOW.has(w));
        assert.deepEqual(extra, [], `日本語の表示の「${label}」に許した名前の外の英単語`);
      }
      for (const p of PROPER) if (label === "左バー") assert.ok(has(text, p), `${lang}: 固有の名前 ${p} が無い`);
      await close();
    }
    // 英語の表示 = 前の英語の字が出る（語の表の全部が、どこかの場面に出る）
    if (lang === "en") assert.deepEqual(EN_WORDS.filter((w) => !seenEn.has(w)), [], "英語の表示で前の英語の字が出ない（英語の値を変えた）");
  }
  // 件数の文: ja =「N 個のラベル / フォルダ」・en = 前の英語（1 個は単数形 = tn）
  for (const [lang, n, lab, fol] of [["ja", 2, "2 個のラベル", "2 個のフォルダ"], ["en", 2, "2 labels total", "2 folders total"], ["ja", 1, "1 個のラベル", "1 個のフォルダ"], ["en", 1, "1 label total", "1 folder total"]]) {
    prefs.ui_lang = lang;
    await S.loadLangPref();
    S.app.collections = S.app.collections.slice(0, n);
    S.app.tags = S.app.tags.slice(0, n);
    S.app.modal = "manageLabels";
    await settle();
    assert.equal($(".ml-total")?.textContent.trim(), lab, `${lang}: ラベルの件数の文（${n} 個）`);
    S.app.modal = null;
    S.openFolderModal("manageFolders");
    await settle();
    assert.equal($(".ml-total")?.textContent.trim(), fol, `${lang}: フォルダの件数の文（${n} 個）`);
    S.app.modal = null;
    await settle();
  }
  // ★ の一括の toast: ja =「✓ N 件に ★ を付けました / の ★ を外しました」・en = 前の英語「✓ N item(s) starred / unstarred」
  for (const [lang, on, want] of [["ja", true, "✓ 1 件に ★ を付けました"], ["ja", false, "✓ 1 件の ★ を外しました"], ["en", true, "✓ 1 item starred"], ["en", false, "✓ 1 item unstarred"]]) {
    prefs.ui_lang = lang;
    await S.loadLangPref();
    S.app.checked.clear();
    S.app.checked.add(S.app.items[0].key);
    await S.bulkStar(on);
    assert.equal(S.app.toast, want, `${lang}: ★ の toast`);
  }
  S.app.checked.clear();
  prefs.ui_lang = "ja";
  await S.loadLangPref();
  Object.assign(S.app, keep);
  await S.closeTab(S.app.tab);
  await settle();
  tests++;
}

// チャット・本棚・タブ列の日本語の表示に、許した名前の外の英単語が出ない（上の表の仕組みに乗せる）・英語の表示は前の字のまま。
// 場面 = チャットの柱（ログイン済み・⋯・未ログイン・ログインの状態不明・claude なし・mcp なし・応答中の道具・終了の注記・新しい会話の確認・
// 変更の一覧と元に戻す）・同意の窓・本棚（グリッド・名前のメニュー・並び ▾）。タブ列はどの場面にも出る（本の読み手 = shelf/viewer.test）。
// 許す = 固有の名前の単語（上の表・Claude / Claude Code / CLI / Anthropic・キーの名前・Finder・モデルの名前・容量の単位）と、まとまりで外す
// 固有の名前（コマンド claude auth login / logout・openref-mcp・wiki_write / wiki_move・CLAUDE.md・~/.claude/projects/・架空のアカウント）。
// 一般の英単語（login・write・user 等）は単独では通さない（理由: 誤って通る語が増える）。データの要素（道具の名前と引数と結果・変更のパス・本棚の名前）は字を集めない
{
  const ALLOW = new Set(["LIBRARY", "NOTE", "SHELF", "LABELS", "OpenRef", "OpenNote", "DOI", "PMID", "PMCID", "PDF", "MeSH", "Claude", "Code", "CLI",
    "Anthropic", "Enter", "Finder", "Fable", "Opus", "Sonnet", "Haiku", "KB", "MB", "GB"]);
  const PHRASES = ["claude auth login", "claude auth logout", "claude を", "claude コマンド", "openref-mcp", "wiki_write", "wiki_move", "CLAUDE.md",
    "~/.claude/projects/", "Shift+Enter", "user@example.com"];
  // データの要素（利用者 / 試験のデータ = 訳さない）: 道具の名前・本棚 / フォルダ / 本の名前とパス
  const DATA = ".wk-ch-tool-name, .wk-ch-tool-arg, .wk-ch-tool-out, .wk-ch-change-label, .sh-head-name, .sh-menu-name, .sh-book-title, .sh-book-meta, .sh-chip, .sh-folder-name";
  // データの値そのもの（字の全体が一致する物だけ = タブの題名の本棚の名前・その title のパス・パンくずのフォルダ名）。パンくずの今の位置（.sh-crumb-cur）は
  // 要素ごとには外さない = 固定の字（すべての本・検索結果）も出る所（理由: 固定の字を取りこぼさない）
  const DATA_VALUES = new Set(["books", "/books", "books\n/books", "Anatomy Atlas", "Anatomy Atlas\n/books"]);
  const words = () => {
    const out = [];
    const w = document.createTreeWalker(document.body, 4);
    for (let n; (n = w.nextNode()); ) if (n.data.trim() && !DATA_VALUES.has(n.data.trim()) && !n.parentElement?.closest(DATA)) out.push(n.data.trim());
    for (const el of document.querySelectorAll("[placeholder],[title],[aria-label]"))
      if (!el.closest(DATA) && !el.matches(".sh-head, .sh-book, .sh-folder, .lmenu-item"))
        for (const a of ["placeholder", "title", "aria-label"]) if (el.getAttribute(a) && !DATA_VALUES.has(el.getAttribute(a))) out.push(el.getAttribute(a));
    for (const o of document.querySelectorAll("option")) out.push(o.textContent);
    return out.join("\n");
  };
  const enWords = (text) => [...new Set(PHRASES.reduce((x, p) => x.split(p).join(" "), text).match(/[A-Za-z][A-Za-z-]+/g) ?? [])].filter((w) => !ALLOW.has(w));
  const keep = { status: CH.chat.status, dock: CH.chat.dock.open };
  const LOGGED = { ...CH.chat.status, enabled: true, cli: "/c", version: "1", logged_in: true, mcp: "/m", auth: { email: "user@example.com" } };
  const notes = () => [...document.querySelectorAll(".wk-ch-note")].map((e) => e.textContent.trim()).filter((x) => /Claude Code/.test(x));
  const exitNote = () => notes().at(-1) ?? null; // いちばん新しい注記
  const SHELF = { root: "/books", dir: null, query: "", sort: "name", scrollTop: 0 };
  S.app.toast = null; // 上の節の toast（英語の表示で出した物）を持ち越さない
  await S.openTab();
  await settle();
  CH.setDockOpen(true);
  await settle();
  const scenes = [
    ["柱（ログイン済み）", async () => (CH.chat.status = LOGGED), ".wk-ch-more"],
    ["柱の ⋯", async () => { CH.chat.status = LOGGED; await settle(); $(".wk-ch-more")?.click(); }, ".wk-ch-menu"],
    ["柱（未ログイン）", async () => (CH.chat.status = { ...LOGGED, logged_in: false, auth: null }), ".wk-ch"],
    ["柱（ログインの状態不明）", async () => (CH.chat.status = { ...LOGGED, logged_in: null, auth: null }), ".wk-ch"],
    ["柱（claude なし）", async () => (CH.chat.status = { ...LOGGED, cli: null, version: null, logged_in: null, mcp: null }), ".wk-ch"],
    ["柱（mcp なし）", async () => (CH.chat.status = { ...LOGGED, mcp: null }), ".wk-ch"],
    ["柱（道具と終了の注記）", async () => {
      CH.chat.status = LOGGED;
      CH.onChatEvent({ type: "line", turn: 1, line: JSON.stringify({ type: "assistant", message: { content: [{ type: "tool_use", id: "tu1", name: "Read", input: { file_path: "/v/甲.md" } }] } }) });
      CH.onChatEvent({ type: "exit", code: 1 });
    }, ".wk-ch-tool"],
    ["柱（新しい会話の確認）", async () => { CH.chat.status = LOGGED; CH.chat.confirmNew = true; }, ".wk-ch-ask"],
    ["柱（変更の一覧と元に戻す）", async () => {
      CH.chat.status = LOGGED;
      // 変更の一覧はターンの束（会話の先頭の束 = 上の場面の道具の turn）に付く
      const turn = CH.chat.conv.entries.find((e) => e.turn != null)?.turn ?? 1;
      CH.onChatEvent({ type: "changes", turn, changes: [{ kind: "write", path: "甲.md" }, { kind: "create", path: "乙.md" }, { kind: "move", from: "甲.md", to: "丙.md" }, { kind: "item", key: "K1", title: "題名" }] });
      CH.chat.undo = { [`${turn}:0`]: { busy: true }, [`${turn}:1`]: { ok: true }, [`${turn}:2`]: { ok: false, reason: "架空の理由" } };
    }, ".wk-ch-changes"],
    ["同意の窓", async () => (CH.chat.consent = true), ".wk-cc-list"],
    ["本棚", async () => { await S.openTab({ kind: "all" }, null, SHELF); }, ".sh-grid"],
    ["本棚の名前のメニュー", async () => { await S.openTab({ kind: "all" }, null, SHELF); await settle(); $(".sh-head")?.click(); }, ".sh-menu"],
    ["本棚の検索結果", async () => { await S.openTab({ kind: "all" }, null, { ...SHELF, query: "本" }); }, ".sh-crumb-cur"],
    ["本棚のフォルダ", async () => {
      // フォルダ名は英語のデータ（訳さない・字の全体で外す）
      shelfKids = [{ id: "f-/books/Anatomy Atlas/", name: "Anatomy Atlas", path: "/books/Anatomy Atlas", books: [], children: [], totalBooks: 0 }];
      SH.sh.trees = {}; // 前の場面で読んだ木（フォルダ無し）を読み直させる
      await S.openTab({ kind: "all" }, null, { ...SHELF, dir: "Anatomy Atlas" });
    }, ".sh-crumb"],
    ["本棚の並び ▾", async () => { await S.openTab({ kind: "all" }, null, SHELF); await settle(); $(".lb-sort")?.click(); }, ".lb-sort-pop"],
  ];
  const tab0 = S.app.tab;
  const close = async () => {
    CH.chat.consent = false;
    CH.chat.confirmNew = false;
    CH.chat.undo = {};
    CH.chat.changes = {};
    if (SH.sh.menu) SH.sh.menu = null;
    for (const t of S.app.tabs.filter((x) => x.shelf)) await S.closeTab(t.id);
    if (shelfKids.length) {
      shelfKids = [];
      SH.sh.trees = {}; // 次の場面は木を読み直す（フォルダの無い木）
    }
    S.app.tab = tab0;
    await settle();
  };
  for (const lang of ["ja", "en"]) {
    prefs.ui_lang = lang;
    await S.loadLangPref();
    await settle();
    for (const [label, open, marker] of scenes) {
      await open();
      await settle();
      assert.ok($(marker), `${lang}: 場面「${label}」が開かない（${marker}）`);
      const text = words();
      if (lang === "ja") {
        const extra = enWords(text);
        // 落ちたときに、その語を含む字の行も出す（どの部品の字か分かるように）
        assert.deepEqual(extra, [], `日本語の表示の「${label}」に許した名前の外の英単語: ${text.split("\n").filter((l) => extra.some((w) => l.includes(w))).slice(0, 5).join(" | ")}`);
      }
      // 固有の名前は残す（訳さない）
      // 未ログインの行: 説明の文と [ログイン] の title の両方にコマンドが残る（片方だけ訳しても落ちる）
      if (label === "柱（未ログイン）")
        assert.deepEqual(
          [[...document.querySelectorAll(".wk-ch .wk-ch-dim")].some((e) => /claude auth login/.test(e.textContent)), /claude auth login/.test($(".wk-ch .wk-ch-primary")?.title ?? ""), /Claude Code/.test(text)],
          [true, true, true],
          `${lang}: コマンド・製品の名前が残らない（説明の文 / [ログイン] の title / Claude Code）`,
        );
      if (label === "本棚") assert.ok(/SHELF/.test(text) && /Finder/.test(text), `${lang}: SHELF / Finder が残らない`);
      // パンくずの今の位置: 固定の字は訳す・フォルダ名はそのまま
      if (label.startsWith("本棚")) {
        const want = { 本棚: ["すべての本", "All books"], 本棚の検索結果: ["検索結果", "Search results"], 本棚のフォルダ: ["Anatomy Atlas", "Anatomy Atlas"] }[label];
        if (want) assert.equal($(".sh-crumb-cur")?.textContent.trim(), want[lang === "ja" ? 0 : 1], `${lang}: 「${label}」のパンくず`);
      }
      if (label === "柱（道具と終了の注記）")
        assert.equal(exitNote(), lang === "ja" ? "Claude Code が終了しました (終了コード 1) · 次の送信で起動し直します" : "Claude Code exited (code 1) · it will start again on the next message", `${lang}: 終了の注記`);
      await close();
    }
    // シグナルで終わった = ja「シグナル SIGKILL」・en = 前の字「signal SIGKILL」
    CH.onChatEvent({ type: "exit", signal: "SIGKILL" });
    await settle();
    assert.equal(exitNote(), lang === "ja" ? "Claude Code が終了しました (シグナル SIGKILL) · 次の送信で起動し直します" : "Claude Code exited (signal SIGKILL) · it will start again on the next message", `${lang}: シグナルの終了の注記`);
    // 名前の無いシグナル = Rust は番号だけ（"30"）→「signal」は 1 つ。前の版の Rust の "signal 30"（注記 "signal signal 30"）も描くときに 1 つ
    const UNNAMED = lang === "ja" ? "Claude Code が終了しました (シグナル 30) · 次の送信で起動し直します" : "Claude Code exited (signal 30) · it will start again on the next message";
    for (const signal of ["30", "signal 30"]) {
      CH.onChatEvent({ type: "exit", signal });
      await settle();
      assert.equal(exitNote(), UNNAMED, `${lang}: 名前の無いシグナルの終了の注記（Rust の signal = ${JSON.stringify(signal)}）`);
    }
  }
  // 注記が出た後に言語を切り替える（Rust の emit "ui-lang" = メニューの切替）= 出ている注記も今の言語の字（来た時点で訳を焼き込まない）
  {
    const JA = ["Claude Code が終了しました (終了コード 1) · 次の送信で起動し直します", "Claude Code が終了しました (シグナル SIGKILL) · 次の送信で起動し直します"];
    const EN = ["Claude Code exited (code 1) · it will start again on the next message", "Claude Code exited (signal SIGKILL) · it will start again on the next message"];
    // 注記は日本語の表示で出す（英語で出すと、焼き込んだ字 "code 1" が言語に依らない値と同じになり、焼き込みを見分けられない = 焼き込みが検出されない）
    emit("ui-lang", "ja");
    await settle();
    CH.chat.status = LOGGED;
    CH.onChatEvent({ type: "exit", code: 1 });
    CH.onChatEvent({ type: "exit", signal: "SIGKILL" });
    await settle();
    assert.deepEqual(notes().slice(-2), JA, "日本語の表示で出した終了の注記");
    for (const [lang, want] of [["en", EN], ["ja", JA], ["en", EN]]) {
      emit("ui-lang", lang);
      await settle();
      assert.deepEqual(notes().slice(-2), want, `言語を ${lang} に切り替えた後の終了の注記`);
    }
  }
  prefs.ui_lang = "ja";
  await S.loadLangPref();
  CH.chat.status = keep.status;
  CH.setDockOpen(keep.dock);
  await S.closeTab(S.app.tab);
  await settle();
  tests++;
}

// OpenNote の日本語の表示に、許した名前の外の英単語が出ない（上の表の仕組みに乗せる）・英語の表示は前の字のまま。
// 場面 = ホーム（左バーの箱・右ペインの概要）・タグの無い左バー・名前のメニュー・フォルダの設定の窓・木の右クリック（フォルダ / ファイル / 除外フォルダ）・[＋ 新規] / ページの [⋯] のメニュー・
// 新規ページ / 新規フォルダ / 名前を変更 / ゴミ箱の窓（ページを開いた右ペイン・本文・エディタ・検索の結果は
// ここでは開かない = 鍵は i18n.test が全部見る）。
// 許す = 固有の名前の単語（上の表・OpenNote・Markdown / HTML / md / html・Finder・macOS・PDF・URL・frontmatter・新規ページの既定の名前 Untitled）と、
// まとまりで外す固有の名前（フォルダの実名 ingest/ ・chat-log/・file://・UTF-8）。左バーの箱の名前（Ingest / Chat log / Tags）と右ペインの概要の見出し
// （OpenNote）は節の呼び名 = LIBRARY の LABELS と同じ扱いで残す = 要素ごとに外す。利用者のデータ（ページ名・フォルダ名・タグ・パス・
// ワークスペース名・frontmatter）は字を集めない（語の表の英語と同じ名前のページ「Tags」・タグ「Search」も訳さない事を見る）
{
  const ALLOW = new Set(["LIBRARY", "NOTE", "SHELF", "LABELS", "OpenRef", "OpenNote", "DOI", "PMID", "PMCID", "PDF", "MeSH", "Claude", "Code", "Finder",
    "Markdown", "HTML", "md", "html", "macOS", "URL", "frontmatter", "Untitled", "Esc", "Enter"]);
  const PHRASES = ["ingest/", "chat-log/", "file://", "UTF-8"];
  // 節の呼び名（案 A = 残す）: 左バーの箱の見出し・右ペインの概要の見出し
  const FIXED = ".wk-box-t, .wk-info > .wk-sec > h3";
  // 利用者のデータ（訳さない = 字を集めない）: 木の行・タグ・ワークスペース名とパス・ページの題名・目次・最近開いたページ・窓の中の対象のパス・文献の題
  // （メニューの見出しのうち title を持つ物 = パス / フォルダ名の見出し・ワークスペースの行。ほかの項目の字は集める = 字の足し込みを見逃さない）
  const DATA = ".wk-row, .wk-tag, .wk-vault, .wk-menu-head[title], .wk-menu-ws, .wk-fs-path, .wk-title, .wk-toc, .wk-home-recent, .wk-home h1, .wk-dlg-lead, .wk-dlg-note, .wk-card-title, .wk-link, .wk-fm, .wk-tags, .wk-info > .wk-sec > .wk-dim, .tabs-tab";
  // データの値そのもの（属性の値の全体が一致する物だけ = フォルダの設定の行のボタンの title = そのパス）
  const DATA_VALUES = new Set(["/Users/u/Papers", "archive"]);
  const textOf = () => {
    const out = [];
    const w = document.createTreeWalker(document.body, 4);
    for (let n; (n = w.nextNode()); ) {
      const el = n.parentElement;
      if (!n.data.trim() || !el?.closest(".wk-sidebar, .wk-page, .wk-info, .wk-menu, .overlay, .wk-head")) continue;
      if (el.closest(FIXED) || el.closest(DATA)) continue;
      out.push(n.data.trim());
    }
    for (const el of document.querySelectorAll(".wk-sidebar [title], .wk-sidebar [aria-label], .wk-page [title], .wk-page [aria-label], .wk-page [placeholder], .wk-info [title], .wk-menu [title], .wk-menu [aria-label], .overlay [placeholder], .overlay [title], .overlay [aria-label], .wk-head [title], .wk-head [aria-label], .wk-head [placeholder]"))
      if (!el.closest(DATA) && !el.matches("section.wk-box"))
        for (const a of ["placeholder", "title", "aria-label"]) if (el.getAttribute(a) && !DATA_VALUES.has(el.getAttribute(a))) out.push(el.getAttribute(a));
    return out.join("\n");
  };
  const enWords = (text) => [...new Set(PHRASES.reduce((x, p) => x.split(p).join(" "), text).match(/[A-Za-z][A-Za-z-]+/g) ?? [])].filter((w) => !ALLOW.has(w));
  const entry = (path, title) => ({ path, kind: "md", title, tags: [], items: [], mtime: 0, size: 1 });
  const keep = { status: W.wk.status, pages: W.wk.pages, tags: W.wk.tags, tagsTotal: W.wk.tagsTotal, inbox: W.wk.inbox, inboxTotal: W.wk.inboxTotal, chatLog: W.wk.chatLog, chatLogTotal: W.wk.chatLogTotal, excluded: W.wk.excluded, refDirs: W.wk.refDirs };
  S.app.toast = null;
  await S.openTab({ kind: "all" }, { path: null, root: "/v", scrollTop: 0, editing: false });
  await settle();
  const fill = () => {
    W.wk.status = { enabled: true, root: "/v", pages: 3, inbox: 1, workspaces: [{ root: "/v", last_used: 2, exists: true, current: true }, { root: "/w/旧ノート", last_used: 1, exists: false, current: false }] };
    // 語の表の英語と同じ名前のページ（Tags）・タグ（Search）= 利用者のデータ = 訳さない
    W.wk.pages = [entry("notes/a.md", "甲"), entry("Tags.md", "Tags"), entry("index.md", "index")];
    W.wk.tags = [{ tag: "Search", count: 2 }, { tag: "liver", count: 1 }];
    W.wk.tagsTotal = 2;
    W.wk.inbox = [{ path: "ingest/clip.md", name: "clip.md", kind: "md", media: null, size: 10, mtime: 0 }];
    W.wk.inboxTotal = 1;
    W.wk.chatLog = [];
    W.wk.chatLogTotal = 0;
    W.wk.excluded = ["archive"];
    W.wk.refDirs = ["/Users/u/Papers"];
  };
  const scenes = [
    ["ホーム（左バーの箱・右ペインの概要）", async () => { fill(); W.wk.page = null; }, ".wk-box"],
    ["タグの無い左バー", async () => { fill(); W.wk.tags = []; W.wk.tagsTotal = 0; }, ".wk-box"],
    ["名前のメニュー", async () => { fill(); W.wk.page = null; await settle(); $(".wk-vault")?.click(); }, ".wk-menu"],
    ["フォルダの設定の窓", async () => { fill(); W.openFolderSettings(); }, ".wk-fs"],
    ["木の右クリック（フォルダ）", async () => { fill(); W.wk.menu = { kind: "dir", path: "notes", x: 10, y: 10 }; }, ".wk-menu"],
    ["木の右クリック（ファイル）", async () => { fill(); W.wk.menu = { kind: "file", path: "notes/a.md", x: 10, y: 10 }; }, ".wk-menu"],
    ["木の右クリック（除外フォルダ）", async () => { fill(); W.wk.menu = { kind: "excluded", path: "archive", x: 10, y: 10 }; }, ".wk-menu"],
    ["[＋ 新規] のメニュー", async () => { fill(); W.wk.menu = { kind: "new", path: "", x: 10, y: 10 }; }, ".wk-menu"],
    ["ページの [⋯] のメニュー", async () => { fill(); W.wk.menu = { kind: "page", path: "notes/a.md", x: 10, y: 10, abs: "/v/notes/a.md", canEdit: true, md: true }; }, ".wk-menu"],
    ["新規ページの窓", async () => { fill(); W.wk.dialog = { kind: "new", dir: "" }; }, ".overlay"],
    ["新規フォルダの窓", async () => { fill(); W.wk.dialog = { kind: "newdir", dir: "notes" }; }, ".overlay"],
    ["名前を変更の窓", async () => { fill(); W.wk.dialog = { kind: "rename", path: "notes/a.md", preview: null, to: "" }; }, ".overlay"],
    ["ゴミ箱の窓", async () => { fill(); W.wk.dialog = { kind: "trash", path: "notes", dir: true }; }, ".overlay"],
  ];
  const close = async () => {
    W.wk.menu = null;
    W.wk.dialog = null;
    await settle();
  };
  // 語を変えた場面の英語の字 = 前の字（鍵を日本語にした物の値）
  const EN_BEFORE = { "タグの無い左バー": "No tags" };
  for (const lang of ["ja", "en"]) {
    prefs.ui_lang = lang;
    await S.loadLangPref();
    await settle();
    for (const [label, open, marker] of scenes) {
      await open();
      await settle();
      assert.ok($(marker), `${lang}: 場面「${label}」が開かない（${marker}）`);
      const text = textOf();
      if (lang === "ja") {
        const extra = enWords(text);
        assert.deepEqual(extra, [], `日本語の表示の「${label}」に許した名前の外の英単語: ${text.split("\n").filter((l) => extra.some((w) => l.includes(w))).slice(0, 5).join(" | ")}`);
        if (label === "タグの無い左バー") assert.ok(text.includes("タグはありません"), `ja: タグの無い左バーの文: ${text}`);
      } else if (EN_BEFORE[label]) assert.ok(text.includes(EN_BEFORE[label]), `en: 「${label}」の字が前と違う（${EN_BEFORE[label]}）: ${text}`);
      // 節の呼び名（案 A）は両方の言語で残る・利用者のデータは訳さない
      if (label.startsWith("ホーム")) {
        assert.deepEqual([...document.querySelectorAll(".wk-box-t")].map((e) => e.textContent.trim()), ["Ingest", "Chat log", "Tags"], `${lang}: 左バーの箱の名前`);
        assert.ok([...document.querySelectorAll(".wk-tag")].some((e) => e.textContent.includes("Search")), `${lang}: タグ「Search」（データ）が訳された`);
        assert.ok([...document.querySelectorAll(".wk-row")].some((e) => e.textContent.trim() === "Tags"), `${lang}: ページ「Tags」（データ）が訳された`);
      }
      await close();
    }
  }
  prefs.ui_lang = "ja";
  await S.loadLangPref();
  Object.assign(W.wk, keep);
  W.wk.page = null;
  await S.closeTab(S.app.tab);
  await settle();
  tests++;
}

// 件数の語を物ごとに分ける。LIBRARY の文献の数 = "件"（英語 items）・引用スタイルの検索の結果 = "件::styles"（styles・
// 「Search results: 12 results」と重ねない）・OpenNote の検索の結果 = "件::results"（results）。日本語の表示はどれも「件」
{
  const en = (await import("./i18n-en.ts")).en;
  assert.deepEqual([en["件"], en["件::styles"], en["件::results"]], ["items", "styles", "results"], "件数の語の英語");
  // 引用スタイルの窓（実部品）: 検索の見出し = 日本語「検索結果 3 件」/ 英語「Search results: 3 styles」
  // 窓は索引（cite_catalog）を最初の 1 回だけ読む = 検索の対象は自作のスタイル（導入済みの custom）で入れる
  const keepInstalled = S.app.cite.installed;
  S.app.cite.installed = [
    { id: "radiology", title: "Radiology", format: "numeric", custom: true },
    { id: "american-journal-of-roentgenology", title: "American Journal of Roentgenology", short: "AJR", format: "numeric", custom: true },
    { id: "vancouver", title: "Vancouver", format: "numeric", custom: true },
  ];
  const head = async (lang, word = "r") => {
    prefs.ui_lang = lang;
    await S.loadLangPref();
    S.app.modal = "citeStyle";
    await settle();
    await waitFor(() => document.querySelector(".sm-q"));
    const q = document.querySelector(".sm-q");
    q.value = word;
    q.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    const t = document.querySelector(".sm-sec")?.textContent.trim();
    S.app.modal = null;
    await settle();
    return t;
  };
  assert.equal(await head("ja"), "検索結果 3 件", "日本語の表示の引用スタイルの検索の件数");
  assert.equal(await head("en"), "Search results: 3 styles", "英語の表示の引用スタイルの検索の件数（前 = 3 items）");
  // 1 件 = 単数形（「1 styles」にしない）・0 件は複数形のまま
  assert.deepEqual([await head("en", "vancouver"), await head("en", "zzzz")], ["Search results: 1 style", "Search results: 0 styles"], "英語の表示の 1 件 / 0 件");
  assert.deepEqual([await head("ja", "vancouver"), await head("ja", "zzzz")], ["検索結果 1 件", "検索結果 0 件"], "日本語の表示の 1 件 / 0 件");
  // 200 件で打ち切った「200+」= 複数形のまま・数と単位の間の空白は 1 つ（前は「200+  件」と 2 つ）
  S.app.cite.installed = Array.from({ length: 205 }, (_, i) => ({ id: `s-${i}`, title: `Style ${i}`, format: "numeric", custom: true }));
  assert.deepEqual([await head("en", "style"), await head("ja", "style")], ["Search results: 200+ styles", "検索結果 200+ 件"], "200 件以上の見出し");
  // tn の単数形（OpenNote の検索の結果 = WikiResults の tn(…, "件::results")）・ほかの語は前のまま
  I18N.setLang("en");
  assert.deepEqual([I18N.tn(1, "件::results"), I18N.tn(12, "件::results"), I18N.tn(0, "件::results"), I18N.tn(1, "件::styles"), I18N.tn(1, "件"), I18N.tn(2, "件")], ["1 result", "12 results", "0 results", "1 style", "1 item", "2 items"], "tn の単数形");
  I18N.setLang("ja");
  assert.deepEqual([I18N.tn(1, "件::results"), I18N.tn(12, "件::results")], ["1 件", "12 件"], "日本語の tn");
  prefs.ui_lang = "ja";
  await S.loadLangPref();
  S.app.cite.installed = keepInstalled;
  // OpenNote の検索の結果（WikiResults）: 日本語の表示では鍵を戻しても字が同じ = 使う鍵を字面で見る（英語の値は上）
  assert.match(readFileSync(new URL("./wiki/WikiResults.svelte", import.meta.url), "utf8"), /\$\{tn\(wk\.resultsTotal, "件::results"\)\}/, "OpenNote の検索の件数が専用の鍵でない");
  assert.match(readFileSync(new URL("./StyleModal.svelte", import.meta.url), "utf8"), /tn\(hits\.length, "件::styles"\)/, "引用スタイルの検索の件数が専用の鍵（tn）でない");
  tests++;
}

// 窓（本体のモーダル・OpenNote のダイアログ）・参照エディタが開いている間は PDF のドロップを受けない（取込の窓がその下に開いて
// 見えない窓が焦点を持つ）= handleDrop を呼ばない・ドロップの枠を出さない・理由を toast。覆いが無ければ今のまま取り込む。
// 取込の窓だけが開いているときは今のまま（実行中 = 末尾に足す）
{
  const drop = (type, paths = []) => globalThis.__drop({ payload: { type, paths } });
  const imports = () => calls.filter((c) => c.name === "import_pdf").map((c) => c.args.path);
  S.app.importJob = null;
  S.app.toast = "";
  await S.openTab({ kind: "all" }); // 一覧のタブ（前の節の前面は OpenNote）
  await settle();
  const listTab = S.app.tab;
  assert.equal(S.frontKind(), "list", "前提: 前面が一覧");
  const blockedBy = async (name, open, close) => {
    open();
    await settle();
    const before = imports().length;
    drop("enter");
    const enter = S.app.dropActive;
    drop("over");
    drop("over");
    const over = S.app.dropActive;
    const toastBeforeDrop = S.app.toast; // enter / over では理由を出さない（落としたときに 1 回だけ）
    drop("drop", ["/d/a.pdf"]);
    await settle();
    const out = { enter, over, toastBeforeDrop, importJob: !!S.app.importJob, called: imports().length - before, toast: S.app.toast };
    close();
    S.app.toast = "";
    await settle();
    assert.deepEqual(out, { enter: false, over: false, toastBeforeDrop: "", importJob: false, called: 0, toast: "開いている窓を閉じてからドロップしてください" }, `${name}の間のドロップ`);
  };
  await blockedBy("参照エディタ", () => S.openEditorNew(), () => S.closeEditor());
  await blockedBy("引用スタイルの窓", () => (S.app.modal = "citeStyle"), () => (S.app.modal = null));
  await blockedBy("ラベルを管理の窓", () => (S.app.modal = "manageLabels"), () => (S.app.modal = null));
  // OpenNote / チャットの窓（modalOpen の guard）= チャットの同意の窓は一覧のタブの上にも出る
  await blockedBy("チャットの同意の窓", () => (CH.chat.consent = true), () => (CH.chat.consent = false));
  // 枠が出た後に窓が開いた（enter は覆いなし → 窓 → leave / drop）= 受けない枝でも枠を下ろす（残らない）
  for (const last of ["leave", "drop"]) {
    drop("enter");
    assert.equal(S.app.dropActive, true, "前提: 覆いなしの enter で枠が出る");
    S.openEditorNew();
    await settle();
    drop(last, last === "drop" ? ["/d/a.pdf"] : []);
    await settle();
    assert.deepEqual([S.app.dropActive, !!S.app.importJob], [false, false], `枠が出た後に窓が開いて ${last} = 枠が残る / 取り込んだ`);
    S.closeEditor();
    S.app.toast = "";
    await settle();
  }
  // 英語の表示の理由
  prefs.ui_lang = "en";
  await S.loadLangPref();
  S.openEditorNew();
  await settle();
  drop("drop", ["/d/a.pdf"]);
  await settle();
  assert.equal(S.app.toast, "Close the open window before dropping files", "英語の表示の理由");
  S.closeEditor();
  prefs.ui_lang = "ja";
  await S.loadLangPref();
  S.app.toast = "";
  await settle();
  // 覆いが無い = 今のまま: 枠が出る・PDF でない物は理由・PDF は取込の窓で取り込む
  drop("enter");
  assert.equal(S.app.dropActive, true, "覆いが無いのにドロップの枠が出ない");
  drop("leave");
  assert.equal(S.app.dropActive, false);
  drop("drop", ["/d/note.txt"]);
  await settle();
  assert.deepEqual([S.app.toast, !!S.app.importJob], ["PDF ファイルをドロップしてください", false], "PDF でない物のドロップ");
  let open;
  importGate = new Promise((r) => (open = r));
  const n0 = imports().length;
  drop("drop", ["/d/b.pdf"]);
  await settle();
  assert.deepEqual([!!S.app.importJob, imports().slice(n0)], [true, ["/d/b.pdf"]], "覆いが無いのに取り込まない");
  // 取込の窓が開いている（実行中）間のドロップ = 今のまま末尾に足す（枠も出る）
  drop("enter");
  assert.equal(S.app.dropActive, true, "取込の窓の間にドロップの枠が出ない（前は出た）");
  drop("drop", ["/d/c.pdf"]);
  await settle();
  assert.deepEqual(S.app.importJob.files.map((f) => f.path), ["/d/b.pdf", "/d/c.pdf"], "取込の窓の間のドロップが末尾に足されない");
  open();
  importGate = null;
  await waitFor(() => !S.app.importJob.running);
  assert.deepEqual(imports().slice(n0), ["/d/b.pdf", "/d/c.pdf"], "足した PDF を取り込まない");
  // 覆いに入らない浮きメニュー（「＋ 追加 ▾」）の間 = 今のまま受ける（窓は閉じた後の取込の窓が新しく開く）
  S.app.importJob = null;
  S.app.newMenu = true;
  await settle();
  drop("drop", ["/d/d.pdf"]);
  await settle();
  await waitFor(() => S.app.importJob && !S.app.importJob.running);
  assert.deepEqual(S.app.importJob.files.map((f) => f.path), ["/d/d.pdf"], "浮きメニューの間のドロップ（今のまま受ける）");
  S.app.newMenu = false;
  S.app.importJob = null;
  S.app.toast = "";
  // OpenNote のタブ: 窓（新規 / 改名 / 削除のダイアログ・外部の書き換えの競合・救出・引用の窓・チャットの同意・本体の窓・参照エディタ・
  // 画像の拡大表示）が開いている間は wikiDrop へ流さない（編集中なら窓の下のページの caret に書いていた）。
  // 覆いが無ければ今のまま wikiDrop（編集していない = 「ページへは編集モードで…」）
  {
    const L = await import("./wiki/lightbox.svelte.ts");
    const CI = await import("./wiki/cite.svelte.ts");
    await S.openTab({ kind: "all" }, { path: null, root: "/v", scrollTop: 0, editing: false });
    await settle();
    const wikiTab = S.app.tab;
    assert.equal(S.frontKind(), "wiki", "前提: 前面が OpenNote");
    const wimports = () => calls.filter((c) => c.name === "wiki_import_file").length;
    const wdrop = async (name, open, close, want) => {
      open?.();
      await settle();
      const b = wimports();
      drop("enter");
      drop("over");
      drop("over");
      const frame = S.app.dropActive;
      const toastBeforeDrop = S.app.toast; // OpenNote の枝でも enter / over では理由を出さない（落としたときに 1 回だけ = R5c）
      drop("drop", ["/d/figure.png"]);
      await settle();
      const out = { frame, toastBeforeDrop, toast: S.app.toast, wikiImport: wimports() - b, importJob: !!S.app.importJob };
      close?.();
      S.app.toast = "";
      await settle();
      assert.deepEqual(out, { frame: false, toastBeforeDrop: "", toast: want, wikiImport: 0, importJob: false }, `OpenNote のタブ・${name}`);
    };
    const SHUT = "開いている窓を閉じてからドロップしてください";
    await wdrop("覆いなし（今のまま）", null, null, "ページへは編集モードでドロップしてください（蔵書には取り込みません）");
    await wdrop("新規ページの窓", () => (W.wk.dialog = { kind: "new", dir: "" }), () => (W.wk.dialog = null), SHUT);
    await wdrop("名前の変更の窓", () => (W.wk.dialog = { kind: "rename", path: "a.md", preview: null, to: "b.md" }), () => (W.wk.dialog = null), SHUT);
    await wdrop("削除の窓", () => (W.wk.dialog = { kind: "trash", path: "a.md", dir: false }), () => (W.wk.dialog = null), SHUT);
    await wdrop("外部の書き換えの競合の窓", () => (ED.ed.dialog = true), () => (ED.ed.dialog = false), SHUT);
    await wdrop("救出の窓", () => (ED.ed.stranded = [{ path: "a.md", content: "x", reason: "r" }]), () => (ED.ed.stranded = []), SHUT);
    await wdrop("引用の窓", () => (CI.cite.open = true), () => (CI.cite.open = false), SHUT);
    await wdrop("チャットの同意の窓", () => (CH.chat.consent = true), () => (CH.chat.consent = false), SHUT);
    await wdrop("画像の拡大表示", () => L.openLightbox({ images: [{ src: "data:image/png;base64,", alt: "", nav: null }], index: 0, onNavigate: () => {} }), () => L.closeLightbox(), SHUT);
    await wdrop("参照エディタ", () => S.openEditorNew(), () => S.closeEditor(), SHUT);
    await wdrop("引用スタイルの窓", () => (S.app.modal = "citeStyle"), () => (S.app.modal = null), SHUT);
    await S.closeTab(wikiTab);
    await settle();
    // 編集中のタブ（編集中に窓の間に落とすと窓の下のページに書いていた）: 窓の間は wikiDrop へ流さない。
    // 覆いが無ければ wikiDrop へ行く（この試験ではエディタの部品が無い = dropPaths が受けられず「編集モードで…」= wikiDrop まで届いた印）
    await S.openTab({ kind: "all" }, { path: null, root: "/v", scrollTop: 0, editing: true });
    await settle();
    const editTab = S.app.tab;
    assert.equal(S.app.tabs.find((x) => x.id === editTab)?.wiki?.editing, true, "前提: 前面の OpenNote のタブが編集中");
    await wdrop("編集中・覆いなし（wikiDrop へ届く）", null, null, "ページへは編集モードでドロップしてください（蔵書には取り込みません）");
    await wdrop("編集中・引用の窓", () => (CI.cite.open = true), () => (CI.cite.open = false), SHUT);
    await wdrop("編集中・新規ページの窓", () => (W.wk.dialog = { kind: "new", dir: "" }), () => (W.wk.dialog = null), SHUT);
    await wdrop("編集中・画像の拡大表示", () => L.openLightbox({ images: [{ src: "data:image/png;base64,", alt: "", nav: null }], index: 0, onNavigate: () => {} }), () => L.closeLightbox(), SHUT);
    await S.closeTab(editTab);
    await settle();
  }
  await S.closeTab(listTab);
  await settle();
  tests++;
}

// toast は窓の覆いの上（窓の間の「開いている窓を閉じて…」を覆いの下に沈めない）・下の押下を奪わない（字だけ）。
// jsdom では重なりが効かない = 字面で縛る
{
  const zOf = (src, sel) => Number(new RegExp(`${sel.replace(/[.]/g, "\\.")} \\{[^}]*?z-index: (\\d+)`).exec(src)?.[1] ?? NaN);
  const css = readFileSync(new URL("../app.css", import.meta.url), "utf8");
  const lb = readFileSync(new URL("./wiki/WikiLightbox.svelte", import.meta.url), "utf8");
  const ci = readFileSync(new URL("./wiki/WikiCite.svelte", import.meta.url), "utf8");
  const toastZ = zOf(css, ".toast");
  const others = { overlay: zOf(css, ".overlay"), lmenu: zOf(css, ".lmenu"), dropzone: zOf(css, ".dropzone"), lightbox: zOf(lb, ".wk-lb"), cite: zOf(ci, ".wk-cite-back") };
  for (const [k, z] of Object.entries(others)) assert.ok(Number.isFinite(z) && toastZ > z, `toast（z ${toastZ}）が ${k}（z ${z}）の下`);
  assert.match(/\.toast \{([^}]*)\}/.exec(css)?.[1] ?? "", /pointer-events: none;/, "toast が下の押下を奪う");
  tests++;
}

inv.done();
console.log(`app shell: ok (${tests} tests)`);
process.exit(0);
