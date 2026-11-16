/* 本棚の組立ての試験（node app/src/lib/shelf/shelf.test.mjs・実部品 App.svelte + jsdom・invoke は差し替え）。
   見るのは分岐: 最上段のセグメント（LIBRARY / NOTE / SHELF）・本棚の左バーの見出しのメニュー・タブの永続と復元・
   ⌘N / ⌘A / Esc / `/` / ドロップが見えない一覧に効かないこと・表紙の差し替えと温め・NOTE との排他・
   3 つのモードの往復で見せていた本棚 / 一覧に戻る。読み手（pdf.js）だけ偽物 = 実機で見る。土台は wiki/test-harness.mjs */
import assert from "node:assert/strict";
import { setupHarness } from "../wiki/test-harness.mjs";
import { fakeInvoke } from "../test-invoke.mjs";
import { readFileSync } from "node:fs";

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

const ROOT = "/r/本の棚";
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
  { root: ROOT, name: "本の棚", last_used: 2, exists: true, collapse_single: false, issue_titles: false, aliases: {} },
  { root: OTHER, name: "別の棚", last_used: 1, exists: true, collapse_single: false, issue_titles: false, aliases: {} },
  { root: GONE, name: "外付け", last_used: 0, exists: false, collapse_single: false, issue_titles: false, aliases: {} },
];
let thumbGate = null; // Promise = shelf_thumb の応答を止める
let thumbStep = null; // 配列 = shelf_thumb を 1 本ずつ流す（試験が resolve を呼ぶ）
let switchGate = null; // Promise = shelf_switch_root の応答を止める
let pickGate = null; // Promise = shelf_open_root（native のフォルダの選択）の応答を止める
let pickFails = false; // shelf_open_root が失敗する
const BROKEN = "読めない: CloudStorage の readdir が拒否されました"; // 開けない本棚の Rust の文（Drive の CloudStorage は TCC で readdir を拒否されうる）
const switchFails = new Set(); // この root の shelf_switch_root / shelf_tree は失敗する
let statusNull = false; // shelf_status が null を返す
let thumbFail = new Set();
let pickRoot = null;
let statusFails = false;
let noteGate = null; // Promise = add_note の応答を止める（一覧を離れる前のメモの保存を待たせる）
let opsFail = false; // shelf_set_opts / shelf_reveal / shelf_open_external が、渡した root をそのまま含む文で失敗する（畳む前の Rust の文の形）
let noteFails = false; // add_note を失敗させる（NOTE へ切り替える前のメモの保存）
// 場面の応答だけを上書き（ほかは test-invoke.mjs の既定 = 本物と同じ形の空）
const inv = fakeInvoke(calls, {
  get_ui_pref: (a) => prefs[a.key] ?? null,
  set_ui_pref: (a) => {
    prefs[a.key] = a.value;
    return null;
  },
  list_items: () => items,
  get_item: (a) => detail(a.key),
  add_note: async () => {
    await noteGate;
    if (noteFails) throw new Error("書けない");
    return "N1";
  },
  wiki_settings: () => ({ enabled: wikiOn, root: "/v" }),
  wiki_status: () => ({ enabled: wikiOn, root: "/v" }),
  wiki_chat_status: () => ({ enabled: false, cli: "/c", version: "1", logged_in: true, mcp: "/m", state: "off", session_id: null }),
  shelf_status: () => {
    if (statusFails) throw new Error("読めない");
    return statusNull ? null : { shelves };
  },
  shelf_tree: (a) => {
    // 走査の失敗の文は渡した root をそのまま含む（core wiki::check_root_as の「本棚のフォルダに <root> は使えません（理由）」と同じ形）
    if (switchFails.has(a.root)) throw new Error(`本棚のフォルダに ${a.root} は使えません（見つかりません）`);
    return { root: a.root, dir: null, tree: treeOf(a.root), unreadable: 0, used_find: false, thumbs_dir: "/thumbs" };
  },
  shelf_thumb: async (a) => {
    await thumbGate;
    if (thumbStep) await new Promise((r) => thumbStep.push(r));
    if (thumbFail.has(a.id)) throw new Error("作れない");
    return { path: `/thumbs/${a.id}.png` };
  },
  shelf_open_root: async () => {
    await pickGate;
    if (pickFails) throw new Error("選択の窓を開けません");
    // 選んだフォルダは履歴に入る（Rust の shelf_open_steps と同じ）
    if (pickRoot && !shelves.some((s) => s.root === pickRoot)) {
      shelves = [{ root: pickRoot, name: pickRoot.slice(pickRoot.lastIndexOf("/") + 1), last_used: 9, exists: true, collapse_single: false, issue_titles: false, aliases: {} }, ...shelves];
    }
    return { root: pickRoot };
  },
  shelf_switch_root: async (a) => {
    await switchGate;
    if (switchFails.has(a.root)) throw new Error(BROKEN);
    return { root: a.root };
  },
  shelf_forget_root: (a) => {
    shelves = shelves.filter((s) => s.root !== a.root);
    return { thumbs: 3, unknown: false };
  },
  shelf_reveal: (a) => {
    if (opsFail) throw `本棚のフォルダに ${a.root} は使えません（見つかりません）`;
    return null;
  },
  shelf_open_external: (a) => {
    if (opsFail) throw `本棚のフォルダに ${a.root} は使えません（iCloud Drive / CloudStorage の最上位です。その中のフォルダを選んでください）`;
    return null;
  },
  shelf_set_opts: (a) => {
    if (opsFail) throw `履歴に無い本棚です: ${a.root}（root には shelf_status の root をそのまま渡す）`;
    shelves = shelves.map((s) => (s.root === a.root ? { ...s, collapse_single: a.collapse_single } : s));
    return {};
  },
});
globalThis.__invoke = inv.invoke;

const S = await import("../state.svelte.ts");
const H = await import("./shelf.svelte.ts");
const W = await import("../wiki/wiki.svelte.ts");
const M = await import("../mode.svelte.ts");
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
/** 最上段のセグメント（左バーの最上段 = 前面のモードの左バーの物） */
const segBtns = () => $$(".mode-seg .mode-seg-btn");
const segNames = () => segBtns().map((b) => b.textContent);
const segOn = () => segBtns().filter((b) => b.getAttribute("aria-pressed") === "true").map((b) => b.textContent);
const seg = async (name) => {
  segBtns().find((b) => b.textContent === name).click();
  await settle();
};

const target = document.createElement("div");
document.body.append(target);
mount(App, { target });
await waitFor(() => $$(".row").length === 2);
await settle();

// 文献のとき: 最上段 = [LIBRARY | SHELF]（OpenNote が無効 = NOTE は出ない）・LIBRARY が押された側。
// フォルダ節の見出しは無い = セグメントの直下が「すべて」の行・見出しは LABELS だけ・本棚のメニューは無い
{
  assert.deepEqual(segNames(), ["LIBRARY", "SHELF"], "OpenNote が無効のセグメント");
  assert.deepEqual(segOn(), ["LIBRARY"]);
  assert.ok(!segBtns()[1].title.includes("本棚はまだありません"), "本棚があるのに「まだありません」");
  assert.deepEqual($$("nav.ls-nav .sec-head > span:first-child").map((e) => e.textContent), ["LABELS"], "フォルダ節の見出しが残った / LABELS の見出しが無い");
  assert.ok($("nav.ls-nav > .mode-seg + .ls-all"), "セグメントの直下が「すべて」の行でない");
  assert.deepEqual([$(".sh-head"), $(".sh-head-caret"), $("nav.ls-nav [aria-haspopup='menu']")], [null, null, null], "本棚のメニューが残った");
  tests++;
}

