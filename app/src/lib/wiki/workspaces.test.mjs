/* OpenNote のワークスペースの部品テスト（node app/src/lib/wiki/workspaces.test.mjs・実部品 state.svelte.ts / wiki.svelte.ts / WikiInfo / WikiDialogs /
   WikiSidebar / TabStrip + jsdom）。履歴からの切替 + タブごとのワークスペース。
   Rust の command（wiki_status {root?} / wiki_front_root / wiki_switch_root / wiki_pick_root / wiki_forget_root ほか）は契約の形で偽物（be）が返す。
   偽物は履歴に無い args.root を unknown_root で拒む。確かめること:
   ① タブの行き来（A のページ ⇄ B のホーム）: どちらもホームに戻らない・木 / 箱 / 参照フォルダが各 root・読みは全部その root（args.root）・
      設定は前面に揃う（wiki_front_root・明示の切替は呼ばない = last_used を変えない）・戻ると A の写しを即座に描いて読み直しで追いつく・題名は写しから
   ② 明示の切替（メニュー / ホームの [開く] = wiki_switch_root・別のフォルダを選ぶ… = wiki_pick_root）は前面のタブだけ = 戻り値の root のホームへ・
      他のタブは触らない・front_root は呼ばない・前面のタブの root なら何もしない・取り消し / 失敗は何もしない（console.warn だけ）・未保存は先に保存
   ③ 状態の読み直し（wiki-state）は画面の root で読み、status の root から切替を推測しない・✓ = 画面の root・
      wiki-changed の root が画面の root と違えば捨てる（root の無い payload は受ける）
   ④ 写しの退避 / 復元（全項目・以前の leaveRoot が捨てていた項目との突き合わせ）・初めての root は空・前面のページと前の root を指すものは捨てる・
      表示の好みは共通・戻ったら読み直す（木・状態・参照フォルダ・検索）・前面でない root のタブの題名は写しから
   ⑤ wiki_front_root は直列（連打で重ならない・追い越された揃えは呼ばない・設定は最後の前面）・ページの門（rootReady）は最後の揃えまで開かない
   ⑥ 起動: ui_tabs の root の往復（{path, root}・無い / 壊れた root = null）・最初の状態は root 無しで読む・root の無いタブは設定の root を採る・
      履歴に無い root のタブは設定の root のホームへ + toast 1 回・前面の root が設定と違えば揃える（中身は保つ）・揃うまで門を開けない
   ⑦ 新しい OpenNote タブは前面の OpenNote タブの root を継ぐ（⌘T・⌘クリック）・前面が一覧なら最後の OpenNote の root（ItemDetail の節・セグメント）
   ⑧ 仮の状態のまま状態の読みが失敗 → 明示の切替は戻り値で前面のタブを替える（root の分からないタブはホーム = 新しい root で前の相対パスを開かない）
   ⑨ WikiInfo のホーム: ワークスペースの節（参照フォルダの上・件数・✓・[開く] / [外す]・無いフォルダは dim）・押下の command・
      [外す] はどれかのタブが使っている root では出さない（閉じれば出る）・forgetWorkspace も呼ばない
   ⑩ WikiDialogs の vault メニュー（見出し・履歴の行・✓ = disabled・無い = dim で disabled・「別のフォルダを選ぶ…」・履歴が無くても現在の行）・WikiSidebar の高さの見積もり
   ⑪ TabStrip の OpenNote タブの tooltip =「{path}\n{name} · {root}」・前面でない root のタブも題名が出る
   ⑫ 画面の root が履歴から外れた（上限で落ちた等 = wiki_status {root} が unknown_root）= 設定の root で読み直し・そのタブを設定の root のホームへ + toast・
      画面は設定の root（前の root の木を残さない）・他のタブは触らない・読み直しを繰り返さない
   ⑬ WikiPage（実部品）: 別の root のタブへ移ると、設定を揃え終える（wiki_front_root）まではページを読まない・揃ったらそのタブの root で読む・
      同じ相対パスでも root が違えば読み直す（root と path の組）・描画部品の root（asset URL の起点）= タブの root
   ⑭ まだ前面にしていない root（写しが無い）のタブの題名 = ファイル名（画面の root の同じ相対パスの題名を出さない）
   ⑮ wiki_front_root の失敗: 揃い終えるまで警告（status.frontError）を残す・状態の読み直しで消えない・frontRetry 後に揃え直す
      （予約は 1 本 = 連打しない・toast は 1 回）・揃え直せたら警告が消えて設定が前面の root・行き来で前の失敗を持ち越さない・
      失敗する root を 2 つ続けて前面にしても、揃え直すのは今の失敗の root（予約した root に縛られない）
   ⑯ 履歴にあるが今は無いフォルダ（exists: false = 未接続のディスク等）の root のタブは戻さない（root と path を保つ・ui_tabs にも残る）・
      前面にすると揃えが core の文（…見つかりません）で失敗し、ヘッダーの警告（⚠ ワークスペースを切り替えられません・title = core の文）と toast に出る
   ⑱ 起動: 前面の root = 設定でも wiki_front_root を 1 回通し、ページの読み（WikiPage）はその後
   ⑰ チャットの子が使っている root（wiki_chat_status.root）にも [外す] を出さない・forgetWorkspace も呼ばない
   出力 = `wiki workspaces: ok (N tests)`（N = 列 0 のブロックの数） */
import assert from "node:assert/strict";
import { setupHarness } from "./test-harness.mjs";

const { calls, handlers, emit, sleep, waitFor } = setupHarness();
let tests = 0;

const { mount, unmount, flushSync } = await import("svelte");
const S = await import("../state.svelte.ts");
const W = await import("./wiki.svelte.ts");
const E = await import("./edit.svelte.ts");

/** invoke の生の payload（harness の calls は payload.args ?? payload に均す = { args } で包んだかは見えない）。契約 = invoke(name, { args: {...} }) */
const raw = [];
const inner = globalThis.__invoke;
globalThis.__invoke = (name, payload) => (raw.push({ name, payload }), inner(name, payload));
const rawOf = (name) => raw.filter((r) => r.name === name).map((r) => r.payload);

const reject = (e) => () => {
  throw e;
};
const names = () => calls.map((c) => c.name);
/** console.warn を集める（失敗は toast せず console.warn だけ = pickRoot と同じ） */
const warns = [];
const origWarn = console.warn;
console.warn = (...a) => warns.push(a.map(String).join(" "));
const settle = () => sleep(30);

// ---- Rust の偽物（設定 wiki_root = be.setting・履歴 = be.hist・root ごとの木 / 参照フォルダ）----
const be = {
  setting: "/ws/a",
  hist: ["/ws/a", "/ws/b", "/ws/c"],
  gone: [],
  trees: { "/ws/a": [["notes/x.md", "X の題名"], ["a-only.md", "A だけ"]], "/ws/b": [["b-only.md", "B だけ"]], "/ws/c": [] },
  refs: { "/ws/a": ["/ref-a"] },
  /** wiki_front_root の呼び出し（root の順） */
  front: [],
  /** true = wiki_front_root を待たせる（held の関数で 1 本ずつ終える） */
  holding: false,
  held: [],
  inflight: 0,
  maxInflight: 0,
  /** wiki_pick_root で選ぶフォルダ（null = 取り消し） */
  pick: null,
  /** wiki_front_root を失敗させる root */
  failFront: [],
};
const entry = ([path, title]) => ({ path, kind: "md", title, tags: [], items: [], mtime: 0, size: 1 });
const rootOf = (a) => a?.root ?? be.setting;
/** 履歴に無い args.root = unknown_root（core の CoreError::wiki の JSON 文字列） */
function known(root) {
  if (!be.hist.includes(root)) throw JSON.stringify({ code: "unknown_root", message: `履歴に無いワークスペースです: ${root}` });
  return root;
}
const workspaces = () => be.hist.map((x, i) => ({ root: x, last_used: 100 - i, exists: !be.gone.includes(x), current: x === be.setting }));
const statusHandler = (a) => {
  const r = known(rootOf(a));
  return { enabled: true, root: r, pages: (be.trees[r] ?? []).length, inbox: 0, workspaces: workspaces() };
};
const listHandler = (a) => ({ pages: (be.trees[known(rootOf(a))] ?? []).map(entry) });
const switchHandler = (a) => {
  be.setting = known(a.root);
  return { root: a.root, restart: false };
};
handlers.wiki_status = statusHandler;
handlers.wiki_list = listHandler;
handlers.wiki_tags = (a) => ({ tags: [{ tag: "t-" + W.workspaceName(rootOf(a)), count: 1 }], total: 1 });
handlers.wiki_inbox = () => ({ files: [], total: 0 });
handlers.wiki_ref_dirs_get = (a) => ({ dirs: be.refs[rootOf(a)] ?? [] });
handlers.wiki_search = () => ({ pages: [], total: 0 });
handlers.wiki_front_root = (a) => {
  known(a.root);
  be.front.push(a.root);
  // 消えたフォルダ・失敗させたい root = Rust の check_root が拒む（設定は変えない）
  if (be.gone.includes(a.root)) throw `ワークスペースのフォルダに ${a.root} は使えません（見つかりません）`; // core の check_root の文
  if (be.failFront.includes(a.root)) throw `フォルダを使えません: ${a.root}`;
  be.inflight++;
  be.maxInflight = Math.max(be.maxInflight, be.inflight);
  const done = () => {
    be.inflight--;
    be.setting = a.root;
    return { root: a.root, restart: false };
  };
  if (!be.holding) return done();
  return new Promise((r) => be.held.push(() => r(done())));
};
handlers.wiki_switch_root = switchHandler;
handlers.wiki_pick_root = () => {
  if (!be.pick) return null;
  const r = be.pick;
  if (!be.hist.includes(r)) be.hist.unshift(r);
  be.setting = r;
  return { root: r, restart: false };
};
handlers.wiki_forget_root = (a) => {
  be.hist = be.hist.filter((x) => x !== a.root);
  return {};
};
handlers.wiki_settings = () => ({ enabled: true, root: be.setting });
handlers.list_items = () => [];
const prefs = {};
handlers.get_ui_pref = (a) => prefs[a.key] ?? null;
handlers.set_ui_pref = (a) => {
  prefs[a.key] = a.value;
  return null;
};

