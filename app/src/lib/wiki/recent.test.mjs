/* OpenNote の最近開いたページの部品テスト（node app/src/lib/wiki/recent.test.mjs・実部品 recent.svelte.ts / wiki.svelte.ts /
   WikiSearch / WikiResults / WikiPage + jsdom・invoke は差し替え・設定は偽の保存先 = set_ui_pref / get_ui_pref の往復）。確かめること:
   ① 設定の値の読み（parseRecent）: 壊れた JSON・配列・形の違う値は空 / 捨てる・重複は 1 つ・上限 50
   ② 設定を読めなかった間は記録しない（空の記録で全ワークスペースの分を上書きしない）
   ③ 記録（initRecent = 前面の OpenNote タブで読めたページ）: 新しい順・既に頭なら書かない・上限 50・参照フォルダのファイル / メディア /
      前面のタブと違うページは入れない・保存の値は `{root: [path, …]}`・壊れた JSON が入っていても落ちず、次の記録で正しい形に書き直す
   ④ 書くときに、ワークスペースの履歴から外した root の分を捨てる（画面の root と履歴にある root は残す）
   ⑤ 出す一覧: 画面のワークスペースの分だけ・今の木に無いページは出さない（記録からは消さない）・前面のタブの今のページは除く・10 件・
      ワークスペースが変われば別の一覧
   ⑥ 画面（実部品 WikiSearch）: 検索欄に入って何も打っていない = 欄の真下に重ねる一覧・打つと消える・× で外す（書く）・一覧の押下は欄の
      フォーカスを奪わない・行のクリック / ↓ + Enter で開いて一覧を畳む・⌘クリック = 新しいタブ・Esc で畳む・IME の変換中は出し入れを変えない
   ⑦ 旗は必ず下ろす: 欄にいたまま欄が消えた（OpenNote 以外のタブへ = blur が来ない）→ 戻っても一覧は出ていない・前面のタブの変化で下ろす
   ⑧ 実部品 WikiPage: 欄に入っても本文とエディタを隠さない（エディタを作り直さない = undo と caret が残る）・IME で打っても本文 → 結果の 1 回だけ・
      検索の結果一覧から本文へ戻ると読んでいた位置へ戻る・編集 → 表示 / 結果一覧 → 編集 → 表示では戻さない（今どおり先頭）
   ⑨ 覆い（モーダル・参照エディタ）の下では出さない・↑↓ / Enter も効かない・欄と一覧の外の押下で畳んで欄から出る・重なりはモーダルの覆いより下・
      変換中に欄を出たら凍結も解く・語を打っている間のタブの切替では欄のフォーカスを外さない
   ⑩ ホームの中央（実部品 WikiPage）: 最近開いたページ（同じ記録・今の木に在る物だけ = 消えた / 改名 / ゴミ箱のページは出ない・10 件）・
      押す = 開く・⌘クリック / 中クリック / ⌘Enter = 新しいタブ・0 件の案内・ワークスペースの切替で一覧が替わる・[＋ 新規ページ…] = [＋ 新規] と同じ窓・
      覆いの下では行も新規ページも効かない・右ペインと重なる root / ページ数 / ingest/ の素材は中央に出さない・
      「0 件」と言うのは記録と木の両方を読めた後で本当に 0 件のときだけ（記録の読み込み中・木の読み込み中 / 作り直し中・別の root の木 = 「読込中…」・
      木を読めなかった = 何も言わない・記録を読めなかった = 「読めませんでした」）・ホームを出すだけでは記録を書かない・素の Enter は奪わない
   （② では、読めなかったとき一覧に「読めませんでした」と出ることも確かめる）
   出力 = `wiki recent: ok (N tests)` */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { setupHarness } from "./test-harness.mjs";

const { calls, handlers, sleep, waitFor } = setupHarness();
let tests = 0;

const { mount, unmount, flushSync } = await import("svelte");
const S = await import("../state.svelte.ts");
const W = await import("./wiki.svelte.ts");
const R = await import("./recent.svelte.ts");

// 偽の設定の保存先
const prefs = { ui_wiki_recent: "{broken json" };
handlers.get_ui_pref = (a) => prefs[a.key] ?? null;
handlers.set_ui_pref = (a) => {
  prefs[a.key] = a.value;
  return null;
};
const writes = () => calls.filter((c) => c.name === "set_ui_pref" && c.args.key === "ui_wiki_recent").length;
const stored = () => JSON.parse(prefs.ui_wiki_recent);
handlers.wiki_list = () => ({ total: 0, count: 0, pages: [] });
handlers.wiki_tags = () => ({ tags: [], total: 0 });
handlers.wiki_inbox = () => ({ files: [], total: 0 });
handlers.wiki_chat_log = () => ({ pages: [], total: 0 });
handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
handlers.list_items = () => [];