// 「すべて」の行 = [すべて][＋][…][件数]・＋ / … は常に出す。行は Enter / Space・⌘クリック / 中クリックで新しいタブ（フォルダの行と同じ）・
// ＋ = 新規フォルダ・… = フォルダメニュー（並び順・フォルダを管理）。中のボタンの押下・Enter・中クリックは行の操作にしない。キーの順 = すべて → ＋ → … → Starred
// 行 = 枠の div（role なし）+ 行を覆うボタン（.ls-all-hit・名前 =「すべて N」）+ 兄弟の ＋ / …（ボタンの中にボタンを入れない = 読み上げで届く）
{
  const all = $(".ls-all");
  const hit = all.querySelector(":scope > button.ls-all-hit");
  assert.deepEqual([all.getAttribute("role"), all.getAttribute("tabindex")], [null, null], "「すべて」の行の枠に role / tabindex が残った");
  assert.ok(hit && hit === all.firstElementChild, "行を覆うボタンが枠の最初の子でない");
  assert.equal(hit.getAttribute("aria-label"), `すべて ${S.app.counts.all}`, "行を覆うボタンの名前が「すべて 件数」でない");
  assert.equal(hit.children.length + hit.textContent.length, 0, "行を覆うボタンに中身がある（字は aria-label・ボタンを入れない）");
  // 覆いのボタンは読み上げに残す（aria-hidden を付けると行が消える = 目的と逆）・tabindex="0" を明示（macOS の WebKit は
  // フルキーボードアクセスが切だと素の <button> に Tab が止まらず、押しても焦点を取らない = 前の div tabindex="0" の行と同じに）
  assert.deepEqual([hit.closest("[aria-hidden='true']"), hit.getAttribute("tabindex")], [null, "0"], "「すべて」の覆いのボタンが読み上げから消える / tabindex=0 が無い");
  assert.equal(all.querySelectorAll("button button, [role='button'] button").length, 0, "ボタンの中にボタンがある");
  assert.deepEqual([...all.querySelectorAll(":scope > .name, :scope > .n")].map((e) => e.getAttribute("aria-hidden")), ["true", "true"], "見えている名前と件数を二重に読ませる");
  assert.deepEqual([...all.children].map((c) => (c === hit ? "覆い" : c.getAttribute("aria-label") ?? c.textContent.trim())), ["覆い", "すべて", "新規フォルダ", "フォルダメニュー", String(S.app.counts.all)], "「すべて」の行の並び");
  assert.equal(all.querySelectorAll(".ls-all-act").length, 2);
  for (const b of all.querySelectorAll(".ls-all-act")) assert.equal(b.title, b.getAttribute("aria-label"), "＋ / … の title と aria-label が揃わない");
  const focusables = $$("nav.ls-nav button, nav.ls-nav [tabindex='0']").filter((e) => !e.closest(".mode-seg"));
  assert.deepEqual(focusables.slice(0, 4).map((e) => (e.getAttribute("aria-label") ?? (e.querySelector(".name") ?? e).textContent).trim().replace(/\s*\d+$/, "")), ["すべて", "新規フォルダ", "フォルダメニュー", "★ スター付き"], "キーの順が すべて → ＋ → … → スター付き でない");
  const [plus, more] = all.querySelectorAll(".ls-all-act");
  const tabsBefore = S.app.tabs.length;
  await S.setView({ kind: "starred" });
  await settle();
  // ＋ = 新規フォルダの窓（行のクリックにしない = ビューは変えない）
  plus.click();
  await settle();
  assert.deepEqual([S.app.modal, S.app.view.kind], ["newFolder", "starred"], "＋ が新規フォルダを開かない / 行のクリックになった");
  S.app.modal = null;
  await settle();
  // … = フォルダメニュー（並び順・フォルダを管理）・もう一度押すと閉じる
  more.click();
  await settle();
  assert.equal(S.app.labelMenu, "folders", "… がフォルダメニューを開かない");
  assert.equal(S.app.view.kind, "starred", "… が行のクリックになった");
  assert.ok($$(".lmenu .lmenu-item").some((b) => b.textContent.includes("フォルダを管理…")), "フォルダメニューの中身が違う");
  more.click();
  await settle();
  assert.equal(S.app.labelMenu, false, "… の再押下で閉じない");
  // 中のボタンの Enter・中クリックは行の操作にしない
  plus.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  more.dispatchEvent(new MouseEvent("auxclick", { button: 1, bubbles: true, cancelable: true }));
  await settle();
  assert.deepEqual([S.app.view.kind, S.app.tabs.length], ["starred", tabsBefore], "中のボタンの Enter / 中クリックが行に効いた");
  // 行（覆いのボタン）: Enter・Space = 前面のタブのビュー（素のボタンの押下は止める = 既定を止めた）
  const enter = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
  hit.dispatchEvent(enter);
  assert.equal(enter.defaultPrevented, true, "行の Enter の既定（素のボタンの click）を止めない = 二重に動く");
  await settle();
  assert.equal(S.app.view.kind, "all", "行の Enter で「すべて」にならない");
  await S.setView({ kind: "starred" });
  hit.dispatchEvent(new KeyboardEvent("keydown", { key: " ", bubbles: true, cancelable: true }));
  await settle();
  assert.equal(S.app.view.kind, "all", "行の Space で「すべて」にならない");
  assert.ok($(".ls-all").classList.contains("active"), "「すべて」が選ばれた印にならない");
  // クリック = 前面のタブのビュー / ⌘クリック・中クリック = 新しいタブ（前面のタブは変えない）
  await S.setView({ kind: "starred" });
  hit.click();
  await settle();
  assert.deepEqual([S.app.view.kind, S.app.tabs.length], ["all", tabsBefore], "行のクリックで「すべて」にならない / タブが増えた");
  hit.dispatchEvent(new MouseEvent("click", { button: 0, metaKey: true, bubbles: true, cancelable: true }));
  await settle();
  assert.equal(S.app.tabs.length, tabsBefore + 1, "⌘クリックで新しいタブにならない");
  hit.dispatchEvent(new MouseEvent("auxclick", { button: 1, bubbles: true, cancelable: true }));
  await settle();
  assert.equal(S.app.tabs.length, tabsBefore + 2, "中クリックで新しいタブにならない");
  await reset();
  tests++;
}

