/* PDF ビューアのヘッダーの段の試験（node app/src/lib/viewer/head.test.mjs・実部品 PdfViewer.svelte / ItemDetail.svelte + jsdom）。
   pdf.js は偽物（描画しない）= 見るのは段の出し分けと、診断の置き場所。確かめること:
   ① 読込中 = 題名と × の段がある（× で閉じる）・ツールバーはまだ無い
   ② 読めた後 = 段が無い・診断（テキスト層なし）はツールバー・頁数 / テキストの件数 / ビルド指紋は頁の欄の title
   ③ テキスト層がある = 警告は出ない（title にテキストの件数）・テキスト層の途中のエラー = ツールバーに出る
   ④ 読込に失敗（頁数を読めない）= 題名と × の段が残り、エラーは本文の上
   ⑤ 頁数を読めた後に落ちた（途中のエラー）= 段は出さず、エラーはツールバー（前は画面のどこにも出なかった）
   ⑥ Esc = タブを閉じる（読込中・読めた後とも）
   ⑦ 詳細ペインの「この添付を読む」は 2 本目以降の添付だけ（主 PDF は「PDF を読む」）
   ⑧ 大きい PDF: 段とツールバーが同時に出る間が無い（読み終えた瞬間にツールバーが跳ねない）・読込中は段がある・最初の合わせは段が外れた後
   ⑨ 読めた瞬間に閉じた = 見張りを始めない（tick の後の disposed の確かめは、外しても後の関数がどれも container / doc / disposed を
      見て止まるので jsdom では差が出ない。ここは閉じた後に何も作らない事だけを見る）
   ツールバーが診断で 2 行に折れない（レイアウト）は jsdom では測れない（ここでは見ない）
   土台は wiki/test-harness.mjs */
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
globalThis.__BUILD_ID__ = "test0·1234"; // vite の define
let roMade = 0; // ResizeObserver を作った数（unmount の後に作らないことを見る）
globalThis.ResizeObserver = class {
  constructor() { roMade++; }
  observe() {}
  disconnect() {}
};
// .pages の高さを読んだとき、段がまだ DOM にあったか（最初の合わせは段が外れた後の tick）
const heightReads = [];
Object.defineProperty(Element.prototype, "clientHeight", { configurable: true, get() {
  if (this.classList?.contains("pages")) heightReads.push(!!document.querySelector(".v-head"));
  return 0;
} });
globalThis.IntersectionObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
handlers.get_ui_pref = () => null;
handlers.set_ui_pref = () => null;
handlers.list_annotations = () => [[], []];

// 場面ごとの偽物: data = attachment_data の答え（Promise・投げる関数）・text = 1 ページの文字の数・textThrows・page2Throws
let sc = {};
const item = (s) => ({ str: s, dir: "ltr", width: 10, height: 10, transform: [10, 0, 0, 10, 72, 700], fontName: "f", hasEOL: false });
const mkPage = (n) => ({
  rotate: 0,
  getViewport: ({ scale }) => ({ width: 600 * scale, height: 800 * scale, clone() { return this; } }),
  getAnnotations: async () => [],
  streamTextContent: () => {
    if (sc.textThrows || sc.textThrowsOn === n) throw new Error("bad text stream");
    return new ReadableStream({ start(c) { c.enqueue({ items: Array.from({ length: sc.text ?? 0 }, (_, i) => item("w" + i)), styles: {} }); c.close(); } });
  },
  getTextContent: async () => ({ items: [], styles: {} }),
  cleanup() {},
});
globalThis.__pdf = () => ({
  promise: Promise.resolve({
    numPages: sc.pages ?? 2,
    getPage: async (n) => {
      if (sc.slow) await sleep(sc.slow); // 寸法を読む間（大きい PDF の代わり）
      if (n === 2 && sc.page2Throws) throw new Error("Invalid page request.");
      return mkPage(n);
    },
    getOutline: async () => null,
    destroy: async () => {},
  }),
  destroy: async () => {},
});
handlers.attachment_data = () => (sc.data ? sc.data() : new ArrayBuffer(4));

const { mount, unmount, flushSync } = await import("svelte");
const S = await import("../state.svelte.ts");
const { default: PdfViewer } = await import("../PdfViewer.svelte");
const { default: ItemDetail } = await import("../ItemDetail.svelte");
const $ = (q) => document.querySelector(q);
const settle = async () => {
  flushSync();
  await sleep(20);
  flushSync();
};
let tests = 0;
let closed = 0;
async function open(scene) {
  sc = scene;
  document.body.replaceChildren();
  const v = mount(PdfViewer, { target: document.body, props: { viewer: { itemKey: "K1", attachmentId: 1, title: "架空の論文の題名" }, front: true, onClose: () => closed++ } });
  flushSync();
  return v;
}
const esc = () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
const loadedNow = () => !$(".pages-overlay.dim");

