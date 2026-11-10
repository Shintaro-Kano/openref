/* PDF ビューアの右バー（検索 / 注釈 / 目次 / ページ）の開閉の試験（node app/src/lib/viewer/side.test.mjs・実部品 PdfViewer.svelte + jsdom）。
   pdf.js は偽物（2 ページ・テキスト層なし・描画しない）= 見るのは右バーの開閉の配線だけ。確かめること:
   ① 既定（ui_viewer_side が無い）= 開いている・閉じた状態から mount すると最初の描画から右バーが無い（出て消えない）
   ② ツールバーの右端のボタン = 開閉して覚える（ui_viewer_side "0" / "1"・絵と title / aria-label・aria-pressed）
   ③ 閉じた状態で ⌘F / ツールバーの ⌕ = 右バーが開いて検索欄に focus（右バーの中が目的地）。開いている間の ⌘F / ⌕ は従来どおり（検索 ⇄ 注釈）
   ④ 隠れている（前面でない）ビューアは ⌘F で右バーを開かない
   ⑤ 右バーの中に focus があるまま閉じる = 本文（.pages）へ移す（body へ落とさない）・閉じている間のタブの記憶
   ⑥ 注釈カードを編集中に閉じる = 見えない編集は Esc を吸わない（開いていれば今どおり Esc は編集の取消）・開き直せば編集の続き
   ⌘⌥I（App の keydown → togglePane）は app.test の PDF のタブの節。土台は wiki/test-harness.mjs */
import assert from "node:assert/strict";
import { setupHarness } from "../wiki/test-harness.mjs";

const { handlers, sleep, waitFor } = setupHarness({
  fakes: {
    "pdfjs-dist": "export const GlobalWorkerOptions = {}; export const getDocument = (o) => globalThis.__pdf(o); export class TextLayer {} export class AnnotationLayer {}",
    "pdfjs-dist/build/pdf.worker.mjs?url": "export default 'worker.js';",
    "pdfjs-dist/web/images/annotation-note.svg?url": "export default 'note.svg';",
    "pdfjs-dist/web/pdf_viewer.css": "export default '';",
  },
});
globalThis.__BUILD_ID__ = "test"; // vite の define
globalThis.ResizeObserver = class {
  observe() {}
  disconnect() {}
};
globalThis.IntersectionObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
const prefs = {};
handlers.get_ui_pref = (a) => prefs[a.key] ?? null;
handlers.set_ui_pref = (a) => {
  prefs[a.key] = a.value;
  return null;
};
// 注釈 1 件（⑥ の注釈カードの編集）
handlers.list_annotations = () => [[{ key: "A1", author: "user", kind: "highlight", color: "yellow", comment: "c", page: 1, quote: "引用の文", prefix: null, suffix: null, dateAdded: "2026-01-01" }], []];
handlers.attachment_data = () => new ArrayBuffer(4);
// 2 ページの文書（寸法だけ・テキスト層なし・注釈なし）
const page = {
  rotate: 0,
  getViewport: ({ scale }) => ({ width: 600 * scale, height: 800 * scale, clone() { return this; } }),
  getAnnotations: async () => [],
  streamTextContent: () => new ReadableStream({ start(c) { c.enqueue({ items: [], styles: {} }); c.close(); } }),
  getTextContent: async () => ({ items: [], styles: {} }),
  cleanup() {},
};
globalThis.__pdf = () => ({ promise: Promise.resolve({ numPages: 2, getPage: async () => page, getOutline: async () => null, destroy: async () => {} }), destroy: async () => {} });

const { mount, unmount, flushSync } = await import("svelte");
const S = await import("../state.svelte.ts");
const { default: PdfViewer } = await import("../PdfViewer.svelte");
const $ = (q) => document.querySelector(q);
const side = () => $(".viewer-overlay .v-side");
const pane = () => $(".viewer-overlay .tb-pane");
const activeSideTab = () => [...document.querySelectorAll(".viewer-overlay .side-tabs button.active")].map((b) => b.textContent.trim().split(/\s+/)[0]); // 件数を除いたタブ名
const cmdF = () => window.dispatchEvent(new KeyboardEvent("keydown", { metaKey: true, key: "f", bubbles: true, cancelable: true }));
const settle = async () => {
  flushSync();
  await sleep(10);
  flushSync();
};
let tests = 0;

let closed = 0; // onClose（タブを閉じる）の呼ばれた回数
async function open(front = true) {
  document.body.replaceChildren();
  const v = mount(PdfViewer, { target: document.body, props: { viewer: { itemKey: "K1", attachmentId: 1, title: "T" }, front, onClose: () => closed++ } });
  flushSync();
  return v;
}