// フォルダの行 = 枠の div（role なし・data-drop・選ばれた / メニューの印）+ 行を覆うボタン（.fd-hit・名前 =「<フォルダ名> 件数」）+
// 兄弟の chevron（aria-expanded）/「…」。クリック・Enter / Space = 前面のタブのビュー・⌘ = 新しいタブ・中クリック / 右クリックは
// 行のどこでも（chevron / 「…」の上でも = 前の div role="button" と同じ）。chevron・「…」の押下と Enter は行の操作にしない
{
  S.app.collections = [
    { id: 81, key: "C81", name: "親のフォルダ", parentId: null, nItems: 4 },
    { id: 82, key: "C82", name: "子のフォルダ", parentId: 81, nItems: 1 },
  ];
  await settle();
  const row = (name) => $$("nav.ls-nav .fd-row").find((r) => r.querySelector(".name").textContent === name);
  const parent = row("親のフォルダ");
  const hit = parent.querySelector(":scope > button.fd-hit");
  const chev = parent.querySelector("button.fd-chev");
  const moreB = parent.querySelector("button.fd-more");
  assert.deepEqual([parent.getAttribute("role"), parent.getAttribute("tabindex"), parent.dataset.drop], [null, null, "col:81"], "フォルダの行の枠に role / tabindex が残った / ドロップ先の印が無い");
  assert.ok(hit && hit === parent.firstElementChild, "行を覆うボタンが枠の最初の子でない");
  assert.deepEqual([hit.getAttribute("aria-label"), row("子のフォルダ").querySelector(".fd-hit").getAttribute("aria-label")], ["親のフォルダ 4", "子のフォルダ 1"], "行を覆うボタンの名前が「名前 件数」でない");
  assert.equal(hit.children.length + hit.textContent.length, 0, "行を覆うボタンに中身がある");
  assert.deepEqual([hit.closest("[aria-hidden='true']"), hit.getAttribute("tabindex")], [null, "0"], "フォルダの覆いのボタンが読み上げから消える / tabindex=0 が無い");
  // 子の無いフォルダの chevron の欄は空の span（ボタンでない）
  assert.deepEqual([row("子のフォルダ").querySelector(".fd-chev").tagName, row("子のフォルダ").querySelector("button.fd-chev")], ["SPAN", null], "子の無いフォルダの chevron の欄");
  // CSS（jsdom では効かない = app.css の字面で縛る）:
  {
    const css = readFileSync(new URL("../../app.css", import.meta.url), "utf8").replace(/\/\*[^]*?\*\//g, "");
    const rules = [...css.matchAll(/([^{}]+)\{([^}]*)\}/g)].map((m) => [m[1].trim().replace(/\s+/g, " "), m[2]]);
    const body = (sel) => rules.filter(([s]) => s.split(",").map((x) => x.trim()).includes(sel)).map(([, b]) => b).join(";");
    // 覆い = 枠の全体（inset 0・absolute）・字の色の面も透明
    for (const sel of [".ls-nav .ls-all-hit", ".ls-nav .fd-hit"]) assert.match(body(sel), /position: absolute;[^;]*?\s*inset: 0;/, `${sel} が枠の全体を覆わない`);
    // 覆いの上に重ねる物（後に置いて position を持つ = 覆いより後に描く）: ＋ / … = relative・chevron のボタン = relative・「…」= absolute。
    // 覆いに z-index を付けない（付けると上に重ねた物を覆う）
    assert.match(body(".ls-nav .ls-all .ls-all-act"), /position: relative;/, "＋ / … が覆いの上に無い");
    assert.match(body(".ls-nav button.fd-chev"), /position: relative;/, "chevron のボタンが覆いの上に無い");
    assert.match(body(".ls-nav .fd-more"), /position: absolute;/, "「…」が覆いの上に無い");
    assert.doesNotMatch(body(".ls-nav .ls-all-hit") + body(".ls-nav .fd-hit"), /z-index/, "覆いに z-index がある");
    // relative はボタンの chevron だけ（子の無いフォルダの空の span = 覆いの上に乗せると 16×16 の欄がクリックを吸う）
    const chevPos = rules.filter(([s, b]) => /fd-chev(?![\w-])/.test(s) && /position:/.test(b)).map(([s]) => s);
    assert.deepEqual(chevPos, [".ls-nav button.fd-chev"], `chevron の欄に position を付ける規則がボタンの外にもある: ${chevPos}`);
    // 焦点の輪は枠が描く（覆いが焦点を持つとき）・覆い自身の輪は消す
    assert.match(body(".ls-nav .ls-all:has(> .ls-all-hit:focus-visible)"), /outline: 2px solid var\(--accent\); outline-offset: -2px;/, "「すべて」の行の焦点の輪");
    assert.match(body(".ls-nav .fd-row:has(> .fd-hit:focus-visible)"), /outline: 2px solid var\(--accent\); outline-offset: -2px;/, "フォルダの行の焦点の輪");
    // 直下の Starred の行も relative（「すべて」の行が Starred の焦点の輪の縁を覆わない = 描く順を DOM の順に）
    assert.match(body(".ls-nav .ls-all + button"), /position: relative;/, "Starred の行の relative が無い");
  }
  assert.equal($$("nav.ls-nav button button, nav.ls-nav [role='button'] button").length, 0, "左バーにボタンの中のボタンがある");
  assert.deepEqual([...parent.querySelectorAll(":scope > .name, :scope > .n")].map((e) => e.getAttribute("aria-hidden")), ["true", "true"], "見えている名前と件数を二重に読ませる");
  assert.deepEqual([chev.getAttribute("aria-expanded"), chev.getAttribute("aria-label")], ["true", "折り畳む"], "chevron の aria-expanded / 名前");
  const focusables = $$("nav.ls-nav button, nav.ls-nav [tabindex='0']").filter((e) => parent.contains(e));
  assert.deepEqual(focusables.map((e) => e.className.replace(/\s*s-\w+/g, "").split(" ")[0]), ["fd-hit", "fd-chev", "fd-more"], "フォルダの行のキーの順が 行 → chevron → … でない");
  const tabsBefore = S.app.tabs.length;
  await S.setView({ kind: "starred" });
  // クリック = そのフォルダ（前面のタブ）・選ばれた印は枠に
  hit.click();
  await settle();
  assert.deepEqual([S.app.view.kind, S.app.view.id, S.app.tabs.length], ["collection", 81, tabsBefore], "行のクリックでフォルダにならない");
  assert.ok(row("親のフォルダ").classList.contains("active"), "選ばれた印が枠に付かない");
  // Enter / Space（既定 = 素のボタンの click は止める）・⌘Enter = 新しいタブ
  await S.setView({ kind: "starred" });
  const enter = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
  row("親のフォルダ").querySelector(".fd-hit").dispatchEvent(enter);
  await settle();
  assert.deepEqual([S.app.view.kind, enter.defaultPrevented], ["collection", true], "行の Enter でフォルダにならない / 既定を止めない");
  await S.setView({ kind: "starred" });
  row("親のフォルダ").querySelector(".fd-hit").dispatchEvent(new KeyboardEvent("keydown", { key: " ", bubbles: true, cancelable: true }));
  await settle();
  assert.equal(S.app.view.kind, "collection", "行の Space でフォルダにならない");
  await S.setView({ kind: "starred" });
  row("親のフォルダ").querySelector(".fd-hit").dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true, cancelable: true }));
  await settle();
  assert.deepEqual([S.app.tabs.length, front().view.kind, front().view.id], [tabsBefore + 1, "collection", 81], "⌘Enter でそのフォルダの新しいタブにならない");
  // ⌘クリック・中クリック（行・chevron の上）= 新しいタブ
  row("親のフォルダ").querySelector(".fd-hit").dispatchEvent(new MouseEvent("click", { button: 0, metaKey: true, bubbles: true, cancelable: true }));
  await settle();
  assert.equal(S.app.tabs.length, tabsBefore + 2, "⌘クリックで新しいタブにならない");
  row("親のフォルダ").querySelector(".fd-hit").dispatchEvent(new MouseEvent("auxclick", { button: 1, bubbles: true, cancelable: true }));
  await settle();
  assert.equal(S.app.tabs.length, tabsBefore + 3, "行の中クリックで新しいタブにならない");
  row("親のフォルダ").querySelector(".fd-chev").dispatchEvent(new MouseEvent("auxclick", { button: 1, bubbles: true, cancelable: true }));
  await settle();
  assert.equal(S.app.tabs.length, tabsBefore + 4, "中クリック（行・chevron の上）で新しいタブにならない");
  // 右クリック（行・chevron の上）= フォルダのメニュー
  row("親のフォルダ").querySelector(".fd-hit").dispatchEvent(new MouseEvent("contextmenu", { button: 2, clientX: 50, clientY: 60, bubbles: true, cancelable: true }));
  await settle();
  assert.equal(S.app.folderMenu?.id, 81, "行の右クリックでメニューが出ない");
  assert.ok(row("親のフォルダ").classList.contains("menu-open"), "メニューの印が枠に付かない");
  S.app.folderMenu = null;
  row("子のフォルダ").querySelector(".fd-more").dispatchEvent(new MouseEvent("contextmenu", { button: 2, clientX: 50, clientY: 60, bubbles: true, cancelable: true }));
  await settle();
  assert.equal(S.app.folderMenu?.id, 82, "「…」の上の右クリックでメニューが出ない");
  S.app.folderMenu = null;
  // chevron の押下 = 折畳だけ（行のクリックにしない）・Enter も chevron が受ける
  await S.setView({ kind: "starred" });
  row("親のフォルダ").querySelector(".fd-chev").click();
  await settle();
  assert.deepEqual([S.app.view.kind, !!row("子のフォルダ"), row("親のフォルダ").querySelector(".fd-chev").getAttribute("aria-expanded")], ["starred", false, "false"], "chevron が折り畳まない / 行のクリックになった");
  row("親のフォルダ").querySelector(".fd-chev").dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  row("親のフォルダ").querySelector(".fd-chev").click();
  await settle();
  assert.deepEqual([S.app.view.kind, !!row("子のフォルダ")], ["starred", true], "chevron の Enter が行に効いた / 展開しない");
  // 「…」の押下 = メニュー（行のクリックにしない）
  row("親のフォルダ").querySelector(".fd-more").click();
  await settle();
  assert.deepEqual([S.app.folderMenu?.id, S.app.view.kind], [81, "starred"], "「…」がメニューを開かない / 行のクリックになった");
  S.app.folderMenu = null;
  // ドロップ先の強調は枠に（data-drop も枠）
  S.app.drag = { kind: "rows", label: "x", x: 0, y: 0, over: "col:82" };
  await settle();
  assert.deepEqual($$("nav.ls-nav .drophover").map((e) => [e.className.includes("fd-row"), e.dataset.drop]), [[true, "col:82"]], "ドロップ先の強調が枠に付かない");
  S.app.drag = null;
  S.app.collections = [];
  await reset();
  tests++;
}