let nextId = 1000;
const mkTab = (wiki) => ({
  id: nextId++,
  view: { kind: "all" },
  search: "",
  filters: { author: null, journal: null },
  items: [],
  checked: [],
  checkAnchor: null,
  selected: null,
  detail: null,
  scrollTop: 0,
  viewer: null,
  wiki: wiki === null ? null : { path: wiki[0], root: wiki[1], scrollTop: 0, editing: false },
});
/** Wiki を OFF → ON（写しと仮の状態を初期化）・タブを specs（[path, root] / null = 一覧）で並べ・root 無しで状態を読んで画面の root を採る（起動と同じ）。
    戻り値 = 状態の側（proxy）のタブ */
async function reset(specs, front = 0, setting = "/ws/a") {
  handlers.wiki_status = () => ({ enabled: false, root: null });
  await W.loadWikiStatus();
  Object.assign(be, { setting, hist: ["/ws/a", "/ws/b", "/ws/c"], gone: [], front: [], holding: false, held: [], inflight: 0, maxInflight: 0, pick: null, failFront: [] });
  handlers.wiki_status = statusHandler;
  handlers.wiki_list = listHandler;
  handlers.wiki_switch_root = switchHandler;
  S.app.wikiOn = true;
  const tabs = specs.map(mkTab);
  S.app.tabs.splice(0, S.app.tabs.length, ...tabs);
  S.app.tab = tabs[front].id;
  await W.loadWikiStatus();
  await settle();
  calls.length = 0;
  raw.length = 0;
  be.front = []; // 起動の揃え（設定と同じ root でも 1 回）は各節の数に入れない
  return tabs.map((x) => S.app.tabs.find((y) => y.id === x.id));
}
const page = (path, content, sha) => ({ path, abs_path: "/ws/a/" + path, kind: "md", title: path, size: content.length, mtime: 0, content, sha256: sha, truncated: false });
const HOME = (root) => ({ path: null, root, scrollTop: 0, editing: false });

// ---- ① タブの行き来 = どちらもホームに戻らない・各 root の木・設定が前面に追従・写しを即座に描く ----
{
  const [t1, t2] = await reset([["notes/x.md", "/ws/a"], [null, "/ws/b"]], 0);
  assert.equal(W.shownRoot(), "/ws/a");
  assert.deepEqual(W.wk.pages.map((p) => p.path), ["notes/x.md", "a-only.md"]);
  await S.activateTab(t2.id);
  assert.equal(W.shownRoot(), "/ws/b", "B のタブへ移っても画面の root が B にならない");
  assert.deepEqual([t1.wiki.path, t1.wiki.root], ["notes/x.md", "/ws/a"], "行き来で A のタブがホームに戻った");
  assert.deepEqual([t2.wiki.path, t2.wiki.root], [null, "/ws/b"]);
  assert.ok(await waitFor(() => W.wk.pages.length === 1 && W.wk.pages[0].path === "b-only.md"), "B の木を読まない");
  await settle();
  assert.deepEqual(be.front, ["/ws/b"], "設定を前面（B）に揃えない");
  assert.equal(be.setting, "/ws/b");
  assert.deepEqual(rawOf("wiki_front_root"), [{ args: { root: "/ws/b" } }], "wiki_front_root を args で包んでいない");
  assert.ok(!names().some((n) => n === "wiki_switch_root" || n === "wiki_pick_root"), "行き来で明示の切替を呼んだ（last_used が変わる）");
  for (const c of calls.filter((c) => ["wiki_list", "wiki_tags", "wiki_inbox", "wiki_status", "wiki_ref_dirs_get"].includes(c.name))) {
    assert.equal(c.args?.root, "/ws/b", `${c.name} を B の root で読まない: ${JSON.stringify(c.args)}`);
  }
  assert.deepEqual(W.wk.refDirs, [], "A の参照フォルダが B に残った");
  assert.deepEqual(W.wk.tags.map((x) => x.tag), ["t-b"]);
  // 前面でない root（A）のタブの題名は A の写しから
  assert.equal(W.wikiTabTitle(t1.wiki), "X の題名", "前面でない root のタブの題名が出ない");
  // 戻る = A の写しを即座に描く（読み直しを待たない）→ 読み直しで追いつく
  let releaseList = null;
  handlers.wiki_list = (a) => (rootOf(a) === "/ws/a" ? new Promise((r) => (releaseList = () => r(listHandler(a)))) : listHandler(a));
  calls.length = 0;
  await S.activateTab(t1.id);
  assert.equal(W.shownRoot(), "/ws/a");
  assert.deepEqual(W.wk.pages.map((p) => p.path), ["notes/x.md", "a-only.md"], "A の写しを戻さない（読み直しを待った）");
  assert.deepEqual(W.wk.refDirs, ["/ref-a"], "A の参照フォルダを戻さない");
  assert.deepEqual([t2.wiki.path, t2.wiki.root], [null, "/ws/b"], "行き来で B のタブを変えた");
  assert.ok(await waitFor(() => !!releaseList), "戻った A の木を読み直さない");
  assert.equal(calls.find((c) => c.name === "wiki_list").args.root, "/ws/a");
  releaseList();
  handlers.wiki_list = listHandler;
  await settle();
  assert.deepEqual(be.front, ["/ws/b", "/ws/a"]);
  assert.equal(be.setting, "/ws/a");
  // 前面が一覧のタブ = 画面の root は最後の OpenNote（A）のまま・揃えない
  S.app.tabs.push(mkTab(null));
  const lib = S.app.tabs.at(-1);
  await S.activateTab(lib.id);
  await settle();
  assert.equal(W.shownRoot(), "/ws/a", "一覧のタブで画面の root が変わった");
  assert.deepEqual(be.front, ["/ws/b", "/ws/a"], "一覧のタブで設定を揃えた");
  tests++;
}

