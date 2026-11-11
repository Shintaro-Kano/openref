/* OpenNote の遷移の部品テスト（node app/src/lib/wiki/nav.test.mjs・実部品 state.svelte.ts / wiki.svelte.ts / files.svelte.ts /
   WikiHeader / WikiPage / MdView / WikiLinkMenu + jsdom・invoke は差し替え）。確かめること:
   ① 戻る / 進む（タブの中の履歴）: 同じタブの遷移で前のページ（ホームも）とスクロール位置を積む・戻ると位置ごと戻る・進むで往復・
      新しい遷移で「進む」を捨てる・同じページ（見出しへの移動）と新しいタブは積まない・タブごとに別・上限 100・表示で着地・結果一覧を畳む
   ② 編集中の未保存は保存してから戻る・保存できず「破棄して移る」を断ったら移らない（履歴も崩さない）・保存を待つ間にタブを移った /
      リンクで別のページへ移ったら何もしない
   ③ 改名・ゴミ箱を履歴に映す（改名 = 新しい名前へ・ゴミ箱 = 捨てて隣り合った同じページを 1 つに・端が今のページなら捨てる = 押せるのに動かない ‹ を作らない・
      別の root のタブは触らない・最後の 1 枚で今のページを捨ててホームへ戻っても履歴は残る）
   ④ ヘッダーの ‹ ›（実部品 WikiHeader）: 押せないときは disabled・押すと戻る / 進む
   ⑤ ⌘[ / ⌘]（実部品 WikiPage）: 戻る / 進む・入力欄の中では奪わない（shadow の中の入力欄も）・覆い（参照エディタ等）の下では動かさない・
      ⌘⇧[ はタブの巡回（ここでは扱わない）・戻った先の位置に後から伸びる中身で届かなければ伸びるたびに追う（利用者の操作で止める）
   ⑥ リンクの右クリック（実部品 MdView + WikiLinkMenu）: 既定のメニューを止めて種類ごとの項目（効く物だけ）・リンクの外は既定のまま・
      「新しいタブで開く」= onNavigate(nav, true)・「リンクをコピー」= 書かれたまま・文字を選んでいれば頭に「コピー」・
      閉じる = Esc（背後へ流さない）/ 外側の左ボタン / 外側の右クリック（右ボタンの mousedown では閉じない）/ メニューを出した部品を動かすスクロール
      （別の箱のスクロール = チャットの自動追従では閉じない）
   ⑦ 中クリック = 新しいタブ・右ボタンの auxclick では開かない
   ⑧ linkItems（項目の表）: md / md 以外 / 参照フォルダ / 外部で開くだけ / 文献 / URL / 見出し / 未解決
   出力 = `wiki nav: ok (N tests)`（N = 列 0 のブロックの数） */
import assert from "node:assert/strict";
import { setupHarness } from "./test-harness.mjs";

const { calls, handlers, sleep, waitFor } = setupHarness();
let tests = 0;

const { mount, unmount, flushSync } = await import("svelte");
const S = await import("../state.svelte.ts");
const W = await import("./wiki.svelte.ts");
const E = await import("./edit.svelte.ts");
const FL = await import("./files.svelte.ts");
const LM = await import("./linkmenu.svelte.ts");
const { createEditor } = await import("./editor/cm.ts");
E.cfg.retry = 100000;

const page = (path, content, sha = path + "-0") => ({ path, abs_path: "/v/" + path, kind: "md", title: path, size: content.length, mtime: 0, content, sha256: sha, truncated: false, links: [] });
const tab = S.app.tabs[0];
S.app.wikiOn = true;
W.wk.status = { enabled: true, root: "/v" };
handlers.wiki_list = () => ({ total: 0, count: 0, pages: [] });
handlers.wiki_tags = () => ({ tags: [], total: 0 });
handlers.wiki_inbox = () => ({ files: [], total: 0 });
handlers.wiki_chat_log = () => ({ pages: [], total: 0 });
handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
handlers.list_items = () => [];
handlers.get_ui_pref = () => null;
handlers.set_ui_pref = () => null;
const hist = (w) => ({ back: (w.back ?? []).map((h) => [h.path, h.scrollTop]), fwd: (w.fwd ?? []).map((h) => [h.path, h.scrollTop]) });