// 選んでいる文献があっても本棚へ入ると外す（文脈のチップが見えない文献を指さない）・本棚の中身に切り替わる
{
  $$(".row")[1].click();
  await settle();
  assert.equal(S.app.selected, "K2");
  // SHELF = 覚えた本棚が無い = 履歴の新しい順で最初の実在する本棚（本の棚）へ・走査し直す（見出しのメニューの行と同じ）
  await seg("SHELF");
  assert.deepEqual(called("shelf_switch_root").at(-1).args, { root: ROOT });
  assert.deepEqual(segOn(), ["SHELF"], "本棚で SHELF が押された側にならない");
  assert.equal(front().shelf?.root, ROOT, "前面のタブが本棚にならない");
  assert.equal(front().shelf.dir, null);
  assert.deepEqual([S.app.selected, S.app.checked.size], [null, 0], "一覧の選択が残った");
  assert.equal($(".sh-head-name").textContent, "📚 本の棚");
  assert.equal($(".sh-head").title, ROOT);
  // 本棚の左バーの見出しのメニュー = 本棚の選択（履歴・無いフォルダは押せない・別のフォルダ…・今の本棚の設定）。✓ 文献 の行は無い（セグメントへ移した）
  await openMenu();
  assert.ok(menuRow("本の棚").textContent.includes("✓"), "今の本棚に ✓ が無い");
  assert.ok(menuRow("別の棚") && !menuRow("別の棚").textContent.includes("✓"));
  assert.equal(menuRow("外付け").disabled, true, "無いフォルダが押せる");
  assert.ok(menuRow("別のフォルダを本棚として開く…"));
  assert.ok(menuRow("Finder で表示") && menuRow("この本棚を履歴から外す") && menuRow("PDF が 1 つだけのフォルダ"), "本棚の設定が無い");
  assert.equal($$(".sh-menu .lmenu-item").some((b) => b.textContent.trim() === "文献"), false, "本棚のメニューに文献の行が残った");
  key({ key: "Escape" });
  await settle();
  assert.equal($(".sh-menu"), null, "Esc でメニューが閉じない");
  // LABELS・ゴミ箱・＋・… は描かない。QuickAdd・＋新規・一覧も無い
  assert.deepEqual([$(".ls-all"), $(".head-actions")], [null, null], "本棚に「すべて」の行（＋ / …）・ラベルの見出しが出た");
  assert.equal($("nav.ls-nav").textContent.includes("LABELS"), false);
  assert.equal($("nav.ls-nav").textContent.includes("ゴミ箱"), false);
  assert.deepEqual([$("#quickadd"), $(".newref"), $(".rows")], [null, null, null], "本棚に文献の部品が残った");
  assert.ok($("#sh-search"), "本棚の検索欄が無い");
  assert.equal($$(".sh-book").length, 4, "すべての本が全部出ない");
  assert.equal($$(".sh-folder").length, 2, "直下のフォルダが出ない");
  assert.equal($("nav.ls-nav > button .n").textContent, "4", "すべての本の冊数");
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
  assert.deepEqual($$(".sh-crumbs > *").map((e) => e.textContent), ["本の棚", "›", "腹部", "›", "肝臓"]);
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
  assert.deepEqual([...$("main").classList].sort(), ["lib-nodetail", "sbw"], "2 列にならない（本棚にも左バーの幅 = sbw）");
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
  assert.equal($(".tabs-item.front .tabs-title").textContent, "本の棚", "本棚のタブの題名");
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
    { root: ROOT, name: "本の棚", last_used: 2, exists: true, collapse_single: false, issue_titles: false, aliases: {} },
    { root: OTHER, name: "別の棚", last_used: 1, exists: true, collapse_single: false, issue_titles: false, aliases: {} },
  ];
  tests++;
}

