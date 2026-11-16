/* 左の列の幅（境界のつまみ）の試験（node app/src/lib/sidebarw.test.mjs・実部品 App.svelte + SidebarGrip + sidebarw.svelte.ts + jsdom・invoke は差し替え）。確かめること:
   ① 設定の値の読み（parseSidebarW）: 壊れた JSON・形の違う値は既定 230・範囲の外は端へ / 置き場の CSS（jsdom は配置しない = 静的な表明）:
      main.sbw は positioned・つまみは top と bottom の両方を持つ（top: auto だと高さ 0 = 見えず掴めない）・帯は中央の側へ寄せる
   ② 設定を読めるまでは書かない: 読めなかった間の変更は書かず、読み込みの途中のドラッグも書かず、読めたら 1 回だけ書く（触っていないモードは保存値を残す）
   ③ ドラッグ: 動かしている間は線だけ（幅も設定も変えない）・離したときに 1 回だけ幅が変わり、書き込みも 1 回 = ui_sidebar_w = {"lib": n, "wiki": n}・
      Esc = 取り消し（元の幅のまま・書かない・背後へ流さない）・ボタンを離したまま動いた / pointer を失った = そこで確定
   ④ 範囲: 下限 180・上限 = main の幅 − 中央の最小幅 360 − 右ペイン 380（出しているとき）
   ⑤ モードごとに別の幅（LIBRARY と NOTE）・タブを行き来すると、そのモードの幅に戻る・途中でモードが変わっても始めたときのモードへ書く
   ⑥ ダブルクリック（dblclick と、押下の間隔の自前の判定）で既定の幅・同じ幅は書かない
   ⑦ キーボード: ← → は押している間は線だけ・離したとき（keyup）に 1 回確定（キーリピートで書かない）・起点は窓で詰めた表示の幅・
      aria-valuenow = 表示の幅・aria-valuemax = 上限
   ⑧ 集中モード（NOTE）ではつまみを出さず（ドラッグの途中で消えたら確定しない）、格子は集中モードの 1 列のまま（main に sbw を付けない）・
      本棚にもつまみ（幅は shelf の鍵）
   ⑨ 3 つの左バー（LIBRARY・本棚 = nav.ls-nav・NOTE = nav.wk-sidebar）は同じ外形（.sidepane = 角丸・枠・影・面）で、右隣の列は影を覆わない（static）。pdf.js の .sidebar
      （pdfjs-dist/web/pdf_viewer.css = PdfViewer が読み、画面の全体に効く・width 239px ほか）はどれにも効かない:
      画面の部品と app.css に素の class「sidebar」が残っていない・3 つの部品が sidepane を持つ・pdf.js の素の .sidebar の規則を抜き出して
      app.css と一緒に jsdom の cascade に掛けると、3 つとも同じ角丸・影・枠・位置で、幅は pdf.js の値にならない
   ⑪ 本棚のつまみ: ドラッグ・← → ・Home・ダブルクリック・下限 180 = shelf の鍵だけを書く（lib / wiki は残す）/
      前後の版の読み書き = 前の版（parseSidebarW と書き方の写し）は新しい値を読んでも lib / wiki をそのまま読む・
      前の版が書いた値（shelf が無い）を新しい版は shelf = 既定 230 で読む
   出力 = `sidebar width: ok (N tests)`

   配置の実測の手順（jsdom は配置を計算しない = この試験では位置を測れない）:
   1) 実部品の App を jsdom に mount（蔵書 8 件・フォルダ 10）して document.body の HTML を書き出す（PdfViewer は偽物で可）
   2) その HTML に、組み立てた CSS（cd app && npm run build → app/dist/assets/index-*.css）をそのまま入れる
      （つまみの svelte の scoped の hash は jsdom と vite で違う = class を組み立てた CSS の hash に付け替える）
   3) WKWebView（実機と同じ WebKit = swift で隠した窓に読み込む・画面に出さない）と headless Chrome（使い捨ての profile・proxy を潰してネットワークなし）で開き、
      main の --sb-w を 230 → 420 → 180 → 230 と変え、詳細ペインあり / なし（main.lib-nodetail・aside.detail を外す）で測る
   4) あるべき位置（窓 1280・ヘッダーの下）: 左バー（nav.ls-nav / nav.wk-sidebar）の右端 = .list の左端 = 列の幅（180 / 230 / 420）・行の .cb の左端 = .list の左端 + 32.5・
      全選択（.lb-checkbtn .cb）= .list の左端 + 33・.sbw-grip の左端 = 列の幅 − 1・高さ = main の高さ
      直す前の CSS は、どの幅でも LIBRARY の左バーの右端 = 239（pdf.js の --sidebar-width）だった。
      外形: 3 つとも border-radius 8px・box-shadow = --side-shadow・4 辺とも 1px solid var(--border)・background var(--panel)・
      padding 8px 6px・position relative（ライト / ダークとも・本棚の幅 = 列の 230 = はみ出しなし） */
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { setupHarness } from "./wiki/test-harness.mjs";
import { fakeInvoke } from "./test-invoke.mjs";

