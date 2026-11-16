/* OpenNote の画像を大きく見る覆いの部品テスト（node app/src/lib/wiki/lightbox.test.mjs・実部品 lightbox.svelte.ts / WikiLightbox /
   ShadowPage + jsdom・invoke は差し替え）。確かめること:
   ① 埋め込みの画像（`![[画像]]`・`![alt](相対)`・data: の画像）を押すと覆い（App に 1 つ = 描画の外）・並び = ページの中の画像（文書順・
      リンクで包んだ画像は外す）・押した画像から・下に alt と「n / 全数」
   ② ← → で前後（端で止まる）・修飾キーの無いキーは背後へ流さない（`/` 等）・⌘ のキーは流す・IME の変換中は取らない
   ③ Esc で閉じて背後へ流さない・フォーカスを元へ・本文のスクロール位置に触らない・背景のクリックで閉じる（画像のクリック・ドラッグの後では閉じない）・タブの切替で閉じる
   ④ リンクで包んだ画像 = リンクが勝つ（覆いを出さない）・メディアの「外部で開く」（data-wk-open）は今どおり
   ⑤ ⌘クリック / 中クリック = その画像のファイルを新しいタブ（onNavigate({kind:"page", path}, true) = MediaView）・右ボタンの auxclick では何もしない・
      root の外（data:）は開かない・覆いの [新しいタブで開く] も同じ（data: には出さない）
   ⑥ 幅合わせ（画面に収めて中央）/ 等倍・ホイールでポインタを中心に拡大・ドラッグで移動・ダブルクリックで切替
   ⑦ 背後のページ（前面の OpenNote タブの path）が変わったら閉じる・Tab / ⇧Tab は覆いの中で巡る（背後へ抜けない）・
      ⌘W / ⌃Tab / ⌘⇧[ ] / ⌘⌥C は背後へ通す・背後を変える ⌘E / ⌘⇧N / ⌘F と ⌘⌥K（引用の窓）は止める・alt の無い data: の SVG（壊れた % の並び）でも開く
   出力 = `wiki lightbox: ok (N tests)` */
import assert from "node:assert/strict";
import { setupHarness } from "./test-harness.mjs";

const { calls, handlers, sleep, waitFor } = setupHarness();
let tests = 0;

const { mount, unmount, flushSync } = await import("svelte");
const S = await import("../state.svelte.ts");
const W = await import("./wiki.svelte.ts");
const L = await import("./lightbox.svelte.ts");
const { assetUrl } = await import("./render/asset.ts");
const { default: ShadowPage } = await import("./render/ShadowPage.svelte");
const { default: Lightbox } = await import("./WikiLightbox.svelte");

S.app.wikiOn = true;
W.wk.status = { enabled: true, root: "/v" };
handlers.get_ui_pref = () => null;
handlers.set_ui_pref = () => null;

const DATA = "data:image/png;base64,iVBORw0KGgo=";
const content = [
  "# 画像のページ",
  "",
  "![[a.png]]",
  "",
  "文 [![リンクの画像](c.png)](https://example.com/) 文",
  "",
  "![[b.png]]",
  "",
  "![説明つき](img/d.png)",
  "",
  `![データ](${DATA})`,
  "",
  "![[v.mp4]]",
].join("\n");
const links = [
  { kind: "embed", target: "a.png", path: "img/a.png", ambiguous: false },
  { kind: "embed", target: "b.png", path: "img/b.png", ambiguous: false },
  { kind: "embed", target: "v.mp4", path: "img/v.mp4", ambiguous: false },
];
const page = { path: "notes/p.md", abs_path: "/v/notes/p.md", kind: "md", title: "p", size: content.length, mtime: 0, content, links };
const navs = [];
const scroller = document.createElement("div");
document.body.append(scroller);
const host = document.createElement("div");
scroller.append(host);
const view = mount(ShadowPage, { target: host, props: { mode: "md", page, root: "/v", refs: [], onNavigate: (n, nt) => navs.push([n, !!nt]), readPage: async () => page } });
const lt = document.createElement("div");
document.body.append(lt);
const lb = mount(Lightbox, { target: lt });
flushSync();
const sr = () => host.querySelector(".wk-md")?.shadowRoot;
assert.ok(await waitFor(() => sr()?.querySelectorAll("img").length === 5), "画像を描かない");
const img = (alt) => [...sr().querySelectorAll("img")].find((i) => i.getAttribute("alt") === alt) ?? assert.fail(`画像が無い: ${alt}`);
const click = (el, o = {}) => {
  const ev = new MouseEvent(o.type ?? "click", { bubbles: true, cancelable: true, composed: true, button: 0, ...o });
  el.dispatchEvent(ev);
  flushSync();
  return ev;
};
const overlay = () => lt.querySelector(".wk-lb");
const key = (k, o = {}) => {
  const ev = new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...o });
  window.dispatchEvent(ev);
  flushSync();
  return ev;
};
const settle = async () => {
  for (let i = 0; i < 3; i++) {
    await sleep(0);
    flushSync();
  }
};