// ---------- ① 戻る / 進む ----------
{
  tab.wiki = { path: null, root: "/v", scrollTop: 0, editing: false };
  S.app.tab = tab.id;
  assert.deepEqual([W.canGo(-1), W.canGo(1)], [false, false], "履歴が無いのに押せる");
  await W.openWikiPage("a.md");
  tab.wiki.scrollTop = 120;
  await W.openWikiPage("b.md");
  tab.wiki.scrollTop = 40;
  await W.openWikiPage("c.md");
  assert.equal(tab.wiki.path, "c.md");
  assert.deepEqual(hist(tab.wiki), { back: [[null, 0], ["a.md", 120], ["b.md", 40]], fwd: [] }, "離れたページと位置を積まない");
  assert.deepEqual([W.canGo(-1), W.canGo(1)], [true, false]);
  // 同じページ（見出しへ）・新しいタブでは積まない
  await W.openWikiPage("c.md", false, "見出し");
  assert.equal(tab.wiki.back.length, 3, "同じページの見出しへの移動を積んだ");
  // 戻る = 位置ごと・表示で・結果一覧を畳む
  tab.wiki.scrollTop = 7;
  tab.wiki.editing = true;
  W.wk.showResults = true;
  W.wk.scrollTo = { id: "x", nonce: 1 };
  await W.goHistory(-1);
  assert.deepEqual([tab.wiki.path, tab.wiki.scrollTop, tab.wiki.editing], ["b.md", 40, false], "戻った先の位置 / 表示");
  assert.equal(W.wk.showResults, false, "結果一覧を畳まない");
  assert.equal(W.wk.scrollTo, null, "前のページの見出しへの着地を残した");
  assert.deepEqual(hist(tab.wiki), { back: [[null, 0], ["a.md", 120]], fwd: [["c.md", 7]] });
  await W.goHistory(-1);
  await W.goHistory(-1);
  assert.deepEqual([tab.wiki.path, W.canGo(-1), W.canGo(1)], [null, false, true], "ホームまで戻らない");
  await W.goHistory(-1); // 端 = 何もしない
  assert.equal(tab.wiki.path, null);
  await W.goHistory(1);
  await W.goHistory(1);
  assert.deepEqual([tab.wiki.path, tab.wiki.scrollTop], ["b.md", 40], "進むで戻らない");
  // 新しい遷移 = 進むを捨てる
  await W.openWikiPage("d.md");
  assert.deepEqual([W.canGo(1), tab.wiki.back.at(-1).path], [false, "b.md"], "新しい遷移で進むを捨てない");
  // 新しいタブ = 元のタブの履歴は変えない・新しいタブは空
  const before = JSON.stringify(hist(tab.wiki));
  await W.openWikiPage("e.md", true);
  const other = S.app.tabs.find((x) => x.id === S.app.tab);
  assert.notEqual(other.id, tab.id, "新しいタブを開かない");
  assert.deepEqual([W.canGo(-1), W.canGo(1)], [false, false], "新しいタブに履歴がある");
  assert.equal(JSON.stringify(hist(tab.wiki)), before, "新しいタブで元のタブの履歴が変わった");
  await S.closeTab(other.id);
  assert.equal(S.app.tab, tab.id);
  // 上限 = 100（古い方から捨てる）
  tab.wiki = { path: "p0.md", root: "/v", scrollTop: 0, editing: false };
  for (let i = 1; i <= 130; i++) await W.openWikiPage(`p${i}.md`);
  assert.equal(tab.wiki.back.length, 100, "上限で捨てない");
  assert.equal(tab.wiki.back[0].path, "p30.md");
  // ui_tabs には履歴を書かない（path と root だけ）
  assert.ok(!S.tabsPref().includes("p29.md") && S.tabsPref().includes("p130.md"), S.tabsPref());
  tests++;
}