// 別のフォルダ…: 取り消し = 何もしない・選んだら前面のタブへ。メニューの collapse / Finder。文献へはセグメントの LIBRARY
{
  await H.loadShelves();
  await seg("SHELF");
  assert.equal(front().shelf?.root, ROOT);
  await openMenu();
  pickRoot = null;
  menuRow("別のフォルダを本棚として開く…").click();
  await settle();
  assert.equal(front().shelf?.root, ROOT, "取り消しで本棚が変わった");
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
  await seg("LIBRARY");
  assert.equal(front().shelf, null);
  assert.ok($(".rows"), "文献に戻らない");
  assert.deepEqual([!!$("nav.ls-nav > .mode-seg + .ls-all"), $(".sh-head")], [true, null]);
  assert.deepEqual(segOn(), ["LIBRARY"]);
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

// 3 つのモード: OpenNote が有効 = [LIBRARY | NOTE | SHELF]。本棚のタブから NOTE / LIBRARY = shelf を外す（中身は常に 1 つ）・
// 見せていた本棚は tab.shelfAway に覚える（NOTE を挟んで LIBRARY へ行っても）・SHELF でそこへ戻る
{
  wikiOn = true;
  await W.loadWikiStatus();
  await settle();
  await S.activateTab(S.app.tabs.find((t) => t.shelf).id);
  await settle();
  assert.ok(S.frontShelf(), "本棚のタブが前面でない");
  assert.deepEqual(segNames(), ["LIBRARY", "NOTE", "SHELF"], "OpenNote が有効のセグメント");
  assert.deepEqual(segOn(), ["SHELF"]);
  const tab = S.app.tab;
  const root = front().shelf.root;
  await seg("NOTE");
  assert.equal(S.app.tab, tab, "同じタブのまま切り替わらない");
  assert.deepEqual([front().shelf, !!front().wiki, front().shelfAway?.root], [null, true, root], "本棚と Wiki が同居した / 本棚を覚えない");
  assert.deepEqual(segOn(), ["NOTE"]);
  const saved = JSON.parse(S.tabsPref()).tabs.find((x) => x.wiki);
  assert.equal(saved.shelf, undefined);
  await seg("LIBRARY");
  assert.deepEqual([front().wiki, front().shelf, front().shelfAway?.root], [null, null, root], "NOTE から LIBRARY が一覧にならない / 本棚を忘れた");
  assert.ok($(".rows"), "一覧が出ない");
  assert.deepEqual(segOn(), ["LIBRARY"]);
  await seg("SHELF");
  assert.deepEqual([front().shelf?.root, front().shelfAway, S.app.tab], [root, null, tab], "SHELF で覚えた本棚に戻らない");
  wikiOn = false;
  await W.loadWikiStatus();
  await settle();
  tests++;
}

// モードの往復で、そのモードで見せていた物に戻る（本棚で開いていたフォルダの状態が解除されない）:
// 本棚 = 同じ本棚・同じフォルダ・同じ並び（tab.shelfAway に覚える・OpenNote のタブは ui_tabs にも）/ 一覧 = 同じフォルダ・ラベル・検索語（app.* に持ったまま）/
// 消えたフォルダ・覚えた物が無い（最初から OpenNote のタブ）= 「すべて」/ 履歴から外した本棚 = 戻る先を捨てる（SHELF は履歴の本棚へ）
{
  await reset();
  wikiOn = true;
  await W.loadWikiStatus();
  await settle();
  // 本棚: 本の棚の「腹部」を開き、並びを変えて NOTE → LIBRARY
  await H.switchShelf(ROOT);
  await settle();
  const tab = front();
  tab.shelf.dir = "腹部";
  tab.shelf.sort = "size";
  await settle();
  const titles = () => $$(".sh-book-title").map((e) => e.textContent);
  const inAbd = titles();
  assert.ok(inAbd.includes("腹部 CT") && !inAbd.includes("総論"), `腹部を開けていない: ${inAbd}`);
  await M.switchMode("wiki");
  await settle();
  assert.deepEqual([tab.shelf, !!tab.wiki, tab.shelfAway?.root, tab.shelfAway?.dir], [null, true, ROOT, "腹部"], "NOTE へ切り替えても本棚を覚えない");
  const pref = JSON.parse(S.tabsPref()).tabs.find((x) => x.wiki);
  assert.deepEqual([pref.shelf, pref.shelfAway], [undefined, { root: ROOT, dir: "腹部" }], "ui_tabs に戻る先の本棚が無い / 本棚と同居した");
  await M.switchMode("shelf");
  await settle();
  assert.equal(S.app.tab, tab.id, "同じタブのまま戻らない");
  assert.deepEqual([tab.wiki, tab.shelf?.root, tab.shelf?.dir, tab.shelf?.sort, tab.shelfAway], [null, ROOT, "腹部", "size", null], "SHELF へ戻すと同じ本棚・フォルダ・並びに戻らない");
  assert.deepEqual(titles(), inAbd, "戻した本棚の表示が違う");
  assert.equal(JSON.parse(S.tabsPref()).tabs.find((x) => x.shelf)?.shelfAway, undefined, "本棚に戻した後も ui_tabs に戻る先が残った");
  const switches = called("shelf_switch_root").length;
  // 本棚 ⇄ 一覧も同じ（走査し直さない = 覚えた本棚をそのまま）
  await M.switchMode("library");
  await settle();
  assert.deepEqual([tab.shelf, tab.shelfAway?.root, tab.shelfAway?.dir, tab.shelfAway?.sort], [null, ROOT, "腹部", "size"], "LIBRARY へ切り替えても本棚を覚えない");
  assert.ok($(".rows"), "LIBRARY で一覧が出ない");
  // ui_tabs: 一覧のタブの戻る先は書かない（書くのは OpenNote のタブの物だけ = 前の版の .app が読んでも一覧のタブの意味が変わらない）
  const listPref = JSON.parse(S.tabsPref()).tabs.find((x) => !x.wiki && !x.shelf && !x.viewer && !x.shelfBook);
  assert.ok(listPref, "前提: 一覧のタブ");
  assert.equal(listPref.shelfAway, undefined, "一覧のタブの戻る先を ui_tabs に書いた");
  await M.switchMode("shelf");
  await settle();
  assert.deepEqual([tab.shelf?.root, tab.shelf?.dir, tab.shelf?.sort, tab.shelfAway], [ROOT, "腹部", "size", null], "LIBRARY を挟むと本棚・フォルダ・並びに戻らない");
  assert.deepEqual(titles(), inAbd, "一覧を挟んで戻した本棚の表示が違う");
  // SHELF → NOTE → LIBRARY → SHELF = NOTE からは一覧へ（本棚へ戻らない）・覚えた本棚は持ったまま
  await M.switchMode("wiki");
  await M.switchMode("library");
  await settle();
  assert.deepEqual([tab.wiki, tab.shelf, tab.shelfAway?.dir], [null, null, "腹部"], "NOTE から LIBRARY で本棚に戻った / 本棚を忘れた");
  await M.switchMode("shelf");
  await settle();
  assert.deepEqual([tab.shelf?.root, tab.shelf?.dir], [ROOT, "腹部"], "NOTE と一覧を挟むと本棚に戻らない");
  assert.equal(called("shelf_switch_root").length, switches, "覚えた本棚へ戻るのに走査し直した");

  // 一覧: フォルダ + 検索語 → NOTE → LIBRARY = 同じフォルダ・検索語で読み直す
  await M.switchMode("library");
  await settle();
  S.app.collections = [...S.app.collections, { id: 77, key: "C77", name: "Abdominal Radiology", parent_id: null, count: 2 }];
  await S.setView({ kind: "collection", id: 77, name: "Abdominal Radiology" });
  await S.doSearch("liver");
  await settle();
  await M.switchMode("wiki");
  await settle();
  assert.equal(tab.shelfAway?.dir, "腹部", "一覧から NOTE へ入ると覚えた本棚を捨てた");
  const reads = called("list_items").length;
  await M.switchMode("library");
  await settle();
  assert.deepEqual([S.app.view.kind, S.app.view.id, S.app.search], ["collection", 77, "liver"], "LIBRARY へ戻すと同じフォルダ・検索語に戻らない");
  const last = called("list_items").at(-1)?.args.filter;
  assert.ok(called("list_items").length > reads && last.collectionId === 77 && last.search === "liver", "戻した一覧を同じ条件で読み直さない");
  // ラベルも
  S.app.tags = [...S.app.tags, { id: 5, name: "liver", color: null, count: 1 }];
  await S.setView({ kind: "tag", id: 5, name: "liver" });
  await M.switchMode("wiki");
  await M.switchMode("library");
  await settle();
  assert.deepEqual([S.app.view.kind, S.app.view.id], ["tag", 5], "ラベルのビューに戻らない");
  // その間にフォルダが消えた = 「すべて」
  await S.setView({ kind: "collection", id: 77, name: "Abdominal Radiology" });
  await M.switchMode("wiki");
  S.app.collections = S.app.collections.filter((c) => c.id !== 77);
  await M.switchMode("library");
  await settle();
  assert.equal(S.app.view.kind, "all", "消えたフォルダのまま戻した");
  await S.doSearch("");

  // 覚えた物が無い（最初から OpenNote のタブ）= 「すべて」
  await S.openTab({ kind: "all" }, { path: null, root: "/v", scrollTop: 0, editing: false });
  await settle();
  const wtab = front();
  await M.switchMode("library");
  await settle();
  assert.deepEqual([wtab.wiki, wtab.shelf, S.app.view.kind], [null, null, "all"], "最初から OpenNote のタブが「すべて」に戻らない");

  // 履歴から外した本棚 = 戻る先を捨てる（「すべて」）
  const keepShelves = shelves;
  await H.switchShelf(OTHER);
  await settle();
  await M.switchMode("wiki");
  await settle();
  assert.equal(front().shelfAway?.root, OTHER);
  await H.forgetShelf(OTHER);
  await settle();
  assert.equal(front().shelfAway, null, "履歴から外した本棚を戻る先に残した");
  await M.switchMode("library");
  await settle();
  assert.deepEqual([front().shelf, S.app.view.kind], [null, "all"], "外した本棚へ戻った");
  await M.switchMode("shelf");
  await settle();
  assert.deepEqual([front().shelf?.root, front().shelf?.dir], [ROOT, null], "戻る先を捨てた後の SHELF が履歴の本棚の入口へ行かない");
  shelves = keepShelves;

  // 再起動: ui_tabs の shelfAway を戻す（scope を先に足す）・履歴に無い root は黙って捨てる・Wiki でないタブの shelfAway は無視
  S.app.tabsRestored = false;
  prefs.ui_tabs = JSON.stringify({
    active: 0,
    tabs: [
      { view: { kind: "all" }, search: "", filters: {}, wiki: { path: null, root: "/v" }, shelfAway: { root: ROOT, dir: "神経" } },
      { view: { kind: "all" }, search: "", filters: {}, wiki: { path: null, root: "/v" }, shelfAway: { root: "/r/履歴に無い", dir: null } },
      { view: { kind: "starred" }, search: "", filters: {}, shelfAway: { root: ROOT, dir: null } },
    ],
  });
  const before = calls.length;
  await S.restoreTabs();
  await settle();
  assert.ok(calls.slice(before).some((c) => c.name === "shelf_front_root" && c.args.root === ROOT), "戻る先の本棚の scope を足さない");
  assert.deepEqual(S.app.tabs.map((t) => [!!t.wiki, t.shelfAway?.root ?? null, t.shelfAway?.dir ?? null, !!t.shelf]), [
    [true, ROOT, "神経", false],
    [true, null, null, false],
    [false, null, null, false],
  ], "戻した戻る先の本棚が違う");
  await M.switchMode("shelf");
  await settle();
  assert.deepEqual([front().shelf?.root, front().shelf?.dir], [ROOT, "神経"], "再起動の後に SHELF へ戻すと覚えた本棚に戻らない");
  await reset();
  wikiOn = false;
  await W.loadWikiStatus();
  await settle();
  tests++;
}

// 同じ往復の続き: 一覧のスクロール位置・メモ欄（タブの切替と同じ = NOTE へ切り替える前に保存して閉じる・
// 保存できなければ切り替えない）・詳細の読み直し・保存を待つ間に前面が変わったら効かせない・最後の 1 枚を閉じると戻る先も外す
{
  await reset();
  wikiOn = true;
  await W.loadWikiStatus();
  await settle();
  const tab = front();
  // 一覧のスクロール位置: 覚えて戻す（OpenNote の間は一覧の部品が外れる）。間に別のタブを行き来しても 0 で消さない
  $(".list > .rows").scrollTop = 120;
  S.app.selected = "K2";
  S.app.checked.add("K2");
  await M.switchMode("wiki");
  await settle();
  assert.equal($(".list > .rows"), null, "OpenNote の間も一覧が残っている前提になった");
  assert.equal(tab.scrollTop, 120, "NOTE へ切り替えるときに一覧の位置を覚えない");
  await S.openTab();
  await settle();
  await S.activateTab(tab.id);
  await settle();
  assert.equal(tab.scrollTop, 120, "OpenNote の間にタブを行き来すると覚えた位置を 0 で消した");
  const details = called("get_item").length;
  await M.switchMode("library");
  await settle();
  assert.equal($(".list > .rows")?.scrollTop, 120, "LIBRARY へ戻しても一覧の位置が戻らない");
  assert.ok(called("get_item").length > details, "戻したときに詳細を読み直さない");
  assert.equal(S.app.selected, "K2");
  await S.closeTab(S.app.tabs.find((t) => t.id !== tab.id).id);
  await settle();

  // メモ欄: 未保存の下書きは NOTE へ切り替える前に保存して閉じる
  S.app.noteEditor = { key: "K2", noteKey: null, text: "下書き", saved: "" };
  const adds = called("add_note").length;
  await M.switchMode("wiki");
  await settle();
  assert.equal(called("add_note").length, adds + 1, "NOTE へ切り替える前にメモを保存しない");
  assert.deepEqual([S.app.noteEditor, !!tab.wiki], [null, true], "保存したメモ欄を閉じない / 切り替わらない");
  await M.switchMode("library");
  await settle();
  // 保存できない = 切り替えない（下書きを持ったまま一覧に残る）
  noteFails = true;
  S.app.noteEditor = { key: "K2", noteKey: null, text: "失敗する下書き", saved: "" };
  await M.switchMode("wiki");
  await settle();
  assert.deepEqual([tab.wiki, S.app.noteEditor?.text], [null, "失敗する下書き"], "メモを保存できないのに切り替えた / 下書きを捨てた");
  noteFails = false;
  S.app.noteEditor = null;

  // 保存を待つ間に前面が変わった = そのタブ（別のタブ）に戻す処理を効かせない
  await M.switchMode("wiki");
  await settle();
  await S.openTab({ kind: "starred" });
  await settle();
  const other = front();
  await S.activateTab(tab.id);
  await settle();
  const p = M.switchMode("library");
  S.app.tab = other.id; // canLeave を待つ間に前面が変わった（切替の中身は試験の外 = 前面の印だけ動かす）
  await p;
  assert.ok(tab.wiki, "前面でなくなったタブを LIBRARY に戻した");
  S.app.tab = tab.id;
  await settle();

  // 最後の 1 枚を閉じる = 中身を外して一覧に戻す（戻る先の本棚も外す = 次に OpenNote へ入るまで古い本棚を持たない）
  await S.closeTab(other.id);
  await settle();
  tab.shelfAway = { root: ROOT, dir: "腹部", query: "", sort: "title", scrollTop: 0 };
  await S.closeTab(tab.id);
  await settle();
  assert.deepEqual([S.app.tabs.length, front().wiki, front().shelfAway], [1, null, null], "最後の 1 枚を閉じても戻る先の本棚が残った");
  await reset();
  wikiOn = false;
  await W.loadWikiStatus();
  await settle();
  tests++;
}

// 中身は常に 1 つ: 走査を待つ間に NOTE を押す = 本棚は新しいタブで・同じタブに同居しない
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
  await M.switchMode("wiki");
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

// 同じ: タブが変わったら本棚のメニューは閉じる。PDF のタブの下のセグメント（ビューアの下に生きている = Tab キーで届く）で SHELF を押しても
// PDF のタブには足さない = 新しいタブ
{
  await reset();
  await M.switchMode("shelf");
  await settle();
  const lib = S.app.tab;
  assert.ok(S.frontShelf(), "前提: 本棚のタブ");
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
  assert.deepEqual(segOn(), ["LIBRARY"], "PDF のタブの下のセグメント（下は一覧）");
  const n = S.app.tabs.length;
  await seg("SHELF"); // 前面は PDF のタブ
  await waitFor(() => S.frontShelf());
  await settle();
  assert.equal(S.app.tabs.length, n + 1, "PDF のタブから SHELF が新しいタブにならない");
  assert.deepEqual([pdf.viewer?.itemKey, pdf.shelf], ["K1", null], "PDF のタブに本棚が足された");
  assert.equal(front().shelf?.root, ROOT, "本棚が新しいタブで開かない");
  assert.ok(JSON.parse(S.tabsPref()).tabs.every((x) => !(x.viewer && x.shelf)), "ui_tabs に同居した");
  await reset();
  tests++;
}

// 読み手のタブから NOTE / LIBRARY / SHELF = 新しいタブ（読み手のタブの中身は変えない）・読み手の前面で ⌘N は下の QuickAdd へ行かない
{
  await H.switchShelf(ROOT);
  await settle();
  $(".sh-grid .sh-book").click();
  await settle();
  const reader = front();
  assert.ok(reader.shelfBook, "読み手のタブでない");
  assert.ok($(".newref"), "読み手の下のヘッダーに「＋ 追加」がある前提が崩れた");
  key({ metaKey: true, key: "n" });
  await settle();
  assert.deepEqual([S.app.newMenu, document.activeElement?.id === "quickadd"], [false, false], "⌘N が読み手の下の「＋ 追加」を開いた");
  wikiOn = true;
  await W.loadWikiStatus();
  await settle();
  await M.switchMode("wiki");
  await settle();
  assert.deepEqual([reader.wiki, !!reader.shelfBook], [null, true], "読み手のタブに Wiki が入った");
  assert.ok(front().wiki && front().id !== reader.id, "新しいタブにならない");
  await S.activateTab(reader.id);
  await settle();
  const n = S.app.tabs.length;
  await M.switchMode("library");
  await settle();
  assert.equal(S.app.tabs.length, n + 1, "LIBRARY が新しいタブにならない");
  assert.ok(reader.shelfBook, "読み手のタブの中身が変わった");
  await S.activateTab(reader.id);
  await settle();
  await M.switchMode("shelf");
  await settle();
  assert.deepEqual([S.app.tabs.length, front().shelf?.root, front().id !== reader.id, !!reader.shelfBook, reader.shelf], [n + 2, ROOT, true, true, null], "読み手のタブから SHELF が新しいタブにならない");
  await reset();
  wikiOn = false;
  await W.loadWikiStatus();
  await settle();
  tests++;
}

// 本棚が 1 つも無い: SHELF の title に案内・押すと「別のフォルダを本棚として開く…」（native の選択）。取り消し = 元のモードのまま
// （一覧のビュー・OpenNote のタブを変えない）・選んだらその本棚へ（同じタブ）。覆い（窓・参照エディタ・取込）の下ではセグメントは効かない
{
  await reset();
  const keep = shelves;
  shelves = [];
  await H.loadShelves();
  await settle();
  assert.ok(segBtns().at(-1).title.includes("本棚はまだありません"), "本棚が無いのに案内が無い");
  await S.setView({ kind: "starred" });
  await settle();
  const picks = called("shelf_open_root").length;
  pickRoot = null;
  // フォルダの選択の窓が開くまで（応答を待つ間）に二度押しても、選択は 1 回だけ（切替の処理中は二度目を無視）
  let release;
  pickGate = new Promise((r) => (release = r));
  const p1 = M.switchMode("shelf");
  const p2 = M.switchMode("shelf");
  await settle();
  release();
  pickGate = null;
  await Promise.all([p1, p2]);
  await settle();
  assert.equal(called("shelf_open_root").length, picks + 1, "二度押しでフォルダの選択を 2 回開いた / 選択へ進まない");
  await seg("SHELF");
  assert.equal(called("shelf_open_root").length, picks + 2, "処理が終わった後の押下を受け付けない");
  assert.deepEqual([front().shelf, S.app.view.kind, segOn()], [null, "starred", ["LIBRARY"]], "取り消したのに一覧が変わった");
  // OpenNote から: 取り消し = OpenNote のまま・選んだら同じタブがその本棚に
  wikiOn = true;
  await W.loadWikiStatus();
  await settle();
  await seg("NOTE");
  const wtab = front();
  assert.ok(wtab.wiki, "NOTE にならない");
  await seg("SHELF");
  assert.equal(called("shelf_open_root").length, picks + 3);
  assert.deepEqual([!!wtab.wiki, wtab.shelf, segOn()], [true, null, ["NOTE"]], "取り消したのに OpenNote を離れた");
  pickRoot = OTHER;
  await seg("SHELF");
  assert.deepEqual([S.app.tab, wtab.wiki, wtab.shelf?.root, segOn()], [wtab.id, null, OTHER, ["SHELF"]], "選んだフォルダの本棚にならない");
  assert.ok(!segBtns().at(-1).title.includes("本棚はまだありません"), "本棚ができたのに案内が残った");
  pickRoot = null;
  // 覆いの下 = 押しても何も変わらない（タブの切替と同じ tabOpsAllowed）
  for (const [label, on, off] of [
    ["窓", () => (S.app.modal = "migration"), () => (S.app.modal = null)],
    ["参照エディタ", () => S.openEditorNew(), () => S.closeEditor()],
    ["取込", () => (S.app.importJob = { files: [], running: false, cancel: false }), () => (S.app.importJob = null)],
  ]) {
    on();
    await settle();
    for (const name of ["LIBRARY", "NOTE"]) {
      await seg(name);
      assert.deepEqual([front().shelf?.root, front().wiki], [OTHER, null], `${label}の下で ${name} が効いた`);
    }
    off();
    await settle();
  }
  shelves = keep;
  await H.loadShelves();
  await reset();
  wikiOn = false;
  await W.loadWikiStatus();
  await settle();
  tests++;
}

// SHELF の行き先の細部:
// ① 履歴の先頭が無いフォルダ = 飛ばして最初の実在する本棚へ（フォルダの選択へ進まない）
// ② 覚えた本棚が履歴から外れていた（MCP 等で外れた = 画面の履歴は古いまま）= 押したときに履歴を読み直して、外れた本棚へは戻らない
// ③ 本棚の見出しのメニューの切替を待つ間に LIBRARY へ移った = 遅れて着いた本棚は同じタブに入る（今までどおり）が、戻る先は残さない（中身は常に 1 つ）
{
  await reset();
  const keep = shelves;
  const gone = { root: GONE, name: "外付け", last_used: 5, exists: false, collapse_single: false, issue_titles: false, aliases: {} };
  const other = { root: OTHER, name: "別の棚", last_used: 1, exists: true, collapse_single: false, issue_titles: false, aliases: {} };
  const root = { root: ROOT, name: "本の棚", last_used: 2, exists: true, collapse_single: false, issue_titles: false, aliases: {} };
  // ①
  shelves = [gone, other];
  await H.loadShelves();
  await settle();
  const picks = called("shelf_open_root").length;
  await seg("SHELF");
  assert.deepEqual([front().shelf?.root, called("shelf_open_root").length], [OTHER, picks], "履歴の先頭の無いフォルダへ行った / フォルダの選択へ進んだ");
  // ②
  shelves = [root, other];
  await H.loadShelves();
  await seg("LIBRARY");
  assert.equal(front().shelfAway?.root, OTHER);
  shelves = [root]; // 画面の外で外れた（sh.shelves はまだ OTHER を持つ）
  assert.ok(H.sh.shelves.some((x) => x.root === OTHER), "前提: 画面の履歴は古いまま");
  await seg("SHELF");
  assert.deepEqual([front().shelf?.root, front().shelf?.dir, front().shelfAway], [ROOT, null, null], "履歴から外れた本棚へ戻った");
  // ③
  shelves = [root, other];
  await H.loadShelves();
  await settle();
  let open;
  switchGate = new Promise((r) => (open = r));
  await openMenu();
  menuRow("別の棚").click();
  await settle();
  await seg("LIBRARY");
  assert.deepEqual([front().shelf, front().shelfAway?.root], [null, ROOT], "前提: 一覧へ移って本の棚を覚えた");
  open();
  switchGate = null;
  await waitFor(() => front().shelf?.root === OTHER);
  await settle();
  assert.deepEqual([front().shelf?.root, front().shelfAway], [OTHER, null], "本棚が入ったタブに古い戻る先が残った");
  shelves = keep;
  await H.loadShelves();
  await reset();
  tests++;
}

// 本棚を開けない（行き止まり）: 履歴の先頭の本棚の切替が失敗しても（Drive の readdir 拒否など）、その本棚のモードには入る =
// 中央に読めない理由と案内・本棚の左バーのメニューから、ほかの本棚へ・別のフォルダを開く・壊れた本棚を外す に届く。
// 一覧の状態（ビュー）は壊さない。メニューから開けない本棚を選んでも同じ（入って理由を見せる = 外せる）
{
  await reset();
  const keep = shelves;
  const mk = (root, name, last_used) => ({ root, name, last_used, exists: true, collapse_single: false, issue_titles: false, aliases: {} });
  shelves = [mk(ROOT, "本の棚", 2), mk(OTHER, "別の棚", 1)];
  await H.loadShelves();
  assert.ok(H.shelfIndex(ROOT), "前提: 本の棚の木は前に読めている（開けなくなった後に古い木を見せない = 理由を出す を確かめる）");
  switchFails.add(ROOT);
  await S.setView({ kind: "starred" });
  await settle();
  await seg("SHELF");
  await waitFor(() => !!$(".sh-err"));
  await settle();
  assert.equal(front().shelf?.root, ROOT, "開けない本棚で何も起きない（行き止まり）");
  assert.ok($(".sh-state").textContent.includes("本棚のフォルダに 本の棚 は使えません（見つかりません）"), "読めない理由（Rust の文）が出ない");
  assert.ok(!$(".sh-state").textContent.includes(ROOT), "読めない理由にフルパスが出た（名前まで）");
  assert.ok($(".sh-state").textContent.includes("見出し"), "本棚のメニューへの案内が出ない");
  // 「読み直す」= もう 1 度切り替えて読む（失敗している間は理由のまま・直れば本が出る）
  const retries = called("shelf_switch_root").filter((c) => c.args.root === ROOT).length;
  $(".sh-retry").click();
  await settle();
  assert.equal(called("shelf_switch_root").filter((c) => c.args.root === ROOT).length, retries + 1, "読み直すで切り替え直さない");
  assert.ok($(".sh-err"), "失敗している間に理由が消えた");
  switchFails.delete(ROOT);
  $(".sh-retry").click();
  await waitFor(() => $$(".sh-book").length > 0);
  await settle();
  assert.equal($(".sh-err"), null, "直ったのに理由が残った / 本が出ない");
  switchFails.add(ROOT);
  // 一覧へ戻ると元のビュー・もう一度 SHELF = 同じ本棚（覚えた本棚）
  await seg("LIBRARY");
  assert.deepEqual([front().shelf, S.app.view.kind], [null, "starred"], "一覧の状態が壊れた");
  await seg("SHELF");
  assert.equal(front().shelf?.root, ROOT);
  // メニュー: 別のフォルダを本棚として開く…
  pickRoot = "/r/新しい棚";
  await openMenu();
  menuRow("別のフォルダを本棚として開く…").click();
  await waitFor(() => front().shelf?.root === "/r/新しい棚");
  await settle();
  pickRoot = null;
  // メニュー: ほかの本棚へ（開ける）→ 開けない本棚を選ぶ = 入って理由を見せる
  await openMenu();
  menuRow("別の棚").click();
  await waitFor(() => front().shelf?.root === OTHER);
  await settle();
  await openMenu();
  menuRow("本の棚").click();
  await waitFor(() => front().shelf?.root === ROOT && !!$(".sh-err"));
  await settle();
  // 壊れた本棚を外す
  await openMenu();
  menuRow("この本棚を履歴から外す").click();
  await settle();
  assert.deepEqual(called("shelf_forget_root").at(-1).args, { root: ROOT });
  assert.equal(H.sh.shelves.some((x) => x.root === ROOT), false, "開けない本棚を外せない");
  assert.equal(front().shelf, null, "外した本棚のタブが残った");
  switchFails.clear();
  shelves = keep;
  await H.loadShelves();
  await reset();
  tests++;
}

// 処理中の印: 遅い SHELF（大きな本棚の初回の走査）の間に押した NOTE / LIBRARY が効く・遅れて返った SHELF は画面を奪わない・
// 別のタブの操作を妨げない・同じタブの SHELF の二度押しは 1 回（上の本棚 0 件の節）・失敗や取り消しの後に印が残らない
{
  await reset();
  wikiOn = true;
  await W.loadWikiStatus();
  await settle();
  const keep = shelves;
  const mk = (root, name, last_used) => ({ root, name, last_used, exists: true, collapse_single: false, issue_titles: false, aliases: {} });
  shelves = [mk(ROOT, "本の棚", 2), mk(OTHER, "別の棚", 1)];
  await H.loadShelves();
  const slow = () => {
    let open;
    switchGate = new Promise((r) => (open = r));
    return () => {
      switchGate = null;
      open();
    };
  };
  const tab = front();
  // 遅い SHELF の間の NOTE が効く → SHELF が返っても NOTE のまま
  let release = slow();
  const p1 = M.switchMode("shelf");
  await settle();
  await seg("NOTE");
  assert.deepEqual([!!tab.wiki, tab.shelf], [true, null], "遅い SHELF の間に NOTE が効かない");
  release();
  await p1;
  await settle();
  assert.deepEqual([!!tab.wiki, tab.shelf, segOn()], [true, null, ["NOTE"]], "遅れて返った SHELF が画面を奪った");
  // 遅い SHELF の間の LIBRARY（今のモード）= その場に留まる意思 → SHELF が返っても一覧のまま
  await seg("LIBRARY");
  release = slow();
  const p2 = M.switchMode("shelf");
  await settle();
  await seg("LIBRARY");
  release();
  await p2;
  await settle();
  assert.deepEqual([tab.wiki, tab.shelf, segOn()], [null, null, ["LIBRARY"]], "LIBRARY を押したのに遅れて返った SHELF へ入った");
  // 別のタブ: タブ 1 の SHELF を待つ間にタブ 2 で NOTE が効く・タブ 1 は変わらない（前面でなくなった）
  release = slow();
  const p3 = M.switchMode("shelf");
  await settle();
  await S.openTab();
  await settle();
  const tab2 = front();
  await seg("NOTE");
  assert.ok(tab2.wiki, "別のタブの NOTE が効かない（処理中の印が別のタブまで飲んだ）");
  release();
  await p3;
  await settle();
  assert.deepEqual([tab.shelf, tab.wiki, !!tab2.wiki, S.app.tab], [null, null, true, tab2.id], "前面でないタブを本棚にした / 前面を奪った");
  // 別のタブの同じ行き先（印のキーを全体で 1 つにしない）: タブ 1 の SHELF を待つ間に、タブ 3 で押した SHELF も効く
  await S.activateTab(tab.id);
  await settle();
  release = slow();
  const p3b = M.switchMode("shelf");
  await settle();
  await S.openTab();
  await settle();
  const tab3 = front();
  const p3c = M.switchMode("shelf");
  await settle();
  release();
  await Promise.all([p3b, p3c]);
  await settle();
  assert.deepEqual([tab3.shelf?.root, tab.shelf, S.app.tab], [ROOT, null, tab3.id], "別のタブの SHELF が、タブ 1 の処理中の印に飲まれた");
  // 押下が 2 つの待ちのどちらの間に来ても後の押下が勝つ:
  // ① 行き先を決める間（遅い走査）に LIBRARY = 追い越された SHELF は一覧を離れる前の処理（メモの保存）もしない
  await S.activateTab(tab.id);
  await settle();
  S.app.noteEditor = { key: "K2", noteKey: null, text: "下書き", saved: "" };
  const adds = called("add_note").length;
  release = slow();
  const p4 = M.switchMode("shelf");
  await settle();
  await seg("LIBRARY");
  release();
  await p4;
  await settle();
  assert.equal(called("add_note").length, adds, "追い越された SHELF が一覧を離れる前のメモの保存をした");
  assert.deepEqual([tab.shelf, S.app.noteEditor?.text], [null, "下書き"], "追い越された SHELF が一覧を変えた");
  // ② 一覧を離れる前のメモの保存を待つ間（処理中 = app.busy）の押下は何もしない（タブの切替と同じ）= 保存の後に SHELF に入る
  //    （保存の後の live() は、ここでは押下が届かないので試験で落とせない = OpenNote の保存の間の押下の守り）
  tab.shelfAway = { root: ROOT, dir: null, query: "", sort: "title", scrollTop: 0 }; // 覚えた本棚 = 行き先は待たずに決まる
  let saved;
  noteGate = new Promise((r) => (saved = r));
  const p5 = M.switchMode("shelf");
  await settle();
  assert.equal(called("add_note").length, adds + 1, "前提: メモの保存を待っている");
  assert.ok(S.app.busy, "前提: メモの保存の間は処理中");
  await seg("LIBRARY");
  noteGate = null;
  saved();
  await p5;
  await settle();
  assert.deepEqual([tab.shelf?.root, segOn()], [ROOT, ["SHELF"]], "保存の間の押下が効いた / 保存の後に SHELF に入らない");
  await seg("LIBRARY");
  tab.shelfAway = null;
  S.app.noteEditor = null;
  // 失敗・取り消しの後に印が残らない: フォルダの選択が失敗 → 次の押下は通る
  await S.activateTab(tab.id);
  await settle();
  shelves = [];
  await H.loadShelves();
  pickFails = true;
  const picks = called("shelf_open_root").length;
  await seg("SHELF");
  pickFails = false;
  pickRoot = null;
  await seg("SHELF");
  assert.equal(called("shelf_open_root").length, picks + 2, "失敗の後に SHELF を受け付けない（印が残った）");
  assert.deepEqual([tab.shelf, segOn()], [null, ["LIBRARY"]]);
  shelves = keep;
  await H.loadShelves();
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
    { root: ROOT, name: "本の棚", last_used: 2, exists: true, collapse_single: false, issue_titles: false, aliases: {} },
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

// 本棚の操作（表示の設定・Finder で表示・外部で開く）の失敗の toast に、本棚のフォルダの親のパスを出さない（名前まで）。理由の文
// （/ を含む物も）は欠けない。畳むのは 200 字で切る前（長い root でも名前まで）
{
  opsFail = true;
  const LONG = `/r/${"あ".repeat(210)}/長い棚`;
  const toastOf = async (f) => {
    S.app.toast = null;
    await f();
    await settle();
    return S.app.toast ?? "";
  };
  for (const [label, f, want] of [
    ["表示の設定", () => H.setCollapse(ROOT, true), "✗ 履歴に無い本棚です: 本の棚（root には shelf_status の root をそのまま渡す）"],
    ["表示の設定（長い root）", () => H.setCollapse(LONG, true), "✗ 履歴に無い本棚です: 長い棚（root には shelf_status の root をそのまま渡す）"],
    ["本棚の Finder で表示", () => H.revealBook(ROOT), "✗ 本棚のフォルダに 本の棚 は使えません（見つかりません）"],
    ["本の Finder で表示", () => H.revealBook(ROOT, `id-${ROOT}/総論.pdf`), "✗ 本棚のフォルダに 本の棚 は使えません（見つかりません）"],
    ["外部で開く", () => H.openExternal(ROOT, `id-${ROOT}/総論.pdf`), "✗ 本棚のフォルダに 本の棚 は使えません（iCloud Drive / CloudStorage の最上位です。その中のフォルダを選んでください）"],
  ]) {
    const got = await toastOf(f);
    assert.equal(got, want, `${label}: 失敗の toast`);
    assert.ok(!got.includes("/r/"), `${label}: 親のパスが残る`);
  }
  opsFail = false;
  S.app.toast = null;
  tests++;
}

inv.done();
console.log(`shelf: ok (${tests} tests)`);
process.exit(0);