const page = (path, kind = "md", extra = {}) => ({ path, abs_path: "/v/" + path, kind, title: path, size: 1, mtime: 0, content: kind === "media" ? null : `# ${path}\n`, sha256: path, truncated: false, links: [], ...extra });
const entry = (path, title = path.replace(/\.md$/, "").toUpperCase()) => ({ path, kind: "md", title, tags: [], items: [], mtime: 0, size: 1 });
const tab = S.app.tabs[0];
S.app.wikiOn = true;
S.app.tab = tab.id;
W.wk.status = { enabled: true, root: "/v", workspaces: [{ root: "/v", last_used: 2, exists: true, current: true }, { root: "/w", last_used: 1, exists: true, current: false }] };
const settle = async () => {
  for (let i = 0; i < 5; i++) {
    await sleep(0);
    flushSync();
  }
};

// ---------- ① parseRecent ----------
{
  for (const bad of [null, "", "{broken", "[]", "[1,2]", "1", '"x"', "null", '{"/v": "a.md"}', '{"/v": 3}']) assert.deepEqual(R.parseRecent(bad), {}, `空として読まない: ${bad}`);
  assert.deepEqual(R.parseRecent('{"/v": ["a.md", 3, "", null, "b.md", "a.md"], "": ["x"], "/w": []}'), { "/v": ["a.md", "b.md"] }, "形の違う値を捨てない");
  const many = Array.from({ length: 80 }, (_, i) => `p${i}.md`);
  assert.equal(R.parseRecent(JSON.stringify({ "/v": many }))["/v"].length, 50, "上限で切らない");
  tests++;
}

const { default: Search } = await import("./WikiSearch.svelte");

// ---------- ② 設定を読めなかった間は記録しない ----------
{
  const keep = JSON.stringify({ "/v": ["keep.md"], "/w": ["w1.md"] });
  prefs.ui_wiki_recent = keep;
  const get = handlers.get_ui_pref;
  const warn = console.warn;
  console.warn = () => {};
  handlers.get_ui_pref = () => {
    throw new Error("database is locked");
  };
  await R.recordVisit("/v", "x.md");
  await R.removeRecent("/v", "keep.md");
  await sleep(10);
  assert.equal(writes(), 0, "読めなかったのに書いた");
  assert.equal(prefs.ui_wiki_recent, keep, "読めなかったのに記録を上書きした");
  // 一覧 = 「読めませんでした」（「ありません」と区別する）
  const st = document.createElement("div");
  document.body.append(st);
  const sc = mount(Search, { target: st });
  st.querySelector("#wk-search").focus();
  await settle();
  assert.match(st.querySelector(".wk-recent-pop")?.textContent ?? "", /最近開いたページを読めませんでした/, "読めなかったことを出さない");
  st.querySelector("#wk-search").blur();
  unmount(sc);
  st.remove();
  handlers.get_ui_pref = get;
  console.warn = warn;
  prefs.ui_wiki_recent = "{broken json";
  tests++;
}

// ---------- ②b ホームの「読込中…」→「0 件」/ 行 を本物の読み込みの道で ----------
// ⑩ は木と記録を読めた印（wk.treeRoot・recent.loaded）を試験の中で直に入れる = 印を立てる側（loadTree・loadRecent）が壊れても通る。
// ここは印に触らず、wiki_list と get_ui_pref の応答だけでホームを進める（② で記録の読みは失敗した = まだ読めていない所から）
{
  const { default: WikiPage } = await import("./WikiPage.svelte");
  assert.equal(R.recent.loaded, false, "前提: 記録はまだ読めていない（② は読めなかった）");
  assert.equal(W.wk.treeRoot, null, "前提: 木はまだ読めていない");
  const listWas = handlers.wiki_list;
  const pagesWas = W.wk.pages;
  let release = () => {};
  handlers.wiki_list = () => new Promise((r) => (release = () => r({ total: 1, count: 1, pages: [entry("a.md")] })));
  tab.wiki = { path: null, root: "/v", scrollTop: 0, editing: false };
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  await settle();
  const home = () => target.querySelector(".wk-home");
  const rows = () => [...target.querySelectorAll(".wk-home-recent")];
  const said = () => {
    const tx = home().textContent;
    return tx.includes("最近開いたページはありません") ? "0件" : tx.includes("読めませんでした") ? "読めない" : tx.includes("読込中") ? "読込中" : rows().length ? "行" : "無言";
  };
  assert.equal(said(), "読込中", "木を読む前に「読込中…」と言わない");
  const tree = W.loadTree();
  await settle();
  assert.equal(said(), "読込中", "木の読み込み中に「読込中…」と言わない");
  release();
  await tree;
  await settle();
  // 記録（壊れた JSON = 空）はホームの effect の loadRecent が読み、木は loadTree が読んだ = 両方の印が立って本当の 0 件
  assert.equal(said(), "0件", `木（loadTree）と記録（loadRecent）を読めたのに「0 件」と言わない（${said()}・treeRoot=${W.wk.treeRoot}・loaded=${R.recent.loaded}）`);
  R.recent.map = { "/v": ["a.md"] };
  await settle();
  assert.equal(rows().length, 1, "読み込みの後、今の木に在る記録の行が出ない");
  // L-b: 状態（wiki_status）を読めない = ヘッダーが警告を出す → 中央は「読込中…」と言わない
  R.recent.map = {};
  W.wk.treeRoot = null; // 木もまだ（起動直後に状態を読めなかった）
  W.wk.status = { ...W.wk.status, statusError: "database is locked" };
  await settle();
  assert.equal(said(), "無言", `状態を読めないのに中央が「${said()}」と言う（ヘッダーの警告と二重）`);
  assert.ok([...home().querySelectorAll("button")].some((b) => b.textContent.includes("新規ページ")), "状態を読めないとき [＋ 新規ページ…] が無い");
  const { statusError: _, ...okStatus } = W.wk.status;
  W.wk.status = okStatus;
  unmount(cmp);
  target.remove();
  tab.wiki = null;
  handlers.wiki_list = listWas;
  W.wk.pages = pagesWas;
  W.wk.treeRoot = null;
  R.recent.map = {};
  tests++;
}