// ---------- ② 編集中の未保存は保存してから・断ったら移らない・待つ間にタブを移ったら何もしない ----------
{
  const host = document.createElement("div");
  document.body.append(host);
  const session = (path) => {
    E.startEdit(page(path, "x", path + "-0"), "/v");
    const h = createEditor(host, { doc: "x", mode: "md", onChange: E.changed });
    E.attach(h);
    h.view.dispatch({ changes: { from: 1, insert: "y" }, userEvent: "input" });
    return h;
  };
  const close = (h) => {
    E.detach(h);
    h.destroy();
    E.discardEdit();
  };
  tab.wiki = { path: "a.md", root: "/v", scrollTop: 0, editing: false };
  await W.openWikiPage("b.md");
  tab.wiki.editing = true;
  // 保存できる = 保存してから戻る
  handlers.wiki_save = (a) => ({ ok: true, path: a.path, sha256: "S1", size: a.content.length, mtime: 1, created: false });
  let h = session("b.md");
  calls.length = 0;
  await W.goHistory(-1);
  assert.equal(calls.filter((c) => c.name === "wiki_save").at(-1)?.args.content, "xy", "戻る前に保存しない");
  assert.equal(tab.wiki.path, "a.md");
  close(h);
  // 保存できない → 「破棄して移る」を断る = 移らない・履歴もそのまま
  await W.goHistory(1);
  tab.wiki.editing = true;
  handlers.wiki_save = () => {
    throw new Error("disk full");
  };
  h = session("b.md");
  const p = W.goHistory(-1);
  assert.ok(await waitFor(() => W.wk.dialog?.kind === "discard"), "保存できないのに尋ねない");
  W.answerDiscard(false);
  await p;
  assert.equal(tab.wiki.path, "b.md", "保存できないのに移った");
  assert.deepEqual(hist(tab.wiki), { back: [["a.md", 0]], fwd: [] }, "移らなかったのに履歴が変わった");
  close(h);
  // 保存を待つ間に別のタブへ = 何もしない
  let release;
  handlers.wiki_save = (a) => new Promise((r) => (release = () => r({ ok: true, path: a.path, sha256: "S2", size: a.content.length, mtime: 1, created: false })));
  tab.wiki.editing = true;
  h = session("b.md");
  const q = W.goHistory(-1);
  assert.ok(await waitFor(() => !!release), "保存に行かない");
  // 別のタブ（一覧）を足して前面へ（tabOpsAllowed は編集中でも通る = 前面の id だけ変える）
  S.app.tabs.push({ ...S.app.tabs[0], id: 9999, wiki: null });
  S.app.tab = 9999;
  release();
  await q;
  assert.equal(tab.wiki.path, "b.md", "待つ間にタブを移ったのに元のタブを戻した");
  assert.equal(tab.wiki.back.length, 1, "待つ間にタブを移ったのに履歴が変わった");
  S.app.tabs.splice(S.app.tabs.findIndex((x) => x.id === 9999), 1);
  S.app.tab = tab.id;
  close(h);
  // 保存を待つ間にリンクで別のページへ移った = 戻らない（押したリンクの行き先を奪わない・移る前のページを履歴から落とさない）
  release = null;
  tab.wiki.editing = true;
  h = session("b.md");
  const r1 = W.goHistory(-1);
  assert.ok(await waitFor(() => !!release));
  const r2 = W.openWikiPage("c.md");
  release();
  await r1;
  await r2;
  assert.equal(tab.wiki.path, "c.md", "保存を待つ間に押したリンクの行き先を戻るが奪った");
  assert.deepEqual(hist(tab.wiki).back.map((x) => x[0]), ["a.md", "b.md"], "移る前のページを履歴から落とした");
  close(h);
  tab.wiki.editing = false;
  tests++;
}