// ---- ② 明示の切替は前面のタブだけ（戻り値の root）----
{
  const [t1, t2, t3] = await reset([["notes/x.md", "/ws/a"], ["a-only.md", "/ws/a"], [null, "/ws/b"]], 0);
  await W.switchWorkspace("/ws/c");
  assert.deepEqual(rawOf("wiki_switch_root"), [{ args: { root: "/ws/c" } }], "args で包んでいない");
  assert.deepEqual({ ...t1.wiki }, HOME("/ws/c"), "前面のタブが戻り値の root のホームにならない");
  assert.deepEqual([t2.wiki.path, t2.wiki.root], ["a-only.md", "/ws/a"], "他のタブ（同じ A）を触った");
  assert.deepEqual([t3.wiki.path, t3.wiki.root], [null, "/ws/b"], "他のタブ（B）を触った");
  assert.equal(W.shownRoot(), "/ws/c");
  await settle();
  assert.deepEqual(be.front, [], "明示の切替の後に wiki_front_root を呼んだ（Rust が済ませた）");
  assert.deepEqual(W.workspaceList().find((w) => w.current)?.root, "/ws/c");
  // 前面のタブの root と同じ・空 = 何もしない
  calls.length = 0;
  await W.switchWorkspace("/ws/c");
  await W.switchWorkspace("");
  assert.deepEqual(calls, [], "前面のタブの root で invoke した");
  // 戻り値の root を使う（引数でなく = Rust が canonical に直した root）
  handlers.wiki_switch_root = () => {
    be.setting = "/ws/b";
    return { root: "/ws/b", restart: false };
  };
  await W.switchWorkspace("/ws/b-alias");
  assert.equal(t1.wiki.root, "/ws/b", "戻り値の root を使わない");
  handlers.wiki_switch_root = switchHandler;
  // 別のフォルダを選ぶ…: 取り消し（null）は何もしない・選んだら前面のタブだけ戻り値の root のホームへ
  t1.wiki.path = "b-only.md";
  be.pick = null;
  await W.pickRoot();
  assert.deepEqual([t1.wiki.path, t1.wiki.root], ["b-only.md", "/ws/b"], "取り消しで変わった");
  be.pick = "/ws/new";
  be.trees["/ws/new"] = [["n.md", "N"]];
  await W.pickRoot();
  assert.deepEqual({ ...t1.wiki }, HOME("/ws/new"));
  assert.deepEqual([t2.wiki.path, t2.wiki.root], ["a-only.md", "/ws/a"]);
  assert.equal(W.shownRoot(), "/ws/new");
  assert.ok(await waitFor(() => W.wk.pages[0]?.path === "n.md"), "選んだフォルダの木を読まない");
  // 失敗 = console.warn だけ（toast は Rust）・タブはそのまま
  S.app.toast = "";
  warns.length = 0;
  handlers.wiki_switch_root = reject("switch failed");
  await W.switchWorkspace("/ws/a");
  assert.equal(t1.wiki.root, "/ws/new", "切替の失敗でタブを替えた");
  assert.equal(S.app.toast, "", `切替の失敗で画面が toast した: ${S.app.toast}`);
  assert.ok(warns.some((w) => w.includes("wiki_switch_root") && w.includes("switch failed")), `console.warn に出ない: ${warns}`);
  handlers.wiki_switch_root = switchHandler;
  // 未保存 = 先に保存してから切り替える・保存できない（conflict）なら呼ばない
  const handle = (doc) => {
    let buf = doc;
    const h = { getText: () => buf, setText: (s) => (buf = s), insert: (s) => (buf += s), focus() {} };
    E.attach(h);
    return { h, type: (s) => (buf += s) };
  };
  handlers.wiki_save = (a) => ({ ok: true, path: a.path, sha256: "S1", size: a.content.length, mtime: 1, created: false });
  t1.wiki = { path: "n.md", root: "/ws/new", scrollTop: 0, editing: true };
  E.startEdit(page("n.md", "base", "S0"), "/ws/new");
  const ed1 = handle("base");
  ed1.type("+未保存");
  calls.length = 0;
  await W.switchWorkspace("/ws/a");
  const writesAndSwitch = () => names().filter((n) => n === "wiki_save" || n === "wiki_switch_root"); // 前の切替の読み直し（背景）は除く
  assert.deepEqual(writesAndSwitch(), ["wiki_save", "wiki_switch_root"], `保存の前に切り替えた: ${names()}`);
  const save0 = calls.find((c) => c.name === "wiki_save");
  assert.deepEqual([save0.args.content, save0.args.root], ["base+未保存", "/ws/new"], "元の root へ保存しない");
  E.detach(ed1.h);
  handlers.wiki_save = (a) => ({ ok: false, conflict: true, path: a.path, sha256: "THEIRS" });
  t1.wiki = { path: "b.md", root: "/ws/a", scrollTop: 0, editing: true };
  E.startEdit(page("b.md", "b", "B0"), "/ws/a");
  const ed2 = handle("b");
  ed2.type("+mine");
  calls.length = 0;
  await W.switchWorkspace("/ws/b");
  assert.deepEqual(writesAndSwitch(), ["wiki_save"], `保存できないのに切り替えた: ${names()}`);
  assert.equal(t1.wiki.root, "/ws/a");
  E.detach(ed2.h);
  E.ed.dialog = false;
  E.discardEdit();
  tests++;
}

// ---- ③ 状態の読み直しは画面の root で・status の root から切替を推測しない・wiki-changed の root ----
{
  const [t1, t2] = await reset([["notes/x.md", "/ws/a"], ["b-only.md", "/ws/b"]], 0);
  const off = W.initWiki();
  be.setting = "/ws/c"; // 設定が別の root（MCP の既定・揃える途中）
  emit("wiki-state");
  assert.ok(await waitFor(() => calls.some((c) => c.name === "wiki_status")), "wiki-state で状態を読まない");
  await settle();
  assert.deepEqual(calls.find((c) => c.name === "wiki_status").args, { root: "/ws/a" }, "画面の root で読まない");
  assert.equal(W.shownRoot(), "/ws/a", "設定の root で画面を替えた");
  assert.deepEqual([t1.wiki.path, t2.wiki.path], ["notes/x.md", "b-only.md"], "状態の root から切替を推測してタブを戻した");
  assert.equal(W.workspaceList().find((w) => w.current)?.root, "/ws/a", "✓ が画面の root でない");
  assert.equal(W.workspaceList().filter((w) => w.current).length, 1);
  // wiki-changed: 画面の root と違う root の通知は捨てる・同じ root / root 無し（旧い形）は受ける
  const s0 = W.wk.stamp;
  emit("wiki-changed", { all: false, added: ["z.md"], removed: [], modified: [], root: "/ws/b" });
  assert.equal(W.wk.stamp, s0, "別の root の通知を受けた");
  emit("wiki-changed", { all: false, added: ["z.md"], removed: [], modified: [], root: "/ws/a" });
  assert.equal(W.wk.stamp, s0 + 1, "画面の root の通知を捨てた");
  assert.equal(W.wk.changed.root, "/ws/a");
  emit("wiki-changed", { all: false, added: ["z.md"], removed: [], modified: [] });
  assert.equal(W.wk.stamp, s0 + 2, "root の無い通知を捨てた");
  off();
  tests++;
}