// ---------- ③ 記録（壊れた JSON から書き直す） ----------
const stop = R.initRecent();
{
  const visit = async (path, kind = "md", extra = {}) => {
    tab.wiki = { path, root: "/v", scrollTop: 0, editing: false };
    W.wk.page = page(path, kind, extra);
    await settle();
  };
  await visit("a.md");
  assert.ok(await waitFor(() => writes() === 1), "開いたページを書かない");
  assert.deepEqual(stored(), { "/v": ["a.md"] }, "壊れた JSON を正しい形に書き直さない");
  await visit("b.md");
  await visit("c.md");
  await waitFor(() => writes() === 3);
  assert.deepEqual(stored()["/v"], ["c.md", "b.md", "a.md"], "新しい順でない");
  // 既に頭 = 書かない（保存のたびの wk.page の置き換え）
  W.wk.page = { ...page("c.md"), sha256: "new" };
  await settle();
  await sleep(10);
  assert.equal(writes(), 3, "既に頭なのに書いた");
  await visit("a.md");
  await waitFor(() => writes() === 4);
  assert.deepEqual(stored()["/v"], ["a.md", "c.md", "b.md"], "開き直したページを頭へ動かさない");
  // 入れない物: 参照フォルダのファイル・メディア・前面のタブと違うページ
  await visit("/refs/x.html", "html", { external: "/refs" });
  await visit("img/p.png", "media");
  tab.wiki = { path: "z.md", root: "/v", scrollTop: 0, editing: false };
  W.wk.page = page("other.md");
  await settle();
  await sleep(10);
  assert.equal(writes(), 4, "参照フォルダ / メディア / 前面と違うページを記録した");
  // html ページは入れる
  await visit("h.html", "html");
  await waitFor(() => writes() === 5);
  assert.equal(stored()["/v"][0], "h.html", "html ページを記録しない");
  // 上限 50
  for (let i = 0; i < 60; i++) await visit(`n${i}.md`);
  await waitFor(() => stored()["/v"].length === 50 && stored()["/v"][0] === "n59.md");
  assert.equal(stored()["/v"].length, 50, "上限で切らない");
  tests++;
}

// ---------- ④ 履歴から外した root の分を捨てる ----------
{
  R.recent.map = { ...R.recent.map, "/w": ["w1.md"], "/gone": ["g.md"] };
  tab.wiki = { path: "q.md", root: "/v", scrollTop: 0, editing: false };
  W.wk.page = page("q.md");
  await settle();
  await waitFor(() => stored()["/v"]?.[0] === "q.md");
  assert.deepEqual(Object.keys(stored()).sort(), ["/v", "/w"], "履歴から外した root の分を捨てない（履歴にある root を捨てた）");
  tests++;
}

// ---------- ⑤ 出す一覧 ----------
{
  R.recent.map = { "/v": ["c.md", "gone.md", "b.md", "a.md", ...Array.from({ length: 20 }, (_, i) => `m${i}.md`)], "/w": ["w1.md"] };
  W.wk.pages = [entry("a.md"), entry("b.md"), entry("c.md", ""), ...Array.from({ length: 20 }, (_, i) => entry(`m${i}.md`))];
  const list = R.recentList();
  assert.equal(list.length, 10, "10 件でない");
  assert.deepEqual(list.slice(0, 3), [{ path: "c.md", title: "c.md" }, { path: "b.md", title: "B" }, { path: "a.md", title: "A" }], "消えたページを出した / 題名が違う");
  assert.ok(R.recent.map["/v"].includes("gone.md"), "出さないページを記録から消した");
  // ワークスペースが変われば別の一覧
  const st = W.wk.status;
  W.wk.status = { ...st, root: "/w" };
  W.wk.pages = [entry("w1.md")];
  assert.deepEqual(R.recentList().map((r) => r.path), ["w1.md"], "別のワークスペースの一覧にならない");
  W.wk.status = st;
  // 前面のタブの今のページは除く（`/` → Enter で別のページへ）
  W.wk.pages = [entry("a.md"), entry("b.md"), entry("c.md")];
  R.recent.map = { "/v": ["b.md", "a.md", "c.md"] };
  tab.wiki = { path: "b.md", root: "/v", scrollTop: 0, editing: false };
  assert.deepEqual(R.recentList().map((r) => r.path), ["a.md", "c.md"], "今のページを出した");
  tests++;
}