// ---------- ③ 改名・ゴミ箱を履歴に映す ----------
{
  tab.wiki = { path: "x.md", root: "/v", scrollTop: 0, editing: false };
  for (const p of ["old.md", "k.md", "d/g.md", "k.md"]) await W.openWikiPage(p);
  await W.goHistory(-1); // 今 = d/g.md・進む = k.md
  // 別の root のタブ（同じ相対パス = 別のファイル）は触らない
  S.app.tabs.push({ ...S.app.tabs[0], id: 9998, wiki: { path: "z.md", root: "/other", scrollTop: 0, editing: false, back: [{ path: "old.md", scrollTop: 0 }], fwd: [] } });
  handlers.wiki_rename = (a) => ({ path: "new.md", rewritten: [], skipped: [] });
  await FL.doRename("old.md", "new.md", false);
  assert.deepEqual(hist(tab.wiki).back.map((x) => x[0]), ["x.md", "new.md", "k.md"], "改名を履歴に映さない");
  assert.equal(S.app.tabs.find((x) => x.id === 9998).wiki.back[0].path, "old.md", "別の root のタブの履歴を書き換えた");
  // ゴミ箱 = 戻る / 進むの k.md を捨てる（今のページ d/g.md は残る）
  handlers.wiki_trash = () => ({ trashed: true });
  await FL.doTrash("k.md");
  assert.deepEqual(hist(tab.wiki), { back: [["x.md", 0], ["new.md", 0]], fwd: [] }, "ゴミ箱のページを履歴から捨てない");
  // 捨てて隣り合った同じページは 1 つ
  tab.wiki.back = [{ path: "a.md", scrollTop: 0 }, { path: "gone.md", scrollTop: 0 }, { path: "a.md", scrollTop: 5 }];
  W.rewriteHistory((p) => (p === "gone.md" ? null : p));
  assert.deepEqual(hist(tab.wiki).back, [["a.md", 0]], "隣り合った同じページを詰めない");
  // 今と同じページの項目は飛ばす（戻ったのに画面が変わらない、を作らない）
  tab.wiki.back = [{ path: "a.md", scrollTop: 3 }, { path: tab.wiki.path, scrollTop: 0 }];
  await W.goHistory(-1);
  assert.deepEqual([tab.wiki.path, tab.wiki.scrollTop], ["a.md", 3], "今と同じページの項目で止まった");
  // A → B → A の後に B を捨てる = 戻る先が今のページだけになる → ‹ は押せない（押しても動かない ‹ を作らない）
  tab.wiki = { path: "A.md", root: "/v", scrollTop: 0, editing: false };
  await W.openWikiPage("B.md");
  await W.openWikiPage("A.md");
  await FL.doTrash("B.md");
  assert.equal(W.canGo(-1), false, "戻る先が今のページだけなのに ‹ が押せる");
  // 最後の 1 枚で今のページを捨てた = ホームへ・履歴は残る
  S.app.tabs.splice(S.app.tabs.findIndex((x) => x.id === 9998), 1);
  assert.equal(S.app.tabs.length, 1);
  tab.wiki = { path: "s.md", root: "/v", scrollTop: 0, editing: false };
  await W.openWikiPage("t.md");
  await FL.doTrash("t.md");
  assert.equal(tab.wiki.path, null, "捨てたページのままホームへ戻らない");
  assert.deepEqual(hist(tab.wiki).back, [["s.md", 0]], "ホームへ戻したら履歴が消えた");
  tests++;
}

// ---------- ④ ヘッダーの ‹ ›（実部品 WikiHeader） ----------
{
  tab.wiki = { path: null, root: "/v", scrollTop: 0, editing: false };
  const { default: Header } = await import("./WikiHeader.svelte");
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(Header, { target });
  flushSync();
  const [back, fwd] = target.querySelectorAll(".wk-head-histbtn");
  assert.ok(back && fwd, "‹ › が無い");
  assert.deepEqual([back.disabled, fwd.disabled], [true, true], "履歴が無いのに押せる");
  assert.equal(back.getAttribute("title"), "戻る（⌘[）");
  await W.openWikiPage("h.md");
  flushSync();
  assert.deepEqual([back.disabled, fwd.disabled], [false, true]);
  back.click();
  assert.ok(await waitFor(() => tab.wiki.path === null), "‹ で戻らない");
  flushSync();
  assert.deepEqual([back.disabled, fwd.disabled], [true, false]);
  fwd.click();
  assert.ok(await waitFor(() => tab.wiki.path === "h.md"), "› で進まない");
  unmount(cmp);
  target.remove();
  tests++;
}