// ---------- ① 押すと覆い・並び・alt と n / 全数 ----------
{
  const before = document.createElement("button");
  document.body.append(before);
  before.focus();
  scroller.scrollTop = 0;
  const ev = click(img("b.png"));
  await settle();
  assert.ok(ev.defaultPrevented, "画像のクリックを止めない");
  assert.ok(L.lightbox.cur, "覆いを開かない");
  assert.ok(overlay() && !host.contains(overlay()), "覆いが描画の外に無い");
  assert.deepEqual(
    L.lightbox.cur.images.map((i) => [i.alt, i.nav?.path ?? null]),
    [["a.png", "img/a.png"], ["b.png", "img/b.png"], ["説明つき", "notes/img/d.png"], ["データ", null]],
    "並び（文書順・リンクの画像を外す・data: は開く先なし）"
  );
  assert.equal(L.lightbox.cur.images[1].src, assetUrl("/v/img/b.png"));
  assert.equal(L.lightbox.cur.index, 1, "押した画像から始めない");
  assert.match(overlay().querySelector(".wk-lb-n").textContent, /2 \/ 4/);
  assert.equal(overlay().querySelector(".wk-lb-alt").textContent, "b.png");
  assert.equal(overlay().querySelector(".wk-lb-img").getAttribute("src"), assetUrl("/v/img/b.png"));
  tests++;

  // ---------- ② ← →・キーを背後へ流さない ----------
  let behind = [];
  const bubble = (e) => behind.push(e.key);
  window.addEventListener("keydown", bubble);
  key("ArrowRight");
  assert.equal(L.lightbox.cur.index, 2);
  assert.equal(overlay().querySelector(".wk-lb-alt").textContent, "説明つき");
  key("ArrowRight");
  key("ArrowRight");
  assert.equal(L.lightbox.cur.index, 3, "端で止まらない");
  assert.match(overlay().querySelector(".wk-lb-n").textContent, /4 \/ 4/);
  key("ArrowLeft");
  assert.equal(L.lightbox.cur.index, 2);
  key("ArrowLeft", { isComposing: true });
  assert.equal(L.lightbox.cur.index, 2, "変換中の ← を取った");
  key("/");
  assert.deepEqual(behind, ["ArrowLeft"], "修飾キーの無いキーを背後へ流した（変換中の物だけ流す）");
  key("w", { metaKey: true });
  assert.deepEqual(behind, ["ArrowLeft", "w"], "⌘ のキーを流さない");
  // 覆いの前後のボタン
  overlay().querySelector(".wk-lb-prev").click();
  flushSync();
  assert.equal(L.lightbox.cur.index, 1);
  tests++;

  // ---------- ③ Esc・フォーカス・スクロール・背景のクリック・タブ ----------
  behind = [];
  key("Escape");
  await settle();
  window.removeEventListener("keydown", bubble);
  assert.equal(L.lightbox.cur, null, "Esc で閉じない");
  assert.equal(overlay(), null);
  assert.deepEqual(behind, [], "Esc が背後へ流れた");
  assert.equal(document.activeElement, before, "フォーカスを元へ戻さない");
  assert.equal(scroller.scrollTop, 0);
  before.remove();
  // 背景のクリック = 閉じる・画像のクリック = 閉じない
  click(img("a.png"));
  await settle();
  click(overlay().querySelector(".wk-lb-img"));
  assert.ok(L.lightbox.cur, "画像のクリックで閉じた");
  click(overlay().querySelector(".wk-lb-stage"));
  await settle();
  assert.equal(L.lightbox.cur, null, "背景のクリックで閉じない");
  // タブの切替
  click(img("a.png"));
  await settle();
  const tab0 = S.app.tab;
  S.app.tab = tab0 + 1000;
  await settle();
  assert.equal(L.lightbox.cur, null, "タブを切り替えても閉じない");
  S.app.tab = tab0;
  await settle();
  tests++;
}