const { calls, sleep, waitFor } = setupHarness({
  fakes: {
    "@tauri-apps/api/webview": "export const getCurrentWebview = () => ({ onDragDropEvent: async () => () => {} });",
    "@tauri-apps/plugin-dialog": "export const open = async () => null; export const save = async () => null; export const confirm = async () => false; export const message = async () => {};",
    "./lib/PdfViewer.svelte": "export default function PdfViewer() {}",
    "./lib/shelf/ShelfViewer.svelte": "export default function ShelfViewer() {}",
  },
});
let tests = 0;

const prefs = { ui_sidebar_w: JSON.stringify({ lib: 300, wiki: 400, shelf: 260 }) }; // shelf = 起動時に本棚の幅も読む
/** ui_sidebar_w の読み: ok / fail = 投げる / gate = gateOpen まで待つ */
let spMode = "ok";
let gateOpen;
let gate = null;
// 場面の応答だけを上書き（ほかは test-invoke.mjs の既定 = 本物と同じ形の空）
const inv = fakeInvoke(calls, {
  get_ui_pref: async (a) => {
    if (a.key === "ui_sidebar_w") {
      if (spMode === "fail") throw new Error("database is locked");
      if (spMode === "gate") await gate;
    }
    return prefs[a.key] ?? null;
  },
  set_ui_pref: (a) => {
    prefs[a.key] = a.value;
    return null;
  },
  wiki_settings: () => ({ enabled: true, root: "/v" }),
  wiki_status: () => ({ enabled: true, root: "/v", workspaces: [{ root: "/v", last_used: 1, exists: true, current: true }] }),
  wiki_list: () => ({ total: 1, count: 1, pages: [{ path: "a.md", kind: "md", title: "A", tags: [], items: [], mtime: 0, size: 1 }] }),
  wiki_read_gui: (a) => ({ path: a.path, abs_path: "/v/" + a.path, kind: "md", title: a.path, size: 1, mtime: 0, content: "# " + a.path, sha256: a.path, truncated: false, links: [] }),
  shelf_tree: (a) => ({ root: a.root, dir: null, tree: { id: "r", name: "s", path: a.root, children: [], books: [], totalBooks: 0 }, unreadable: 0, used_find: false, thumbs_dir: null }),
  // ⑩ のセグメントの SHELF（履歴に本棚 "/s" が 1 つ）
  shelf_status: () => ({ shelves: [{ root: "/s", name: "s", last_used: 1, exists: true, collapse_single: false, issue_titles: false, aliases: {} }] }),
  shelf_switch_root: (a) => ({ root: a.root }),
});
globalThis.__invoke = inv.invoke;

const SW = await import("./sidebarw.svelte.ts");

const writes = () => calls.filter((c) => c.name === "set_ui_pref" && c.args.key === "ui_sidebar_w");
const storedAll = () => JSON.parse(prefs.ui_sidebar_w);
/** 保存値の lib / wiki（shelf の鍵は storedAll で見る） */
const stored = () => {
  const v = storedAll();
  return { lib: v.lib, wiki: v.wiki };
};