// ① 既定 = 開いている（今と同じ見た目）・閉じた状態から mount = 最初の描画から右バーが無い
{
  assert.equal(S.app.viewerSideOpen, true, "既定（ui_viewer_side なし）で右バーが閉じている");
  let v = await open();
  assert.ok(side(), "既定で右バーが出ない");
  unmount(v);
  prefs.ui_viewer_side = "0";
  await S.loadViewerSidePref(); // main.ts が mount の前に待つ読み
  v = await open();
  assert.equal(side(), null, "閉じた状態なのに最初の描画で右バーが出た");
  assert.ok(await waitFor(() => pane()), "ツールバーに右バーの開閉ボタンが無い");
  await settle();
  assert.equal(side(), null, "閉じた状態なのに読込の後で右バーが出た");
  assert.deepEqual([pane().getAttribute("aria-pressed"), pane().title, pane().getAttribute("aria-label")], ["false", "右のペインを表示（⌘⌥I）", "右のペインを表示（⌘⌥I）"], "閉じた絵でない");
  unmount(v);
  delete prefs.ui_viewer_side;
  await S.loadViewerSidePref();
  assert.equal(S.app.viewerSideOpen, true);
  tests++;
}

// ② ツールバーの右端のボタン = 開閉して覚える
{
  const v = await open();
  assert.ok(await waitFor(() => pane()), "ツールバーに右バーの開閉ボタンが無い");
  assert.equal($(".v-toolbar > button:last-child"), pane(), "開閉ボタンがツールバーの右端でない");
  assert.deepEqual([pane().getAttribute("aria-pressed"), pane().title, pane().getAttribute("aria-label"), pane().classList.contains("tb-pane-off")], ["true", "右のペインを隠す（⌘⌥I）", "右のペインを隠す（⌘⌥I）", false], "開いた絵でない");
  pane().click();
  await settle();
  assert.deepEqual([side(), S.app.viewerSideOpen, prefs.ui_viewer_side], [null, false, "0"], "ボタンで閉じない / 覚えない");
  assert.deepEqual([pane().getAttribute("aria-pressed"), pane().title, pane().classList.contains("tb-pane-off")], ["false", "右のペインを表示（⌘⌥I）", true], "閉じた絵にならない");
  pane().click();
  await settle();
  assert.deepEqual([!!side(), S.app.viewerSideOpen, prefs.ui_viewer_side], [true, true, "1"], "ボタンで開き直さない / 覚えない");
  assert.deepEqual(activeSideTab(), ["注釈"], "開き直したら右バーのタブが変わった");
  unmount(v);
  tests++;
}

// ③ 閉じた状態で ⌘F / ⌕ = 開いて検索欄に focus・開いている間は従来どおり（検索 ⇄ 注釈）
{
  const v = await open();
  assert.ok(await waitFor(() => pane()));
  pane().click(); // 閉じる
  await settle();
  cmdF();
  await settle();
  assert.ok(side(), "閉じた状態で ⌘F を押しても右バーが開かない");
  assert.deepEqual([S.app.viewerSideOpen, prefs.ui_viewer_side], [true, "1"], "⌘F で開いたことを覚えない");
  assert.ok(activeSideTab()[0]?.includes("検索"), "⌘F で検索のタブにならない");
  assert.equal(document.activeElement, $(".v-side .sr-input"), "⌘F で開いても検索欄に focus が入らない");
  cmdF(); // 開いていて検索のタブ = 従来どおり注釈へ戻す（右バーは開いたまま）
  await settle();
  assert.deepEqual([!!side(), activeSideTab()], [true, ["注釈"]], "開いている間の ⌘F が従来どおり検索 → 注釈に戻さない");
  // 検索のタブのまま閉じる → ⌕ = 開いて検索欄に focus（見えていない検索を「閉じる」にしない）
  cmdF();
  await settle();
  pane().click();
  await settle();
  assert.equal(side(), null);
  $(".tb-search").click();
  await settle();
  assert.ok(side(), "閉じた状態で ⌕ を押しても右バーが開かない");
  assert.ok(activeSideTab()[0]?.includes("検索"), "⌕ で検索のタブにならない");
  assert.equal(document.activeElement, $(".v-side .sr-input"), "⌕ で開いても検索欄に focus が入らない");
  assert.ok($(".tb-search").classList.contains("active"), "検索が見えているのに ⌕ が押された絵でない");
  $(".tb-search").click(); // 開いていて検索のタブ = 従来どおり注釈へ
  await settle();
  assert.deepEqual([!!side(), activeSideTab(), $(".tb-search").classList.contains("active")], [true, ["注釈"], false], "開いている間の ⌕ が従来どおりでない");
  // 閉じている間は検索のタブでも ⌕ を押された絵にしない
  cmdF();
  await settle();
  pane().click();
  await settle();
  assert.equal($(".tb-search").classList.contains("active"), false, "右バーを閉じているのに ⌕ が押された絵");
  pane().click();
  await settle();
  unmount(v);
  tests++;
}