// ---------- ④ リンクで包んだ画像 = リンクが勝つ・メディアの「外部で開く」は今どおり ----------
{
  navs.length = 0;
  click(img("リンクの画像"));
  await settle();
  assert.equal(L.lightbox.cur, null, "リンクで包んだ画像で覆いを出した");
  assert.deepEqual(navs, [[{ kind: "external", url: "https://example.com/" }, false]], "リンクが勝たない");
  navs.length = 0;
  const open = sr().querySelector("button[data-wk-open]");
  assert.ok(open, "動画の「外部で開く」が無い");
  click(open);
  await settle();
  assert.deepEqual(navs, [[{ kind: "open", abs: "/v/img/v.mp4" }, false]], "「外部で開く」が変わった");
  assert.equal(L.lightbox.cur, null);
  tests++;
}

// ---------- ⑤ ⌘ / 中クリック = 新しいタブ・右ボタン・data:・覆いの [新しいタブで開く] ----------
{
  navs.length = 0;
  click(img("a.png"), { metaKey: true });
  click(img("説明つき"), { type: "auxclick", button: 1 });
  click(img("b.png"), { type: "auxclick", button: 2 });
  click(img("データ"), { metaKey: true });
  await settle();
  assert.equal(L.lightbox.cur, null, "⌘ / 中クリックで覆いを出した");
  assert.deepEqual(navs, [
    [{ kind: "page", path: "img/a.png" }, true],
    [{ kind: "page", path: "notes/img/d.png" }, true],
  ], "⌘ / 中クリックで画像を新しいタブで開かない（右ボタン・data: は何もしない）");
  navs.length = 0;
  click(img("説明つき"));
  await settle();
  const btn = () => [...overlay().querySelectorAll(".wk-lb-bar button")].find((b) => b.textContent === "新しいタブで開く");
  btn().click();
  await settle();
  assert.deepEqual(navs, [[{ kind: "page", path: "notes/img/d.png" }, true]], "[新しいタブで開く] が MediaView のタブを開かない");
  assert.equal(L.lightbox.cur, null, "[新しいタブで開く] で閉じない");
  click(img("データ"));
  await settle();
  assert.equal(btn(), undefined, "data: の画像に [新しいタブで開く] を出した");
  L.closeLightbox();
  await settle();
  tests++;
}