// ---- ④ 写しの退避 / 復元・前面でない root のタブの題名 ----
{
  const [t1] = await reset([["notes/x.md", "/ws/a"], [null, "/ws/b"]], 0);
  W.wk.open.add("notes");
  W.wk.titleOverrides.set("notes/x.md", "X の覚え");
  W.wk.newDirs.add("empty-a");
  W.wk.q = "foo";
  W.wk.qTags = ["t-a"];
  W.wk.showResults = true;
  W.wk.results = [{ path: "notes/x.md", kind: "md", title: "X", tags: [], mtime: 0, size: 1, match: "title", snippet: null }];
  W.wk.resultsTotal = 1;
  W.wk.sel = 0;
  W.wk.error = "A の監視のエラー";
  W.wk.treeError = "A の木のエラー";
  W.wk.page = page("notes/x.md", "x", "X0");
  W.wk.headings = [{ id: "h", text: "H", level: 1 }];
  W.wk.scrollTo = { id: "h", nonce: 1 };
  W.wk.menu = { kind: "file", path: "notes/x.md", x: 0, y: 0 };
  const snap = () => ({
    status: [W.wk.status?.root, W.wk.status?.pages],
    pages: W.wk.pages,
    treeError: W.wk.treeError,
    tags: W.wk.tags,
    tagsTotal: W.wk.tagsTotal,
    inbox: W.wk.inbox,
    inboxTotal: W.wk.inboxTotal,
    chatLog: W.wk.chatLog,
    chatLogTotal: W.wk.chatLogTotal,
    open: [...W.wk.open],
    titleOverrides: [...W.wk.titleOverrides],
    newDirs: [...W.wk.newDirs],
    q: W.wk.q,
    qTags: [...W.wk.qTags],
    showResults: W.wk.showResults,
    results: W.wk.results,
    resultsTotal: W.wk.resultsTotal,
    sel: W.wk.sel,
    searchError: W.wk.searchError,
    error: W.wk.error,
    refDirs: [...W.wk.refDirs],
  });
  const a = snap();
  assert.deepEqual(Object.keys(a).sort(), [...W.SHELF_KEYS].sort(), "試験の写しの項目が SHELF_KEYS と違う（試験を追随させる）");
  const gen = W.wk.embedGen;
  const prefs0 = [W.wk.infoOpen, W.wk.headColors, W.wk.split];
  W.wk.infoOpen = !prefs0[0];
  W.wk.split = !prefs0[2];
  W.enterRoot("/ws/b"); // 同期で入れ替える（行き来の rootGuard と同じ）
  const b = snap();
  assert.equal(W.wk.status.root, "/ws/b");
  assert.deepEqual(
    [b.pages, b.tags, b.inbox, b.chatLog, b.open, b.titleOverrides, b.newDirs, b.q, b.qTags, b.showResults, b.results, b.error, b.treeError, b.refDirs],
    [[], [], [], [], [], [], [], "", [], false, [], null, null, []],
    "初めての root に前の root の状態が残った"
  );
  assert.deepEqual(W.wk.status.workspaces.map((w) => w.root), be.hist, "履歴を引き継がない（ワークスペースによらない）");
  // 前面のページと前の root を指すものは捨てる（写しに入れない）
  assert.deepEqual([W.wk.page, W.wk.headings, W.wk.scrollTo, W.wk.menu], [null, [], null, null]);
  assert.ok(W.wk.embedGen > gen, "埋め込みの読みの世代を進めていない");
  // 表示の好みは共通
  assert.deepEqual([W.wk.infoOpen, W.wk.headColors, W.wk.split], [!prefs0[0], prefs0[1], !prefs0[2]], "表示の好みを root ごとにした");
  // A のタブの題名は A の写しから（覚えた題名 → 索引）
  assert.equal(W.wikiTabTitle(t1.wiki), "X の覚え");
  assert.equal(W.wikiTabTitle({ path: "a-only.md", root: "/ws/a", scrollTop: 0, editing: false }), "A だけ");
  assert.equal(W.wikiTabTitle({ path: "zz/none.md", root: "/ws/zz", scrollTop: 0, editing: false }), "none", "写しの無い root はファイル名");
  calls.length = 0;
  W.enterRoot("/ws/a");
  const back = snap();
  for (const k of ["pages", "tags", "inbox", "chatLog", "results"]) assert.equal(back[k], a[k], `${k} を戻さない（同じ配列 = 複製しない）`);
  assert.deepEqual(back, a, "A の写しが戻らない");
  W.wk.infoOpen = prefs0[0];
  W.wk.split = prefs0[2];
  // 戻ったら読み直しで追いつく（状態・木・参照フォルダ・検索の結果）
  assert.ok(await waitFor(() => calls.some((c) => c.name === "wiki_list") && calls.some((c) => c.name === "wiki_search")), `読み直さない: ${names()}`);
  for (const n of ["wiki_status", "wiki_list", "wiki_ref_dirs_get", "wiki_search"]) assert.equal(calls.find((c) => c.name === n)?.args?.root, "/ws/a", `${n} の root`);
  // 以前の leaveRoot が捨てていた項目 = 写し（SHELF_KEYS）か、捨てる側（前面のページ・前の root を指すもの）のどちらか
  const LEAVE_ROOT = ["open", "titleOverrides", "newDirs", "page", "pageError", "headings", "scrollTo", "error", "embedGen", "pages", "treeError", "tags", "tagsTotal", "inbox", "inboxTotal", "chatLog", "chatLogTotal", "q", "qTags", "sel", "showResults", "results", "resultsTotal", "searching", "searchError", "dialog", "menu"];
  const RESET = ["page", "pageError", "headings", "scrollTo", "embedGen", "dialog", "menu", "searching"];
  for (const k of LEAVE_ROOT) assert.ok(W.SHELF_KEYS.includes(k) || RESET.includes(k), `leaveRoot の ${k} を写しにも捨てる側にも入れていない`);
  for (const k of ["pages", "tags", "inbox", "chatLog", "open", "titleOverrides", "newDirs", "q", "qTags", "results", "showResults", "sel", "error", "treeError", "refDirs", "status"]) {
    assert.ok(W.SHELF_KEYS.includes(k), `${k} が写しに無い`);
  }
  for (const k of ["infoOpen", "headColors", "split", "page"]) assert.ok(!W.SHELF_KEYS.includes(k), `共通の ${k} を写しに入れた`);
  tests++;
}

// ---- ⑤ wiki_front_root は直列・追い越された揃えは呼ばない・ページの門は最後の揃えまで開かない ----
{
  const [t1, t2] = await reset([["notes/x.md", "/ws/a"], ["b-only.md", "/ws/b"]], 0);
  be.holding = true;
  await S.activateTab(t2.id); // B（揃えは待たされる）
  let ready = false;
  void W.rootReady().then(() => (ready = true));
  await S.activateTab(t1.id); // A
  await S.activateTab(t2.id); // B
  await sleep(10);
  assert.deepEqual(be.front, ["/ws/b"], `揃えを並行に呼んだ: ${be.front}`);
  assert.equal(ready, false, "揃え終える前にページの門が開いた");
  be.held.shift()();
  assert.ok(await waitFor(() => be.front.length === 2), "最後の揃えを呼ばない");
  assert.deepEqual(be.front, ["/ws/b", "/ws/b"], "追い越された揃え（A）を呼んだ");
  assert.equal(be.maxInflight, 1, "揃えが重なった");
  await sleep(10);
  assert.equal(ready, false, "最後の揃えの前に門が開いた");
  be.held.shift()();
  assert.ok(await waitFor(() => ready), "最後の揃えの後に門が開かない");
  assert.equal(be.setting, "/ws/b", "設定が前面（B）とずれた");
  assert.equal(W.shownRoot(), "/ws/b");
  be.holding = false;
  tests++;
}

// ---- ⑥ 起動: ui_tabs の root・履歴に無い root を戻す + toast 1 回・前面の root を設定へ揃える・揃うまで門を開けない ----
{
  handlers.wiki_status = () => ({ enabled: false, root: null });
  await W.loadWikiStatus();
  handlers.wiki_status = statusHandler;
  Object.assign(be, { setting: "/ws/a", hist: ["/ws/a", "/ws/b"], front: [], holding: true, held: [] });
  W.wk.status = null;
  prefs.ui_tabs = JSON.stringify({
    active: 1,
    tabs: [
      { view: { kind: "all" }, wiki: { path: "a.md", root: "/ws/a" } },
      { view: { kind: "all" }, wiki: { path: "b.md", root: "/ws/b" } },
      { view: { kind: "all" }, wiki: { path: "gone.md", root: "/vol/gone" } },
      { view: { kind: "all" }, wiki: { path: "gone2.md", root: "/vol/gone2" } },
      { view: { kind: "all" }, wiki: { path: "old.md" } },
      { view: { kind: "all" }, wiki: { path: "bad.md", root: 42 } },
    ],
  });
  handlers.wiki_settings = () => ({ enabled: true, root: "/ws/a/" }); // 仮の root = canonical とは限らない字面
  await W.loadWikiEnabled();
  await S.restoreTabs();
  const tabs = S.app.tabs;
  assert.deepEqual(
    tabs.map((x) => x.wiki && [x.wiki.path, x.wiki.root]),
    [["a.md", "/ws/a"], ["b.md", "/ws/b"], ["gone.md", "/vol/gone"], ["gone2.md", "/vol/gone2"], ["old.md", null], ["bad.md", null]],
    "ui_tabs の root を戻さない（無い / 壊れた root は null）"
  );
  assert.equal(S.app.tab, tabs[1].id);
  assert.equal(W.shownRoot(), null, "仮の root を canonical として使った");
  let ready = false;
  void W.rootReady().then(() => (ready = true));
  await sleep(5);
  assert.equal(ready, false, "最初の状態の前にページの門が開いた");
  calls.length = 0;
  S.app.toast = "";
  await W.loadWikiStatus();
  assert.deepEqual(calls.find((c) => c.name === "wiki_status").args, {}, "最初の状態を root 付きで読んだ（仮の root）");
  // 履歴に無い root のタブ = 設定の root のホーム・toast 1 回（2 枚でも 1 回）
  assert.deepEqual({ ...tabs[2].wiki }, HOME("/ws/a"), "履歴に無い root のタブを戻さない");
  assert.deepEqual({ ...tabs[3].wiki }, HOME("/ws/a"));
  assert.equal(S.app.toast, "履歴に無いワークスペースのタブを a のホームに戻しました");
  // root の無いタブ = 設定の root を採る（中身は保つ）
  assert.deepEqual([tabs[4].wiki.path, tabs[4].wiki.root], ["old.md", "/ws/a"]);
  assert.deepEqual([tabs[5].wiki.path, tabs[5].wiki.root], ["bad.md", "/ws/a"]);
  // 前面（b.md・/ws/b）≠ 設定（/ws/a）= 揃える（中身は保つ）・揃うまで門を開けない
  assert.deepEqual([tabs[1].wiki.path, tabs[1].wiki.root], ["b.md", "/ws/b"], "揃えで前面のタブの中身を変えた");
  assert.deepEqual([tabs[0].wiki.path, tabs[0].wiki.root], ["a.md", "/ws/a"]);
  assert.equal(W.shownRoot(), "/ws/b");
  await sleep(10);
  assert.deepEqual(be.front, ["/ws/b"], "前面の root へ設定を揃えない");
  assert.equal(ready, false, "前面の root を揃える前にページの門が開いた");
  be.held.shift()();
  be.holding = false;
  assert.ok(await waitFor(() => ready), "揃えの後に門が開かない");
  assert.equal(be.setting, "/ws/b");
  await settle();
  assert.ok(calls.some((c) => c.name === "wiki_list" && c.args.root === "/ws/b"), "揃えの後に B の木を読まない");
  // toast は 1 回（次の状態の読みで繰り返さない）
  S.app.toast = "";
  await W.loadWikiStatus();
  assert.equal(S.app.toast, "", `知らせを繰り返した: ${S.app.toast}`);
  // ui_tabs の保存 = {path, root}（往復）
  const saved = JSON.parse(S.tabsPref());
  assert.deepEqual(saved.tabs.map((x) => x.wiki), [
    { path: "a.md", root: "/ws/a" },
    { path: "b.md", root: "/ws/b" },
    { path: null, root: "/ws/a" },
    { path: null, root: "/ws/a" },
    { path: "old.md", root: "/ws/a" },
    { path: "bad.md", root: "/ws/a" },
  ]);
  // 前面の root = 設定 = 別の root へは揃えない（同じ root の揃えを 1 回だけ通す・Rust では何もしない）
  handlers.wiki_status = () => ({ enabled: false, root: null });
  await W.loadWikiStatus();
  handlers.wiki_status = statusHandler;
  Object.assign(be, { setting: "/ws/a", front: [] });
  W.wk.status = null;
  prefs.ui_tabs = JSON.stringify({ active: 0, tabs: [{ view: { kind: "all" }, wiki: { path: "a.md", root: "/ws/a" } }, { view: { kind: "all" }, wiki: { path: "b.md", root: "/ws/b" } }] });
  handlers.wiki_settings = () => ({ enabled: true, root: "/ws/a" });
  await W.loadWikiEnabled();
  await S.restoreTabs();
  await W.loadWikiStatus();
  await settle();
  assert.deepEqual(be.front, ["/ws/a"], "前面の root = 設定なのに別の root へ揃えた / 起動の揃えを通さない");
  assert.equal(W.shownRoot(), "/ws/a");
  tests++;
}