// ---------- ⑥ 画面（WikiSearch の重ねる一覧） ----------
{
  W.wk.pages = [entry("a.md"), entry("b.md"), entry("c.md"), entry("z.md")];
  tab.wiki = { path: "z.md", root: "/v", scrollTop: 0, editing: false };
  W.wk.page = page("z.md");
  await settle();
  R.recent.map = { "/v": ["c.md", "b.md", "a.md", "z.md"] }; // 記録の effect が z を頭へ動かした分を戻す
  const st = document.createElement("div");
  document.body.append(st);
  const sc = mount(Search, { target: st });
  const input = st.querySelector("#wk-search");
  const pop = () => st.querySelector(".wk-recent-pop");
  const rows = () => [...st.querySelectorAll(".wk-recent")];
  const paths = () => rows().map((r) => r.querySelector(".wk-recent-path").textContent);
  assert.equal(pop(), null, "欄に入る前に出した");
  input.focus();
  await settle();
  assert.ok(pop(), "欄に入っても一覧を出さない");
  assert.deepEqual(rows().map((r) => r.querySelector(".wk-recent-title").textContent), ["C", "B", "A"], "一覧が違う（今のページ z を除く）");
  // × = 外す（書く）
  const before = writes();
  rows()[0].querySelector(".wk-recent-x").click();
  await settle();
  assert.deepEqual(paths(), ["b.md", "a.md"], "× で外さない");
  assert.ok(await waitFor(() => writes() === before + 1), "× を書かない");
  assert.deepEqual(stored()["/v"], ["b.md", "a.md", "z.md"]);
  // 一覧の押下は欄のフォーカスを奪わない
  const md = new MouseEvent("mousedown", { bubbles: true, cancelable: true });
  rows()[0].dispatchEvent(md);
  assert.ok(md.defaultPrevented, "一覧の押下でフォーカスが移る");
  // 打つと消える・消すと戻る
  handlers.wiki_search = () => ({ total: 0, pages: [] });
  W.wk.q = "x";
  await settle();
  assert.equal(pop(), null, "打っても一覧のまま");
  W.wk.q = "";
  await settle();
  assert.equal(rows().length, 2, "消すと一覧に戻らない");
  // IME の変換中は出し入れを変えない（bind:value は変換中も wk.q を書く）
  const seen = [];
  const watch = () => seen.push(pop() ? "recent" : "none");
  input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
  for (const v of ["か", "かん", "肝"]) {
    input.value = v;
    input.dispatchEvent(new InputEvent("input", { bubbles: true, isComposing: true, data: v, inputType: "insertCompositionText" }));
    await settle();
    watch();
  }
  assert.equal(W.wk.q, "肝", "変換中の値が欄に入らない（試験の前提）");
  input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "肝" }));
  await settle();
  watch();
  assert.deepEqual(seen, ["recent", "recent", "recent", "none"], "変換中に一覧の出し入れが変わった");
  W.wk.q = "";
  input.value = "";
  await sleep(250);
  await settle();
  assert.ok(pop(), "消しても一覧に戻らない");
  // ⌘クリック = 新しいタブ（一覧はそのまま）
  const tabs0 = S.app.tabs.length;
  rows()[1].dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, metaKey: true, button: 0 }));
  await settle();
  assert.equal(S.app.tabs.length, tabs0 + 1, "⌘クリックで新しいタブを開かない");
  assert.equal(S.app.tabs.at(-1).wiki?.path, "a.md");
  await S.activateTab(tab.id);
  await settle();
  input.focus();
  await settle();
  // ↓ + Enter = 2 行目を開いて一覧を畳む
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
  flushSync();
  assert.ok(rows()[1].classList.contains("wk-recent-sel"), "↓ で選ばない");
  const second = paths()[1];
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  await settle();
  assert.equal(tab.wiki.path, second, "↓ + Enter で 2 行目を開かない");
  assert.equal(pop(), null, "開いても一覧を畳まない");
  assert.notEqual(document.activeElement, input, "開いても欄にフォーカスが残る");
  // 行のクリック = 開く
  input.focus();
  await settle();
  const first = paths()[0];
  rows()[0].dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
  await settle();
  assert.equal(tab.wiki.path, first, "行のクリックで開かない");
  assert.equal(pop(), null);
  // Esc = 畳む
  input.focus();
  await settle();
  assert.ok(pop());
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  await settle();
  assert.equal(pop(), null, "Esc で畳まない");
  assert.equal(R.recent.focus, false);
  unmount(sc);
  st.remove();
  tests++;
}

