/* OpenNote の集中モードの試験（node app/src/lib/wiki/focus.test.mjs・実部品 App.svelte + jsdom・invoke は差し替え）。
   確かめること: ⌘⌥S（e.code = KeyS）とツールバーのボタンで切り替わる・入ると木・右ペイン・ヘッダーが消え main は 1 列・本文は中央の幅
   （wk-focus）・右上に「集中 ×」の札・設定（ui_wiki_info・ui_chat_dock）に書かない・集中中の ⌘⌥I と `/` は効かない・⌘⌥C は効く・
   モーダルの下では切り替えない・キーリピート / IME では切り替えない・一覧のタブには効かない・もう一度で元の配置に戻る。
   入るときに隠す所（ヘッダーの検索欄・木）の focus を外す（本文の focus は外さない）。
   PdfViewer と ShelfViewer は偽物（app.test.mjs と同じ土台） */
import assert from "node:assert/strict";
import { setupHarness } from "./test-harness.mjs";
import { fakeInvoke } from "../test-invoke.mjs";

const { calls, sleep, waitFor } = setupHarness({
  fakes: {
    "@tauri-apps/api/webview": "export const getCurrentWebview = () => ({ onDragDropEvent: async () => () => {} });",
    "@tauri-apps/plugin-dialog": "export const open = async () => null; export const save = async () => null; export const confirm = async () => false; export const message = async () => {};",
    "./lib/PdfViewer.svelte": "export default function PdfViewer() {}",
    "./lib/shelf/ShelfViewer.svelte": "export default function ShelfViewer() {}",
  },
});
let tests = 0;

const prefs = { ui_wiki_info: "1", ui_chat_dock: JSON.stringify({ open: false, w: 400 }) };
const page = { path: "f.md", abs_path: "/v/f.md", kind: "md", title: "f", size: 10, mtime: 0, content: "# 題\n\n本文\n", sha256: "F0", truncated: false, links: [] };
// 場面の応答だけを上書き（ほかは test-invoke.mjs の既定 = 本物と同じ形の空）
const inv = fakeInvoke(calls, {
  get_ui_pref: (a) => prefs[a.key] ?? null,
  set_ui_pref: (a) => {
    prefs[a.key] = a.value;
    return null;
  },
  wiki_chat_status: () => ({ enabled: true, cli: "/c", version: "1", logged_in: true, mcp: "/m", state: "off", session_id: null }),
  wiki_settings: () => ({ enabled: true, root: "/v" }),
  wiki_status: () => ({ enabled: true, root: "/v" }),
  wiki_read_gui: () => page,
});
globalThis.__invoke = inv.invoke;

const S = await import("../state.svelte.ts");
const W = await import("./wiki.svelte.ts");
const CH = await import("../chat/chat.svelte.ts");
const F = await import("./focus.svelte.ts");
const { mount, flushSync } = await import("svelte");
const { default: App } = await import("../../App.svelte");

const $ = (q) => document.querySelector(q);
const settle = async () => {
  flushSync();
  await sleep(10);
  flushSync();
};
const key = (o, el = window) => {
  const ev = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...o });
  el.dispatchEvent(ev);
  return ev;
};
const optS = (o = {}, el) => key({ metaKey: true, altKey: true, code: "KeyS", key: "ß", ...o }, el); // ⌥S は配列により ß 等 = e.code で見る
const optI = () => key({ metaKey: true, altKey: true, code: "KeyI", key: "ˆ" });
const optC = () => key({ metaKey: true, altKey: true, code: "KeyC", key: "ç" });

