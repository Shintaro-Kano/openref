/* OpenNote の IME ガードの部品テスト（node app/src/lib/wiki/ime.test.mjs・Svelte 5 の実部品 + jsdom）。
   かなの確定 Enter（WebKit は compositionend の後に isComposing=false・keyCode=229 で送る）と変換の取消 Esc は入力欄の物 =
   部品の Enter / Esc の動作に使わない（判定は ../input の composing）。確かめること:
   ① composing: isComposing / keyCode 229 / 素のキー・InputEvent（keyCode が無い）
   ② ページ内検索の欄（ShadowPage）: 確定 Enter で次の一致へ飛ばない・取消 Esc で閉じない・素の Enter / ⇧Enter / Esc は飛ぶ・閉じる
   ③ Wiki の検索欄（WikiSearch）: 取消 Esc で検索を消さない・確定 Enter で結果を開かない・素の Esc は消す・変換中の input で検索しない
   ④ 結果一覧の Esc（WikiPage の window）: 入力欄から上がってきた取消 Esc で検索ごと消さない・素の Esc は消す
   ④' ⌘E（WikiPage の window）: IME の ⌘E で表示 ⇄ 編集を切り替えない・素の ⌘E は切り替える
   ⑤ window の Esc で閉じるモーダル（WikiConflict・WikiDialogs・WikiChatConsent）: 取消 Esc で閉じない・素の Esc は閉じる
   ⑥ 結果一覧の hover（WikiResults・IME ではないが同じ検索欄の ↑↓ の部品なのでここ）: hover の選択ではリストをスクロールしない・↑↓ ではする
   出力 = `wiki ime: ok (N tests)`（N = 列 0 のブロックの数） */
import assert from "node:assert/strict";
import { setupHarness } from "./test-harness.mjs";

const { handlers, sleep, waitFor } = setupHarness();
let tests = 0;

const { mount, unmount, flushSync } = await import("svelte");
const { proxy } = await import("svelte/internal/client");
const { composing } = await import("../input.ts");

/** 3 種のキー: 変換中（isComposing）・確定 / 取消（keyCode 229 だけ = WebKit）・素 */
const IME = [
  ["isComposing", { isComposing: true }],
  ["keyCode 229", { keyCode: 229 }],
];
const key = (k, init = {}) => new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init });
const HIT = { path: "hit.md", kind: "md", title: "hit", tags: [], mtime: 0, size: 0, match: "title", snippet: null };
const mkTarget = () => {
  const t = document.createElement("div");
  document.body.append(t);
  return t;
};

// ---- ① composing ----
{
  assert.equal(composing(key("Enter", { isComposing: true })), true);
  assert.equal(composing(key("Enter", { keyCode: 229 })), true, "WebKit の確定 Enter（isComposing=false・keyCode 229）");
  assert.equal(composing(key("Escape", { keyCode: 229 })), true, "WebKit の取消 Esc");
  assert.equal(composing(key("Enter", { keyCode: 13 })), false);
  assert.equal(composing(key("Escape")), false);
  assert.equal(composing(new InputEvent("input", { isComposing: true })), true, "InputEvent（WikiSearch の oninput）");
  assert.equal(composing(new InputEvent("input", { isComposing: false })), false, "InputEvent に keyCode は無い");
  tests++;
}