// ---- ⑦ 新しい OpenNote タブは前面の OpenNote タブの root を継ぐ・前面が一覧なら最後の OpenNote の root ----
{
  const [, t2] = await reset([["notes/x.md", "/ws/a"], ["b-only.md", "/ws/b"]], 1);
  assert.equal(W.shownRoot(), "/ws/b");
  await S.openTabInMode(); // ⌘T・＋
  assert.deepEqual({ ...S.app.tabs.at(-1).wiki }, HOME("/ws/b"), "⌘T のタブが前面の root を継がない");
  await S.activateTab(t2.id);
  await W.openWikiPage("b2.md", true); // ⌘クリック
  assert.deepEqual([S.app.tabs.at(-1).wiki.path, S.app.tabs.at(-1).wiki.root], ["b2.md", "/ws/b"]);
  // 前面が一覧のタブ（ItemDetail の Wiki 節・セグメント MY LIBRARY → MY NOTE）= 最後に前面だった OpenNote の root
  S.app.tabs.push(mkTab(null));
  const lib = S.app.tabs.at(-1);
  await S.activateTab(lib.id);
  await W.openWikiPage("from-item.md", true);
  assert.deepEqual([S.app.tabs.at(-1).wiki.path, S.app.tabs.at(-1).wiki.root], ["from-item.md", "/ws/b"], "ItemDetail の節のタブが最後の OpenNote の root を継がない");
  await S.activateTab(lib.id);
  await W.switchMode("wiki");
  assert.deepEqual({ ...lib.wiki }, HOME("/ws/b"), "セグメントの MY NOTE が最後の OpenNote の root を継がない");
  tests++;
}

// ---- ⑧ 仮の状態のまま状態の読みが失敗 → 明示の切替は戻り値で前面のタブを替える----
{
  handlers.wiki_status = () => ({ enabled: false, root: null });
  await W.loadWikiStatus();
  Object.assign(be, { setting: "/ws/a", hist: ["/ws/a", "/ws/b", "/ws/c"], front: [] });
  W.wk.status = null;
  handlers.wiki_settings = () => ({ enabled: true, root: "/ws/a" });
  await W.loadWikiEnabled();
  const tabs = [mkTab(["a-only.md", null]), mkTab(["notes/x.md", null])];
  S.app.tabs.splice(0, S.app.tabs.length, ...tabs);
  S.app.tab = tabs[0].id;
  const [t1, t2] = tabs.map((x) => S.app.tabs.find((y) => y.id === x.id));
  handlers.wiki_status = reject("ipc down");
  await W.loadWikiStatus();
  assert.ok(W.wk.status?.statusError, "状態の失敗を再現できない");
  assert.equal(W.shownRoot(), null);
  handlers.wiki_status = statusHandler;
  await W.switchWorkspace("/ws/b");
  assert.deepEqual({ ...t1.wiki }, HOME("/ws/b"), "前面のタブが戻り値の root のホームにならない");
  assert.deepEqual({ ...t2.wiki }, HOME(null), "root の分からないタブで前の相対パスを新しい root で開く");
  assert.equal(W.shownRoot(), "/ws/b");
  // 次の状態（読める）= root の分からないタブは設定の root を採る
  assert.ok(await waitFor(() => t2.wiki.root === "/ws/b"), "root の分からないタブが設定の root を採らない");
  tests++;
}

// ---- ⑨ WikiInfo のホームのワークスペースの節・[外す] はタブが使っている root では出さない ----
{
  const [, tb] = await reset([[null, "/ws/cur"], ["x.md", "/ws/b"]], 0, "/ws/cur");
  Object.assign(be, { hist: ["/ws/cur", "/ws/blog", "/ws/b", "/vol/gone"], gone: ["/vol/gone"] });
  await W.loadWikiStatus();
  await settle();
  const { default: WikiInfo } = await import("./WikiInfo.svelte");
  W.wk.page = null;
  W.wk.refDirs = ["/ref"];
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiInfo, { target });
  flushSync();
  const heads = [...target.querySelectorAll("h3")].map((h) => h.textContent.trim());
  const iWs = heads.findIndex((h) => h.startsWith("ワークスペース"));
  const iRef = heads.findIndex((h) => h.startsWith("参照フォルダ"));
  assert.ok(iWs >= 0, `ワークスペースの節が無い: ${heads}`);
  assert.ok(iWs < iRef, `ワークスペースの節が参照フォルダの上に無い: ${heads}`);
  assert.equal(heads[iWs], "ワークスペース4", `件数のバッジ: ${heads[iWs]}`);
  const rows = () => [...target.querySelectorAll(".wk-ws")];
  const row = (name) => rows().find((r) => r.querySelector(".wk-ws-name").textContent === name);
  const btns = (r) => [...r.querySelectorAll("button")].map((b) => b.textContent.trim());
  assert.equal(rows().length, 4);
  // 画面の root = ✓・ボタンなし
  assert.equal(row("cur").querySelector(".wk-ws-cur")?.textContent, "✓");
  assert.deepEqual(btns(row("cur")), []);
  assert.ok(row("cur").querySelector(".wk-ref-path").textContent.includes("/ws/cur"), "パスが出ない");
  // どのタブも使っていない = [開く] [外す]
  assert.deepEqual(btns(row("blog")), ["開く", "外す"]);
  assert.ok(!row("blog").classList.contains("wk-ws-gone"));
  // 別のタブが使っている = [外す] なし
  assert.deepEqual(btns(row("b")), ["開く"], "タブが使っている root に [外す] が出た");
  calls.length = 0;
  await W.forgetWorkspace("/ws/b");
  assert.ok(!names().includes("wiki_forget_root"), "タブが使っている root を外した");
  // 無い = dim・[開く] なし・title に「見つかりません」
  assert.deepEqual(btns(row("gone")), ["外す"]);
  assert.ok(row("gone").classList.contains("wk-ws-gone"), "無いフォルダが dim でない");
  assert.ok(row("gone").title.includes("見つかりません"), `title: ${row("gone").title}`);
  // 押下: [外す] = wiki_forget_root {args:{root}}
  calls.length = 0;
  raw.length = 0;
  row("gone").querySelector("button").click();
  assert.ok(await waitFor(() => calls.length > 0));
  assert.deepEqual(calls, [{ name: "wiki_forget_root", args: { root: "/vol/gone" } }]);
  assert.deepEqual(rawOf("wiki_forget_root"), [{ args: { root: "/vol/gone" } }], "args で包んでいない");
  // B のタブを閉じる = [外す] が出る
  await S.closeTab(tb.id);
  flushSync();
  assert.deepEqual(btns(row("b")), ["開く", "外す"], "タブを閉じても [外す] が出ない");
  // [開く] = 前面のタブだけ切替（wiki_switch_root {args:{root}}）
  calls.length = 0;
  [...row("blog").querySelectorAll("button")].find((b) => b.textContent.trim() === "開く").click();
  assert.ok(await waitFor(() => calls.some((c) => c.name === "wiki_switch_root")), "[開く] が wiki_switch_root を呼ばない");
  assert.deepEqual(calls.find((c) => c.name === "wiki_switch_root").args, { root: "/ws/blog" });
  await settle();
  flushSync();
  assert.equal(target.querySelector(".wk-ws-cur")?.closest(".wk-ws")?.querySelector(".wk-ws-name").textContent, "blog", "✓ が切り替えた root に移らない");
  const pick = [...target.querySelectorAll("button")].find((b) => b.textContent.trim() === "別のフォルダを選ぶ…");
  assert.ok(pick, "[別のフォルダを選ぶ…] が無い");
  calls.length = 0;
  pick.click();
  assert.ok(await waitFor(() => calls.some((c) => c.name === "wiki_pick_root")), "[別のフォルダを選ぶ…] が wiki_pick_root を呼ばない");
  // 一覧の読み直し（wiki-state → wk.status の置き換え）に追随する
  W.wk.status = { ...W.wk.status, workspaces: W.wk.status.workspaces.slice(0, 2) };
  flushSync();
  assert.equal(rows().length, 2, "一覧の変化に追随しない");
  // workspaceName = root の最後の区画（vaultName は画面の root で同じ規則）
  assert.equal(W.workspaceName("/Users/u/notes/wiki"), "wiki");
  assert.equal(W.workspaceName("/a/b/"), "b");
  assert.equal(W.workspaceName(""), "");
  assert.equal(W.vaultName(), "blog");
  unmount(cmp);
  target.remove();
  tests++;
}