// ① 読込中 = 題名と × の段・ツールバーはまだ無い・× と Esc で閉じる
{
  let release;
  const v = await open({ data: () => new Promise((r) => (release = r)) });
  await settle();
  assert.ok($(".v-head"), "読込中に段が無い");
  assert.deepEqual([$(".v-head b").textContent, $(".v-head .build-id").textContent, !!$(".v-head .x"), $(".v-toolbar")], ["架空の論文の題名", "b test0·1234", true, null], "読込中の段の中身（題名・ビルド指紋・×）");
  assert.ok($(".pages-overlay.dim"), "読込中の表示が無い");
  closed = 0;
  $(".v-head .x").click();
  esc();
  assert.equal(closed, 2, "読込中に × / Esc で閉じない");
  release(new ArrayBuffer(4));
  await settle();
  unmount(v);
  tests++;
}

// ② 読めた後（テキスト層なし）= 段が無い・診断はツールバー・頁の欄の title・Esc で閉じる
{
  const v = await open({});
  assert.ok(await waitFor(() => $(".v-toolbar") && loadedNow()), "読めない");
  await settle();
  assert.equal($(".v-head"), null, "読めた後に段が残る");
  assert.equal($(".tb-diag")?.textContent, "⚠ テキスト層なし", "テキスト層なしがツールバーに出ない");
  assert.equal($(".tb-page").title, "2 ページ · b test0·1234", "頁の欄の title（頁数・ビルド指紋）");
  assert.equal($(".v-toolbar .tb-diag").parentElement.previousElementSibling, $(".tb-page"), "診断が頁の欄の隣（余白の枠）に無い");
  assert.equal($(".tb-diag").parentElement.nextElementSibling, $(".tb-search"), "診断の枠の後ろが検索のボタンでない");
  closed = 0;
  esc();
  assert.equal(closed, 1, "読めた後の Esc で閉じない");
  unmount(v);
  tests++;
}

// ③ テキスト層あり = 警告なし・title にテキストの件数 / テキスト層の途中のエラー = ツールバー
{
  let v = await open({ text: 3 });
  assert.ok(await waitFor(() => $(".v-toolbar") && loadedNow() && /text 6/.test($(".tb-page").title)), "テキストの件数が title に入らない: " + $(".tb-page")?.title);
  await settle();
  assert.deepEqual([$(".v-head"), $(".tb-diag")], [null, null], "テキスト層があるのに段 / 警告が出た");
  assert.ok($(".tb-diagslot") && $(".tb-diagslot").nextElementSibling === $(".tb-search"), "診断が無いときも余白の枠がある（右のボタンの位置が診断の有無で変わらない）");
  assert.equal($(".tb-page").title, "2 ページ · text 6 · b test0·1234");
  unmount(v);
  v = await open({ textThrows: true });
  assert.ok(await waitFor(() => $(".tb-diag") && /prefetch/.test($(".tb-diag").textContent)), "テキスト層の途中のエラーがツールバーに出ない");
  assert.match($(".tb-diag").textContent, /^⚠ テキスト層なし: p\.1 prefetch: Error: bad text stream$/);
  assert.equal($(".tb-diag").title, $(".tb-diag").textContent, "長い診断の全文が title に無い");
  assert.equal($(".v-head"), null);
  unmount(v);
  // テキスト層はあるが 2 ページ目で落ちた = 「テキスト層なし」ではなく、途中のエラーだけを出す
  v = await open({ text: 3, textThrowsOn: 2 });
  assert.ok(await waitFor(() => $(".tb-diag")), "テキスト層がある文書の途中のエラーがツールバーに出ない");
  assert.equal($(".tb-diag").textContent, "⚠ p.2 prefetch: Error: bad text stream");
  assert.equal($(".tb-page").title, "2 ページ · text 3 · b test0·1234");
  unmount(v);
  tests++;
}

// ④ 読込に失敗（頁数を読めない）= 題名と × の段が残り、エラーは本文の上・ツールバーは無い
{
  const v = await open({ data: () => { throw new Error("harness: attachment_data の失敗"); } });
  assert.ok(await waitFor(() => $(".pages-overlay.err")), "失敗の表示が無い");
  await settle();
  assert.deepEqual([!!$(".v-head"), $(".v-head b").textContent, $(".v-head .build-id").textContent, !!$(".v-head .x"), $(".v-toolbar")], [true, "架空の論文の題名", "b test0·1234", true, null], "失敗のときの段（指紋つき）");
  assert.match($(".pages-overlay.err").textContent, /attachment_data の失敗/);
  closed = 0;
  esc();
  assert.equal(closed, 1, "失敗の後の Esc で閉じない");
  unmount(v);
  tests++;
}

