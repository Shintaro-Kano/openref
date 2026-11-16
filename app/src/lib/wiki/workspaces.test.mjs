/* OpenNote のワークスペースの部品テスト（node app/src/lib/wiki/workspaces.test.mjs・実部品 state.svelte.ts / wiki.svelte.ts / WikiInfo / WikiDialogs /
   WikiSidebar / TabStrip + jsdom）。履歴からの切替 + タブごとのワークスペース。
   Rust の command（wiki_status {root?} / wiki_front_root / wiki_switch_root / wiki_pick_root / wiki_forget_root ほか）は契約の形で偽物（be）が返す。
   偽物は履歴に無い args.root を unknown_root で拒む。確かめること:
   ① タブの行き来（A のページ ⇄ B のホーム）: どちらもホームに戻らない・木 / 箱 / 参照フォルダが各 root・読みは全部その root（args.root）・
      設定は前面に揃う（wiki_front_root・明示の切替は呼ばない = last_used を変えない）・戻ると A の写しを即座に描いて読み直しで追いつく・題名は写しから
   ② 明示の切替（名前のメニューの行 = wiki_switch_root・別のフォルダを選ぶ… = wiki_pick_root）は前面のタブだけ = 戻り値の root のホームへ・
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
   ⑨ 名前のメニューの × = 履歴から外す（前は右ペインのホームの [外す]）: × はどれかのタブが使っている root（現在を含む）には出さない
      （閉じれば出る）・forgetWorkspace も呼ばない・押下 = wiki_forget_root {args:{root}}・メニューは開いたまま・キーの焦点は行の外の隣へ・
      一覧の読み直しに追随・右ペインのホームは概要だけ（ワークスペース・参照フォルダ・除外フォルダの節が無い）
   ⑩ WikiDialogs の vault メニュー（ホーム → 見出し → 履歴の行（×）→ 別のフォルダを選ぶ… → フォルダの設定…・全項目にチェックの欄 = 字の頭が揃う・
      ✓ = disabled・無い = dim で disabled・履歴が無くても現在の行）・WikiSidebar の高さの見積もり
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
   ⑰ チャットの子が使っている root（wiki_chat_status.root）にも × を出さない・forgetWorkspace も呼ばない
   ⑲ 写しの表（SHELF）は 1 つ: 表の項目ごとに目印を置き、初めての root = 空・戻る = 同じ物 / 同じ中身・OFF = 空（status だけ残す）を表から回して確かめる・
      wk の項目は表か「写さない」一覧のどちらか（分けない項目を足すと落ちる）・OFF で前は戻していなかった 8 項目
   ㉑ 「索引を作り直す」は名前のメニューの 1 か所: 押すと wiki_reindex 1 回・command を待つ間 / 作り直しの間は押せない・2 回目を呼ばない・
      覆いの下では呼ばない・ヘッダーに [再索引] が無い・ヘッダーの状態の表示（作成中 N%・状態を読めない・自動反映が止まった）は残る
   ⑳ 「フォルダの設定…」の窓（実部品 WikiDialogs + WikiFolders）: メニューから開く（メニューは閉じる）・焦点は [閉じる]・閉じたら開く前の焦点へ・
      Esc で閉じる・参照フォルダ（一覧 = 絶対パスを先頭側で省略・title に全体・外す / 追加… = 今の command）・除外フォルダ（一覧 = wk.excluded の
      vault 相対 + /・索引に入れる = 木の右クリックと同じ wiki_exclude_set）・0 件の文・覆いの下では開かない
   出力 = `wiki workspaces: ok (N tests)`（N = 列 0 のブロックの数） */
import assert from "node:assert/strict";
import { setupHarness } from "./test-harness.mjs";

const { calls, handlers, emit, sleep, waitFor } = setupHarness();
let tests = 0;