// ---------- ① parseSidebarW・置き場の CSS ----------
{
  for (const bad of [null, "", "{broken", "[]", "1", "null", '"x"']) assert.deepEqual(SW.parseSidebarW(bad), { lib: 230, wiki: 230, shelf: 230 }, `既定で読まない: ${bad}`);
  assert.deepEqual(SW.parseSidebarW('{"lib": 300, "wiki": "x"}'), { lib: 300, wiki: 230, shelf: 230 }, "形の違う値を既定にしない");
  assert.deepEqual(SW.parseSidebarW('{"lib": 10, "wiki": 99999, "shelf": 5}'), { lib: 180, wiki: 1200, shelf: 180 }, "範囲の外を端へ詰めない");
  const css = readFileSync(new URL("../app.css", import.meta.url), "utf8");
  const sbw = /^main\.sbw \{([^}]*)\}/m.exec(css)?.[1] ?? "";
  assert.match(sbw, /position: relative/, "main.sbw が positioned でない（つまみの基準）");
  const grip = /\.sbw-grip \{([^}]*)\}/.exec(readFileSync(new URL("./SidebarGrip.svelte", import.meta.url), "utf8"))?.[1] ?? "";
  assert.match(grip, /position: absolute/);
  assert.match(grip, /(^|[\s;])top: 0;/, "つまみに top: 0 が無い（高さ 0 になる）");
  assert.match(grip, /bottom: 0;/, "つまみに bottom: 0 が無い");
  assert.doesNotMatch(grip, /top: auto/, "つまみが top: auto（高さ 0 になる）");
  assert.match(grip, /margin-left: -1px;/, "帯が木の側へ 1px より多く被る（木のスクロールバーを覆う）");
  // 列の幅の数字: TS の定数（sidebarw.svelte.ts）と CSS の数字（app.css・WikiPage の集中モード・SidebarGrip の既定）が対
  const { SIDEBAR_DEFAULT: D, SIDEBAR_MIN: MIN, CENTER_MIN: C, RIGHT_W: R } = SW;
  const rule = (src, sel) => new RegExp(`^\\s*${sel.replace(/[.()>]/g, (c) => `\\${c}`)} \\{([^}]*)\\}`, "m").exec(src)?.[1] ?? "";
  assert.match(rule(css, "main"), new RegExp(`grid-template-columns: ${D}px minmax\\(${C}px, 1fr\\) ${R}px;`), "main の格子が定数と違う");
  assert.match(rule(css, "main.wk-main-noinfo, main.lib-nodetail"), new RegExp(`grid-template-columns: ${D}px minmax\\(${C}px, 1fr\\);`));
  assert.ok(sbw.includes(`--sb-col: max(${MIN}px, min(var(--sb-w, ${D}px), 100% - ${C + R}px));`), "main.sbw の --sb-col が定数と違う");
  assert.ok(sbw.includes(`grid-template-columns: var(--sb-col) minmax(${C}px, 1fr) ${R}px;`), "main.sbw の格子が定数と違う");
  assert.ok(rule(css, "main.sbw.wk-main-noinfo, main.sbw.lib-nodetail").includes(`--sb-col: max(${MIN}px, min(var(--sb-w, ${D}px), 100% - ${C}px)); grid-template-columns: var(--sb-col) minmax(${C}px, 1fr);`), "右ペインなしの main.sbw が定数と違う");
  const page = readFileSync(new URL("./wiki/WikiPage.svelte", import.meta.url), "utf8");
  assert.ok(page.includes(`:global(main.wk-main-focus > .wk-sidebar) { left: 0; width: var(--sb-w, ${D}px); }`), "集中モードの木の幅が定数と違う");
  assert.ok(page.includes(`:global(main.wk-main-focus > .wk-info) { right: 0; width: ${R}px; }`), "集中モードの右ペインの幅が定数と違う");
  assert.match(grip, new RegExp(`left: var\\(--sb-col, ${D}px\\);`), "つまみの --sb-col の既定が定数と違う");
  // 上の照合は各セレクタの最初の規則しか読まない → main の要素を狙って格子（grid-template-columns）か --sb-col を書く規則が、
  // 上の 4 つ（app.css）と集中モードの 1 つ（WikiPage = 別の格子）のほかに無いことも見る（後ろに足した `main { grid-template-columns: … }` や
  // より強いセレクタで上書きされていない）。app.css と全部品の <style> を読む（部品は :global(main…) だけが main に届く）
  const cssRules = (src) => {
    const out = [];
    for (const m of src.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]*)\}/g)) out.push({ sel: m[1].trim().replace(/\s+/g, " "), body: m[2] });
    return out;
  };
  const targetsMain = (sel) =>
    sel.split(",").some((s) => /(^|[\s>+~])main(?=$|[.#:[])[^\s>+~]*$/.test(s.trim().replace(/:global\(([^)]*)\)/g, "$1")));
  const writers = [];
  const scan = (name, src) => {
    for (const r of cssRules(src)) if (targetsMain(r.sel) && /grid-template-columns|--sb-col/.test(r.body)) writers.push(`${name}: ${r.sel.replace(/:global\(([^)]*)\)/g, "$1")}`);
  };
  scan("app.css", css);
  const walk = (dir) =>
    readdirSync(dir, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? walk(join(dir, e.name)) : e.name.endsWith(".svelte") ? [join(dir, e.name)] : []));
  const srcDir = fileURLToPath(new URL("..", import.meta.url));
  for (const f of walk(srcDir)) for (const m of readFileSync(f, "utf8").matchAll(/<style[^>]*>([\s\S]*?)<\/style>/g)) scan(relative(srcDir, f), m[1]);
  assert.deepEqual(
    writers.sort(),
    [
      "app.css: main",
      "app.css: main.sbw",
      "app.css: main.sbw.wk-main-noinfo, main.sbw.lib-nodetail",
      "app.css: main.wk-main-noinfo, main.lib-nodetail",
      "lib/wiki/WikiPage.svelte: main.wk-main-focus",
    ],
    "main の格子 / --sb-col を書く規則が増えた（後ろの規則が照合した値を上書きしうる）"
  );
  tests++;
}

// ---------- ② 設定を読めるまでは書かない ----------
// 読めない間に lib を変えた = 書かない（覚えるだけ）
spMode = "fail";
const warn = console.warn;
console.warn = () => {};
SW.setSidebarW("lib", 310);
await sleep(20);
assert.equal(writes().length, 0, "読めなかったのに書いた（触っていないモードの保存値を消す）");
assert.equal(prefs.ui_sidebar_w, JSON.stringify({ lib: 300, wiki: 400, shelf: 260 }));
console.warn = warn;
// 読み込みの途中（App の起動の読みを止める）にドラッグ = 書かない → 読めたら 1 回だけ、触っていない wiki は保存値のまま
spMode = "gate";
gate = new Promise((r) => (gateOpen = r));

const S = await import("./state.svelte.ts");
const W = await import("./wiki/wiki.svelte.ts");
const F = await import("./wiki/focus.svelte.ts");
const { mount, flushSync } = await import("svelte");
const { default: App } = await import("../App.svelte");
const $ = (q) => document.querySelector(q);
const settle = async () => {
  for (let i = 0; i < 4; i++) {
    flushSync();
    await sleep(10);
  }
  flushSync();
};
const target = document.createElement("div");
document.body.append(target);
mount(App, { target });
await settle();
const main = () => $("main");
const grip = () => $(".sbw-grip");
const shown = () => main().style.getPropertyValue("--sb-w");
// main の箱（jsdom に配置は無い）= 幅 mainWidth の窓
let mainWidth = 1200;
HTMLElement.prototype.getBoundingClientRect = function () {
  return this.tagName === "MAIN"
    ? { left: 0, top: 40, right: mainWidth, bottom: 800, width: mainWidth, height: 760, x: 0, y: 40 }
    : { left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0, x: 0, y: 0 };
};
const down = (x = 230) => grip().dispatchEvent(new PointerEvent("pointerdown", { button: 0, buttons: 1, clientX: x, bubbles: true, cancelable: true }));
const drag = async (xs, { up = true } = {}) => {
  const g = grip();
  down();
  const seen = [];
  for (const x of xs) {
    g.dispatchEvent(new PointerEvent("pointermove", { buttons: 1, clientX: x, bubbles: true }));
    flushSync();
    seen.push({ w: shown(), guide: $(".sbw-guide")?.style.left ?? null, writes: writes().length });
  }
  if (up) g.dispatchEvent(new PointerEvent("pointerup", { clientX: xs.at(-1), bubbles: true }));
  await settle();
  await sleep(450); // 次の押下を自前のダブルクリックと見なさない
  return seen;
};
const key = (k, o = {}, type = "keydown") => {
  const ev = new KeyboardEvent(type, { key: k, bubbles: true, cancelable: true, ...o });
  grip().dispatchEvent(ev);
  return ev;
};

{
  assert.ok(grip(), "つまみが無い");
  assert.ok(main().classList.contains("sbw"), "LIBRARY の main に sbw が無い");
  assert.equal(grip().getAttribute("role"), "separator");
  assert.equal(grip().parentElement, main(), "つまみが main の子でない");
  await drag([320]);
  assert.equal(shown(), "320px");
  assert.equal(writes().length, 0, "読み込みの途中に書いた");
  gateOpen();
  spMode = "ok";
  assert.ok(await waitFor(() => writes().length === 1), "読めても書かない");
  await settle();
  assert.equal(writes().length, 1, "読めた後の書き込みが 1 回でない");
  assert.deepEqual(stored(), { lib: 320, wiki: 400 }, "触っていない wiki の保存値を消した / lib の変更を書かない");
  assert.equal(storedAll().shelf, 260, "書く値に本棚の幅（shelf の鍵）が無い / 起動時に読んだ本棚の幅を書き戻さない");
  assert.equal(shown(), "320px", "読んだ値でドラッグの幅を上書きした");
  tests++;
}

// ---------- ③ ドラッグ = 離したときに 1 回だけ・Esc で取り消し ----------
{
  const before = writes().length;
  const seen = await drag([260, 300, 340]);
  assert.deepEqual(seen.map((s) => s.w), ["320px", "320px", "320px"], "動かしている間に幅が変わった");
  assert.deepEqual(seen.map((s) => s.guide), ["260px", "300px", "340px"], "動かしている間に線が動かない");
  assert.ok(seen.every((s) => s.writes === before), "動かしている間に書いた");
  assert.equal(shown(), "340px", "離しても幅が変わらない");
  assert.equal($(".sbw-guide"), null, "離しても線が残る");
  assert.equal(writes().length, before + 1, "書き込みが 1 回でない");
  assert.deepEqual(stored(), { lib: 340, wiki: 400 });
  assert.ok(!document.body.classList.contains("dragging"), "body の印が残る");
  // Esc = 取り消し
  let behind = 0;
  const bubble = (e) => e.key === "Escape" && behind++;
  window.addEventListener("keydown", bubble);
  await drag([500], { up: false });
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  await settle();
  grip().dispatchEvent(new PointerEvent("pointerup", { clientX: 500, bubbles: true }));
  await settle();
  window.removeEventListener("keydown", bubble);
  assert.equal(shown(), "340px", "Esc で取り消さない");
  assert.equal($(".sbw-guide"), null, "Esc の後に線が残る");
  assert.equal(writes().length, before + 1, "取り消したのに書いた");
  assert.equal(behind, 0, "ドラッグ中の Esc が背後へ流れた");
  await sleep(450);
  // ボタンを離したまま動いた（pointerup の取りこぼし）= そこで確定・lostpointercapture でも確定
  down();
  grip().dispatchEvent(new PointerEvent("pointermove", { buttons: 1, clientX: 380, bubbles: true }));
  grip().dispatchEvent(new PointerEvent("pointermove", { buttons: 0, clientX: 600, bubbles: true }));
  await settle();
  assert.equal(shown(), "380px", "ボタンを離したまま動いても確定しない / 離した後の位置で確定した");
  assert.equal($(".sbw-guide"), null, "取りこぼしの後に線が残る");
  await sleep(450);
  down();
  grip().dispatchEvent(new PointerEvent("pointermove", { buttons: 1, clientX: 340, bubbles: true }));
  grip().dispatchEvent(new PointerEvent("lostpointercapture", { bubbles: true }));
  await settle();
  assert.equal(shown(), "340px", "pointer を失ったときに確定しない");
  assert.equal(writes().length, before + 3, "取りこぼし / pointer を失ったときの書き込みが 1 回ずつでない");
  assert.ok(!document.body.classList.contains("dragging"), "pointer を失った後に body の印が残る");
  await sleep(450);
  tests++;
}

// ---------- ④ 範囲 ----------
{
  await drag([50]);
  assert.equal(shown(), "180px", "下限で詰めない");
  const right = S.app.detailOpen;
  await drag([2000]);
  assert.equal(shown(), `${1200 - 360 - (right ? 380 : 0)}px`, "上限（中央の最小幅と右ペインを残す）で詰めない");
  S.app.detailOpen = !right;
  await settle();
  await drag([2000]);
  assert.equal(shown(), `${1200 - 360 - (!right ? 380 : 0)}px`, "右ペインの有無で上限が変わらない");
  S.app.detailOpen = right;
  await settle();
  await drag([320]);
  assert.equal(shown(), "320px");
  tests++;
}

// ---------- ⑤ モードごとに別の幅・始めたときのモードへ書く ----------
const libTab = S.app.tab;
let wikiTab;
{
  S.app.wikiOn = true;
  await settle();
  await W.openWikiPage("a.md", true);
  await settle();
  await sleep(30);
  await settle();
  assert.ok(S.frontWiki(), "NOTE のタブが前面にならない");
  wikiTab = S.app.tab;
  assert.equal(shown(), "400px", "NOTE が LIBRARY の幅を使った");
  await drag([450]);
  assert.equal(shown(), "450px");
  assert.deepEqual(stored(), { lib: 320, wiki: 450 }, "モードごとに覚えない");
  await S.activateTab(libTab);
  await settle();
  assert.equal(shown(), "320px", "LIBRARY に戻っても LIBRARY の幅にならない");
  // LIBRARY で始めたドラッグを、NOTE のタブで離す = LIBRARY へ書く
  await drag([360], { up: false });
  await S.activateTab(wikiTab);
  await settle();
  grip().dispatchEvent(new PointerEvent("pointerup", { clientX: 360, bubbles: true }));
  await settle();
  assert.deepEqual(stored(), { lib: 360, wiki: 450 }, "途中でモードが変わったら新しいモードへ書いた");
  assert.equal(shown(), "450px", "NOTE の幅が変わった");
  await sleep(450);
  tests++;
}

// ---------- ⑥ ダブルクリック ----------
{
  const before = writes().length;
  grip().dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  await settle();
  assert.equal(shown(), "230px", "dblclick で既定に戻らない");
  assert.equal(writes().length, before + 1);
  grip().dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  await settle();
  assert.equal(writes().length, before + 1, "同じ幅なのに書いた");
  await drag([420]);
  // dblclick が届かなくても、押下の間隔で既定へ（押下 → 離す → 押下）
  down();
  grip().dispatchEvent(new PointerEvent("pointerup", { clientX: 230, bubbles: true }));
  down();
  grip().dispatchEvent(new PointerEvent("pointerup", { clientX: 230, bubbles: true }));
  await settle();
  assert.equal(shown(), "230px", "押下の間隔のダブルクリックで既定に戻らない");
  assert.equal(writes().length, before + 3, "押下の間隔のダブルクリックの書き込みが 1 回でない");
  await sleep(450);
  tests++;
}

// ---------- ⑦ キーボード ----------
{
  await drag([400]);
  const before = writes().length;
  // 押している間（キーリピート）は線だけ・離したときに 1 回
  key("ArrowRight");
  key("ArrowRight", { repeat: true });
  key("ArrowRight", { repeat: true });
  await settle();
  assert.equal(shown(), "400px", "押している間に幅が変わった");
  assert.equal($(".sbw-guide")?.style.left, "448px", "押している間に線が動かない");
  assert.equal(writes().length, before, "キーリピートで書いた");
  key("ArrowRight", {}, "keyup");
  await settle();
  assert.equal(shown(), "448px", "離しても幅が変わらない");
  assert.equal($(".sbw-guide"), null);
  assert.equal(writes().length, before + 1, "離したときの書き込みが 1 回でない");
  // 窓で詰めた状態: 覚えた 800・表示 = 上限（右ペインを隠して main 900 = 540）→ ← の起点は表示の幅
  const right = S.app.detailOpen;
  W.wk.infoOpen = false;
  await settle();
  mainWidth = 1200;
  await drag([800]);
  assert.equal(shown(), "800px");
  mainWidth = 900;
  window.dispatchEvent(new Event("resize"));
  await settle();
  assert.equal(grip().getAttribute("aria-valuenow"), "540", "aria-valuenow が表示の幅でない");
  assert.equal(grip().getAttribute("aria-valuemax"), "540", "aria-valuemax が無い / 上限でない");
  key("ArrowLeft");
  key("ArrowLeft", {}, "keyup");
  await settle();
  assert.equal(shown(), "524px", "← の起点が表示の幅でない（覚えた幅から数えた）");
  mainWidth = 1200;
  window.dispatchEvent(new Event("resize"));
  W.wk.infoOpen = true;
  S.app.detailOpen = right;
  await settle();
  tests++;
}

// ---------- ⑧ 集中モード・本棚 ----------
{
  await drag([300]);
  // 終わったドラッグの古い取り消し（つまみは最後の取り消しを持ったまま）が、つまみの消えるときにもう一度呼ばれても何もしない
  // = 別のドラッグ（柱のつまみ・行のドラッグ）が付けた body の印を外さない（前の実装の abort = null・dragGuide の done）
  {
    document.body.classList.add("dragging"); // 別のドラッグの最中
    F.focusMode.on = true;
    await settle();
    assert.equal(grip(), null);
    assert.ok(document.body.classList.contains("dragging"), "終わったドラッグの取り消しが、別のドラッグの body の印を外した");
    document.body.classList.remove("dragging");
    F.focusMode.on = false;
    await settle();
  }
  // ドラッグの途中でつまみが消えた（集中モード）= 途中の幅を確定しない・body の印を残さない・その後の blur でも確定しない
  {
    const before = writes().length;
    await drag([520], { up: false });
    F.focusMode.on = true;
    await settle();
    assert.equal(grip(), null);
    assert.ok(!document.body.classList.contains("dragging"), "つまみが消えても body の印が残る");
    window.dispatchEvent(new Event("blur"));
    await settle();
    assert.equal(writes().length, before, "つまみが消えたときに途中の幅を確定した");
    F.focusMode.on = false;
    await settle();
    assert.equal(shown(), "300px", "つまみが消えたドラッグの幅が残った");
  }
  F.focusMode.on = true;
  await settle();
  assert.equal(grip(), null, "集中モードでつまみを出した");
  assert.ok(main().classList.contains("wk-main-focus"), "集中モードの格子にならない");
  assert.ok(!main().classList.contains("sbw"), "集中モードの main に sbw を付けた（集中モードの 1 列を上書きしうる）");
  F.focusMode.on = false;
  await settle();
  assert.ok(grip(), "集中モードを出てもつまみが戻らない");
  assert.equal(shown(), "300px", "集中モードの後に幅が変わった");
  // 本棚 = つまみがある（幅は shelf の鍵 = 起動時に読んだ 260）
  const errs = [];
  const onErr = (e) => errs.push(String(e?.error ?? e?.message ?? e));
  window.addEventListener("error", onErr);
  await S.openTab({ kind: "all" }, null, S.shelfHome("/s"));
  await settle();
  window.removeEventListener("error", onErr);
  assert.ok(S.frontShelf(), "本棚のタブが前面にならない");
  assert.deepEqual(errs, [], "本棚のタブでエラー");
  assert.ok(grip(), "本棚につまみが無い");
  assert.ok(main().classList.contains("sbw"), "本棚の main に sbw が無い");
  assert.equal(shown(), "260px", "本棚の main の --sb-w が起動時に読んだ本棚の幅（260）でない");
  tests++;
}

// ---------- ⑨ 3 つの左バーは同じ外形・pdf.js の .sidebar はどれにも効かない ----------
{
  const src = new URL("..", import.meta.url);
  const read = (p) => readFileSync(new URL(p, src), "utf8");
  const css = read("app.css");
  // 画面の部品と app.css に素の class「sidebar」が残っていない（pdf.js の素の .sidebar に掴まれる）
  const noComments = css.replace(/\/\*[^]*?\*\//g, "");
  assert.doesNotMatch(noComments, /\.sidebar(?![\w-])/, "app.css に素の .sidebar の選択子が残る（pdf.js の .sidebar と同じ名前）");
  const parts = { "lib/Sidebar.svelte": "ls-nav", "lib/shelf/ShelfSidebar.svelte": "ls-nav", "lib/wiki/WikiSidebar.svelte": "wk-sidebar" };
  for (const [f, cls] of Object.entries(parts)) {
    const t = read(f);
    assert.doesNotMatch(t, /class="([^"]*\s)?sidebar(\s[^"]*)?"/, `${f} に素の class「sidebar」が残る`);
    assert.match(t, new RegExp(`<nav class="${cls} sidepane">`), `${f} の左バーが外形（sidepane）を持たない`);
  }
  assert.doesNotMatch(read("lib/shelf/ShelfHead.svelte"), /\.sidebar(?![\w-])/, "ShelfHead が素の .sidebar を見ている");
  // pdf.js の素の `.sidebar{…}` の規則（括弧を数えて抜き出す・入れ子は外して直下の宣言だけ。jsdom は pdf_viewer.css の全体を読めない）
  const pdfCss = readFileSync(new URL("../../node_modules/pdfjs-dist/web/pdf_viewer.css", import.meta.url), "utf8");
  const blocks = [];
  for (const m of pdfCss.matchAll(/(^|[}\s])\.sidebar\s*\{/g)) {
    let i = m.index + m[0].length;
    let depth = 1;
    const from = i;
    while (depth && i < pdfCss.length) {
      if (pdfCss[i] === "{") depth++;
      else if (pdfCss[i] === "}") depth--;
      i++;
    }
    const body = pdfCss.slice(from, i - 1);
    const decls = [];
    let seg = "";
    for (let j = 0, d = 0; j < body.length; j++) {
      const c = body[j];
      if (c === "{") {
        d++;
        seg = "";
      } else if (c === "}") d--;
      else if (d === 0) {
        if (c === ";") {
          if (seg.trim()) decls.push(seg.trim());
          seg = "";
        } else seg += c;
      }
    }
    blocks.push(decls.filter((x) => !x.startsWith("--")).join(";"));
  }
  // 3 つの左バー（部品の markup と同じ class）+ 素の class「sidebar」の対照に、pdf.js の .sidebar と app.css を掛ける
  const doc = new (await import("jsdom")).JSDOM(
    `<main class="sbw"><nav class="ls-nav sidepane" id="lib"></nav></main><main><nav class="ls-nav sidepane" id="shelf"></nav></main>` +
      `<main class="sbw"><nav class="wk-sidebar sidepane" id="wiki"></nav></main><main><nav class="sidebar" id="ctl"></nav></main>`
  ).window;
  for (const t of [blocks.map((b) => `.sidebar{${b}}`).join("\n"), css]) {
    const st = doc.document.createElement("style");
    st.textContent = t;
    doc.document.head.append(st);
  }
  const cs = (id) => doc.getComputedStyle(doc.document.getElementById(id));
  // 外形の宣言は .sidepane の 1 か所だけ（LIBRARY / 本棚の .ls-nav と NOTE の .wk-sidebar の規則は外形を持たない = 上書きしない）
  const SHAPE = ["border", "border-right", "border-radius", "box-shadow", "background", "position"];
  const sidepane = /^\.sidepane \{([^}]*)\}/m.exec(noComments)?.[1] ?? "";
  assert.match(sidepane, /border: 1px solid var\(--border\)/, ".sidepane の枠が 1px solid var(--border) でない");
  assert.match(sidepane, /border-radius: 8px/);
  assert.match(sidepane, /box-shadow: var\(--side-shadow\)/);
  assert.match(sidepane, /background: var\(--panel\)/);
  assert.match(sidepane, /position: relative/);
  assert.match(noComments, /--side-shadow: 0 0\.25px 0\.75px rgb\(0 0 0 \/ 0\.05\), 0 2px 6px rgb\(0 0 0 \/ 0\.1\)/, "ライトの --side-shadow が無い");
  assert.match(noComments, /--side-shadow: 0 0\.25px 0\.75px rgb\(0 0 0 \/ 0\.2\), 0 2px 6px rgb\(0 0 0 \/ 0\.4\)/, "ダークの --side-shadow が無い");
  const wkRule = /\.wk-sidebar \{([^}]*)\}/.exec(read("lib/wiki/WikiSidebar.svelte"))?.[1] ?? "";
  for (const [name, body] of [[".ls-nav", /^\.ls-nav \{([^}]*)\}/m.exec(noComments)?.[1] ?? ""], [".wk-sidebar", wkRule]]) {
    assert.ok(body, `${name} の規則が見つからない`);
    for (const p of SHAPE) assert.doesNotMatch(body, new RegExp(`(^|[\\s;])${p}:`), `${name} が外形（${p}）を持つ = .sidepane とずれうる`);
  }
  // 左バーの右隣の列（NOTE の .wk-page）は通常の配置 = 左バー（relative）の影を覆わない（LIBRARY の .list・本棚の中央は元から static）。
  // 集中モードの札（.wk-focus-float）の基準は集中モードのときだけ relative
  const wp = read("lib/wiki/WikiPage.svelte");
  const wkPage = /\n  \.wk-page \{([^}]*)\}/.exec(wp)?.[1] ?? "";
  assert.ok(wkPage, ".wk-page の規則が見つからない");
  assert.doesNotMatch(wkPage, /position:/, ".wk-page が position を持つ = 後ろに描かれて NOTE の木の影を覆う");
  assert.match(wp, /:global\(main\.wk-main-focus\) > \.wk-page \{ position: relative; \}/, "集中モードの札の基準（.wk-page の relative）が無い");
  // jsdom の cascade（var() の略記は開かないので、角丸・影・位置で比べる）
  const shape = (id) => {
    const c = cs(id);
    return { radius: c.borderRadius, shadow: c.boxShadow, position: c.position };
  };
  assert.deepEqual(shape("lib"), { radius: "8px", shadow: "var(--side-shadow)", position: "relative" }, "LIBRARY の左バーの外形が OpenRef の値でない");
  for (const id of ["shelf", "wiki"]) assert.deepEqual(shape(id), shape("lib"), `${id} の外形が LIBRARY と違う`);
  for (const id of ["lib", "shelf", "wiki"]) {
    assert.equal(cs(id).width, "auto", `${id} に pdf.js の width が効く`);
    assert.ok(!/sidebar/.test(cs(id).boxShadow + cs(id).borderTopColor + cs(id).maxWidth), `${id} に pdf.js の .sidebar の値が効く`);
  }
  // 対照: pdf.js の .sidebar が改名・削除されたら止める（黙って飛ばさない = この切り離しと試験の前提を見直す）
  assert.ok(blocks.length, "pdf.js の .sidebar の規則が見つからない（pdf.js が変わった = 切り離しの前提を見直す）");
  assert.notEqual(cs("ctl").width, "auto", "試験の前提: 素の class「sidebar」に pdf.js の width が jsdom の cascade で乗らない");
  tests++;
}