// ---- ② ページ内検索の欄（ShadowPage = MdView の中身）----
{
  const { default: MdView } = await import("./MdView.svelte");
  const target = mkTarget();
  const props = proxy({
    page: { path: "f.md", abs_path: "/v/f.md", kind: "md", title: "f", size: 0, mtime: 0, content: "alpha 1\n\nalpha 2\n\nalpha 3\n", sha256: "F0", truncated: false, links: [] },
    root: "/v",
    refs: [],
    findRequest: 0,
    onNavigate: () => {},
    readPage: async () => {
      throw new Error("no");
    },
  });
  const cmp = mount(MdView, { target, props });
  flushSync();
  assert.ok(await waitFor(() => (target.querySelector(".wk-md")?.shadowRoot?.textContent ?? "").includes("alpha 3")), "描かない");
  props.findRequest = 1; // ⌘F
  flushSync();
  const input = () => target.querySelector(".wk-find-input");
  const count = () => target.querySelector(".wk-find-count")?.textContent.trim();
  assert.ok(input(), "⌘F で検索欄が開かない");
  input().value = "alpha";
  input().dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
  assert.equal(count(), "1 / 3", "検索しない");
  for (const [name, init] of IME) {
    const e = key("Enter", init);
    input().dispatchEvent(e);
    flushSync();
    assert.equal(count(), "1 / 3", `IME の Enter（${name}）で次の一致へ飛んだ`);
    assert.equal(e.defaultPrevented, false, `IME の Enter（${name}）の既定の動作を止めた`);
    input().dispatchEvent(key("Escape", init));
    flushSync();
    assert.ok(input(), `IME の Esc（${name}）で検索欄が閉じた`);
  }
  input().dispatchEvent(key("Enter"));
  flushSync();
  assert.equal(count(), "2 / 3", "素の Enter で次へ飛ばない");
  input().dispatchEvent(key("Enter", { shiftKey: true }));
  flushSync();
  assert.equal(count(), "1 / 3", "⇧Enter で前へ戻らない");
  const esc = key("Escape");
  input().dispatchEvent(esc);
  flushSync();
  assert.equal(input(), null, "素の Esc で検索欄が閉じない");
  assert.equal(esc.defaultPrevented, true);
  unmount(cmp);
  target.remove();
  tests++;
}

const S = await import("../state.svelte.ts");
const W = await import("./wiki.svelte.ts");
handlers.wiki_search = () => ({ hits: [], total: 0 });

// ---- ③ Wiki の検索欄（WikiSearch）----
{
  const { default: WikiSearch } = await import("./WikiSearch.svelte");
  const target = mkTarget();
  const cmp = mount(WikiSearch, { target });
  flushSync();
  const input = target.querySelector("#wk-search");
  const opened = [];
  handlers.wiki_read_gui = (a) => (opened.push(a.path), { path: a.path, abs_path: "/v/" + a.path, kind: "md", title: a.path, size: 0, mtime: 0, content: "", sha256: "R", truncated: false });
  const arm = () => {
    W.wk.q = "かな";
    W.wk.showResults = true;
    W.wk.results = [HIT];
    W.wk.sel = 0;
    flushSync();
  };
  arm();
  for (const [name, init] of IME) {
    const e = key("Escape", init);
    input.dispatchEvent(e);
    flushSync();
    assert.equal(W.wk.q, "かな", `IME の Esc（${name}）で検索を消した`);
    assert.equal(e.defaultPrevented, false);
    input.dispatchEvent(key("Enter", init));
    flushSync();
    assert.equal(W.wk.showResults, true, `IME の Enter（${name}）で結果を開いた`);
    input.dispatchEvent(key("ArrowDown", init));
    assert.equal(W.wk.sel, 0, `IME の ↓（${name}）で選択を動かした`);
  }
  await sleep(10);
  assert.deepEqual(opened, [], "IME の Enter でページを読んだ");
  const esc = key("Escape");
  input.dispatchEvent(esc);
  flushSync();
  assert.equal(W.wk.q, "", "素の Esc で検索を消さない");
  assert.equal(esc.defaultPrevented, true);
  tests++;

  // 変換中の input（oninput）では検索しない（確定 = compositionend で 1 回）。searchSoon の debounce（200ms）を待って数える
  let searched = 0;
  const search0 = handlers.wiki_search;
  handlers.wiki_search = (a) => (searched++, search0(a));
  S.app.wikiOn = true; // runSearch は Wiki OFF なら invoke しない
  input.value = "か"; // bind:value が input の event で wk.q へ写す
  input.dispatchEvent(new InputEvent("input", { isComposing: true, bubbles: true }));
  await sleep(260);
  assert.equal(searched, 0, "変換中の input で検索した");
  input.dispatchEvent(new InputEvent("input", { isComposing: false, bubbles: true }));
  assert.ok(await waitFor(() => searched === 1, 1000), "素の input で検索しない（試験の前提）");
  handlers.wiki_search = search0;
  W.wk.q = "";
  W.wk.showResults = false;
  W.wk.results = [];
  unmount(cmp);
  target.remove();
  tests++;
}