// ---------- ⑦ 旗は必ず下ろす（欄が消えた・前面のタブの変化） ----------
{
  const st = document.createElement("div");
  document.body.append(st);
  let sc = mount(Search, { target: st });
  let blurs = 0;
  st.querySelector("#wk-search").addEventListener("blur", () => blurs++);
  st.querySelector("#wk-search").focus();
  await settle();
  assert.ok(st.querySelector(".wk-recent-pop"));
  // OpenNote 以外のタブへ = ヘッダーごと外れる（blur は来ない）
  unmount(sc);
  await settle();
  assert.equal(blurs, 0, "試験の前提: 外れても blur は来ない");
  assert.equal(R.recent.focus, false, "欄が消えても旗が残る");
  sc = mount(Search, { target: st });
  await settle();
  assert.equal(st.querySelector(".wk-recent-pop"), null, "戻ると一覧が出ている");
  // 前面のタブの変化
  st.querySelector("#wk-search").focus();
  await settle();
  assert.ok(R.recent.focus);
  const tab0 = S.app.tab;
  S.app.tab = tab0 + 1000;
  await settle();
  assert.equal(R.recent.focus, false, "前面のタブが変わっても旗が残る");
  assert.notEqual(document.activeElement, st.querySelector("#wk-search"), "前面のタブが変わっても欄にフォーカスが残る");
  S.app.tab = tab0;
  await settle();
  assert.equal(st.querySelector(".wk-recent-pop"), null);
  unmount(sc);
  st.remove();
  tests++;
}

// ---------- ⑧ WikiPage: 本文とエディタを隠さない・IME・結果一覧から戻ると位置へ ----------
{
  const files = { "a.md": "# A\n" + "本文\n\n".repeat(50) };
  handlers.wiki_read_gui = (a) => page(a.path, "md", { content: files[a.path] ?? "# x\n" });
  handlers.wiki_save = (a) => ({ ok: true, path: a.path, sha256: "S", size: a.content.length, mtime: 1, created: false });
  W.wk.page = null;
  W.wk.pages = [entry("a.md"), entry("b.md")];
  tab.wiki = { path: "a.md", root: "/v", scrollTop: 0, editing: false };
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const target = document.createElement("div");
  const st = document.createElement("div");
  document.body.append(st, target);
  const sc = mount(Search, { target: st });
  const cmp = mount(WikiPage, { target });
  assert.ok(await waitFor(() => W.wk.page?.path === "a.md" && target.querySelector(".wk-body .wk-md")), "ページを描かない");
  const input = st.querySelector("#wk-search");
  const view = () => (target.querySelector(".wk-res") ? "results" : target.querySelector(".wk-body") ? "page" : "?");
  // 欄に入っても本文のまま
  const body0 = target.querySelector(".wk-body");
  input.focus();
  await settle();
  assert.ok(st.querySelector(".wk-recent-pop"), "一覧が出ない");
  assert.equal(target.querySelector(".wk-body"), body0, "欄に入ると本文を作り直した / 隠した");
  // IME で打つ: 本文 → 結果の 1 回だけ（最近 ↔ 本文の行き来をしない）
  const seen = [view()];
  input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
  for (const v of ["か", "かん"]) {
    input.value = v;
    input.dispatchEvent(new InputEvent("input", { bubbles: true, isComposing: true, data: v, inputType: "insertCompositionText" }));
    await settle();
    seen.push(view());
  }
  input.value = "肝";
  input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "肝" }));
  W.wk.q = "肝";
  assert.ok(await waitFor(() => view() === "results"), "確定しても結果を出さない");
  seen.push(view());
  assert.deepEqual(seen, ["page", "page", "page", "results"], "IME の変換中に画面が行き来した");
  // 結果一覧から本文へ戻る = 読んでいた位置へ（タブの scrollTop）
  const proto = HTMLElement.prototype;
  Object.defineProperty(proto, "scrollTop", { configurable: true, get() { return this.__st ?? 0; }, set(v) { this.__st = v; } });
  try {
    tab.wiki.scrollTop = 240;
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await settle();
    const body = target.querySelector(".wk-body");
    assert.ok(body, "Esc で本文に戻らない");
    assert.ok(await waitFor(() => body.scrollTop === 240), `読んでいた位置へ戻らない（${body.scrollTop}）`);
  } finally {
    delete proto.scrollTop;
  }
  // 編集中に欄に入ってもエディタを作り直さない（undo と caret が残る）
  tab.wiki.editing = true;
  assert.ok(await waitFor(() => target.querySelector(".cm-editor"), 3000), "エディタが出ない");
  const cm1 = target.querySelector(".cm-editor");
  const { EditorView } = await import("@codemirror/view");
  const { undoDepth } = await import("@codemirror/commands");
  const v1 = EditorView.findFromDOM(cm1);
  v1.dispatch({ changes: { from: v1.state.doc.length, insert: "\n追記" }, selection: { anchor: v1.state.doc.length + 3 }, userEvent: "input" });
  const before = { undo: undoDepth(v1.state), head: v1.state.selection.main.head };
  input.focus();
  await settle();
  assert.ok(st.querySelector(".wk-recent-pop"), "編集中に一覧が出ない");
  assert.equal(target.querySelector(".cm-editor"), cm1, "欄に入るとエディタが外れた");
  input.blur();
  await settle();
  assert.equal(target.querySelector(".cm-editor"), cm1, "欄を出るとエディタを作り直した");
  assert.deepEqual({ undo: undoDepth(v1.state), head: v1.state.selection.main.head }, before, "undo / caret が消えた");
  // 編集 → 表示 = 今どおり先頭（結果一覧から戻ったときの位置の戻しを効かせない）
  Object.defineProperty(proto, "scrollTop", { configurable: true, get() { return this.__st ?? 0; }, set(v) { this.__st = v; } });
  try {
    tab.wiki.scrollTop = 240;
    tab.wiki.editing = false;
    assert.ok(await waitFor(() => target.querySelector(".wk-body")), "表示に戻らない");
    await sleep(30);
    await settle();
    assert.equal(target.querySelector(".wk-body").scrollTop, 0, "編集 → 表示で編集前の位置へ戻した");
    // 結果一覧 → 編集 → 表示 = 戻さない（結果一覧から編集へ戻った時点で旗を下ろす）
    tab.wiki.editing = true;
    assert.ok(await waitFor(() => target.querySelector(".cm-editor"), 3000));
    W.wk.q = "肝";
    W.wk.showResults = true;
    assert.ok(await waitFor(() => target.querySelector(".wk-res")), "結果一覧が出ない");
    W.wk.q = "";
    W.wk.showResults = false;
    assert.ok(await waitFor(() => target.querySelector(".cm-editor") && !target.querySelector(".wk-res")), "結果一覧から編集へ戻らない");
    await settle();
    tab.wiki.scrollTop = 240;
    tab.wiki.editing = false;
    assert.ok(await waitFor(() => target.querySelector(".wk-body")), "表示に戻らない");
    await sleep(30);
    await settle();
    assert.equal(target.querySelector(".wk-body").scrollTop, 0, "結果一覧 → 編集 → 表示で位置を戻した");
  } finally {
    delete proto.scrollTop;
  }
  unmount(cmp);
  unmount(sc);
  target.remove();
  st.remove();
  tests++;
}