// ---- ⑩ WikiDialogs の vault メニュー・WikiSidebar のメニューの位置 ----
{
  await reset([[null, "/ws/cur"]], 0, "/ws/cur");
  Object.assign(be, { hist: ["/ws/cur", "/ws/blog", "/vol/gone"], gone: ["/vol/gone"] });
  await W.loadWikiStatus();
  await settle();
  const { default: WikiDialogs } = await import("./WikiDialogs.svelte");
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiDialogs, { target });
  W.wk.menu = { kind: "vault", path: "/ws/cur", x: 10, y: 20 };
  flushSync();
  const menu = target.querySelector(".wk-menu");
  assert.ok(menu, "メニューが出ない");
  const items = () => [...target.querySelectorAll(".wk-menu button")].map((b) => b.textContent.trim());
  assert.ok(menu.textContent.includes("最近のワークスペース"), "見出しが無い");
  assert.ok(!menu.textContent.includes("選び直す"), "旧い文言が残った");
  const ws = [...target.querySelectorAll(".wk-menu-ws")];
  assert.deepEqual(ws.map((b) => b.querySelector(".wk-menu-name").textContent), ["cur", "blog", "gone"]);
  // 並び: ホーム → 見出し → 履歴 → 別のフォルダを選ぶ… → 参照フォルダを追加…
  assert.deepEqual(items(), ["ホーム（参照フォルダ・ページ情報）", "✓cur", "blog", "gone", "別のフォルダを選ぶ…", "参照フォルダ（読み取り専用）を追加…"]);
  const [cur, blog, gone] = ws;
  assert.equal(cur.disabled, true, "現在の行が押せる");
  assert.equal(cur.querySelector(".wk-menu-check").textContent, "✓");
  assert.equal(cur.title, "/ws/cur");
  assert.equal(blog.disabled, false);
  assert.equal(blog.querySelector(".wk-menu-check").textContent, "");
  assert.equal(gone.disabled, true, "無いフォルダの行が押せる");
  assert.ok(gone.classList.contains("wk-menu-gone"), "無いフォルダが dim でない");
  assert.ok(gone.title.includes("見つかりません") && gone.title.includes("/vol/gone"), `title: ${gone.title}`);
  // 押下 = メニューを閉じて切り替える（前面のタブだけ）
  calls.length = 0;
  blog.click();
  flushSync();
  assert.equal(W.wk.menu, null, "押下でメニューが閉じない");
  assert.ok(await waitFor(() => calls.some((c) => c.name === "wiki_switch_root")));
  assert.deepEqual(calls.find((c) => c.name === "wiki_switch_root").args, { root: "/ws/blog" });
  assert.ok(await waitFor(() => S.frontWiki()?.root === "/ws/blog"), "メニューの切替で前面のタブが替わらない");
  await settle();
  // 履歴が無い（現在だけ）= 見出しと ✓ の行は出す
  W.wk.status = { enabled: true, root: "/only/one" };
  W.wk.menu = { kind: "vault", path: "/only/one", x: 10, y: 20 };
  flushSync();
  assert.ok(target.querySelector(".wk-menu").textContent.includes("最近のワークスペース"));
  const only = [...target.querySelectorAll(".wk-menu-ws")];
  assert.equal(only.length, 1);
  assert.equal(only[0].disabled, true);
  assert.equal(only[0].textContent.trim(), "✓one");
  W.wk.menu = null;
  flushSync();

  // WikiSidebar の vault 行: メニューの y の上限 = 窓の高さ − 見積もり（165 + 28 × 件数）
  const { default: WikiSidebar } = await import("./WikiSidebar.svelte");
  W.wk.status = { enabled: true, root: "/ws/cur", workspaces: workspaces() };
  W.wk.pages = [];
  const side = mount(WikiSidebar, { target });
  flushSync();
  const vault = target.querySelector(".wk-vault");
  assert.ok(vault, "vault 行が無い");
  assert.ok(vault.textContent.includes("cur"), "vault 行の名前が画面の root でない");
  vault.getBoundingClientRect = () => ({ left: 8, right: 200, top: 680, bottom: 700, width: 192, height: 20, x: 8, y: 680 });
  const ih = window.innerHeight;
  vault.click();
  flushSync();
  assert.equal(W.wk.menu?.kind, "vault");
  assert.equal(W.wk.menu.y, ih - (165 + 28 * 3), `y の上限が件数の見積もりでない: ${W.wk.menu.y} (innerHeight ${ih})`);
  W.wk.menu = null;
  unmount(side);
  unmount(cmp);
  target.remove();
  tests++;
}

// ---- ⑪ TabStrip: OpenNote タブの tooltip =「{path}\n{name} · {root}」・前面でない root のタブの題名 ----
{
  const [, t2] = await reset([["notes/x.md", "/ws/a"], [null, "/ws/b"]], 0);
  const { default: TabStrip } = await import("../TabStrip.svelte");
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(TabStrip, { target });
  flushSync();
  const tabEls = () => [...target.querySelectorAll(".tabs-tab")];
  assert.equal(tabEls()[0].title, "notes/x.md\na · /ws/a", `tooltip: ${JSON.stringify(tabEls()[0].title)}`);
  assert.equal(tabEls()[1].title, "OpenNote\nb · /ws/b", `tooltip: ${JSON.stringify(tabEls()[1].title)}`);
  assert.equal(tabEls()[0].querySelector(".tabs-title").textContent, "X の題名");
  await S.activateTab(t2.id);
  await settle();
  flushSync();
  assert.equal(tabEls()[0].querySelector(".tabs-title").textContent, "X の題名", "前面でない root のタブの題名が出ない");
  assert.equal(W.wikiTabWorkspace({ path: null, root: null, scrollTop: 0, editing: false }), "b · /ws/b", "root の分からないタブは画面の root");
  unmount(cmp);
  target.remove();
  tests++;
}

// ---- ⑫ 画面の root が履歴から外れた = 設定の root で読み直し・そのタブを戻す ----
{
  const [t1, t2] = await reset([["notes/x.md", "/ws/a"], ["b-only.md", "/ws/b"]], 0);
  const off = W.initWiki();
  Object.assign(be, { hist: ["/ws/c", "/ws/b"], setting: "/ws/c", front: [] }); // A が履歴から落ちた（別のタブで新しいフォルダを選んだ等）
  be.trees["/ws/c"] = [["c.md", "C"]];
  S.app.toast = "";
  emit("wiki-state");
  assert.ok(await waitFor(() => W.shownRoot() === "/ws/c"), `設定の root を採らない: ${W.shownRoot()}`);
  await settle();
  const reads = calls.filter((c) => c.name === "wiki_status").map((c) => c.args);
  assert.deepEqual(reads.slice(0, 2), [{ root: "/ws/a" }, {}], `画面の root で読み、拒まれたら root 無しで読み直さない: ${JSON.stringify(reads)}`);
  assert.ok(reads.length <= 4, `状態の読みを繰り返した: ${reads.length}`);
  assert.deepEqual({ ...t1.wiki }, HOME("/ws/c"), "履歴から外れた root のタブを設定の root のホームへ戻さない");
  assert.deepEqual([t2.wiki.path, t2.wiki.root], ["b-only.md", "/ws/b"], "他のタブを触った");
  assert.equal(S.app.toast, "履歴に無いワークスペースのタブを c のホームに戻しました");
  assert.ok(await waitFor(() => W.wk.pages.map((p) => p.path).join() === "c.md"), "前の root の木が残った");
  assert.deepEqual(be.front, ["/ws/c"], "設定以外の root へ揃えた / 揃えを繰り返した（root を採ったときの揃えは設定と同じ root に 1 回・Rust では何もしない）");
  off();
  tests++;
}