const target = document.createElement("div");
document.body.append(target);
mount(App, { target });
S.app.wikiOn = true;
await settle();
await S.openTab({ kind: "all" }, { path: "f.md", root: "/v", scrollTop: 0, editing: false });
assert.ok(await waitFor(() => (flushSync(), !!$(".wk-bar") && !!$(".wk-info") && W.wk.prefsReady), 3000), "OpenNote のページと右ペインが出ない");
/** 見えているか（集中モードは作り直さずに表示だけ隠す = ヘッダーは display・木と右ペインは visibility） */
const shown = (el) => !!el && el.style.display !== "none" && getComputedStyle(el).visibility !== "hidden";
const layout = () => ({
  header: shown($(".tabs-page > header")),
  tree: shown($("main > .wk-sidebar")),
  info: shown($("main > .wk-info")),
  main: [...$("main").classList].filter((c) => c !== "sbw").join(" "), // 左の列の幅の印 sbw（集中モードでは付かない = sidebarw.test）は除く
  focus: !!$(".wk-page.wk-focus"),
  tag: $(".wk-focus-tag")?.textContent.trim() ?? null,
});
const normal = layout();
assert.deepEqual(normal, { header: true, tree: true, info: true, main: "", focus: false, tag: null }, "通常の配置でない");

// ⌘⌥S で入る: 木・右ペイン・ヘッダーが消え、main は 1 列・札が出る・設定（ui_wiki_info・ui_chat_dock）に書かない。
// 木と右ペインとヘッダーは作り直さない（同じ要素のまま隠す = スクロール位置と中身を保つ・右ペインを読み直さない）
const els = () => [$(".tabs-page > header"), $("main > .wk-sidebar"), $("main > .wk-info")];
const kept = els();
const reads = () => calls.filter((c) => c.name === "wiki_backlinks").length;
const reads0 = reads();
/** 集中モードが触ってはいけない設定（計画 = この 2 つに書かない。他の機能 = 最近開いたページ等の記録は比べない） */
const guarded = () => ({ ui_wiki_info: prefs.ui_wiki_info, ui_chat_dock: prefs.ui_chat_dock });
/** 切り替えの間（calls の from 番目から）に書いた設定。集中モードは何も書かない = 0 件であるべき。
    除くのは他の機能が開いたページを記録する ui_wiki_recent（1a の最近開いたページ）だけ */
const wrote = (from) => calls.slice(from).filter((c) => c.name === "set_ui_pref" && c.args.key !== "ui_wiki_recent").map((c) => c.args.key);
/** 表明の区間に入る前に、手前の操作が予約した遅れて書く設定を出し切る: App はタブの状態（tabsPref）が変わってから 300ms 後に
    ui_tabs を書く（ページを開いた・復元した）= 遅い環境（並行の試験）ではその書込が切り替えの区間に落ちて「設定を書いた」に見えた。
    書き終わる（保存した ui_tabs = 今のタブの状態）まで待ってから base を取る。区間の後は、切り替えがタブの状態そのものを
    変えていない（= ui_tabs の保存を起こさない）ことを tabsPref で比べる（300ms を待たずに後退を捕まえる） */
async function drainTabs() {
  assert.ok(await waitFor(() => (flushSync(), prefs.ui_tabs === S.tabsPref()), 3000), "ui_tabs の遅れた保存が済まない");
}
{
  await drainTabs();
  const before = guarded();
  const tabs0 = S.tabsPref();
  const base = calls.length;
  const ev = optS();
  await settle();
  assert.equal(ev.defaultPrevented, true, "⌘⌥S を奪わない");
  assert.deepEqual(layout(), { header: false, tree: false, info: false, main: "wk-main-focus", focus: true, tag: "集中 ×" }, "集中の配置にならない");
  assert.equal(W.wk.infoOpen, true, "右ペインの開閉の状態を変えた");
  assert.deepEqual(guarded(), before, "設定に書いた");
  assert.deepEqual(wrote(base), [], "集中に入ると設定を書いた");
  assert.equal(S.tabsPref(), tabs0, "集中に入るとタブの状態が変わった（ui_tabs の保存を起こす）");
  assert.deepEqual(els(), kept, "集中に入ると木・右ペイン・ヘッダーを作り直した（隠すだけでない）");
  // キーリピート・IME では切り替えない
  optS({ repeat: true });
  optS({ keyCode: 229 });
  await settle();
  assert.equal(F.focusMode.on, true, "キーリピート / IME で切り替えた");
  tests++;
}