// ---------- ⑨ 覆いの下・外側の押下・重なり・変換中に出る・語を打っている間のタブの切替 ----------
{
  W.wk.pages = [entry("a.md"), entry("b.md"), entry("c.md")];
  R.recent.map = { "/v": ["b.md", "c.md"] };
  tab.wiki = { path: "a.md", root: "/v", scrollTop: 0, editing: false };
  S.app.tab = tab.id;
  await settle();
  const st = document.createElement("div");
  const other = document.createElement("button");
  document.body.append(st, other);
  const sc = mount(Search, { target: st });
  const input = st.querySelector("#wk-search");
  const pop = () => st.querySelector(".wk-recent-pop");
  const key = (k) => input.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
  input.focus();
  await settle();
  assert.ok(pop());
  // 参照エディタ（初期フォーカスを取らない = 欄にフォーカスが残る）・モーダルの下では出さない・↑↓ / Enter も効かない
  for (const [k, v] of [["editorOpen", true], ["modal", "tags"], ["importJob", { id: 1 }]]) {
    const was = S.app[k];
    S.app[k] = v;
    await settle();
    assert.equal(pop(), null, `覆い（${k}）の下で一覧を出した`);
    key("ArrowDown");
    key("Enter");
    await settle();
    assert.equal(tab.wiki.path, "a.md", `覆い（${k}）の下で Enter が背後のページを替えた`);
    S.app[k] = was;
    await settle();
    assert.ok(pop(), `覆い（${k}）を閉じても一覧に戻らない`);
  }
  // 欄と一覧の外の押下（WKWebView はボタンでフォーカスを移さない）= 畳んで欄から出る
  other.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
  await settle();
  assert.equal(pop(), null, "外側の押下で畳まない");
  assert.equal(R.recent.focus, false);
  assert.notEqual(document.activeElement, input, "外側の押下で欄から出ない");
  // 重なりはモーダルの覆い（app.css の .overlay）より下
  const css = readFileSync(new URL("../../app.css", import.meta.url), "utf8");
  const src = readFileSync(new URL("./WikiSearch.svelte", import.meta.url), "utf8");
  const zo = Number(/\.overlay \{[^}]*z-index: (\d+)/.exec(css)[1]);
  const zp = Number(/\.wk-recent-pop \{[^}]*z-index: (\d+)/.exec(src)[1]);
  assert.ok(zp < zo, `一覧（${zp}）がモーダルの覆い（${zo}）より上`);
  // 変換中に欄を出た（compositionend が来ない）= 凍結も解く
  input.focus();
  await settle();
  input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
  await settle();
  input.blur();
  await settle();
  assert.equal(R.recent.frozen, null, "欄を出ても凍結が残る");
  assert.equal(pop(), null, "変換中に欄を出ても一覧が残る");
  // 語を打っている間（一覧なし）のタブの切替では欄のフォーカスを外さない
  input.focus();
  W.wk.q = "abc";
  await settle();
  assert.equal(pop(), null);
  const tab0 = S.app.tab;
  S.app.tab = tab0 + 1000;
  await settle();
  assert.equal(document.activeElement, input, "語を打っている間のタブの切替で欄から出た");
  S.app.tab = tab0;
  W.wk.q = "";
  input.blur();
  await settle();
  unmount(sc);
  st.remove();
  other.remove();
  tests++;
}