// ---- ⑬ WikiPage: 揃えを待ってから・タブの root で読む・同じ相対パスでも root が違えば読み直す ----
{
  const [t1, t2] = await reset([["same.md", "/ws/a"], ["same.md", "/ws/b"]], 0);
  const files = { "/ws/a": "# A の same\n", "/ws/b": "# B の same\n" };
  handlers.wiki_read_gui = (a) => ({ ...page(a.path, files[a.root] ?? "?", "S-" + a.root), abs_path: `${a.root}/${a.path}`, title: `same@${a.root}` });
  handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  const reads = () => calls.filter((c) => c.name === "wiki_read_gui").map((c) => c.args.root);
  assert.ok(await waitFor(() => W.wk.page?.content === "# A の same\n"), "A のページを読まない");
  assert.deepEqual(reads(), ["/ws/a"]);
  // B のタブへ（揃えは待たされる）= 揃えるまで読まない
  be.holding = true;
  calls.length = 0;
  await S.activateTab(t2.id);
  await sleep(20);
  assert.deepEqual(be.front, ["/ws/b"]);
  assert.deepEqual(reads(), [], "設定を揃え終える前にページを読んだ（初めての root の画像が 403 になる）");
  assert.notEqual(W.wk.page?.content, "# A の same\n", "別の root の同じ相対パスのページを出したまま");
  be.held.shift()();
  be.holding = false;
  assert.ok(await waitFor(() => W.wk.page?.content === "# B の same\n"), "揃えた後に B のページを読まない");
  assert.deepEqual(reads(), ["/ws/b"], "B のページを B の root で読まない");
  // A へ戻る = 同じ相対パスでも A を読み直す
  calls.length = 0;
  await S.activateTab(t1.id);
  assert.ok(await waitFor(() => W.wk.page?.content === "# A の same\n"), "root だけが違うタブへ戻っても読み直さない");
  assert.deepEqual(reads(), ["/ws/a"]);
  unmount(cmp);
  target.remove();
  tests++;
}

// ---- ⑭ 写しの無い root のタブの題名 = ファイル名（画面の root の同じ相対パスの題名を出さない）----
{
  be.trees["/ws/a"] = [["index.md", "A のホーム"], ["notes/x.md", "X の題名"]];
  const [t1, t2] = await reset([["index.md", "/ws/a"], ["index.md", "/ws/b"]], 0); // 起動の復元と同じ = B はまだ前面にしていない
  assert.equal(W.shownRoot(), "/ws/a");
  assert.equal(W.wikiTabTitle(t1.wiki), "A のホーム");
  assert.equal(W.wikiTabTitle(t2.wiki), "index", "写しの無い root のタブに画面の root（A）の題名を出した");
  // 一度前面にして写しができた後は B の題名
  be.trees["/ws/b"] = [["index.md", "B のホーム"], ["b-only.md", "B だけ"]];
  await S.activateTab(t2.id);
  assert.ok(await waitFor(() => W.wikiTabTitle(t2.wiki) === "B のホーム"), "B の題名にならない");
  await S.activateTab(t1.id);
  await settle();
  assert.equal(W.wikiTabTitle(t2.wiki), "B のホーム", "B の写しから題名を引かない");
  assert.equal(W.wikiTabTitle(t1.wiki), "A のホーム");
  be.trees["/ws/a"] = [["notes/x.md", "X の題名"], ["a-only.md", "A だけ"]];
  be.trees["/ws/b"] = [["b-only.md", "B だけ"]];
  tests++;
}

// ---- ⑮ wiki_front_root の失敗 = 警告を残す・frontRetry 後に揃え直す（1 本ずつ）・toast 1 回 ----
{
  const [t1, t2] = await reset([["notes/x.md", "/ws/a"], ["b-only.md", "/ws/b"]], 0);
  const retry0 = W.wikiCfg.frontRetry;
  W.wikiCfg.frontRetry = 80;
  const off = W.initWiki();
  be.failFront = ["/ws/b"];
  S.app.toast = "";
  await S.activateTab(t2.id);
  assert.ok(await waitFor(() => S.app.toast.includes("ワークスペースを切り替えられません")), "揃えの失敗を知らせない");
  await settle(); // 揃えの後の状態の読み（loadWikiStatus = wk.status の置き換え）
  assert.equal(be.setting, "/ws/a");
  assert.ok(W.wk.status?.frontError?.includes("フォルダを使えません"), `状態の読み直しで揃えの失敗の警告が消えた: ${JSON.stringify(W.wk.status)}`);
  emit("wiki-state"); // 別の状態の読み（wiki-state）でも消えない・予約を足さない
  await settle();
  assert.ok(W.wk.status?.frontError, "wiki-state の読み直しで警告が消えた");
  // frontRetry 後に揃え直す（失敗が続いても予約は 1 本ずつ = 連打しない）・toast は繰り返さない
  S.app.toast = "";
  const n0 = be.front.length;
  await sleep(300);
  const tries = be.front.length - n0;
  assert.ok(tries >= 1, "揃え直さない");
  assert.ok(tries <= 5, `揃え直しを連打した: ${tries} 回 / 300ms（間隔 80ms）`);
  assert.equal(S.app.toast, "", `揃え直しの失敗で toast を繰り返した: ${S.app.toast}`);
  assert.ok(W.wk.status?.frontError, "揃え直しの失敗の後に警告が消えた");
  // 揃え直せた = 警告が消え、設定が前面の root・以後は揃え直さない
  be.failFront = [];
  assert.ok(await waitFor(() => be.setting === "/ws/b", 1000), "揃え直せない");
  assert.ok(await waitFor(() => !W.wk.status?.frontError, 1000), "揃え直せたのに警告が残った");
  await sleep(250);
  const n1 = be.front.length;
  await sleep(250);
  assert.equal(be.front.length, n1, "揃え直せた後も揃えを呼び続けた");
  // 失敗中に別の root のタブへ移って揃えられた = 前の失敗を持ち越さない
  be.failFront = ["/ws/a"];
  await S.activateTab(t1.id);
  await settle();
  assert.ok(W.wk.status?.frontError, "A の揃えの失敗の警告が出ない");
  be.failFront = [];
  await S.activateTab(t2.id); // B へ（揃えは成功 = 設定 B）
  await settle();
  assert.equal(be.setting, "/ws/b");
  assert.ok(!W.wk.status?.frontError, "B へ揃えられたのに A の失敗の警告が残った");
  const n2 = be.front.length;
  await sleep(250);
  assert.equal(be.front.length, n2, "前の root（A）の揃え直しが B の前面で走った");
  // 揃えに失敗する root を frontRetry の内に 2 つ続けて前面に（B → C） = 揃え直すのは今の失敗の root（C）
  W.wikiCfg.frontRetry = 150;
  const tc = mkTab(["c.md", "/ws/c"]);
  S.app.tabs.push(tc);
  const t3 = S.app.tabs.find((x) => x.id === tc.id);
  await S.activateTab(t1.id); // A（揃えは成功）
  await settle();
  be.failFront = ["/ws/b", "/ws/c"];
  await S.activateTab(t2.id); // B: 失敗 → 予約
  await sleep(30);
  await S.activateTab(t3.id); // C: 失敗（予約は B の物 = 足さない）
  await sleep(30);
  assert.equal(W.shownRoot(), "/ws/c");
  assert.ok(W.wk.status?.frontError?.includes("/ws/c"), "C の揃えの失敗の警告が出ない");
  be.failFront = [];
  const n3 = be.front.length;
  assert.ok(await waitFor(() => be.setting === "/ws/c", 1500), `C を揃え直さない（予約した B の root で降りた）: ${be.front.slice(n3)} / 設定 ${be.setting}`);
  assert.deepEqual(be.front.slice(n3), ["/ws/c"], "揃え直しが今の失敗の root でない");
  assert.ok(await waitFor(() => !W.wk.status?.frontError, 1000), "揃え直せたのに警告が残った");
  off();
  W.wikiCfg.frontRetry = retry0;
  tests++;
}