const { mount, unmount, flushSync } = await import("svelte");
const S = await import("../state.svelte.ts");
const W = await import("./wiki.svelte.ts");
const M = await import("../mode.svelte.ts"); // セグメントの切替（wiki.svelte から分けた）
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
  W.wk.tocOpen = false; // 目次を畳んだまま明示の切替（切替で開き直さない）
  await W.switchWorkspace("/ws/c");
  assert.equal(W.wk.tocOpen, false, "ワークスペースを替えたら目次が開き直った（畳んだ状態は全ワークスペースで共通）");
  W.wk.tocOpen = true;
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
  // 「別のフォルダを選ぶ…」も同じ門（canLeave）= 保存できない間はダイアログを出さない（門を飛ばす変異を捕まえる）
  calls.length = 0;
  await W.pickRoot();
  assert.ok(!names().includes("wiki_pick_root"), `保存できないのに別のフォルダを選ぶダイアログを出した: ${names()}`);
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
  W.wk.excluded = ["app/node_modules"];
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
  // 写しの全項目（表 SHELF の鍵から = 項目を表に足せば自動でここにも入る）。そのまま預ける配列は同じ物かを比べる（複製しない）・Set / Map と複製する配列は中身
  const snap = () =>
    Object.fromEntries(
      W.SHELF_KEYS.map((k) => {
        const v = W.wk[k];
        if (k === "status") return [k, [v?.root, v?.pages]];
        if (W.SHELF[k].copy === "same" && Array.isArray(v)) return [k, v];
        return [k, v instanceof Set || v instanceof Map || Array.isArray(v) ? [...v] : v];
      })
    );
  const a = snap();
  const gen = W.wk.embedGen;
  const prefs0 = [W.wk.infoOpen, W.wk.headColors, W.wk.split, W.wk.tocOpen];
  W.wk.infoOpen = !prefs0[0];
  W.wk.split = !prefs0[2];
  W.wk.tocOpen = !prefs0[3]; // 右ペインの目次の開閉も共通 = 畳んだまま切り替えても畳んだまま
  W.enterRoot("/ws/b"); // 同期で入れ替える（行き来の rootGuard と同じ）
  const b = snap();
  assert.equal(W.wk.status.root, "/ws/b");
  assert.deepEqual(
    [b.pages, b.excluded, b.tags, b.inbox, b.chatLog, b.open, b.titleOverrides, b.newDirs, b.q, b.qTags, b.showResults, b.results, b.error, b.treeError, b.refDirs],
    [[], [], [], [], [], [], [], [], "", [], false, [], null, null, []],
    "初めての root に前の root の状態が残った"
  );
  assert.deepEqual(W.wk.status.workspaces.map((w) => w.root), be.hist, "履歴を引き継がない（ワークスペースによらない）");
  // 前面のページと前の root を指すものは捨てる（写しに入れない）
  assert.deepEqual([W.wk.page, W.wk.headings, W.wk.scrollTo, W.wk.menu], [null, [], null, null]);
  assert.ok(W.wk.embedGen > gen, "埋め込みの読みの世代を進めていない");
  // 表示の好みは共通
  assert.deepEqual([W.wk.infoOpen, W.wk.headColors, W.wk.split, W.wk.tocOpen], [!prefs0[0], prefs0[1], !prefs0[2], !prefs0[3]], "表示の好みを root ごとにした");
  // A のタブの題名は A の写しから（覚えた題名 → 索引）
  assert.equal(W.wikiTabTitle(t1.wiki), "X の覚え");
  assert.equal(W.wikiTabTitle({ path: "a-only.md", root: "/ws/a", scrollTop: 0, editing: false }), "A だけ");
  assert.equal(W.wikiTabTitle({ path: "zz/none.md", root: "/ws/zz", scrollTop: 0, editing: false }), "none", "写しの無い root はファイル名");
  calls.length = 0;
  W.enterRoot("/ws/a");
  const back = snap();
  for (const k of ["pages", "tags", "inbox", "chatLog", "results"]) assert.equal(back[k], a[k], `${k} を戻さない（同じ配列 = 複製しない）`);
  assert.deepEqual(back, a, "A の写しが戻らない");
  assert.equal(W.wk.tocOpen, !prefs0[3], "元の root へ戻ったら目次の開閉が変わった");
  W.wk.infoOpen = prefs0[0];
  W.wk.split = prefs0[2];
  W.wk.tocOpen = prefs0[3];
  // 戻ったら読み直しで追いつく（状態・木・参照フォルダ・検索の結果）
  assert.ok(await waitFor(() => calls.some((c) => c.name === "wiki_list") && calls.some((c) => c.name === "wiki_search")), `読み直さない: ${names()}`);
  for (const n of ["wiki_status", "wiki_list", "wiki_ref_dirs_get", "wiki_search"]) assert.equal(calls.find((c) => c.name === n)?.args?.root, "/ws/a", `${n} の root`);
  // 以前の leaveRoot が捨てていた項目 = 写し（SHELF_KEYS）か、捨てる側（前面のページ・前の root を指すもの）のどちらか
  const LEAVE_ROOT = ["open", "titleOverrides", "newDirs", "page", "pageError", "headings", "scrollTo", "error", "embedGen", "pages", "treeError", "tags", "tagsTotal", "inbox", "inboxTotal", "chatLog", "chatLogTotal", "q", "qTags", "sel", "showResults", "results", "resultsTotal", "searching", "searchError", "dialog", "menu"];
  const RESET = ["page", "pageError", "headings", "scrollTo", "embedGen", "dialog", "menu", "searching"];
  for (const k of LEAVE_ROOT) assert.ok(W.SHELF_KEYS.includes(k) || RESET.includes(k), `leaveRoot の ${k} を写しにも捨てる側にも入れていない`);
  for (const k of ["pages", "excluded", "tags", "inbox", "chatLog", "open", "titleOverrides", "newDirs", "q", "qTags", "results", "showResults", "sel", "error", "treeError", "refDirs", "status"]) {
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
  // 前面が一覧のタブ（ItemDetail の Wiki 節・セグメント LIBRARY → NOTE）= 最後に前面だった OpenNote の root
  S.app.tabs.push(mkTab(null));
  const lib = S.app.tabs.at(-1);
  await S.activateTab(lib.id);
  await W.openWikiPage("from-item.md", true);
  assert.deepEqual([S.app.tabs.at(-1).wiki.path, S.app.tabs.at(-1).wiki.root], ["from-item.md", "/ws/b"], "ItemDetail の節のタブが最後の OpenNote の root を継がない");
  await S.activateTab(lib.id);
  await M.switchMode("wiki");
  assert.deepEqual({ ...lib.wiki }, HOME("/ws/b"), "セグメントの NOTE が最後の OpenNote の root を継がない");
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

// ---- ⑨ 名前のメニューの × = 履歴から外す・× はタブが使っている root では出さない・右ペインのホームは概要だけ ----
{
  const [, tb] = await reset([[null, "/ws/cur"], ["x.md", "/ws/b"]], 0, "/ws/cur");
  Object.assign(be, { hist: ["/ws/cur", "/ws/site", "/ws/b", "/vol/gone"], gone: ["/vol/gone"] });
  await W.loadWikiStatus();
  await settle();
  // 右ペインのホーム = 概要だけ（ワークスペース・参照フォルダ・除外フォルダの節は名前のメニューへ移した）
  const { default: WikiInfo } = await import("./WikiInfo.svelte");
  W.wk.page = null;
  W.wk.refDirs = ["/ref"];
  W.wk.excluded = ["archive"];
  const infoT = document.createElement("div");
  document.body.append(infoT);
  const info = mount(WikiInfo, { target: infoT });
  flushSync();
  const heads = [...infoT.querySelectorAll("h3")].map((h) => h.textContent.trim());
  assert.deepEqual(heads, ["OpenNote"], `右ペインのホームに概要の外の節が残った: ${heads}`);
  assert.equal(infoT.querySelectorAll("button").length, 0, `右ペインのホームにボタンが残った: ${[...infoT.querySelectorAll("button")].map((b) => b.textContent)}`);
  assert.ok(infoT.textContent.includes("/ws/cur"), "概要の root が無い");
  unmount(info);
  infoT.remove();
  W.wk.excluded = [];
  const { default: WikiDialogs } = await import("./WikiDialogs.svelte");
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiDialogs, { target });
  const open = () => {
    W.wk.menu = { kind: "vault", path: W.wk.status.root, x: 10, y: 20 };
    flushSync();
  };
  open();
  const rows = () => [...target.querySelectorAll(".wk-menu-wsrow")];
  const row = (name) => rows().find((r) => r.querySelector(".wk-menu-name").textContent === name);
  const x = (name) => row(name)?.querySelector(".wk-menu-x") ?? null;
  assert.equal(rows().length, 4);
  // 画面の root = ✓・× なし・別のタブが使っている = × なし・どのタブも使っていない / 無いフォルダ = ×
  assert.equal(x("cur"), null, "現在のワークスペースに × が出た");
  assert.equal(x("b"), null, "タブが使っている root に × が出た");
  assert.ok(x("site") && x("gone"), "使っていない root に × が無い");
  assert.equal(x("gone").getAttribute("role"), "menuitem", "× がキーで届かない（menuitem でない）");
  assert.match(x("gone").getAttribute("aria-label"), /^このワークスペースを履歴から外す.*: gone$/, "× の読み上げに名前が無い");
  assert.ok(x("gone").title.includes("/vol/gone"), `× の title に root が無い: ${x("gone").title}`);
  calls.length = 0;
  await W.forgetWorkspace("/ws/b");
  assert.ok(!names().includes("wiki_forget_root"), "タブが使っている root を外した");
  // 押下: × = 確認なしで外す・メニューを閉じて名前のボタン（opener）へ焦点・wiki_forget_root {args:{root}} を 1 回
  calls.length = 0;
  raw.length = 0;
  const forgets = () => calls.filter((c) => c.name === "wiki_forget_root");
  // 消える物を title と読み上げで言う（確認は無いが嘘は書かない = 参照フォルダ・除外フォルダの設定と索引）
  for (const w of ["参照フォルダ", "除外フォルダ", "索引", "最近開いたページ"]) {
    assert.ok(x("gone").title.includes(w), `× の title に「${w}」が無い: ${x("gone").title}`);
    assert.ok(x("gone").getAttribute("aria-label").includes(w), `× の読み上げに「${w}」が無い`);
  }
  const opener = document.createElement("button");
  opener.className = "wk-vault-stub";
  document.body.append(opener);
  W.wk.menu = { ...W.wk.menu, opener };
  flushSync();
  let release = () => {};
  const forgetWas = handlers.wiki_forget_root;
  handlers.wiki_forget_root = (a) => new Promise((r) => (release = () => r(forgetWas(a))));
  const goneX = x("gone");
  goneX.focus();
  goneX.click();
  flushSync();
  assert.equal(W.wk.menu, null, "× でメニューが閉じない（行が詰まって 2 回目の押下が下の行に当たる）");
  assert.ok(document.activeElement === opener, `× の後の焦点が名前のボタンに無い: ${document.activeElement?.className}`);
  assert.ok(!target.querySelector(".overlay"), "× で確認の窓が出た（確認なしのはず）");
  assert.ok(await waitFor(() => forgets().length === 1), "× で wiki_forget_root を呼ばない");
  // 二度押し（ダブルクリックの 2 回目・Enter の連打）: 閉じたメニューの × と、実行中の forgetWorkspace の 2 回目 = どちらも外さない
  goneX.click();
  void W.forgetWorkspace("/vol/gone");
  await settle();
  assert.equal(forgets().length, 1, `二度押しで 2 回外した（${forgets().length} 回）`);
  release();
  await settle();
  handlers.wiki_forget_root = forgetWas;
  assert.deepEqual(rawOf("wiki_forget_root"), [{ args: { root: "/vol/gone" } }], "args で包んでいない / 2 回外した");
  opener.remove();
  open();
  // B のタブを閉じる = × が出る
  await S.closeTab(tb.id);
  open();
  assert.ok(x("b"), "タブを閉じても × が出ない");
  // 一覧の読み直し（wiki-state → wk.status の置き換え）に追随する
  W.wk.status = { ...W.wk.status, workspaces: W.wk.status.workspaces.slice(0, 2) };
  flushSync();
  assert.equal(rows().length, 2, "一覧の変化に追随しない");
  W.wk.menu = null;
  flushSync();
  // workspaceName = root の最後の区画（vaultName は画面の root で同じ規則）
  assert.equal(W.workspaceName("/Users/u/notes/wiki"), "wiki");
  assert.equal(W.workspaceName("/a/b/"), "b");
  assert.equal(W.workspaceName(""), "");
  assert.equal(W.vaultName(), "cur");
  unmount(cmp);
  target.remove();
  tests++;
}