// ---------- ⑤ ⌘[ / ⌘]（実部品 WikiPage） ----------
{
  const files = { "a.md": "# A\n", "b.md": "# B\n" };
  handlers.wiki_read_gui = (a) => page(a.path, files[a.path]);
  W.wk.page = null;
  tab.wiki = { path: "a.md", root: "/v", scrollTop: 0, editing: false };
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  assert.ok(await waitFor(() => W.wk.page?.path === "a.md"), "ページを読まない");
  await W.openWikiPage("b.md");
  assert.ok(await waitFor(() => W.wk.page?.path === "b.md"));
  const key = (k, at = window, mod = {}) => {
    const ev = new KeyboardEvent("keydown", { key: k, metaKey: true, bubbles: true, cancelable: true, ...mod });
    at.dispatchEvent(ev);
    return ev;
  };
  let ev = key("[");
  assert.ok(ev.defaultPrevented, "⌘[ を奪わない");
  assert.ok(await waitFor(() => tab.wiki.path === "a.md" && W.wk.page?.path === "a.md"), "⌘[ で戻らない");
  key("]");
  assert.ok(await waitFor(() => tab.wiki.path === "b.md"), "⌘] で進まない");
  // 入力欄の中（検索欄・チャット・CodeMirror）では奪わない
  const input = document.createElement("textarea");
  document.body.append(input);
  ev = key("[", input);
  await sleep(20);
  assert.ok(!ev.defaultPrevented && tab.wiki.path === "b.md", "入力欄の中の ⌘[ を奪った");
  input.remove();
  // shadow の中の入力欄（html ページのフォーム）= target は host に付け替わる → composedPath の頭で見る
  const shost = document.createElement("div");
  document.body.append(shost);
  const sinput = shost.attachShadow({ mode: "open" }).appendChild(document.createElement("input"));
  ev = new KeyboardEvent("keydown", { key: "[", metaKey: true, bubbles: true, composed: true, cancelable: true });
  sinput.dispatchEvent(ev);
  await sleep(20);
  assert.ok(!ev.defaultPrevented && tab.wiki.path === "b.md", "shadow の中の入力欄の ⌘[ を奪った");
  shost.remove();
  // 覆い（参照エディタ等 = タブの操作と同じ門）の下では動かさない
  S.app.editorOpen = true;
  ev = key("[");
  await sleep(20);
  assert.ok(!ev.defaultPrevented && tab.wiki.path === "b.md", "参照エディタの下で戻った");
  S.app.editorOpen = false;
  // ⌘⇧[ はタブの巡回（App）= ここでは戻らない
  ev = key("[", window, { shiftKey: true });
  await sleep(20);
  assert.equal(tab.wiki.path, "b.md", "⌘⇧[ で戻った");
  // 戻った先の位置: 後から伸びる中身で届かなければ、伸びるたびに追う（ResizeObserver）・利用者の操作で止める
  const el = target.querySelector(".wk-body");
  let max = 1000;
  let st = 0;
  Object.defineProperty(el, "scrollTop", {
    configurable: true,
    get: () => st,
    set: (v) => {
      st = Math.max(0, Math.min(v, max));
      el.dispatchEvent(new Event("scroll"));
    },
  });
  const ros = [];
  globalThis.ResizeObserver = class {
    constructor(cb) {
      this.cb = cb;
      this.dead = false;
      ros.push(this);
    }
    observe() {}
    disconnect() {
      this.dead = true;
    }
  };
  await W.openWikiPage("a.md");
  assert.ok(await waitFor(() => W.wk.page?.path === "a.md"));
  el.scrollTop = 600;
  assert.equal(tab.wiki.scrollTop, 600, "スクロールをタブに持たない");
  await W.openWikiPage("b.md");
  assert.ok(await waitFor(() => W.wk.page?.path === "b.md"));
  max = 100; // 戻った直後の a.md はまだ短い（画像・埋め込みが後から伸びる）
  await W.goHistory(-1);
  assert.ok(await waitFor(() => W.wk.page?.path === "a.md" && ros.length === 1), "届かないのに追わない");
  assert.equal(st, 100);
  max = 400;
  ros[0].cb();
  assert.equal(st, 400, "伸びたのに追わない");
  assert.equal(ros[0].dead, false);
  max = 2000;
  ros[0].cb();
  assert.deepEqual([st, ros[0].dead, tab.wiki.scrollTop], [600, true, 600], "届いたのに止めない / 位置をタブに残さない");
  // 利用者の操作（ホイール）で止める
  await W.goHistory(1);
  assert.ok(await waitFor(() => W.wk.page?.path === "b.md"));
  el.scrollTop = 0;
  await W.openWikiPage("a.md"); // 進むを捨てて a.md（位置 0）→ b.md の位置を残して戻る
  assert.ok(await waitFor(() => W.wk.page?.path === "a.md"));
  max = 50;
  tab.wiki.back.at(-1).scrollTop = 300; // b.md の位置
  await W.goHistory(-1);
  assert.ok(await waitFor(() => W.wk.page?.path === "b.md" && ros.length === 2));
  el.dispatchEvent(new Event("wheel"));
  assert.equal(ros[1].dead, true, "ホイールで追うのを止めない");
  delete globalThis.ResizeObserver;
  unmount(cmp);
  target.remove();
  tests++;
}