// ---- ⑯ 履歴にあるが今は無いフォルダ（exists: false）の root のタブは戻さない・前面なら揃えの失敗の警告----
{
  const [t1, t2, t3] = await reset([["notes/x.md", "/ws/a"], ["g.md", "/ws/b"], ["c.md", "/ws/c"]], 0);
  Object.assign(be, { gone: ["/ws/b"], front: [] }); // B = 未接続の外付け等（履歴には残る・灰色）
  S.app.toast = "";
  await W.loadWikiStatus();
  assert.deepEqual([t2.wiki.path, t2.wiki.root], ["g.md", "/ws/b"], "未接続のフォルダの root のタブを戻した（path を失う）");
  assert.deepEqual([t1.wiki.path, t1.wiki.root], ["notes/x.md", "/ws/a"]);
  assert.deepEqual([t3.wiki.path, t3.wiki.root], ["c.md", "/ws/c"]);
  assert.equal(S.app.toast, "", `戻していないのに知らせた: ${S.app.toast}`);
  assert.deepEqual(JSON.parse(S.tabsPref()).tabs[1].wiki, { path: "g.md", root: "/ws/b" }, "ui_tabs から root と path が落ちた");
  // 履歴に無い root は今までどおり戻す（未接続のフォルダと一緒でも）
  t3.wiki = { path: "n.md", root: "/vol/none", scrollTop: 0, editing: false };
  await W.loadWikiStatus();
  assert.deepEqual([{ ...t3.wiki }, [t2.wiki.path, t2.wiki.root]], [HOME("/ws/a"), ["g.md", "/ws/b"]]);
  assert.equal(S.app.toast, "履歴に無いワークスペースのタブを a のホームに戻しました");
  // 前面にする = 揃えが core の文（…見つかりません）で失敗 = タブはそのまま・ヘッダーの警告と toast に core の文
  const { default: WikiHeader } = await import("./WikiHeader.svelte");
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiHeader, { target });
  const retry0 = W.wikiCfg.frontRetry;
  W.wikiCfg.frontRetry = 60;
  S.app.toast = "";
  await S.activateTab(t2.id);
  await settle();
  flushSync();
  assert.deepEqual([t2.wiki.path, t2.wiki.root], ["g.md", "/ws/b"], "前面にした未接続のフォルダのタブを戻した");
  assert.equal(W.shownRoot(), "/ws/b");
  assert.equal(be.setting, "/ws/a", "揃えられないのに設定が動いた");
  assert.ok(W.wk.status?.frontError?.includes("見つかりません"), `警告が core の文でない: ${W.wk.status?.frontError}`);
  const bad = target.querySelector(".wk-head-bad");
  assert.equal(bad?.textContent, "⚠ ワークスペースを切り替えられません", `ヘッダーの警告: ${bad?.textContent}`);
  assert.ok(bad.title.includes("/ws/b") && bad.title.includes("見つかりません"), `警告の title が core の文でない: ${bad.title}`);
  assert.ok(S.app.toast.includes("ワークスペースを切り替えられません") && S.app.toast.includes("見つかりません"), `toast: ${S.app.toast}`);
  // 繋ぎ直した（exists に戻った）= 揃え直しで設定が B・警告が消える
  be.gone = [];
  await sleep(10);
  assert.ok(await waitFor(() => be.setting === "/ws/b", 2000), "繋ぎ直しても揃え直さない");
  assert.ok(await waitFor(() => !W.wk.status?.frontError, 1000), "揃え直せたのに警告が残った");
  flushSync();
  assert.equal(target.querySelector(".wk-head-bad"), null, "揃え直せたのにヘッダーの警告が残った");
  W.wikiCfg.frontRetry = retry0;
  unmount(cmp);
  target.remove();
  tests++;
}

// ---- ⑰ チャットの子が使っている root には [外す] を出さない ----
{
  const CH = await import("./chat.svelte.ts");
  await reset([[null, "/ws/a"]], 0);
  CH.chat.status = { enabled: true, cli: "/c", version: "1", logged_in: true, auth: null, model: null, mcp: "/m", state: "idle", session_id: "S", root: "/ws/b" };
  assert.equal(W.rootInUse("/ws/b"), true, "チャットの子の root を使用中と見ない");
  assert.equal(W.rootInUse("/ws/c"), false);
  calls.length = 0;
  await W.forgetWorkspace("/ws/b");
  assert.ok(!names().includes("wiki_forget_root"), "チャットの子の root を外した");
  const { default: WikiInfo } = await import("./WikiInfo.svelte");
  W.wk.page = null;
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiInfo, { target });
  flushSync();
  const row = (name) => [...target.querySelectorAll(".wk-ws")].find((r) => r.querySelector(".wk-ws-name").textContent === name);
  const btns = (r) => [...r.querySelectorAll("button")].map((b) => b.textContent.trim());
  assert.deepEqual(btns(row("b")), ["開く"], "チャットの子の root に [外す] が出た");
  assert.deepEqual(btns(row("c")), ["開く", "外す"]);
  // 子が止まった（root = null）= 出る
  CH.chat.status = { ...CH.chat.status, state: "off", root: null };
  flushSync();
  assert.deepEqual(btns(row("b")), ["開く", "外す"], "子が止まっても [外す] が出ない");
  unmount(cmp);
  target.remove();
  CH.chat.status = null;
  tests++;
}

// ---- ⑱ 起動: 前面の root = 設定でも wiki_front_root を 1 回通し、ページの読みはその後 ----
{
  handlers.wiki_status = () => ({ enabled: false, root: null });
  await W.loadWikiStatus();
  handlers.wiki_status = statusHandler;
  Object.assign(be, { setting: "/ws/a", hist: ["/ws/a", "/ws/b", "/ws/c"], gone: [], front: [], holding: true, held: [], failFront: [] });
  be.trees["/ws/a"] = [["notes/x.md", "X の題名"], ["a-only.md", "A だけ"]];
  W.wk.status = null;
  prefs.ui_tabs = JSON.stringify({ active: 0, tabs: [{ view: { kind: "all" }, wiki: { path: "notes/x.md", root: "/ws/a" } }] });
  handlers.wiki_settings = () => ({ enabled: true, root: "/ws/a" });
  handlers.wiki_read_gui = (a) => ({ ...page(a.path, "# X\n", "X0"), abs_path: `${a.root}/${a.path}` });
  handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
  await W.loadWikiEnabled();
  await S.restoreTabs();
  calls.length = 0;
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target }); // 起動の復元と同じ = 状態が届く前にページの部品が出る
  await sleep(10);
  await W.loadWikiStatus(); // 最初の状態（root 無し）= 設定 /ws/a = 前面の root
  await sleep(20);
  assert.deepEqual(be.front, ["/ws/a"], "起動で前面の root（= 設定）の揃えを通さない");
  assert.ok(!names().includes("wiki_read_gui"), "起動の揃え（= Rust の起動時の適用）の前にページを読んだ");
  be.held.shift()();
  be.holding = false;
  assert.ok(await waitFor(() => names().includes("wiki_read_gui")), "揃えの後にページを読まない");
  const iFront = names().indexOf("wiki_front_root");
  assert.ok(iFront >= 0 && iFront < names().indexOf("wiki_read_gui"), `ページの読みが揃えの前: ${names()}`);
  assert.equal(calls.find((c) => c.name === "wiki_read_gui").args.root, "/ws/a");
  assert.ok(await waitFor(() => W.wk.page?.path === "notes/x.md"));
  await settle();
  assert.deepEqual(be.front, ["/ws/a"], "起動の揃えを 2 回以上呼んだ");
  unmount(cmp);
  target.remove();
  // 起動の揃えが失敗した（Rust の起動時の適用が失敗 = 設定の root が使えない）= 警告を残し、状態の読みを待たずに揃え直しを予約する
  handlers.wiki_status = () => ({ enabled: false, root: null });
  await W.loadWikiStatus();
  handlers.wiki_status = statusHandler;
  const retry0 = W.wikiCfg.frontRetry;
  W.wikiCfg.frontRetry = 60;
  Object.assign(be, { setting: "/ws/a", front: [], failFront: ["/ws/a"] });
  W.wk.status = null;
  await W.loadWikiEnabled();
  await S.restoreTabs();
  await W.loadWikiStatus();
  assert.ok(await waitFor(() => !!W.wk.status?.frontError), "起動の揃えの失敗を警告に出さない");
  be.failFront = [];
  assert.ok(await waitFor(() => be.front.length >= 2, 1500), `起動の揃えの失敗を揃え直さない: ${be.front}`);
  assert.ok(await waitFor(() => !W.wk.status?.frontError, 1000), "揃え直せたのに警告が残った");
  W.wikiCfg.frontRetry = retry0;
  tests++;
}

console.warn = origWarn;
await sleep(5);
console.log(`wiki workspaces: ok (${tests} tests)`);
process.exit(0);