// ---- ⑩ WikiDialogs の vault メニュー・WikiSidebar のメニューの位置 ----
{
  await reset([[null, "/ws/cur"]], 0, "/ws/cur");
  Object.assign(be, { hist: ["/ws/cur", "/ws/site", "/vol/gone"], gone: ["/vol/gone"] });
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
  assert.deepEqual(ws.map((b) => b.querySelector(".wk-menu-name").textContent), ["cur", "site", "gone"]);
  // 並び: ホーム → 見出し → 履歴（使っていない root に ×）→ 別のフォルダを選ぶ… → フォルダの設定…（参照フォルダの追加は窓の中）
  assert.deepEqual(items(), ["ホーム", "✓cur", "site", "×", "gone", "×", "別のフォルダを選ぶ…", "フォルダの設定…", "索引を作り直す"]);
  // 字の頭が揃う: × の外の全項目にチェックの欄・見出しも同じ字下げ（ページの [⋯] と同じ）
  const main = [...target.querySelectorAll(".wk-menu button:not(.wk-menu-x)")];
  assert.ok(main.every((b) => b.firstElementChild?.classList.contains("wk-menu-check")), `チェックの欄の無い項目: ${main.filter((b) => !b.firstElementChild?.classList.contains("wk-menu-check")).map((b) => b.textContent)}`);
  assert.ok([...target.querySelectorAll(".wk-menu-head")].every((h) => h.classList.contains("wk-menu-head-ind")), "見出しの字の頭が項目と揃わない");
  const [cur, site, gone] = ws;
  assert.equal(cur.disabled, true, "現在の行が押せる");
  assert.equal(cur.querySelector(".wk-menu-check").textContent, "✓");
  assert.equal(cur.title, "/ws/cur");
  assert.equal(site.disabled, false);
  assert.equal(site.querySelector(".wk-menu-check").textContent, "");
  assert.equal(gone.disabled, true, "無いフォルダの行が押せる");
  assert.ok(gone.classList.contains("wk-menu-gone"), "無いフォルダが dim でない");
  assert.ok(gone.title.includes("見つかりません") && gone.title.includes("/vol/gone"), `title: ${gone.title}`);
  // ホーム = 前面のタブをホームへ（openWikiPage(null)）・別のフォルダを選ぶ… = wiki_pick_root
  S.frontWiki().path = "notes/x.md";
  [...target.querySelectorAll(".wk-menu button")].find((b) => b.textContent.trim() === "ホーム").click();
  assert.ok(await waitFor(() => S.frontWiki()?.path === null), "ホームでホームへ移らない");
  assert.equal(W.wk.menu, null);
  W.wk.menu = { kind: "vault", path: "/ws/cur", x: 10, y: 20 };
  flushSync();
  calls.length = 0;
  [...target.querySelectorAll(".wk-menu button")].find((b) => b.textContent.trim() === "別のフォルダを選ぶ…").click();
  assert.ok(await waitFor(() => calls.some((c) => c.name === "wiki_pick_root")), "[別のフォルダを選ぶ…] が wiki_pick_root を呼ばない");
  assert.equal(W.wk.menu, null);
  await settle();
  W.wk.menu = { kind: "vault", path: "/ws/cur", x: 10, y: 20 };
  flushSync();
  const site2 = [...target.querySelectorAll(".wk-menu-ws")].find((b) => b.querySelector(".wk-menu-name").textContent === "site");
  // 押下 = メニューを閉じて切り替える（前面のタブだけ）
  calls.length = 0;
  site2.click();
  flushSync();
  assert.equal(W.wk.menu, null, "押下でメニューが閉じない");
  assert.ok(await waitFor(() => calls.some((c) => c.name === "wiki_switch_root")));
  assert.deepEqual(calls.find((c) => c.name === "wiki_switch_root").args, { root: "/ws/site" });
  assert.ok(await waitFor(() => S.frontWiki()?.root === "/ws/site"), "メニューの切替で前面のタブが替わらない");
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

  // WikiSidebar の vault 行: メニューの y の上限 = 窓の高さ − 見積もり（197 + 28 × 件数 = 「索引を作り直す」の 1 行を含む）
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
  assert.equal(W.wk.menu.y, ih - (197 + 28 * 3), `y の上限が件数の見積もりでない: ${W.wk.menu.y} (innerHeight ${ih})`);
  W.wk.menu = null;
  flushSync();
  // キーで開く（前は keyboard / opener を付けず、Enter で開いても焦点は名前の行に残り ↑↓ が効かなかった）:
  // 名前のボタンの Enter / Space / ↓ = 開いて最初の項目へ・↓ で次へ・Esc で閉じて名前のボタンへ。マウスのクリック（detail 1）は焦点を動かさない
  const tick2 = async () => {
    flushSync();
    await sleep(0);
    flushSync();
  };
  const inMenu = () => !!target.querySelector(".wk-menu")?.contains(document.activeElement);
  for (const key of ["Enter", " ", "ArrowDown"]) {
    vault.focus();
    const ev = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
    vault.dispatchEvent(ev);
    await tick2();
    assert.ok(ev.defaultPrevented, `${JSON.stringify(key)} の既定を止めていない（click で開き直す）`);
    assert.equal(W.wk.menu?.kind, "vault", `${JSON.stringify(key)} で名前のメニューが開かない`);
    assert.ok(W.wk.menu.keyboard === true && W.wk.menu.opener === vault, `${JSON.stringify(key)} で開いたメニューがキーの扱いでない`);
    assert.ok(inMenu() && document.activeElement.textContent.trim() === "ホーム", `${JSON.stringify(key)} で開いたら最初の項目へ焦点が行かない: ${document.activeElement?.textContent}`);
    const first = document.activeElement;
    first.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
    await tick2();
    assert.ok(inMenu() && document.activeElement !== first, "↓ で次の項目へ移らない");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await tick2();
    assert.equal(W.wk.menu, null, "Esc で閉じない");
    assert.ok(document.activeElement === vault, `Esc で名前のボタンへ戻らない: ${document.activeElement?.className}`);
  }
  // キーの click（detail 0）もキーの扱い・マウスの click（detail 1）は焦点を動かさない
  vault.focus();
  vault.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
  await tick2();
  assert.ok(W.wk.menu?.kind === "vault" && !W.wk.menu.keyboard && document.activeElement === vault, "マウスで開いたのに焦点を動かした");
  W.wk.menu = null;
  await tick2();
  vault.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 0 }));
  await tick2();
  assert.ok(W.wk.menu?.keyboard === true && inMenu(), "キーの click（detail 0）で最初の項目へ焦点が行かない");
  W.wk.menu = null;
  // 覆いの間は開かない
  S.app.importJob = { files: [], running: false, cancel: false };
  vault.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  vault.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
  await tick2();
  assert.equal(W.wk.menu, null, "覆いの間に名前のメニューが開いた");
  S.app.importJob = null;
  flushSync();
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
  const CH = await import("../chat/chat.svelte.ts");
  await reset([[null, "/ws/a"]], 0);
  CH.chat.status = { enabled: true, cli: "/c", version: "1", logged_in: true, auth: null, model: null, mcp: "/m", state: "idle", session_id: "S", root: "/ws/b" };
  assert.equal(W.rootInUse("/ws/b"), true, "チャットの子の root を使用中と見ない");
  assert.equal(W.rootInUse("/ws/c"), false);
  calls.length = 0;
  await W.forgetWorkspace("/ws/b");
  assert.ok(!names().includes("wiki_forget_root"), "チャットの子の root を外した");
  const { default: WikiDialogs } = await import("./WikiDialogs.svelte");
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiDialogs, { target });
  W.wk.menu = { kind: "vault", path: W.wk.status.root, x: 10, y: 20 };
  flushSync();
  const hasX = (name) => !![...target.querySelectorAll(".wk-menu-wsrow")].find((r) => r.querySelector(".wk-menu-name").textContent === name)?.querySelector(".wk-menu-x");
  assert.equal(hasX("b"), false, "チャットの子の root に × が出た");
  assert.equal(hasX("c"), true);
  // 子が止まった（root = null）= 出る
  CH.chat.status = { ...CH.chat.status, state: "off", root: null };
  flushSync();
  assert.equal(hasX("b"), true, "子が止まっても × が出ない");
  W.wk.menu = null;
  flushSync();
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