// ---------- ⑥ リンクの右クリック（実部品 MdView + WikiLinkMenu）・⑦ 中クリック ----------
{
  const { default: MdView } = await import("./MdView.svelte");
  const { default: Menu } = await import("./WikiLinkMenu.svelte");
  const copied = [];
  Object.defineProperty(navigator, "clipboard", { value: { writeText: async (s) => void copied.push(s) }, configurable: true });
  const navs = [];
  const content = "[[x]] [[doc.pdf]] [[gone]] [外](https://example.com/a) [節](#h-a) 地の文\n\n## A\n";
  const links = [
    { kind: "wiki", target: "x", path: "notes/x.md", ambiguous: false },
    { kind: "wiki", target: "doc.pdf", path: "docs/doc.pdf", ambiguous: false },
    { kind: "wiki", target: "gone", path: null, ambiguous: false },
  ];
  const target = document.createElement("div");
  document.body.append(target);
  const view = mount(MdView, { target, props: { page: { ...page("p.md", content), links }, root: "/v", refs: [], onNavigate: (n, nt) => navs.push([n, !!nt]), readPage: async () => page("x", "") } });
  const mtarget = document.createElement("div");
  document.body.append(mtarget);
  const menu = mount(Menu, { target: mtarget });
  flushSync();
  const sr = () => target.querySelector(".wk-md")?.shadowRoot;
  assert.ok(await waitFor(() => sr()?.querySelectorAll("a").length >= 5), "リンクを描かない");
  const a = (text) => [...sr().querySelectorAll("a")].find((x) => x.textContent.trim() === text);
  const ctx = (el) => {
    const ev = new MouseEvent("contextmenu", { bubbles: true, cancelable: true, composed: true, clientX: 30, clientY: 40 });
    el.dispatchEvent(ev);
    flushSync();
    return ev;
  };
  const labels = () => [...mtarget.querySelectorAll(".wk-lm button")].map((b) => b.textContent.trim());
  const head = () => mtarget.querySelector(".wk-lm-head")?.textContent.trim();

  // md のページ = 開く / 新しいタブで開く / リンクをコピー（既定のメニューは止める）
  let ev = ctx(a("x"));
  assert.ok(ev.defaultPrevented, "WebView の既定のメニューを止めない");
  assert.deepEqual(labels(), ["開く", "新しいタブで開く", "リンクをコピー"]);
  assert.equal(head(), "[[x]]");
  assert.equal(mtarget.querySelectorAll(".wk-lm-sep").length, 1);
  [...mtarget.querySelectorAll(".wk-lm button")][1].click();
  flushSync();
  assert.deepEqual(navs.at(-1), [{ kind: "page", path: "notes/x.md" }, true], "新しいタブで開かない");
  assert.equal(LM.linkMenu.cur, null, "押したのに閉じない");
  assert.equal(mtarget.querySelector(".wk-lm"), null);
  // 開く = 同じタブ
  ctx(a("x"));
  [...mtarget.querySelectorAll(".wk-lm button")][0].click();
  assert.deepEqual(navs.at(-1), [{ kind: "page", path: "notes/x.md" }, false]);
  // リンクをコピー = 書かれたまま
  ctx(a("x"));
  [...mtarget.querySelectorAll(".wk-lm button")][2].click();
  assert.ok(await waitFor(() => copied.length === 1), "コピーしない");
  assert.deepEqual(copied, ["[[x]]"]);
  // md 以外（PDF）= 開く / 外部のアプリで開く（絶対パス）
  ctx(a("doc.pdf"));
  assert.deepEqual(labels(), ["開く", "外部のアプリで開く", "リンクをコピー"]);
  [...mtarget.querySelectorAll(".wk-lm button")][1].click();
  assert.deepEqual(navs.at(-1), [{ kind: "open", abs: "/v/docs/doc.pdf" }, false]);
  // URL = ブラウザで開く / リンクをコピー
  ctx(a("外"));
  assert.deepEqual(labels(), ["ブラウザで開く", "リンクをコピー"]);
  assert.equal(head(), "https://example.com/a");
  [...mtarget.querySelectorAll(".wk-lm button")][0].click();
  assert.deepEqual(navs.at(-1), [{ kind: "external", url: "https://example.com/a" }, false]);
  // 同じページの見出し = 移動（遷移の要求は出さない）
  const n0 = navs.length;
  ctx(a("節"));
  assert.deepEqual(labels(), ["この見出しへ移動", "リンクをコピー"]);
  [...mtarget.querySelectorAll(".wk-lm button")][0].click();
  assert.equal(navs.length, n0, "見出しへの移動で遷移を要求した");
  // 未解決 = コピーだけ（見出しで未解決と知らせる）
  ctx(a("gone"));
  assert.deepEqual(labels(), ["リンクをコピー"]);
  assert.equal(head(), "未解決のリンク: [[gone]]");
  // Esc = 閉じる・背後（App の Esc）へ流さない
  let behind = 0;
  const spy = () => behind++;
  window.addEventListener("keydown", spy);
  const esc = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
  document.body.dispatchEvent(esc);
  flushSync();
  assert.equal(LM.linkMenu.cur, null, "Esc で閉じない");
  assert.equal(behind, 0, "Esc を背後へ流した");
  window.removeEventListener("keydown", spy);
  // 外側の押下 = 閉じる・中の押下では閉じない
  ctx(a("x"));
  mtarget.querySelector(".wk-lm button").dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
  flushSync();
  assert.ok(LM.linkMenu.cur, "中の押下で閉じた");
  // 右ボタンの mousedown では閉じない（WebView が contextmenu より後に送っても、開いたばかりのメニューを閉じない）
  document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 2 }));
  flushSync();
  assert.ok(LM.linkMenu.cur, "右ボタンの mousedown で閉じた");
  // 別の箱のスクロール（チャットの応答の自動追従）では閉じない・メニューを出した部品を動かすスクロールでは閉じる
  const other = document.createElement("div");
  document.body.append(other);
  other.dispatchEvent(new Event("scroll"));
  flushSync();
  assert.ok(LM.linkMenu.cur, "別の箱のスクロールで閉じた");
  other.remove();
  target.dispatchEvent(new Event("scroll"));
  flushSync();
  assert.equal(LM.linkMenu.cur, null, "ページのスクロールで閉じない");
  // 外側の左ボタン = 閉じる
  ctx(a("x"));
  document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
  flushSync();
  assert.equal(LM.linkMenu.cur, null, "外側の押下で閉じない");
  // 外側の右クリック = 閉じる（リンクならそこで開き直す）
  ctx(a("x"));
  ctx(a("外"));
  assert.deepEqual(labels(), ["ブラウザで開く", "リンクをコピー"], "別のリンクの右クリックで開き直さない");
  document.body.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
  flushSync();
  assert.equal(LM.linkMenu.cur, null, "外側の右クリックで閉じない");
  // 文字を選んでいる = 頭に「コピー」（選んだ文字）
  const origSel = window.getSelection;
  window.getSelection = () => ({ toString: () => "選んだ字" });
  ctx(a("x"));
  assert.deepEqual(labels(), ["コピー", "開く", "新しいタブで開く", "リンクをコピー"]);
  [...mtarget.querySelectorAll(".wk-lm button")][0].click();
  assert.ok(await waitFor(() => copied.length === 2), "選んだ字をコピーしない");
  assert.equal(copied[1], "選んだ字");
  window.getSelection = origSel;
  // リンクの外 = 既定のメニューのまま（選択のコピー・調べる）
  const text = [...sr().querySelectorAll("p")].at(0);
  ev = ctx(text);
  assert.ok(!ev.defaultPrevented && LM.linkMenu.cur === null, "リンクの外の右クリックを奪った");
  // ⑦ 中クリック = 新しいタブ・右ボタンの auxclick は開かない
  const n1 = navs.length;
  a("x").dispatchEvent(new MouseEvent("auxclick", { bubbles: true, cancelable: true, composed: true, button: 1 }));
  assert.deepEqual(navs.at(-1), [{ kind: "page", path: "notes/x.md" }, true], "中クリックで新しいタブに開かない");
  a("x").dispatchEvent(new MouseEvent("auxclick", { bubbles: true, cancelable: true, composed: true, button: 2 }));
  assert.equal(navs.length, n1 + 1, "右ボタンの auxclick で開いた");
  // 素のクリック・⌘クリックは従来どおり
  a("x").dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, composed: true }));
  a("x").dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, composed: true, metaKey: true }));
  assert.deepEqual(navs.slice(-2).map((x) => x[1]), [false, true]);
  unmount(menu);
  unmount(view);
  target.remove();
  mtarget.remove();
  tests++;
}