// ---- ④ 結果一覧の Esc（WikiPage の window）: 入力欄（WikiSearch は IME のとき stopPropagation しない）から上がった取消 Esc ----
{
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const { default: WikiSearch } = await import("./WikiSearch.svelte");
  handlers.wiki_status = () => ({ enabled: true, root: "/v", pages: 0, inbox: 0 });
  S.app.wikiOn = true;
  const tab = S.app.tabs.find((t) => t.id === S.app.tab) ?? S.app.tabs[0];
  S.app.tab = tab.id;
  tab.wiki = { path: null, scrollTop: 0, editing: false };
  W.wk.status = { enabled: true, root: "/v" };
  const target = mkTarget();
  const page = mount(WikiPage, { target });
  const search = mount(WikiSearch, { target });
  flushSync();
  assert.ok(S.frontWiki(), "前面が Wiki のタブでない（window の Esc を確かめられない）");
  const input = target.querySelector("#wk-search");
  W.wk.q = "かな";
  W.wk.showResults = true;
  W.wk.results = [HIT];
  flushSync();
  for (const [name, init] of IME) {
    input.dispatchEvent(key("Escape", init)); // 入力欄 → window へ上がる
    window.dispatchEvent(key("Escape", init)); // フォーカスが欄の外（結果一覧）でも
    flushSync();
    assert.equal(W.wk.q, "かな", `IME の Esc（${name}）が window に届いて検索ごと消えた`);
  }
  window.dispatchEvent(key("Escape"));
  flushSync();
  assert.equal(W.wk.q, "", "結果一覧の素の Esc で検索を消さない");
  tests++;

  // ④' ⌘E（編集できるページを前面に出してから）
  handlers.wiki_read_gui = (a) => ({ path: a.path, abs_path: "/v/" + a.path, kind: "md", title: a.path, size: 4, mtime: 0, content: "本文\n", sha256: "E0", truncated: false, links: [] });
  handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
  tab.wiki = { path: "e.md", scrollTop: 0, editing: false };
  flushSync();
  assert.ok(await waitFor(() => W.wk.page?.path === "e.md" && !!target.querySelector(".wk-seg"), 2000), "編集できるページを描かない（⌘E を確かめられない）");
  const cmdE = (init = {}) => window.dispatchEvent(key("e", { metaKey: true, ...init }));
  for (const [name, init] of IME) {
    cmdE(init);
    flushSync();
    await sleep(20);
    assert.equal(tab.wiki.editing, false, `IME の ⌘E（${name}）で編集に切り替えた`);
  }
  cmdE();
  assert.ok(await waitFor(() => tab.wiki.editing === true), "素の ⌘E で編集に切り替えない");
  cmdE();
  assert.ok(await waitFor(() => tab.wiki.editing === false), "素の ⌘E で表示に戻らない");
  unmount(search);
  unmount(page);
  target.remove();
  tests++;
}