// ---------- ⑩ ホームの中央（実部品 WikiPage） ----------
{
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const st = W.wk.status;
  W.wk.status = { ...st, root: "/v", pages: 23, inbox: 2 };
  W.wk.pages = [entry("a.md"), entry("b.md"), entry("c.md", ""), ...Array.from({ length: 20 }, (_, i) => entry(`m${i}.md`))];
  R.recent.map = { "/v": ["c.md", "gone.md", "b.md", "a.md", ...Array.from({ length: 20 }, (_, i) => `m${i}.md`)], "/w": ["w1.md"] };
  W.wk.treeRoot = "/v"; // 木はこのワークスペースの物で読めている（下の「0 件」の節で崩す）
  R.recent.loaded = true;
  tab.wiki = { path: null, root: "/v", scrollTop: 0, editing: false };
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  await settle();
  const home = () => target.querySelector(".wk-home");
  const rows = () => [...target.querySelectorAll(".wk-home-recent")];
  const titles = () => rows().map((b) => b.querySelector(".wk-home-title").textContent);
  assert.ok(home(), "ホームが出ない");
  // 最近開いたページ = 新しい順・今の木に無い物（gone.md）は出さない・題名が空なら名前・10 件
  assert.deepEqual(titles(), ["c.md", "B", "A", "M0", "M1", "M2", "M3", "M4", "M5", "M6"], "ホームの最近開いたページ");
  assert.equal(rows()[1].querySelector(".wk-home-path").textContent, "b.md", "行にパスが無い");
  // 右ペインと重なる情報は中央に出さない
  const text = home().textContent;
  assert.ok(!text.includes("ページ数") && !text.includes("ingest/ の素材") && !text.includes("/v"), `中央に右ペインと同じ情報が残っている: ${text}`);
  // 押す = 開く
  const opens = [];
  const openWas = handlers.wiki_read_gui;
  handlers.wiki_read_gui = (a) => (opens.push(a.path), page(a.path));
  const tabs0 = S.app.tabs.length;
  rows()[1].dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
  assert.ok(await waitFor(() => tab.wiki.path === "b.md", 2000), "行を押しても開かない");
  assert.equal(S.app.tabs.length, tabs0, "素の押下で新しいタブを開いた");
  tab.wiki = { path: null, root: "/v", scrollTop: 0, editing: false };
  await settle();
  assert.equal(titles()[0], "B", "開いたページが一覧の頭へ来ない（同じ記録 = 検索欄の一覧と同じ）");
  // ⌘クリック / 中クリック / ⌘Enter = 新しいタブ（今のタブはホームのまま）
  for (const [label, fire] of [
    ["⌘クリック", (b) => b.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0, metaKey: true }))],
    ["中クリック", (b) => b.dispatchEvent(new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 }))],
    ["⌘Enter", (b) => b.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true, cancelable: true }))],
  ]) {
    const n = S.app.tabs.length;
    const front = S.app.tab;
    const p0 = rows()[0].getAttribute("title"); // 押す行のパス（直前に開いたページが頭へ来る = 記録の順）
    fire(rows()[0]);
    assert.ok(await waitFor(() => S.app.tabs.length === n + 1, 2000), `${label} で新しいタブを開かない`);
    assert.equal(S.app.tabs.at(-1).wiki?.path, p0, `${label} の新しいタブのページ`);
    await S.activateTab(front);
    await settle();
    assert.equal(tab.wiki.path, null, `${label} で今のタブまで移った`);
  }
  // 覆いの下では行も [＋ 新規ページ…] も効かない
  S.app.importJob = { files: [], running: false, cancel: false };
  await settle();
  const n1 = S.app.tabs.length;
  rows()[0].dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0, metaKey: true }));
  rows()[0].dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
  [...home().querySelectorAll("button")].find((b) => b.textContent.includes("新規ページ")).click();
  await settle();
  assert.deepEqual([S.app.tabs.length, tab.wiki.path, W.wk.dialog], [n1, null, null], "覆いの下でホームの行 / 新規ページが効いた");
  S.app.importJob = null;
  await settle();
  // [＋ 新規ページ…] = [＋ 新規] の「新規ページ…」と同じ窓（作る場所 = ワークスペースの直下）
  [...home().querySelectorAll("button")].find((b) => b.textContent.includes("新規ページ")).click();
  await settle();
  assert.deepEqual(W.wk.dialog, { kind: "new", dir: "" }, "[＋ 新規ページ…] が新規ページの窓を開かない");
  W.wk.dialog = null;
  await settle();
  // ワークスペースの切替 = その一覧に替わる（ワークスペースごと）
  W.wk.status = { ...st, root: "/w" };
  W.wk.pages = [entry("w1.md")];
  W.wk.treeRoot = "/w";
  tab.wiki = { path: null, root: "/w", scrollTop: 0, editing: false };
  await settle();
  assert.deepEqual(titles(), ["W1"], "ワークスペースを切り替えても一覧が替わらない");
  // 0 件 = 案内（記録が無い・今の木に在る物が無い）+ 新規ページの入口は残る
  R.recent.map = { "/v": ["gone.md"] };
  W.wk.status = { ...st, root: "/v" };
  W.wk.pages = [entry("a.md")];
  W.wk.treeRoot = "/v";
  tab.wiki = { path: null, root: "/v", scrollTop: 0, editing: false };
  await settle();
  assert.equal(rows().length, 0, "0 件なのに行が出た");
  assert.ok(home().textContent.includes("最近開いたページはありません") && home().textContent.includes("左の木からページを選ぶか"), `0 件の案内が無い: ${home().textContent}`);
  assert.ok([...home().querySelectorAll("button")].some((b) => b.textContent.includes("新規ページ")), "0 件のとき新規ページの入口が無い");
  // ---- 「0 件」と言うのは、記録と木の両方を読めた後で本当に 0 件のときだけ ----
  const said = () => {
    const tx = home().textContent;
    return tx.includes("最近開いたページはありません") ? "0件" : tx.includes("読めませんでした") ? "読めない" : tx.includes("読込中") ? "読込中" : rows().length ? "行" : "無言";
  };
  const reset = () => {
    W.wk.treeLoading = false;
    W.wk.treeError = null;
    W.wk.status = { ...st, root: "/v" };
    W.wk.treeRoot = "/v";
    R.recent.loaded = true;
    R.recent.readError = null;
  };
  R.recent.map = { "/v": ["gone.md"] }; // 木が読めれば本当に 0 件
  reset();
  await settle();
  assert.equal(said(), "0件", "前提 = 記録と木を読めた本当の 0 件で「0 件」と言わない");
  for (const [label, make, want] of [
    ["木をまだ読んでいない（起動直後）", () => (W.wk.treeRoot = null), "読込中"],
    ["木の読み込み中", () => (W.wk.treeLoading = true), "読込中"],
    ["索引の作り直し中", () => (W.wk.status = { ...st, root: "/v", rebuilding: { percent: 10 } }), "読込中"],
    ["別のワークスペースの木（切替の直後）", () => (W.wk.treeRoot = "/w"), "読込中"],
    ["記録の読み込み中", () => (R.recent.loaded = false), "読込中"],
    ["木を読めなかった", () => ((W.wk.treeRoot = null), (W.wk.treeError = "読めない")), "無言"],
    ["記録を読めなかった", () => ((R.recent.loaded = false), (R.recent.readError = "読めない")), "読めない"],
  ]) {
    reset();
    make();
    await settle();
    assert.equal(said(), want, `${label}: ホームが「${want}」でない（${said()}）`);
    assert.ok([...home().querySelectorAll("button")].some((b) => b.textContent.includes("新規ページ")), `${label}: [＋ 新規ページ…] が無い（[＋ 新規] と同じく押せる）`);
  }
  // 別のワークスペースの木のまま、その木の行を出さない（同じパスが別の root にあっても）
  reset();
  R.recent.map = { "/v": ["a.md"] };
  W.wk.treeRoot = "/w";
  await settle();
  assert.equal(rows().length, 0, "別のワークスペースの木で、今のワークスペースの記録の行を出した");
  reset();
  await settle();
  assert.equal(rows().length, 1);
  // ホームを出すだけでは記録を書かない（出し直しても）
  const w0 = writes();
  for (let i = 0; i < 3; i++) {
    tab.wiki = { path: "a.md", root: "/v", scrollTop: 0, editing: false };
    await settle();
    tab.wiki = { path: null, root: "/v", scrollTop: 0, editing: false };
    await settle();
  }
  const visits = writes() - w0; // a.md を開いた記録（頭なら書かない）だけ
  assert.ok(visits <= 1, `ホームを出すたびに ui_wiki_recent を書いた（${visits} 回）`);
  // 覆いの下では ⌘Enter も効かない
  R.recent.map = { "/v": ["b.md", "a.md"] };
  await settle();
  S.app.importJob = { files: [], running: false, cancel: false };
  await settle();
  const n2 = S.app.tabs.length;
  const evCmdEnter = new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true, cancelable: true });
  rows()[0].dispatchEvent(evCmdEnter);
  await settle();
  assert.equal(S.app.tabs.length, n2, "覆いの下で ⌘Enter が新しいタブを開いた");
  // 新しいタブは openTab も覆いの間は断る（守りが 2 重）= 挙動では区別できない → ホームの側が覆いの下のキーを奪わない（ページのキーの門と同じ素通し）ことを見る
  assert.equal(evCmdEnter.defaultPrevented, false, "覆いの下で ⌘Enter を奪った（ホームの側の門を通っていない）");
  S.app.importJob = null;
  await settle();
  // 素の Enter は奪わない（ボタンの既定の click = 開く に任せる。jsdom は Enter から click を起こさない = keydown の扱いを直接見る）
  const evEnter = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
  rows()[0].dispatchEvent(evEnter);
  await settle();
  assert.equal(evEnter.defaultPrevented, false, "素の Enter を奪った（ボタンの click = 開くが起きない）");
  assert.equal(S.app.tabs.length, n2, "素の Enter で新しいタブを開いた");
  handlers.wiki_read_gui = openWas;
  W.wk.status = st;
  unmount(cmp);
  target.remove();
  tab.wiki = null;
  tests++;
}

stop();
console.log(`wiki recent: ok (${tests} tests)`);
process.exit(0);