// ---- ⑲ 写しの表（SHELF）は 1 つ: 表の項目は 退避 / 復元 / 初めての root / OFF の全部に効く・wk の項目は「写す」か「写さない」のどちらかに分けてある ----
// 前は同じ 22 項目を 5 か所（型・SHELF_KEYS・takeShelf・blankShelf・putShelf）+ OFF の初期化に手で並べていた（OFF は 8 項目を戻していなかった）
{
  await reset([[null, "/ws/a"], [null, "/ws/b"]], 0);
  const W_ = W.wk;
  // OFF で戻していなかった項目（前は再び ON にしたとき、読み直しが済むまで / 読めなければずっと前の値が残った）
  const OFF_MISSED = { tagsTotal: 0, inboxTotal: 0, chatLogTotal: 0, resultsTotal: 0, treeError: null, searchError: null, refDirs: [], sel: 0 };
  Object.assign(W_, { tagsTotal: 3, inboxTotal: 4, chatLogTotal: 5, resultsTotal: 6, treeError: "木", searchError: "検索", refDirs: ["/ref-x"], sel: 2 });
  handlers.wiki_status = () => ({ enabled: false, root: "/ws/a" });
  await W.loadWikiStatus();
  assert.equal(S.app.wikiOn, false);
  const plain = (v) => (v instanceof Set || v instanceof Map ? [...v] : Array.isArray(v) ? [...v] : v && typeof v === "object" ? JSON.parse(JSON.stringify(v)) : v);
  for (const [k, want] of Object.entries(OFF_MISSED)) assert.deepEqual(plain(W_[k]), want, `OFF で ${k} を戻さない`);
  assert.deepEqual(plain(W_.status), { enabled: false, root: "/ws/a" }, "OFF の status は読んだ状態（戻さない）");

  await reset([[null, "/ws/a"], [null, "/ws/b"]], 0);
  // wk の項目の全部 = 写す（SHELF_KEYS）か、写さない（前面のページ・表示の好み・進行中の印・メニューとダイアログ = 下の一覧）のどちらか。
  // 新しい項目を wk に足したら、表に足すかこの一覧に足すかを決めないと落ちる
  const NOT_SHELVED = ["treeLoading", "page", "pageError", "headings", "scrollTo", "findRequest", "stamp", "changed", "embedGen", "infoOpen", "headColors", "reindexing", "tocOpen", "prefsReady", "searching", "split", "dialog", "menu"];
  assert.deepEqual(Object.keys(W_).sort(), [...W.SHELF_KEYS, ...NOT_SHELVED].sort(), "wk の項目が写しの表とも写さない一覧とも合わない");
  assert.deepEqual(W.SHELF_KEYS, Object.keys(W.SHELF), "SHELF_KEYS が表の順でない");
  // 表の項目ごとに目印を置く（型は表の空の値から決める = 表に足した項目は自動でここに入る）
  const mark = (k) => {
    const b = W.SHELF[k].blank("/x");
    if (k === "status") return { ...W_.status, pages: 4242 };
    if (b instanceof Set) return new b.constructor([`M-${k}`]);
    if (b instanceof Map) return new b.constructor([[`M-${k}`, k]]);
    if (Array.isArray(b)) return [`M-${k}`];
    if (typeof b === "number") return 42;
    if (typeof b === "boolean") return !b;
    if (typeof b === "string" || b === null) return `M-${k}`;
    throw new Error(`目印の作り方が無い型: ${k}（試験の mark に足す）`);
  };
  const put = {};
  for (const k of W.SHELF_KEYS) W_[k] = put[k] = mark(k);
  const held = Object.fromEntries(W.SHELF_KEYS.map((k) => [k, W_[k]])); // wk に入った物（proxy の配列なら proxy）
  // 期待値は試験が持つ（表から作らない = 表の空の値・写し方を書き間違えたら落ちる）。表に足したばかりでここに無い項目だけは表の空の値で見る
  const BLANK = {
    pages: [], excluded: [], treeError: null, tags: [], tagsTotal: 0, inbox: [], inboxTotal: 0, chatLog: [], chatLogTotal: 0, open: [], titleOverrides: [], newDirs: [],
    q: "", qTags: [], showResults: false, results: [], resultsTotal: 0, sel: 0, searchError: null, error: null, refDirs: [],
  };
  const blankOf = (k) => (Object.hasOwn(BLANK, k) ? BLANK[k] : plain(W.SHELF[k].blank("/x")));
  // 複製して預ける項目（A を離れた後で前の物を書き換えても、A の写しは変わらない）。status は snapshot（同じく変わらない）。ほかは同じ物を預ける
  const COPIED = ["excluded", "qTags", "refDirs"];
  // 初めての root = 全項目が空
  W.enterRoot("/ws/b");
  for (const k of W.SHELF_KEYS) {
    if (k === "status") assert.deepEqual([W_.status.root, W_.status.pages], ["/ws/b", undefined], "初めての root の status");
    else assert.deepEqual(plain(W_[k]), blankOf(k), `初めての root に ${k} の前の値が残った`);
  }
  // A を離れた後に、A で使っていた物（wk に入っていた物）を書き換える = 複製・snapshot の項目は A の写しに映らない
  for (const k of COPIED) held[k].push("後から足した");
  held.status.pages = 999;
  // 離れている間に履歴（workspaces）が変わった = 戻すとき今の履歴に差し替える（写しの中の履歴は古い）
  const ws0 = JSON.parse(JSON.stringify(W_.status.workspaces));
  W_.status = { ...W_.status, workspaces: [...ws0, { ...ws0[0], root: "/ws/後から" }] };
  // 戻る = 全項目が戻る（同じ物を預ける項目は同じ物・複製する項目は預けたときの中身）
  W.enterRoot("/ws/a");
  for (const k of W.SHELF_KEYS) {
    if (k === "status") {
      assert.equal(W_.status.root, "/ws/a");
      assert.equal(W_.status.pages, 4242, "status を snapshot で預けていない（離れた後の書換えが写しに映った）");
      assert.deepEqual(W_.status.workspaces.map((w) => w.root), [...ws0.map((w) => w.root), "/ws/後から"], "戻すとき履歴を今の物に差し替えない");
    } else if (COPIED.includes(k)) {
      assert.deepEqual(plain(W_[k]), [`M-${k}`], `${k} を複製して預けていない（離れた後の書換えが写しに映った）`);
      assert.notEqual(W_[k], held[k], `${k} が同じ物のまま戻った`);
    } else assert.ok(W_[k] === held[k] || W_[k] === put[k], `${k} を同じ物で戻さない`);
  }
  // OFF = 写しの全項目を空に（status だけは OFF の状態を読んだ物 = 戻さない）
  handlers.wiki_status = () => ({ enabled: false, root: "/ws/a" });
  await W.loadWikiStatus();
  for (const k of W.SHELF_KEYS) {
    if (k === "status") assert.deepEqual(plain(W_.status), { enabled: false, root: "/ws/a" }, "OFF の status");
    else assert.deepEqual(plain(W_[k]), blankOf(k), `OFF で ${k} を戻さない`);
  }
  assert.deepEqual(W.SHELF_KEYS.filter((k) => W.SHELF[k].off === "keep"), ["status"], "OFF で戻さない項目は status だけ");
  tests++;
}