// ---------- ⑧ linkItems（項目の表） ----------
{
  const got = [];
  const c = { go: (n, nt) => got.push([n, !!nt]), anchor: (id) => got.push(["anchor", id]), root: "/ref" };
  const labels = (items) => items.map((x) => (x ? x.label : "—"));
  assert.deepEqual(labels(LM.linkItems({ t: "page", path: "n/a.md", heading: "H" }, "[[a#H]]", c)), ["開く", "新しいタブで開く", "—", "リンクをコピー"]);
  LM.linkItems({ t: "page", path: "n/a.md", heading: "H" }, "[[a#H]]", c)[1].run();
  assert.deepEqual(got.at(-1), [{ kind: "page", path: "n/a.md", heading: "H" }, true], "見出しを落とした");
  // html ページ・画像も自分のタブ = 開く / 外部のアプリで開く（root = 描画の root = 参照フォルダならそのフォルダ）
  for (const p of ["s/p.html", "img/a.png"]) {
    const it = LM.linkItems({ t: "page", path: p, heading: null }, p, c);
    assert.deepEqual(labels(it), ["開く", "外部のアプリで開く", "—", "リンクをコピー"], p);
    it[1].run();
    assert.deepEqual(got.at(-1), [{ kind: "open", abs: `/ref/${p}` }, false]);
  }
  const ref = LM.linkItems({ t: "ref", abs: "/r/x.pdf" }, "file:///r/x.pdf", c);
  assert.deepEqual(labels(ref), ["開く", "外部のアプリで開く", "—", "リンクをコピー"]);
  ref[0].run();
  assert.deepEqual(got.at(-1), [{ kind: "ref", abs: "/r/x.pdf" }, false]);
  assert.deepEqual(labels(LM.linkItems({ t: "open", abs: "/r/x.key" }, "file:///r/x.key", c)), ["外部のアプリで開く", "—", "リンクをコピー"]);
  const item = LM.linkItems({ t: "item", key: "K1", page: 3, att: null }, "openref://item/K1?page=3", c);
  assert.deepEqual(labels(item), ["MY LIBRARY に表示", "—", "リンクをコピー"]);
  item[0].run();
  assert.deepEqual(got.at(-1), [{ kind: "item", key: "K1", page: 3 }, false]);
  assert.deepEqual(labels(LM.linkItems({ t: "none" }, "", c)), [], "コピーする物の無いリンクに空でないメニュー");
  // 書かれたままが空 = コピーを出さない（区切りも）
  assert.deepEqual(labels(LM.linkItems({ t: "ext", url: "https://e" }, "", c)), ["ブラウザで開く"]);
  tests++;
}

console.log(`wiki nav: ok (${tests} tests)`);
process.exit(0);