// ---- ⑤ window の Esc で閉じるモーダル ----
{
  const E = await import("./edit.svelte.ts");
  const C = await import("../chat/chat.svelte.ts");
  const { default: WikiConflict } = await import("./WikiConflict.svelte");
  const { default: WikiDialogs } = await import("./WikiDialogs.svelte");
  const { default: WikiChatConsent } = await import("../chat/ChatConsent.svelte");
  handlers.wiki_chat_set_enabled = () => null;
  const cases = [
    // [名前, 部品, 開く, 開いているか]
    // conflict のダイアログ（キャンセル = ダイアログだけ閉じる。ed は編集の状態機械を通さず直に置く = Esc の配線だけを見る）
    ["WikiConflict", WikiConflict, () => Object.assign(E.ed, { path: "c.md", state: "conflict", dialog: true }), () => E.ed.dialog],
    ["WikiDialogs（ダイアログ）", WikiDialogs, () => (W.wk.dialog = { kind: "new", dir: "" }), () => !!W.wk.dialog],
    ["WikiChatConsent", WikiChatConsent, () => (C.chat.consent = true), () => !!C.chat.consent],
  ];
  for (const [name, Cmp, open, isOpen] of cases) {
    const target = mkTarget();
    open();
    const cmp = mount(Cmp, { target });
    flushSync();
    assert.ok(isOpen(), `${name}: 開かない`);
    for (const [iname, init] of IME) {
      window.dispatchEvent(key("Escape", init));
      flushSync();
      await sleep(5);
      assert.ok(isOpen(), `${name}: IME の Esc（${iname}）で閉じた`);
    }
    window.dispatchEvent(key("Escape"));
    flushSync();
    assert.ok(await waitFor(() => !isOpen()), `${name}: 素の Esc で閉じない`);
    unmount(cmp);
    target.remove();
  }
  Object.assign(E.ed, { path: null, state: "clean", dialog: false });
  tests++;
}

// ---- ⑥ 結果一覧の hover（WikiResults）: hover で選択は動くがスクロールしない（半分見えている行に乗ってリストが動かない）・
// 検索欄の ↑↓ はスクロールする（hover した行へ ↑↓ で戻ったときも）----
{
  const { default: WikiResults } = await import("./WikiResults.svelte");
  const { default: WikiSearch } = await import("./WikiSearch.svelte");
  const scrolled = [];
  const had = Object.prototype.hasOwnProperty.call(HTMLElement.prototype, "scrollIntoView");
  const orig = HTMLElement.prototype.scrollIntoView;
  HTMLElement.prototype.scrollIntoView = function () {
    scrolled.push(this.dataset.i);
  };
  W.wk.q = "x";
  W.wk.showResults = true;
  W.wk.results = ["a", "b", "c"].map((p) => ({ ...HIT, path: `${p}.md`, title: p }));
  W.wk.sel = 0;
  const target = mkTarget();
  const res = mount(WikiResults, { target });
  const search = mount(WikiSearch, { target });
  flushSync();
  await sleep(5);
  scrolled.length = 0;
  const rows = target.querySelectorAll(".wk-hit");
  const input = target.querySelector("#wk-search");
  const step = async (fn) => {
    fn();
    flushSync();
    await sleep(5);
  };
  await step(() => rows[2].dispatchEvent(new MouseEvent("mouseenter")));
  assert.equal(W.wk.sel, 2, "hover で選択が動かない");
  assert.deepEqual(scrolled, [], "hover でスクロールした");
  await step(() => input.dispatchEvent(key("ArrowUp")));
  assert.equal(W.wk.sel, 1);
  assert.deepEqual(scrolled, ["1"], "↑ でスクロールしない");
  await step(() => input.dispatchEvent(key("ArrowDown")));
  assert.deepEqual(scrolled, ["1", "2"], "hover した行へ ↓ で戻ったときにスクロールしない");
  // 選択中の行に乗る（選択は変わらない）→ 次の ↓ はスクロールする
  await step(() => rows[2].dispatchEvent(new MouseEvent("mouseenter")));
  await step(() => input.dispatchEvent(key("ArrowDown")));
  assert.equal(W.wk.sel, 0);
  assert.deepEqual(scrolled, ["1", "2", "0"], "選択中の行への hover の後の ↓ でスクロールしない");
  await step(() => rows[1].dispatchEvent(new MouseEvent("mouseenter")));
  assert.deepEqual(scrolled, ["1", "2", "0"], "hover でスクロールした（2 回目）");
  unmount(search);
  unmount(res);
  target.remove();
  if (had) HTMLElement.prototype.scrollIntoView = orig;
  else delete HTMLElement.prototype.scrollIntoView;
  W.wk.q = "";
  W.wk.showResults = false;
  W.wk.results = [];
  tests++;
}

console.log(`wiki ime: ok (${tests} tests)`);
process.exit(0); // toast・debounce のタイマーを待たない