// ---- ⑳ 「フォルダの設定…」の窓: 参照フォルダと除外フォルダの一覧・今の command・焦点の出入り・0 件・覆い ----
{
  await reset([[null, "/ws/a"]], 0);
  await settle();
  const { default: WikiDialogs } = await import("./WikiDialogs.svelte");
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiDialogs, { target });
  handlers.wiki_ref_dir_remove = (a) => ({ dirs: W.wk.refDirs.filter((d) => d !== a.path), restart: false });
  handlers.wiki_ref_dir_add = () => ({ dirs: [...W.wk.refDirs, "/Users/u/added"], added: "/Users/u/added" });
  handlers.wiki_exclude_set = (a) => ({ dir: a.dir, excluded: a.excluded });
  const long = "/Users/u/Library/CloudStorage/GoogleDrive-x/My Drive/very/deep/reference/folder/papers";
  W.wk.refDirs = ["/Users/u/ref", long];
  W.wk.excluded = ["archive", "notes/old 2024"];
  // メニューから開く = メニューを閉じて窓・焦点は [閉じる]
  const opener = document.createElement("button");
  document.body.append(opener);
  opener.focus();
  W.wk.menu = { kind: "vault", path: "/ws/a", x: 10, y: 20 };
  flushSync();
  [...target.querySelectorAll(".wk-menu button")].find((b) => b.textContent.trim() === "フォルダの設定…").click();
  flushSync();
  assert.equal(W.wk.menu, null, "フォルダの設定… でメニューが閉じない");
  assert.equal(W.wk.dialog?.kind, "folders");
  const modal = () => target.querySelector(".modal");
  assert.ok(modal()?.querySelector("h3")?.textContent.includes("フォルダの設定"), "窓の題が無い");
  await sleep(5);
  assert.ok(document.activeElement?.textContent.trim() === "閉じる" && modal().contains(document.activeElement), `焦点が窓の [閉じる] に無い: ${document.activeElement?.textContent}`);
  // 並び = 参照フォルダ → 除外フォルダ・説明の文は窓の中
  const secs = [...modal().querySelectorAll("h4")].map((h) => h.textContent.trim());
  assert.deepEqual(secs, ["参照フォルダ2", "除外フォルダ2"], `節の並び: ${secs}`);
  assert.ok(modal().textContent.includes("ワークスペースの外のフォルダ。その中の PDF・html・画像へのリンクを内蔵の表示で開きます"), "参照フォルダの説明が窓に無い");
  const rows = (i) => [...modal().querySelectorAll(".wk-fs-sec")[i].querySelectorAll(".wk-fs-row")];
  // 参照フォルダ = 絶対パス（先頭側を省略する rtl の型）・title に全体・[外す]
  assert.deepEqual(rows(0).map((r) => r.querySelector(".wk-fs-path").title), ["/Users/u/ref", long]);
  assert.ok(rows(0)[1].querySelector(".wk-fs-path").classList.contains("wk-fs-abs"), "長いパスを先頭側で省略しない");
  // 除外フォルダ = vault 相対 + /（フルパスを出さない）・title = 相対パス
  assert.deepEqual(rows(1).map((r) => r.querySelector(".wk-fs-path").textContent), ["archive/", "notes/old 2024/"]);
  assert.ok(!modal().querySelectorAll(".wk-fs-sec")[1].textContent.includes("/ws/a"), "除外フォルダに絶対パスを出した");
  // 外す / 追加… / 索引に入れる = 今の command（参照フォルダは画面の root・除外は木の右クリックと同じ wiki_exclude_set）
  calls.length = 0;
  raw.length = 0;
  const n = (name) => calls.filter((c) => c.name === name).length;
  // [外す]: 応答を待つ間の二度押しで 2 回呼ばない
  let releaseRm = () => {};
  const rmWas = handlers.wiki_ref_dir_remove;
  handlers.wiki_ref_dir_remove = (a) => new Promise((r) => (releaseRm = () => r(rmWas(a))));
  const rm = rows(0)[0].querySelector("button");
  rm.click();
  flushSync();
  rm.click();
  assert.ok(await waitFor(() => n("wiki_ref_dir_remove") > 0));
  assert.ok(rm.disabled, "応答を待つ間に [外す] が押せる");
  releaseRm();
  handlers.wiki_ref_dir_remove = rmWas;
  assert.deepEqual(rawOf("wiki_ref_dir_remove"), [{ args: { path: "/Users/u/ref", root: "/ws/a" } }], "二度押しで 2 回外した / args");
  assert.ok(await waitFor(() => (flushSync(), rows(0).length === 1)), "外した参照フォルダが一覧に残った");
  // 追加…: 取り消し（dirs: null・added: null）は一覧を変えず知らせない・選んだら一覧が替わる
  const addWas = handlers.wiki_ref_dir_add;
  handlers.wiki_ref_dir_add = () => ({ dirs: null, added: null });
  S.app.toast = "";
  [...modal().querySelectorAll("button")].find((b) => b.textContent.trim() === "追加…").click();
  assert.ok(await waitFor(() => n("wiki_ref_dir_add") === 1), "追加… が wiki_ref_dir_add を呼ばない");
  await settle();
  flushSync();
  assert.deepEqual(W.wk.refDirs, [long], "追加を取り消したのに一覧が変わった");
  assert.equal(S.app.toast, "", `追加を取り消したのに知らせた: ${S.app.toast}`);
  handlers.wiki_ref_dir_add = addWas;
  [...modal().querySelectorAll("button")].find((b) => b.textContent.trim() === "追加…").click();
  assert.ok(await waitFor(() => (flushSync(), rows(0).length === 2)), "追加した参照フォルダが一覧に出ない");
  assert.deepEqual(rawOf("wiki_ref_dir_add"), [{ args: { root: "/ws/a" } }, { args: { root: "/ws/a" } }]);
  // 索引に入れる: 一覧が追う（背景の走査の後）まで押せない = 二度押しで 2 回呼ばない
  const inc = rows(1)[1].querySelector("button");
  inc.click();
  flushSync();
  inc.click();
  assert.ok(await waitFor(() => n("wiki_exclude_set") > 0), "索引に入れる が wiki_exclude_set を呼ばない");
  await settle();
  flushSync();
  assert.ok(inc.disabled, "一覧が追う前に「索引に入れる」が押せる");
  inc.click();
  await settle();
  assert.equal(n("wiki_exclude_set"), 1, `二度押しで 2 回 wiki_exclude_set（${n("wiki_exclude_set")} 回）`);
  assert.deepEqual(calls.find((c) => c.name === "wiki_exclude_set").args, { dir: "notes/old 2024", excluded: false, root: "/ws/a" });
  W.wk.excluded = ["archive"]; // 一覧が追った（wiki-changed → 木の読み直し）
  flushSync();
  assert.deepEqual(rows(1).map((r) => r.querySelector(".wk-fs-path").textContent), ["archive/"]);
  // 失敗（wiki_exclude_set が投げる）= 押せるように戻す
  const exWas = handlers.wiki_exclude_set;
  handlers.wiki_exclude_set = () => {
    throw new Error("書けない");
  };
  const inc2 = rows(1)[0].querySelector("button");
  inc2.click();
  assert.ok(await waitFor(() => (flushSync(), !inc2.disabled && n("wiki_exclude_set") === 2)), "失敗の後に「索引に入れる」が押せないまま");
  handlers.wiki_exclude_set = exWas;
  assert.equal(W.wk.dialog?.kind, "folders", "一覧の操作で窓が閉じた");
  // 0 件の文
  W.wk.refDirs = [];
  W.wk.excluded = [];
  flushSync();
  assert.ok(modal().textContent.includes("参照フォルダはありません") && modal().textContent.includes("除外フォルダはありません"), "0 件の文が無い");
  assert.deepEqual([...modal().querySelectorAll("h4")].map((h) => h.textContent.trim()), ["参照フォルダ0", "除外フォルダ0"]);
  // [閉じる] = 閉じて開く前の焦点へ
  [...modal().querySelectorAll("button")].find((b) => b.textContent.trim() === "閉じる").click();
  flushSync();
  await sleep(5);
  assert.equal(W.wk.dialog, null, "[閉じる] で閉じない");
  assert.ok(document.activeElement === opener, `閉じた後の焦点が開く前の所へ戻らない: ${document.activeElement?.tagName}`);
  // Esc でも閉じる
  W.openFolderSettings();
  flushSync();
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  flushSync();
  assert.equal(W.wk.dialog, null, "Esc で閉じない");
  // 覆いの下では開かない（覆いが出たらメニューも閉じる = 今の決まり）
  S.app.importJob = { files: [], running: false, cancel: false };
  W.openFolderSettings();
  flushSync();
  assert.equal(W.wk.dialog, null, "覆いの下でフォルダの設定の窓が開いた");
  W.wk.menu = { kind: "vault", path: "/ws/a", x: 10, y: 20 };
  flushSync();
  assert.equal(W.wk.menu, null, "覆いの下で名前のメニューが開いたまま");
  S.app.importJob = null;
  flushSync();
  opener.remove();
  unmount(cmp);
  target.remove();
  tests++;
}