// 隠す所（ヘッダーの検索欄・木）に focus があるまま入ると、見えない欄が ↓ / Enter / 打った字を受け続ける = 入るときに外す
{
  optS(); // 一度戻る
  await settle();
  assert.equal(F.focusMode.on, false);
  for (const sel of ["#wk-search", "main > .wk-sidebar button"]) {
    const el = $(sel);
    assert.ok(el, `${sel} が無い`);
    el.focus();
    assert.equal(document.activeElement, el, `${sel} に focus が入らない`);
    optS({}, el);
    await settle();
    assert.equal(F.focusMode.on, true, `${sel} から ⌘⌥S で入らない`);
    assert.notEqual(document.activeElement, el, `${sel} の focus が残った（隠したのに受け続ける）`);
    optS();
    await settle();
  }
  // 本文（隠さない所）の focus は外さない
  const bar = document.querySelector(".wk-bar .wk-more"); // 本文の側（隠さない所）のボタン（[外部で開く] は [⋯] の中）
  bar.focus();
  optS({}, bar);
  await settle();
  assert.equal(F.focusMode.on, true);
  assert.equal(document.activeElement, bar, "本文の focus まで外した");
  tests++;
}

// 集中中: ⌘⌥I と `/` は効かない・⌘⌥C は効く（柱は集中の外）
{
  const info = W.wk.infoOpen;
  optI();
  await settle();
  assert.deepEqual([W.wk.infoOpen, shown($("main > .wk-info")), prefs.ui_wiki_info], [info, false, "1"], "集中中に ⌘⌥I が効いた");
  const slash = key({ key: "/" }, document.body);
  await settle();
  assert.equal(slash.defaultPrevented, false, "集中中に `/` を奪った（検索欄へ行こうとした）");
  const ev = optC();
  await settle();
  assert.equal(ev.defaultPrevented, true, "集中中に ⌘⌥C が効かない");
  assert.equal(CH.chat.dock.open, true, "集中中に ⌘⌥C で柱が開かない");
  assert.equal(F.focusMode.on, true, "⌘⌥C で集中を抜けた");
  optC();
  await settle();
  assert.equal(CH.chat.dock.open, false);
  tests++;
}

// 札を押す = 戻る（元の配置そのまま）。ツールバーのボタンで入る（この間も設定を書かない）
{
  await drainTabs();
  const tabs0 = S.tabsPref();
  const base = calls.length;
  $(".wk-focus-tag").click();
  await settle();
  assert.deepEqual(layout(), normal, "札で元の配置に戻らない");
  assert.deepEqual(els(), kept, "戻すと木・右ペイン・ヘッダーを作り直した（スクロール位置が先頭に戻る）");
  assert.equal(reads(), reads0, "戻すと右ペインを読み直した");
  const btn = [...document.querySelectorAll(".wk-bar button")].find((b) => b.textContent.trim() === "集中");
  assert.ok(btn, "ツールバーに集中のボタンが無い");
  btn.click();
  await settle();
  assert.equal(layout().focus, true, "ボタンで入らない");
  optS();
  await settle();
  assert.deepEqual(layout(), normal, "もう一度の ⌘⌥S で戻らない");
  assert.deepEqual(wrote(base), [], "札・ボタン・⌘⌥S の切り替えで設定を書いた");
  assert.equal(S.tabsPref(), tabs0, "札・ボタン・⌘⌥S の切り替えでタブの状態が変わった（ui_tabs の保存を起こす）");
  tests++;
}

// モーダルの下では切り替えない・一覧のタブには効かない（集中は OpenNote の前面だけ）
{
  S.app.modal = "citeStyle";
  const ev = optS();
  await settle();
  assert.deepEqual([ev.defaultPrevented, F.focusMode.on], [false, false], "モーダルの下で切り替えた");
  S.app.modal = null;
  await settle();
  optS();
  await settle();
  assert.equal(F.focusMode.on, true);
  await S.openTab({ kind: "all" });
  await settle();
  assert.ok(!S.frontWiki());
  assert.ok($(".tabs-page > header"), "一覧のタブでヘッダーを隠した");
  assert.notEqual($("main").className, "wk-main-focus");
  const ev2 = optS();
  assert.equal(ev2.defaultPrevented, false, "一覧のタブで ⌘⌥S を奪った");
  tests++;
}

inv.done();
console.log(`wiki focus: ok (${tests} tests)`);
process.exit(0);