// ---------- ⑩ セグメントで同じタブのモードを替えても、各モードの幅のまま（本棚も）・覚えた幅は書き換えない ----------
{
  const M = await import("./mode.svelte.ts");
  assert.ok(S.frontShelf(), "前提: ⑧ の本棚のタブが前面");
  const w = stored();
  assert.notEqual(w.lib, w.wiki, "前提: 2 つのモードの幅が違う");
  const tab = S.app.tab;
  const order = ["library", "wiki", "shelf", "library", "shelf", "wiki", "library"];
  const seen = [];
  for (const m of order) {
    await M.switchMode(m);
    await settle();
    await sleep(20);
    await settle();
    seen.push([m, M.frontMode(), shown(), !!grip()]);
  }
  const want = (m) => (m === "library" ? `${w.lib}px` : m === "wiki" ? `${w.wiki}px` : `${storedAll().shelf}px`);
  assert.deepEqual(seen, order.map((m) => [m, m, want(m), true]), "セグメントの切替でモードの幅が変わった");
  assert.equal(S.app.tab, tab, "同じタブのまま切り替わらない");
  assert.deepEqual(stored(), w, "切り替えただけで覚えた幅を書き換えた");
  tests++;
}

// ---------- ⑪ 本棚のつまみ・前後の版の ui_sidebar_w ----------
{
  const M = await import("./mode.svelte.ts");
  await M.switchMode("shelf");
  await settle();
  assert.ok(S.frontShelf() && grip(), "前提: 本棚のタブにつまみ");
  const before = stored();
  await drag([300]);
  assert.equal(shown(), "300px", "本棚のドラッグで幅が変わらない");
  assert.deepEqual([storedAll().shelf, stored()], [300, before], "本棚の幅を shelf の鍵に書かない / lib・wiki を変えた");
  // ← → は離したときに 1 回確定・Home = 既定
  key("ArrowRight");
  key("ArrowRight", {}, "keyup");
  await settle();
  assert.equal(storedAll().shelf, 316, "→ で 16px 広がらない");
  key("Home");
  await settle();
  assert.equal(storedAll().shelf, 230, "Home で既定に戻らない");
  await drag([10]);
  assert.equal(storedAll().shelf, 180, "本棚の下限が 180 でない");
  grip().dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  await settle();
  assert.equal(storedAll().shelf, 230, "ダブルクリックで既定に戻らない");
  assert.deepEqual([stored(), SW.sidebarW.lib, SW.sidebarW.wiki], [before, before.lib, before.wiki], "本棚のダブルクリックでほかのモードの幅を戻した");
  await sleep(450);
  // 前の版の読み書きの写し: 読みは lib / wiki だけ・書くのは {lib, wiki}
  const oldClamp = (w) => (Number.isFinite(w) ? Math.round(Math.max(180, Math.min(w, 1200))) : 230);
  const oldParse = (raw) => {
    const out = { lib: 230, wiki: 230 };
    let v;
    try {
      v = JSON.parse(raw);
    } catch {
      return out;
    }
    if (!v || typeof v !== "object" || Array.isArray(v)) return out;
    for (const m of ["lib", "wiki"]) if (typeof v[m] === "number" && Number.isFinite(v[m])) out[m] = oldClamp(v[m]);
    return out;
  };
  const oldWrite = (w) => JSON.stringify({ lib: w.lib, wiki: w.wiki });
  const newSaved = JSON.stringify({ lib: 333, wiki: 444, shelf: 255 });
  assert.deepEqual(oldParse(newSaved), { lib: 333, wiki: 444 }, "前の版が新しい値を読むと lib / wiki が変わる");
  const oldSaved = oldWrite(oldParse(newSaved));
  assert.equal(oldSaved, JSON.stringify({ lib: 333, wiki: 444 }), "前提: 前の版が書くと shelf が消える");
  assert.deepEqual(SW.parseSidebarW(oldSaved), { lib: 333, wiki: 444, shelf: 230 }, "前の版が書いた値（shelf なし）を新しい版が既定で読まない");
  assert.deepEqual(SW.parseSidebarW(newSaved), { lib: 333, wiki: 444, shelf: 255 });
  tests++;
}

inv.done();
console.log(`sidebar width: ok (${tests} tests)`);
process.exit(0);