// ⑤ 頁数を読めた後に落ちた（途中のエラー）= 段は出さず、エラーはツールバー
{
  const v = await open({ page2Throws: true });
  assert.ok(await waitFor(() => $(".tb-diag") && /Invalid page request/.test($(".tb-diag").textContent)), "途中のエラーがツールバーに出ない: " + $(".tb-diag")?.textContent);
  await settle();
  assert.equal($(".v-head"), null, "途中のエラーで段が出た");
  assert.equal($(".pages-overlay.err"), null, "途中のエラーが本文の上にも出た（前と同じく頁を読めたときは出さない）");
  assert.match($(".tb-diag").textContent, /^⚠ Error: Invalid page request\. · ⚠ テキスト層なし$/); // どの診断も ⚠ で始まる（切れても印は残る）
  unmount(v);
  tests++;
}

// ⑧ 大きい PDF（寸法を読む間が長い）: 段とツールバーが同時に出る間が無い・読込中は段（題名と ×）がある・
//    最初の合わせ（.pages の高さを読む）は段が外れた後
{
  const states = [];
  const mo = new MutationObserver(() => states.push([!!$(".v-head"), !!$(".v-toolbar"), !!$(".pages-overlay.dim")]));
  heightReads.length = 0;
  const v = await open({ pages: 60, slow: 2 });
  mo.observe(document.body, { subtree: true, childList: true });
  assert.ok(await waitFor(() => $(".v-toolbar") && loadedNow(), 5000), "読めない");
  await settle();
  mo.disconnect();
  assert.ok(states.length > 0);
  assert.deepEqual(states.filter(([h, tb]) => h && tb), [], "段とツールバーが同時に出た");
  assert.deepEqual(states.filter(([h, , ld]) => ld && !h), [], "読込中なのに段（題名と ×）が無い");
  assert.deepEqual(states.at(-1), [false, true, false], "読めた後の形でない");
  assert.ok(heightReads.length > 0 && heightReads[0] === false, "最初の合わせのとき段がまだあった: " + JSON.stringify(heightReads.slice(0, 3)));
  unmount(v);
  tests++;
}

// ⑨ 読めた瞬間（段が外れる間）に閉じた = 合わせ・見張り（ResizeObserver）を始めない
{
  let v;
  const made0 = roMade;
  const mo = new MutationObserver(() => {
    if (v && !$(".pages-overlay.dim")) { mo.disconnect(); unmount(v); v = null; }
  });
  v = await open({});
  mo.observe(document.body, { subtree: true, childList: true });
  await sleep(50);
  mo.disconnect();
  assert.equal(v, null, "読めた瞬間に閉じる場面を作れていない");
  const after = roMade;
  await sleep(30);
  assert.equal(after - made0, 0, "閉じた後に見張り（ResizeObserver）を作った");
  tests++;
}

// ⑦ 詳細ペイン: 「この添付を読む」は 2 本目以降だけ（主 PDF = 実在する最初の添付は「PDF を読む」）
{
  const att = (id, name, exists = true) => ({ id, key: "A" + id, mode: "linked", path: "/x/" + name, exists, pageCount: 2, doiCheck: null });
  const detail = (attachments) => ({ id: 1, key: "K1", csl: { title: "架空の論文の題名", author: [{ family: "Example", given: "A" }] }, starred: false, deleted: false,
    dateAdded: "2026-01-01", attachments, collections: [], tags: [], retraction: null, notes: [] });
  const readButtons = () => [...document.querySelectorAll(".attachments .att")].map((p) => [p.querySelector(".path").textContent, !!p.querySelector("button.link")]);
  for (const [atts, want] of [
    [[att(1, "a.pdf")], [["a.pdf", false]]],
    [[att(1, "a.pdf"), att(2, "b.pdf")], [["a.pdf", false], ["b.pdf", true]]],
    // 1 本目の実体が無い = 主 PDF は 2 本目（実在する最初）= 2 本目にも出さない・3 本目に出す
    [[att(1, "gone.pdf", false), att(2, "b.pdf"), att(3, "c.pdf")], [["gone.pdf", false], ["b.pdf", false], ["c.pdf", true]]],
  ]) {
    S.app.detail = detail(atts);
    document.body.replaceChildren();
    const v = mount(ItemDetail, { target: document.body });
    flushSync();
    assert.deepEqual(readButtons(), want, "「この添付を読む」の出し分け: " + JSON.stringify(atts.map((a) => a.path)));
    assert.ok([...document.querySelectorAll(".detail .actions button")].some((b) => b.textContent.trim() === "PDF を読む"), "主 PDF の「PDF を読む」が無い");
    unmount(v);
  }
  tests++;
}

console.log(`viewer head: ok (${tests} tests)`);
process.exit(0);