// ---- ㉑ 「索引を作り直す」は名前のメニューの 1 か所・ヘッダーに [再索引] が無い・ヘッダーの状態の表示は残る ----
{
  await reset([[null, "/ws/a"]], 0);
  await settle();
  const { default: WikiDialogs } = await import("./WikiDialogs.svelte");
  const { default: WikiHeader } = await import("./WikiHeader.svelte");
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiDialogs, { target });
  const head = document.createElement("div");
  document.body.append(head);
  const hdr = mount(WikiHeader, { target: head });
  flushSync();
  const reindexes = () => calls.filter((c) => c.name === "wiki_reindex").length;
  const open = () => {
    W.wk.menu = { kind: "vault", path: "/ws/a", x: 10, y: 20 };
    flushSync();
  };
  const item = () => [...target.querySelectorAll(".wk-menu button")].find((b) => b.textContent.trim() === "索引を作り直す");
  // ヘッダーに [再索引] が無い（入口は名前のメニューだけ）・[＋ 新規] と ▣ は残る
  const hbtns = [...head.querySelectorAll("button")].map((b) => b.textContent.trim());
  assert.ok(!hbtns.some((t) => t.includes("再索引") || t.includes("索引")), `ヘッダーに再索引の入口が残った: ${hbtns}`);
  assert.ok(head.querySelector(".wk-head-new") && head.querySelector(".pn-toggle"), "ヘッダーの [＋ 新規] / ▣ が無い");
  // 名前のメニューの「索引を作り直す」= 字の頭を揃える（チェックの欄）・title・押すと閉じて wiki_reindex 1 回（command を待つ間は押せない・2 回目を呼ばない）
  open();
  assert.ok(item(), "名前のメニューに「索引を作り直す」が無い");
  assert.ok(item().firstElementChild?.classList.contains("wk-menu-check"), "「索引を作り直す」の字の頭が揃わない");
  assert.match(item().title, /索引を作り直す/);
  let release = () => {};
  handlers.wiki_reindex = () => new Promise((r) => (release = () => r(null)));
  calls.length = 0;
  W.wk.error = "監視が止まった";
  item().click();
  flushSync();
  assert.equal(W.wk.menu, null, "押してもメニューが閉じない");
  assert.equal(reindexes(), 1, "押しても wiki_reindex を呼ばない");
  assert.deepEqual(rawOf("wiki_reindex"), [undefined], `wiki_reindex の引数が前と違う（引数なし = 設定の root）: ${JSON.stringify(rawOf("wiki_reindex"))}`);
  assert.equal(W.wk.error, null, "作り直しで監視の失敗の印を消さない（前の [再索引] と同じ）");
  open();
  assert.equal(item().disabled, true, "command を待つ間に「索引を作り直す」が押せる");
  void W.reindex();
  await settle();
  assert.equal(reindexes(), 1, `command を待つ間に 2 回 wiki_reindex（${reindexes()} 回）`);
  release();
  assert.ok(await waitFor(() => (flushSync(), item()?.disabled === false)), "command が返っても押せない");
  // 作り直しの間（wiki_status.rebuilding）= 押せない・ヘッダーに進捗
  W.wk.status = { ...W.wk.status, rebuilding: { percent: 40 } };
  flushSync();
  assert.equal(item().disabled, true, "作り直しの間に「索引を作り直す」が押せる");
  void W.reindex();
  await settle();
  assert.equal(reindexes(), 1, "作り直しの間に wiki_reindex を呼んだ");
  assert.match(head.textContent, /索引を作成中… 40%/, "ヘッダーの作り直しの進捗が消えた");
  W.wk.status = { ...W.wk.status, rebuilding: null, statusError: "読めない" };
  flushSync();
  assert.match(head.textContent, /OpenNote の状態を読めません/, "ヘッダーの「状態を読めない」が消えた");
  W.wk.status = { ...W.wk.status, statusError: undefined };
  W.wk.error = "監視が止まった";
  flushSync();
  assert.match(head.textContent, /変更の自動反映が止まっています/, "ヘッダーの「自動反映が止まった」が消えた");
  W.wk.error = null;
  // ワークスペースを揃えられない間（frontError）= 押せない・title で理由・reindex も呼ばない（command は設定の root = 前のワークスペースを作り直す）
  W.wk.status = { ...W.wk.status, frontError: "…見つかりません" };
  open();
  assert.equal(item().disabled, true, "ワークスペースを切り替えられない間に「索引を作り直す」が押せる");
  assert.match(item().title, /切り替えられない間は作り直せません/, `押せない理由が title に無い: ${item().title}`);
  calls.length = 0;
  void W.reindex();
  await settle();
  assert.equal(reindexes(), 0, "ワークスペースを切り替えられない間に wiki_reindex を呼んだ");
  W.wk.status = { ...W.wk.status, frontError: undefined };
  W.wk.menu = null;
  flushSync();
  // 失敗 = toast・押せるように戻る
  handlers.wiki_reindex = () => {
    throw new Error("作れない");
  };
  S.app.toast = "";
  open();
  item().click();
  assert.ok(await waitFor(() => S.app.toast.includes("OpenNote の索引を作り直せません")), `失敗の toast が無い: ${S.app.toast}`);
  assert.equal(W.wk.reindexing, false, "失敗の後も押せないまま");
  // 覆いの下では呼ばない（メニューは覆いで閉じる = 今の決まり・reindex 自身の門）
  handlers.wiki_reindex = () => null;
  S.app.importJob = { files: [], running: false, cancel: false };
  calls.length = 0;
  void W.reindex();
  await settle();
  assert.equal(reindexes(), 0, "覆いの下で wiki_reindex を呼んだ");
  S.app.importJob = null;
  flushSync();
  W.wk.menu = null;
  unmount(hdr);
  head.remove();
  unmount(cmp);
  target.remove();
  tests++;
}

console.warn = origWarn;
await sleep(5);
console.log(`wiki workspaces: ok (${tests} tests)`);
process.exit(0);