// ---------- ⑥ 幅合わせ / 等倍・ホイール・ドラッグ・ダブルクリック ----------
{
  click(img("a.png"));
  await settle();
  const stage = overlay().querySelector(".wk-lb-stage");
  stage.getBoundingClientRect = () => ({ left: 0, top: 0, right: 800, bottom: 600, width: 800, height: 600, x: 0, y: 0 });
  const im = overlay().querySelector(".wk-lb-img");
  Object.defineProperty(im, "naturalWidth", { value: 1600 });
  Object.defineProperty(im, "naturalHeight", { value: 400 });
  im.dispatchEvent(new Event("load"));
  flushSync();
  const tf = () => overlay().querySelector(".wk-lb-img").style.transform;
  const btn = (label) => [...overlay().querySelectorAll(".wk-lb-bar button")].find((b) => b.textContent === label);
  assert.equal(tf(), "translate(0px, 200px) scale(0.5)", "幅合わせ = 画面に収めて中央");
  assert.equal(btn("幅合わせ").getAttribute("aria-pressed"), "true");
  btn("等倍").click();
  flushSync();
  assert.equal(tf(), "translate(-400px, 100px) scale(1)", "等倍 = 1 倍で中央");
  // ホイール（上 = 拡大）: ポインタの位置（400, 300）を動かさない
  const we = new WheelEvent("wheel", { deltaY: -100, clientX: 400, clientY: 300, bubbles: true, cancelable: true });
  stage.dispatchEvent(we);
  flushSync();
  assert.ok(we.defaultPrevented, "ホイールを止めない（背後の箱が動く）");
  const m = /translate\(([-\d.]+)px, ([-\d.]+)px\) scale\(([\d.]+)\)/.exec(tf());
  const [x, y, s] = m.slice(1).map(Number);
  assert.ok(s > 1.1 && s < 1.2, `拡大しない（${s}）`);
  assert.ok(Math.abs((400 - x) / s - 800) < 1e-6 && Math.abs((300 - y) / s - 200) < 1e-6, "ポインタの位置を中心に拡大しない");
  assert.equal(btn("幅合わせ").getAttribute("aria-pressed"), "false");
  assert.equal(btn("等倍").getAttribute("aria-pressed"), "false");
  // ドラッグ = 移動・その後の click で閉じない
  stage.dispatchEvent(new PointerEvent("pointerdown", { button: 0, clientX: 100, clientY: 100, bubbles: true }));
  stage.dispatchEvent(new PointerEvent("pointermove", { clientX: 150, clientY: 130, bubbles: true }));
  stage.dispatchEvent(new PointerEvent("pointerup", { clientX: 150, clientY: 130, bubbles: true }));
  flushSync();
  const m2 = /translate\(([-\d.]+)px, ([-\d.]+)px\)/.exec(tf());
  assert.ok(Math.abs(Number(m2[1]) - (x + 50)) < 1e-6 && Math.abs(Number(m2[2]) - (y + 30)) < 1e-6, "ドラッグで動かない");
  click(stage);
  assert.ok(L.lightbox.cur, "ドラッグの後のクリックで閉じた");
  // ダブルクリック = 幅合わせ ⇄ 等倍
  stage.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  flushSync();
  assert.equal(tf(), "translate(0px, 200px) scale(0.5)", "ダブルクリックで幅合わせに戻らない");
  stage.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  flushSync();
  assert.equal(tf(), "translate(-400px, 100px) scale(1)", "ダブルクリックで等倍にならない");
  // 次の画像 = 幅合わせから
  key("ArrowRight");
  await settle();
  assert.equal(overlay().querySelector(".wk-lb-img").style.transform, "", "次の画像が前の拡大のまま");
  assert.equal(btn("幅合わせ").getAttribute("aria-pressed"), "true");
  L.closeLightbox();
  await settle();
  tests++;
}