// ④ 隠れている（前面でない）ビューアは ⌘F で右バーを開かない（キーは前面のビューアのもの）
{
  const v = await open(false);
  await sleep(30);
  S.app.viewerSideOpen = false;
  await settle();
  cmdF();
  await settle();
  assert.deepEqual([side(), S.app.viewerSideOpen], [null, false], "隠れているビューアが ⌘F で右バーを開いた");
  S.app.viewerSideOpen = true;
  unmount(v);
  tests++;
}

// ⑤ 右バーの中に focus があるまま閉じる（ボタン・⌘⌥I = state の toggleViewerSide）= 本文（.pages）へ移す・閉じている間もタブの選択を覚える
{
  const v = await open();
  assert.ok(await waitFor(() => pane()));
  cmdF();
  await settle();
  assert.equal(document.activeElement, $(".v-side .sr-input"));
  pane().click();
  await settle();
  assert.equal(document.activeElement, $(".viewer-overlay .pages"), "右バーの中に focus があるまま閉じたら本文へ移らない（body へ落ちた）");
  S.toggleViewerSide(); // 開く（⌘⌥I と同じ経路）
  await settle();
  cmdF(); // 開いていて検索 → 注釈へ（focus は検索欄に残らない = 次で入れ直す）
  await settle();
  cmdF();
  await settle();
  assert.equal(document.activeElement, $(".v-side .sr-input"));
  S.toggleViewerSide(); // 閉じる（⌘⌥I と同じ経路）
  await settle();
  assert.equal(document.activeElement, $(".viewer-overlay .pages"), "⌘⌥I で閉じたら本文へ移らない");
  // 右バーの外に focus があるなら動かさない
  S.toggleViewerSide();
  await settle();
  const outside = document.createElement("input");
  document.body.append(outside);
  outside.focus();
  S.toggleViewerSide();
  await settle();
  assert.equal(document.activeElement, outside, "右バーの外の focus を本文へ動かした");
  outside.remove();
  // タブの記憶: [目次] を選んで閉じ、開き直すと [目次] のまま
  S.toggleViewerSide();
  await settle();
  [...document.querySelectorAll(".v-side .side-tabs button")].find((b) => b.textContent.trim() === "目次").click();
  await settle();
  pane().click();
  await settle();
  pane().click();
  await settle();
  assert.deepEqual(activeSideTab(), ["目次"], "閉じて開き直すと右バーのタブの選択を忘れた");
  unmount(v);
  tests++;
}

// ⑥ 注釈カードを編集中に閉じる: 開き直せば編集の続き・閉じている間の Esc は見えない編集を取り消さず、ビューアの Esc（タブを閉じる）になる・
// 開いている間の Esc は今どおり編集の取消（タブは閉じない）
{
  const v = await open();
  assert.ok(await waitFor(() => $(".v-side .ann button.edit")), "注釈カードが出ない");
  $(".v-side .ann button.edit").click(); // ✎
  await settle();
  assert.ok($(".v-side .ann.editing"), "✎ で編集にならない");
  pane().click();
  await settle();
  pane().click();
  await settle();
  assert.ok($(".v-side .ann.editing"), "閉じて開き直したら編集が解けた");
  pane().click();
  await settle();
  closed = 0;
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  await settle();
  assert.equal(closed, 1, "右バーを閉じている間の Esc を見えない編集の取消が吸った（ビューアの Esc にならない）");
  pane().click();
  await settle();
  assert.ok($(".v-side .ann.editing"), "閉じている間の Esc で見えない編集を取り消した");
  closed = 0;
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  await settle();
  assert.deepEqual([closed, !!$(".v-side .ann.editing")], [0, false], "開いている間の Esc が今どおり編集の取消にならない");
  unmount(v);
  tests++;
}

console.log(`pdf viewer side: ok (${tests} tests)`);
process.exit(0);