// ---------- ⑦ 背後のページの変化・フォーカスを閉じ込める・通すキー・data: の SVG ----------
{
  const tab = S.app.tabs[0];
  S.app.tab = tab.id;
  tab.wiki = { path: "notes/p.md", root: "/v", scrollTop: 0, editing: false };
  await settle();
  click(img("a.png"));
  await settle();
  assert.ok(L.lightbox.cur);
  tab.wiki.path = "notes/q.md"; // ⌘[ ⌘] 等で背後のページが変わった
  await settle();
  assert.equal(L.lightbox.cur, null, "背後のページが変わっても覆いが残る");
  tab.wiki.path = "notes/p.md";
  await settle();
  // Tab / ⇧Tab = 覆いの中を巡る
  click(img("a.png"));
  await settle();
  const buttons = [...overlay().querySelectorAll("button:not(:disabled)")];
  assert.ok(buttons.length >= 4);
  let behind = [];
  const bubble = (e) => behind.push(`${e.metaKey ? "⌘" : ""}${e.ctrlKey ? "⌃" : ""}${e.altKey ? "⌥" : ""}${e.shiftKey ? "⇧" : ""}${e.key}`);
  window.addEventListener("keydown", bubble);
  buttons.at(-1).focus();
  let ev = key("Tab");
  assert.ok(ev.defaultPrevented, "Tab を止めない（背後へ抜ける）");
  assert.equal(document.activeElement, buttons[0], "Tab で最後から最初へ巡らない");
  key("Tab", { shiftKey: true });
  assert.equal(document.activeElement, buttons.at(-1), "⇧Tab で最初から最後へ巡らない");
  overlay().focus();
  key("Tab");
  assert.equal(document.activeElement, buttons[0], "覆いそのものから Tab で最初のボタンへ行かない");
  assert.deepEqual(behind, [], "Tab が背後へ流れた");
  // ⌘W・⌃Tab・⌘⇧[ ]・⌘⌥C は通す（覆いは開いたまま = タブが変われば別の門で閉じる）
  for (const [k, o] of [["w", { metaKey: true }], ["Tab", { ctrlKey: true }], ["[", { metaKey: true, shiftKey: true }], ["]", { metaKey: true, shiftKey: true }], ["c", { metaKey: true, altKey: true }]]) {
    ev = key(k, o);
    assert.ok(!ev.defaultPrevented, `通すキーを止めた: ${k}`);
  }
  // 背後を変える ⌘E（表示 ⇄ 編集）・⌘⇧N（新しい文献）・⌘F（ページ内を検索）は止める
  for (const [k, o] of [["e", { metaKey: true }], ["N", { metaKey: true, shiftKey: true }], ["f", { metaKey: true }], ["˚", { metaKey: true, altKey: true, code: "KeyK" }]]) {
    ev = key(k, o);
    assert.ok(ev.defaultPrevented, `背後を変えるキーを止めない: ${k}`);
  }
  window.removeEventListener("keydown", bubble);
  assert.deepEqual(behind, ["⌘w", "⌃Tab", "⌘⇧[", "⌘⇧]", "⌘⌥c"], "⌘ / ⌃ のキーを背後へ通さない / 背後を変えるキーを通した");
  assert.ok(L.lightbox.cur, "止めたキーで覆いが閉じた");
  L.closeLightbox();
  await settle();
  tests++;
}
{
  // alt の無い data: の SVG（生の % を含む = decodeURIComponent が投げる）でも開く
  const svg = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='100%' height='10'>";
  const bad = "data:image/svg+xml;x=%zz,<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'>"; // 頭の部分にも壊れた %
  const p2 = { ...page, path: "notes/s.md", content: `<img src="${svg}">\n\n<img src="${bad}">\n\n![[a.png]]\n` };
  const h2 = document.createElement("div");
  document.body.append(h2);
  const v2 = mount(ShadowPage, { target: h2, props: { mode: "md", page: p2, root: "/v", refs: [], onNavigate: () => {}, readPage: async () => p2 } });
  flushSync();
  const sr2 = () => h2.querySelector(".wk-md")?.shadowRoot;
  assert.ok(await waitFor(() => sr2()?.querySelectorAll("img").length === 3), "SVG の画像を描かない");
  click(sr2().querySelector("img"));
  await settle();
  assert.ok(overlay(), "alt の無い data: の SVG で覆いが出ない");
  assert.match(overlay().querySelector(".wk-lb-alt").textContent, /^data:image\/svg\+xml/, "名前の行が違う");
  key("ArrowRight");
  await settle();
  assert.equal(overlay()?.querySelector(".wk-lb-alt").textContent, "data:image/svg+xml;x=%zz", "壊れた % の名前で覆いが崩れた");
  L.closeLightbox();
  await settle();
  unmount(v2);
  h2.remove();
  tests++;
}

unmount(lb);
unmount(view);
assert.ok(!calls.some((c) => c.name === "set_ui_pref"), "覆いが設定に書いた");
console.log(`wiki lightbox: ok (${tests} tests)`);
process.exit(0);
