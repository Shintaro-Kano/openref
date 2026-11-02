/* OpenNote の編集の部品テスト（node app/src/lib/wiki/edit.test.mjs・Svelte 5 の実部品 + 実 CodeMirror + jsdom）。
   Tauri の invoke は差し替え（Rust の command は契約 types.ts の形で返す）。確かめること:
   ① 自動保存の debounce（入力停止まで書かない・続けて打っても 1 回・if_sha256 = 読んだ / 書いた hash）
   ② conflict の分岐（自動保存を止める・離れられない・ダイアログの「読み直す」= wiki_read_gui で置き換え /「上書き」= force）
   ③ 外部の書換え（未保存が無ければ黙って読み直す・あれば触らない）
   ④ caret への挿入（OS ドロップの wiki_import_file・貼付画像の wiki_save_blob・書式キー）
   ⑤ 保存に失敗したままタブを閉じようとすると止まる（state.svelte の closeTab → wiki.svelte の leave guard）
   ⑥〜⑮ 本文を失わない終わり方・root の変化・ゴミ箱・改行・終了の要求・OS ドロップ・WikiPage の配線（各節の見出しに書く）
   ⑯ 起動時に参照フォルダの一覧を読む（設定に残っているのに一覧が空のまま = リンクが外部で開く、を防ぐ）
   ⑰ 「/」メニュー（editor/slash.ts）: 開く条件・絞り込み・雛形の挿入（行の頭 / 文の途中）・埋め込みはファイル選択 → wiki_import_file
   ⑱ PICK_EXT = core の media_kind（取り込める拡張子の一覧の一致を Rust のソースを読んで固定）
   ⑲ WikiInfo（右ペイン）は保存のたびにバックリンクと文献カードを読み直さない（wk.page の置換 ≠ ページの変化。別のページ・items の変化では読む）
   ⑳-0 ShadowPage の描画の計測（performance.measure）は旗 __wkMeasure のときだけ残る・`![[ページ]]` の読みは世代（wk.embedGen = 索引の変更 + 保存の成功）に 1 回（失敗は覚えない）
   ⑳ MdView は参照フォルダ（refs）を足すと表示中のリンクの分類を描き直す（refs は描画の依存）
   ㉑ 分割のプレビューは保存・page 以外の props の変化で描き直さない（previewPage は path と ed.preview から組む。打鍵では描き直す）
   ㉒ Wiki のモーダル（wk.dialog・conflict・チャットの説明・救出）が開いている間はタブの切替・開く・閉じる（⌘1〜9・⌘⇧]・⌘T・⌘W）を受け付けない
   ㉓ Wiki OFF でダイアログ・メニュー・チャットの説明を畳む・「破棄して移る」の確認を待っていた閉じるは「移らない」で終わる（約束を宙に残さない・保存が通ってもタブは閉じない）
   ㉔ [＋ 新規] の再押下で閉じる（実部品 WikiHeader + WikiDialogs: ボタンの mousedown では閉じず click が閉じる・外側の mousedown では閉じる）
   出力 = `wiki edit: ok (N tests)`（N = 列 0 のブロックの数 = 検査の単位） */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { setupHarness } from "./test-harness.mjs";

// 土台（test-harness.mjs）: loader・jsdom の global 化・Tauri の偽物（invoke = handlers / dialog の open = globalThis.__dialogOpen）
const { calls, handlers, sleep } = setupHarness();
const writes = () => calls.filter((c) => c.name === "wiki_save");
let tests = 0;

const { mount, unmount, flushSync } = await import("svelte");
const E = await import("./edit.svelte.ts");
const { createEditor } = await import("./editor/cm.ts");
const F = await import("./editor/format.ts");
const notices = [];
E.setNotice((m) => notices.push(m));
E.cfg.delay = 40;
E.cfg.previewDelay = 5;

const page = (path, content, sha) => ({ path, abs_path: "/v/" + path, kind: "md", title: path, size: content.length, mtime: 0, content, sha256: sha, truncated: false });
const host = document.createElement("div");
document.body.append(host);
/** 実 CodeMirror の handle（打鍵の代わりに dispatch） */
function editor(doc) {
  const h = createEditor(host, { doc, mode: "md", onChange: E.changed });
  E.attach(h);
  return h;
}
const type = (h, text, at = h.view.state.doc.length) => h.view.dispatch({ changes: { from: at, insert: text }, userEvent: "input" });

// ---------- ① 自動保存の debounce ----------
{
  calls.length = 0;
  let n = 0;
  handlers.wiki_save = (a) => ({ ok: true, path: a.path, sha256: `S${++n}`, size: a.content.length, mtime: 1, created: false });
  E.startEdit(page("a.md", "hello", "S0"));
  const h = editor("hello");
  type(h, " w");
  await sleep(10);
  type(h, "o");
  await sleep(10);
  type(h, "rld");
  assert.equal(E.ed.state, "dirty");
  await sleep(20);
  assert.equal(writes().length, 0, "入力停止（800ms 相当）の前に書いた");
  await sleep(60);
  assert.equal(writes().length, 1, "続けて打っても保存は 1 回");
  assert.deepEqual(writes()[0].args, { path: "a.md", content: "hello world", if_sha256: "S0" }, "読んだときの sha256 で照合");
  assert.equal(E.ed.state, "saved");
  type(h, "!");
  await sleep(70);
  assert.equal(writes().length, 2);
  assert.equal(writes()[1].args.if_sha256, "S1", "保存の後は書いた sha256 で照合");
  // ⌘S（flush）= 待たずに書く・変更が無ければ書かない
  type(h, "?");
  assert.equal(await E.flush(), true);
  assert.equal(writes().length, 3);
  assert.equal(await E.flush(), true);
  assert.equal(writes().length, 3, "変更なしで書いた");
  assert.equal(E.leave(), true, "保存済みなら同期で離れてよい");
  E.detach(h);
  h.destroy();
  E.stopEdit();
  tests++;
}

// ---------- ② conflict の分岐 + ダイアログ ----------
const { default: WikiConflict } = await import("./WikiConflict.svelte");
async function conflictCase(choice) {
  calls.length = 0;
  handlers.wiki_save = (a) =>
    a.force ? { ok: true, path: a.path, sha256: "MINE", size: 1, mtime: 2, created: false } : { ok: false, conflict: true, path: a.path, sha256: "THEIRS" };
  handlers.wiki_read_gui = (a) => page(a.path, "外部の内容\n", "THEIRS");
  E.startEdit(page("b.md", "base\n", "B0"));
  const h = editor("base\n");
  type(h, "mine");
  await sleep(70);
  assert.equal(writes().length, 1);
  assert.equal(E.ed.state, "conflict", "conflict で黙って勝たない");
  assert.equal(E.ed.dialog, true, "ダイアログを出す");
  assert.equal(E.ed.theirs, "THEIRS");
  type(h, "more");
  await sleep(70);
  assert.equal(writes().length, 1, "conflict の間は自動保存しない");
  const lv = E.leave();
  assert.notEqual(lv, true);
  assert.equal(await lv, false, "conflict のまま離れられない");

  const target = document.createElement("div");
  document.body.append(target);
  let reloaded = null;
  const cmp = mount(WikiConflict, { target, props: { onReloaded: (p) => (reloaded = p) } });
  flushSync();
  const btn = (label) => [...target.querySelectorAll("button")].find((b) => b.textContent.trim() === label);
  assert.ok(btn("読み直す") && btn("上書き") && btn("キャンセル"), "3 つのボタン");
  btn(choice).click();
  await sleep(10);
  flushSync();
  if (choice === "読み直す") {
    assert.ok(calls.some((c) => c.name === "wiki_read_gui" && c.args.path === "b.md"), "読み直す = wiki_read_gui");
    assert.equal(h.getText(), "外部の内容\n", "外部の内容で置き換えた");
    assert.equal(E.ed.state, "clean");
    assert.equal(reloaded?.sha256, "THEIRS");
    assert.equal(writes().length, 1, "読み直すで書いた");
    // 置き換えた後は外部の hash で照合する
    handlers.wiki_save = (a) => ({ ok: true, path: a.path, sha256: "N", size: 1, mtime: 3, created: false });
    type(h, "x");
    assert.equal(await E.flush(), true);
    assert.equal(writes().at(-1).args.if_sha256, "THEIRS");
  } else if (choice === "上書き") {
    const w = writes().at(-1);
    assert.equal(w.args.force, true, "上書き = force");
    assert.equal(w.args.if_sha256, "B0", "上書きも同じ照合（if_sha256）+ force");
    assert.equal(w.args.content, "base\nminemore");
    assert.equal(E.ed.state, "saved");
    assert.equal(E.leave(), true);
  } else {
    assert.equal(E.ed.dialog, false, "キャンセル = ダイアログだけ閉じる");
    assert.equal(E.ed.state, "conflict", "自動保存は止めたまま");
    assert.equal(await E.flush(), false);
    assert.equal(E.ed.dialog, true, "⌘S でまた尋ねる");
  }
  unmount(cmp);
  target.remove();
  E.detach(h);
  h.destroy();
  E.discardEdit();
}
{
  await conflictCase("読み直す");
  await conflictCase("上書き");
  await conflictCase("キャンセル");
  tests++;
}

// ---------- ③ 外部の書換え（wiki-changed） ----------
{
  calls.length = 0;
  handlers.wiki_read_gui = (a) => page(a.path, "claude が書いた\n", "EXT");
  E.startEdit(page("c.md", "orig\n", "C0"));
  const h = editor("orig\n");
  const p = await E.refreshFromDisk();
  assert.equal(p.sha256, "EXT");
  assert.equal(h.getText(), "claude が書いた\n", "未保存が無ければ黙って読み直す");
  assert.equal(E.ed.state, "clean");
  // 自分の保存の知らせ（hash が同じ）では何もしない
  handlers.wiki_read_gui = (a) => page(a.path, "claude が書いた\n", "EXT");
  await E.refreshFromDisk();
  // 未保存があれば本文に触らない（次の保存が conflict）
  type(h, "編集中");
  handlers.wiki_read_gui = (a) => page(a.path, "また外部\n", "EXT2");
  await E.refreshFromDisk();
  assert.equal(h.getText(), "claude が書いた\n編集中", "未保存を外部の内容で消した");
  handlers.wiki_save = (a) => ({ ok: false, conflict: true, path: a.path, sha256: "EXT2" });
  assert.equal(await E.flush(), false);
  assert.equal(writes().at(-1).args.if_sha256, "EXT", "照合は最後に読んだ hash");
  assert.equal(E.ed.state, "conflict");
  E.detach(h);
  h.destroy();
  E.discardEdit();
  tests++;
}

// ---------- ④ caret への挿入（ドロップ・貼付・書式） ----------
{
  calls.length = 0;
  handlers.wiki_import_file = (a) => ({ path: "attachments/2026-10/" + a.abs.split("/").pop(), embed: `![[${a.abs.split("/").pop()}]]`, copied: true });
  E.startEdit(page("d.md", "前後", "D0"));
  const h = editor("前後");
  h.view.dispatch({ selection: { anchor: 1 } }); // 「前|後」
  assert.equal(await E.dropPaths(["/Users/x/a.pdf", "/Users/x/b.mp4"]), true);
  assert.deepEqual(calls.filter((c) => c.name === "wiki_import_file").map((c) => c.args), [
    { abs: "/Users/x/a.pdf", page: "d.md" },
    { abs: "/Users/x/b.mp4", page: "d.md" },
  ]);
  assert.equal(h.getText(), "前![[a.pdf]]\n![[b.mp4]]後", "caret の位置に挿入（マウスの位置ではない）");
  assert.equal(h.view.state.selection.main.head, 1 + "![[a.pdf]]\n![[b.mp4]]".length, "caret は挿入の後ろ");
  assert.equal(E.ed.state, "dirty", "挿入も未保存の変更");

  // 貼付画像 → wiki_save_blob → caret に embed（選択は置き換える）
  handlers.wiki_save_blob = (a) => ({ path: "attachments/2026-10/" + a.name, embed: `![[${a.name}]]` });
  h.view.dispatch({ selection: { anchor: 0, head: 1 } }); // 「前」を選択
  const png = new File([new Uint8Array([137, 80, 78, 71])], "image.png", { type: "image/png" });
  await E.pasteImages([png]);
  const sb = calls.find((c) => c.name === "wiki_save_blob");
  assert.equal(sb.args.data_base64, "iVBORw==", "base64");
  assert.equal(sb.args.page, "d.md");
  assert.match(sb.args.name, /^Pasted image \d{8}-\d{6}\.png$/, "クリップボードの既定名は日時に");
  assert.ok(h.getText().startsWith(`![[${sb.args.name}]]![[a.pdf]]`), "選択を embed で置き換えた");
  assert.equal(E.blobName({ name: "scan.jpg", type: "image/jpeg" }), "scan.jpg", "元名は残す");

  // 一部が失敗しても成功した分は入れる・失敗は知らせる
  handlers.wiki_import_file = (a) => {
    if (a.abs.endsWith("bad.exe")) throw new Error("拒否");
    return { path: "attachments/x.png", embed: "![[x.png]]", copied: false };
  };
  const before = notices.length;
  h.view.dispatch({ selection: { anchor: h.view.state.doc.length } });
  await E.dropPaths(["/a/bad.exe", "/v/x.png"]);
  assert.ok(h.getText().endsWith("![[x.png]]"));
  assert.equal(notices.length, before + 1, "失敗を知らせる");

  // 書式キー
  const g = createEditor(document.createElement("div"), { doc: "abc", mode: "md", onChange: () => {} });
  g.view.dispatch({ selection: { anchor: 0, head: 3 } });
  F.toggleWrap(g.view, "**");
  assert.equal(g.getText(), "**abc**");
  F.toggleWrap(g.view, "**");
  assert.equal(g.getText(), "abc", "もう一度で外す");
  g.view.dispatch({ selection: { anchor: 0, head: 3 } });
  F.insertLink(g.view);
  assert.equal(g.getText(), "[abc]()");
  assert.equal(g.view.state.selection.main.head, 6, "caret は URL の位置");
  g.setText("a\nb");
  g.view.dispatch({ selection: { anchor: 0, head: 3 } });
  F.toggleList(g.view, false);
  assert.equal(g.getText(), "- a\n- b");
  F.toggleList(g.view, true);
  assert.equal(g.getText(), "1. a\n2. b");
  F.toggleList(g.view, true);
  assert.equal(g.getText(), "a\nb");
  // CRLF のファイルは CRLF のまま書き戻す
  const crlf = createEditor(document.createElement("div"), { doc: "x\r\ny", mode: "md", onChange: () => {} });
  assert.equal(crlf.getText(), "x\r\ny");
  // 先頭の frontmatter は 1 つの塊（setext 見出しにしない）・閉じの無い `---` は従来どおり
  const { ensureSyntaxTree } = await import("@codemirror/language");
  const names = (doc) => {
    const ed2 = createEditor(document.createElement("div"), { doc, mode: "md", onChange: () => {} });
    const out = [];
    ensureSyntaxTree(ed2.view.state, doc.length, 5000).iterate({ enter: (n) => void out.push(n.name) });
    ed2.destroy();
    return out;
  };
  const fmNames = names("---\ntitle: a\ntags: [x]\n---\n# 見出し\n");
  assert.ok(fmNames.includes("Frontmatter") && !fmNames.some((n) => /^SetextHeading/.test(n)), `frontmatter: ${fmNames}`);
  assert.ok(fmNames.includes("ATXHeading1"), "frontmatter の後の見出し");
  assert.ok(!names("---\nただの区切り\n").includes("Frontmatter"), "閉じの無い --- を frontmatter にした");
  g.destroy();
  crlf.destroy();

  E.detach(h);
  h.destroy();
  E.discardEdit();
  assert.equal(await E.dropPaths(["/a.pdf"]), false, "編集中でなければ挿入しない（呼び出し側が知らせる）");
  tests++;
}

// 編集できるページの判定（途中まで・UTF-8 でない疑い・media は書き戻さない）
{
  assert.equal(E.editable(page("e.md", "ok", "x")), true);
  assert.equal(E.editable({ ...page("e.md", "ok", "x"), truncated: true }), false);
  assert.equal(E.editable(page("e.md", "bad �", "x")), false);
  assert.equal(E.editable({ ...page("e.png", "", "x"), kind: "media" }), false);
  // base（sha256）の無いページは編集させない（照合なしで書く経路を作らない）
  assert.equal(E.editable({ ...page("e.md", "ok", "x"), sha256: undefined }), false);
  tests++;
}

// ---------- ⑤ 保存に失敗したまま閉じようとすると止まる（破棄して移るは確認付き）・自動の再試行 ----------
const S = await import("../state.svelte.ts");
const W = await import("./wiki.svelte.ts"); // leave guard を登録する
const FL = await import("./files.svelte.ts"); // 新規 / 改名 / ゴミ箱（B-33 で wiki.svelte.ts から分けた）
const tick = () => sleep(5);
const waitFor = async (f, ms = 1000) => {
  for (let i = 0; i < ms / 5 && !f(); i++) await tick();
  return f();
};
const failWrite = () =>
  (handlers.wiki_save = () => {
    throw new Error("disk full");
  });
const okWrite = (sha = "OK") => (handlers.wiki_save = (a) => ({ ok: true, path: a.path, sha256: sha, size: a.content.length, mtime: 7, created: false }));
const tab = S.app.tabs[0];
/** 前面の Wiki タブで path を編集中・未保存あり */
function session(path, content, more = "!") {
  S.app.wikiOn = true;
  tab.wiki = { path, scrollTop: 0, editing: true };
  E.startEdit(page(path, content, path + "-0"));
  const h = editor(content);
  if (more) type(h, more);
  return h;
}
function close(h) {
  E.detach(h);
  h.destroy();
  E.discardEdit();
}
E.cfg.retry = 100000;
{
  calls.length = 0;
  failWrite();
  const h = session("f.md", "x", "y");
  let done = false;
  const p = S.closeTab(S.app.tab).then(() => (done = true)); // 最後の 1 枚 = 中身を外す前に保存
  assert.ok(await waitFor(() => W.wk.dialog?.kind === "discard"), "保存できないとき破棄して移るかを尋ねる");
  assert.equal(writes().length, 1, "閉じる前に保存を試みた");
  assert.ok(S.app.toast.includes("保存できていない変更"), `知らせ: ${S.app.toast}`);
  assert.equal(done, false, "答える前に閉じた");
  W.answerDiscard(false);
  await p;
  assert.ok(tab.wiki, "保存に失敗したのに Wiki タブの中身を外した");
  assert.equal(E.ed.state, "error");
  assert.equal(E.editText(), "xy", "本文を失った");
  // 2 枚目があっても前面は閉じない
  S.app.tabs.push({ ...tab, id: 999, wiki: null, viewer: null });
  const p2 = S.closeTab(S.app.tab);
  await waitFor(() => W.wk.dialog?.kind === "discard");
  W.answerDiscard(false);
  await p2;
  assert.equal(S.app.tabs.length, 2, "保存に失敗したのに前面のタブを閉じた");
  assert.equal(S.app.tab, tab.id);
  S.app.tabs.pop();
  // 別のページへの移動も止める
  const p3 = W.openWikiPage("g.md");
  await waitFor(() => W.wk.dialog?.kind === "discard");
  W.answerDiscard(false);
  await p3;
  assert.equal(tab.wiki.path, "f.md", "保存に失敗したのに別のページへ移った");
  // MY LIBRARY への切替（switchMode）も止める
  const p4 = W.switchMode("library");
  assert.ok(await waitFor(() => W.wk.dialog?.kind === "discard"), "switchMode が guard を通らない");
  W.answerDiscard(false);
  await p4;
  assert.ok(tab.wiki, "保存に失敗したのに MY LIBRARY へ切り替えた");
  // 別のページの rewrite 付き改名の前にも保存する（できなければ改名しない）
  handlers.wiki_rename = (a) => ({ path: a.to, rewritten: [] });
  const p5 = FL.doRename("other.md", "other2.md", true);
  assert.ok(await waitFor(() => W.wk.dialog?.kind === "discard"), "doRename が編集中のページを保存しない");
  W.answerDiscard(false);
  await p5;
  assert.ok(!calls.some((c) => c.name === "wiki_rename"), "保存できないのに改名した");
  // 一定の間合いで自動で再試行する
  E.cfg.retry = 30;
  type(h, "z");
  assert.equal(await E.flush(), false);
  okWrite("F1");
  assert.ok(await waitFor(() => E.ed.state === "saved", 500), `自動で再試行しない: ${E.ed.state}`);
  assert.equal(writes().at(-1).args.content, "xyz");
  E.cfg.retry = 100000;
  // 保存できれば閉じられる
  await S.closeTab(S.app.tab);
  assert.equal(tab.wiki, null, "保存できたのに閉じない");
  E.detach(h);
  h.destroy();
  E.stopEdit();
  tests++;
}
{
  // 破棄して移る = 書かずに閉じる
  failWrite();
  const h = session("f2.md", "a", "b");
  const p = S.closeTab(S.app.tab);
  await waitFor(() => W.wk.dialog?.kind === "discard");
  tab.wiki.editing = true;
  S.app.tabs.push({ ...tab, id: 998, wiki: { path: "x.md", scrollTop: 0, editing: true }, viewer: null });
  W.answerDiscard(true);
  assert.equal(S.app.tabs.at(-1).wiki.editing, false, "破棄したのにタブが編集のまま");
  S.app.tabs.pop();
  await p;
  assert.equal(tab.wiki, null, "破棄を選んだのに閉じない");
  assert.equal(E.ed.path, null);
  assert.equal(E.ed.stranded.length, 0, "破棄を選んだ本文を残した");
  E.detach(h);
  h.destroy();
  tests++;
}

// ---------- ⑥ guard を通らない終わり方でも本文を失わない（Wiki OFF・conflict のまま・書けない） ----------
handlers.wiki_list = () => ({ pages: [] });
handlers.wiki_tags = () => ({ tags: [], total: 0 });
handlers.wiki_inbox = () => ({ files: [], total: 0 });
handlers.wiki_settings = () => ({ enabled: false, root: "/v" });
handlers.get_ui_pref = () => null;
{
  calls.length = 0;
  handlers.wiki_save = () => {
    throw new Error("OpenNote は無効");
  };
  W.wk.status = { enabled: true, root: "/v" };
  const h = session("off.md", "base", "+未保存");
  handlers.wiki_status = () => ({ enabled: false, root: "/v" });
  await W.loadWikiStatus();
  assert.equal(S.app.wikiOn, false);
  assert.equal(E.ed.path, null, "無効にしたのに編集が残った");
  assert.equal(E.ed.stranded.length, 1, "書けなかった本文を残していない");
  assert.equal(E.ed.stranded[0].content, "base+未保存");
  assert.equal(E.ed.stranded[0].path, "off.md");
  assert.equal(writes().length, 1, "無効にする前に保存を試みた");
  E.ed.stranded.length = 0;
  E.detach(h);
  h.destroy();
  tests++;
}
{
  // conflict のまま stopEdit = 書かずに残す
  handlers.wiki_save = (a) => ({ ok: false, conflict: true, path: a.path, sha256: "T" });
  const h = session("cf.md", "c", "+mine");
  assert.equal(await E.flush(), false);
  E.ed.dialog = false;
  E.stopEdit();
  assert.equal(E.ed.stranded.at(-1)?.content, "c+mine", "conflict の本文を黙って捨てた");
  E.detach(h);
  h.destroy();
  // 書けない stopEdit（照合付きで書きに行き、失敗したら残す）
  failWrite();
  const h2 = session("err.md", "e", "+x");
  E.stopEdit();
  assert.ok(await waitFor(() => E.ed.stranded.some((x) => x.path === "err.md")), "書けなかった本文を残していない");
  assert.equal(writes().at(-1).args.if_sha256, "err.md-0", "stopEdit も照合付き");
  E.ed.stranded.length = 0;
  E.detach(h2);
  h2.destroy();
  tests++;
}

// ---------- ⑦ vault のフォルダが変わった = 新しい root へ書かずに止める ----------
{
  S.app.wikiOn = true;
  W.wk.status = { enabled: true, root: "/old" };
  okWrite();
  const h = session("r.md", "r", "+edit");
  calls.length = 0;
  handlers.wiki_status = () => ({ enabled: true, root: "/new", pages: 0, inbox: 0 });
  await W.loadWikiStatus();
  assert.equal(writes().length, 0, "フォルダが変わったのに書いた");
  assert.equal(E.ed.path, null, "編集を止めていない");
  assert.equal(tab.wiki.editing, false, "表示に戻していない");
  assert.equal(E.ed.stranded.at(-1)?.content, "r+edit");
  assert.ok(S.app.toast.includes("フォルダが変わった"), S.app.toast);
  E.ed.stranded.length = 0;
  E.detach(h);
  h.destroy();
  tests++;
}

// ---------- ⑧ ゴミ箱は移せてから編集を捨てる ----------
{
  okWrite();
  const h = session("t.md", "t", "+keep");
  handlers.wiki_trash = () => {
    throw new Error("trash failed");
  };
  await FL.doTrash("t.md");
  assert.equal(E.ed.path, "t.md", "ゴミ箱に移せなかったのに編集を捨てた");
  assert.equal(E.editText(), "t+keep");
  assert.ok(S.app.toast.includes("ゴミ箱に移せません"));
  handlers.wiki_trash = (a) => ({ path: a.path, method: "trash" });
  calls.length = 0;
  await FL.doTrash("t.md");
  assert.equal(E.ed.path, null, "移せたのに編集が残った");
  assert.equal(E.ed.stranded.length, 0, "ゴミ箱へ移したページの本文を残した");
  await sleep(60);
  assert.equal(writes().length, 0, "ゴミ箱へ移した後に書きに行った");
  E.detach(h);
  h.destroy();
  tests++;
}

// ---------- ⑧b フォルダをゴミ箱へ = 中のページの編集を捨て・中のページのタブを閉じ・覚えていた空フォルダを忘れる ----------
{
  okWrite();
  const h = session("d/sub/t.md", "t", "+x");
  W.wk.newDirs.add("d/empty");
  W.wk.newDirs.add("d");
  W.wk.newDirs.add("dx"); // 前方一致で似た名前は巻き込まない
  S.app.tabs.push({ ...tab, id: 998, wiki: { path: "d/other.md", scrollTop: 0, editing: false }, viewer: null });
  S.app.tabs.push({ ...tab, id: 997, wiki: { path: "dx/keep.md", scrollTop: 0, editing: false }, viewer: null });
  calls.length = 0;
  // 移せなかった: 中のページの編集はそのまま・自動保存は再開（打鍵で書きに行く）・タブも newDirs も変わらない
  handlers.wiki_trash = () => {
    throw new Error("trash failed");
  };
  assert.ok(FL.editingUnder("d", true) && !FL.editingUnder("dx", true), "editingUnder");
  await FL.doTrash("d", true);
  assert.ok(S.app.toast.includes("ゴミ箱に移せません"), `toast: ${S.app.toast}`);
  assert.equal(E.ed.path, "d/sub/t.md", "移せなかったのに編集を捨てた");
  assert.equal(S.app.tabs.length, 3);
  assert.equal(W.wk.newDirs.size, 3);
  type(h, "!");
  await sleep(1400);
  assert.ok(writes().length >= 1, "移せなかった後、自動保存が再開していない");
  calls.length = 0;
  handlers.wiki_trash = (a) => ({ path: a.path, method: "trash", dir: true, files: 2 });
  handlers.list_items = () => []; // タブを閉じると次のタブへ切り替わり、下の蔵書の一覧を読み直す
  await FL.doTrash("d", true);
  assert.ok(S.app.toast.includes("ゴミ箱に移しました"), `toast: ${S.app.toast}`);
  assert.deepEqual(calls.filter((c) => c.name === "wiki_trash").map((c) => c.args), [{ path: "d", dir: true }], "wiki_trash はフォルダの path と dir で 1 回");
  assert.equal(E.ed.path, null, "中のページの編集を捨てていない");
  assert.ok(!S.app.tabs.some((x) => x.wiki?.path === "d/other.md"), "中のページのタブが残った");
  assert.ok(S.app.tabs.some((x) => x.wiki?.path === "dx/keep.md"), "似た名前のフォルダのタブを閉じた");
  assert.deepEqual([...W.wk.newDirs].sort(), ["dx"], "覚えていた空フォルダ");
  assert.ok(!S.app.tabs.some((x) => x.id === tab.id), "前面（中のページ）のタブを閉じていない");
  // 元のタブ 1 枚に戻す（以降の節は tab を前面として使う）
  S.app.tabs.splice(0, S.app.tabs.length, tab);
  S.app.tab = tab.id;
  tab.wiki = { path: null, scrollTop: 0, editing: false };
  W.wk.newDirs.clear();
  E.detach(h);
  h.destroy();
  tests++;
}

// ---------- ⑨ 保存と競合した読み直しを採らない・保存で wk.page を揃える ----------
{
  okWrite("NEW");
  const h = session("g.md", "old", "");
  W.wk.page = page("g.md", "old", "g.md-0");
  let release;
  handlers.wiki_read_gui = (a) => new Promise((r) => (release = () => r(page(a.path, "古い外部の内容", "STALE"))));
  const rp = E.refreshFromDisk();
  await tick();
  type(h, "+mine");
  assert.equal(await E.flush(), true); // 読む間に保存（saveGen が進む）
  assert.equal(W.wk.page.content, "old+mine", "保存したのに wk.page の本文が古い");
  assert.equal(W.wk.page.sha256, "NEW");
  release();
  assert.equal(await rp, null, "保存を追い越した読みを返した（adopt される）");
  assert.equal(h.getText(), "old+mine", "保存と競合した古い読みで本文を置き換えた");
  close(h);
  tests++;
}

// ---------- ⑩ ドロップの間に別のページへ移ったら入れない ----------
{
  const h = session("d1.md", "one", "");
  let release;
  handlers.wiki_import_file = () => new Promise((r) => (release = () => r({ path: "a/x.png", embed: "![[x.png]]", copied: true })));
  const dp = E.dropPaths(["/x.png"]);
  await tick();
  E.detach(h);
  h.destroy();
  E.discardEdit();
  const h2 = session("d2.md", "two", "");
  release();
  await dp;
  assert.equal(h2.getText(), "two", "別のページに挿入した");
  close(h2);
  tests++;
}

// ---------- ⑪ 改行（孤立 CR で打鍵なしに書かない・外部の改行コードに追随・CRLF へ LF を貼る）・U+FFFD ----------
{
  calls.length = 0;
  okWrite();
  S.app.wikiOn = true;
  tab.wiki = { path: "cr.md", scrollTop: 0, editing: true };
  E.startEdit(page("cr.md", "a\rb\r\nc", "CR0"));
  const h = editor("a\rb\r\nc");
  assert.equal(E.leave(), true, "開いただけで未保存になった");
  assert.equal(await E.flush(), true);
  assert.equal(writes().length, 0, "打鍵なしに書いた");
  type(h, "\nd"); // LF を入れる
  assert.equal(h.getText(), "a\r\nb\r\nc\r\nd", "CRLF の文書へ入れた LF を CRLF に揃えない");
  close(h);
  // 外部が LF → CRLF に変えた（未保存なし）= 読み直しで CRLF に追随
  const h2 = session("lf.md", "x\ny", "");
  handlers.wiki_read_gui = (a) => page(a.path, "x\r\ny\r\nz", "EXT");
  await E.refreshFromDisk();
  assert.equal(h2.getText(), "x\r\ny\r\nz");
  assert.equal(E.leave(), true);
  close(h2);
  // 自分で U+FFFD を書いた後は「UTF-8 でない」と誤判定しない
  okWrite("FF");
  const h3 = session("ff.md", "ok", "�");
  assert.equal(await E.flush(), true);
  assert.equal(E.editable(page("ff.md", "ok�", "FF")), true, "自分で書いた U+FFFD で編集不可にした");
  close(h3);
  tests++;
}

// ---------- ⑫ 終了の要求（wiki-exit-request → wiki_exit_ready {proceed}・args に包まない） ----------
{
  const answers = [];
  handlers.wiki_exit_ready = (a) => void answers.push(a.proceed);
  okWrite();
  const h = session("q.md", "q", "+1");
  await W.exitRequest();
  assert.deepEqual(answers, [true], "保存できたのに終了を止めた");
  assert.equal(writes().at(-1).args.content, "q+1", "終了の前に保存しない");
  assert.ok(calls.at(-1).name === "wiki_exit_ready" && calls.at(-1).args.proceed === true);
  failWrite();
  type(h, "+2");
  W.wk.dialog = null;
  await W.exitRequest(); // 確認ダイアログを待たずに即答える（待つ間に Rust の 15 秒で終了が進むので）
  assert.deepEqual(answers, [true, false], "保存できないのに終了させた");
  assert.equal(W.wk.dialog, null, "終了の要求で確認ダイアログを出した");
  assert.ok(S.app.toast.includes("終了を取りやめました"), S.app.toast);
  assert.equal(E.editText(), "q+1+2", "本文を失った");
  // conflict も即 false
  handlers.wiki_save = (a) => ({ ok: false, conflict: true, path: a.path, sha256: "T" });
  await E.flush();
  await W.exitRequest();
  assert.deepEqual(answers, [true, false, false], "conflict なのに終了させた");
  close(h);
  E.ed.stranded.push({ path: "s.md", content: "残り", reason: "x" });
  await W.exitRequest();
  assert.deepEqual(answers, [true, false, false, false], "残った本文があるのに終了させた");
  E.ed.stranded.length = 0;
  tests++;
}

// ---------- ⑬ openTab が guard で拒否されたら空のタブを残さない ----------
{
  failWrite();
  const h = session("o.md", "o", "+1");
  const n = S.app.tabs.length;
  const p = S.openTab({ kind: "all" });
  await waitFor(() => W.wk.dialog?.kind === "discard");
  W.answerDiscard(false);
  await p;
  assert.equal(S.app.tabs.length, n, "切り替えられなかったタブが残った");
  close(h);
  tests++;
}

// ---------- ⑭ 編集していなければ OS ドロップは取り込まず知らせるだけ・名前の組立てと検査 ----------
{
  tab.wiki = { path: "f.md", scrollTop: 0, editing: false };
  calls.length = 0;
  await W.wikiDrop(["/Users/x/a.pdf"]);
  assert.equal(calls.length, 0, "編集中でないのに取り込んだ");
  assert.ok(S.app.toast.includes("編集モードでドロップ"), `知らせ: ${S.app.toast}`);
  assert.equal(FL.renameTarget("dir/a.md", "PI-RADS v2.1"), "dir/PI-RADS v2.1.md");
  assert.equal(FL.renameTarget("dir/a.md", "b.md"), "dir/b.md");
  assert.equal(FL.renameTarget("a.html", "x/y"), "x/y.html");
  assert.equal(FL.pageName("note"), "note.md");
  assert.equal(FL.pageName("p.html"), "p.html");
  for (const bad of ["a[b", "a]b", "a|b", "a#b", "a^b", "a\\b", "a:b"]) assert.ok(FL.nameProblem(bad), `弾かない: ${bad}`);
  assert.equal(FL.nameProblem("PI-RADS v2.1"), null);
  assert.equal(FL.nameProblem("dir/sub"), null, "改名の / は通す");
  // 新規ページの形式: 名前の拡張子が優先・無ければ選んだ形式
  assert.equal(FL.pageExt("note", "md"), "md");
  assert.equal(FL.pageExt("note", "html"), "html");
  assert.equal(FL.pageExt("p.HTML", "md"), "html");
  assert.equal(FL.pageExt("p.htm ", "md"), "html");
  assert.equal(FL.pageExt("p.md", "html"), "md");
  assert.equal(FL.pageExt("p.md.", "html"), "md", "末尾の . は pageName と同じく落とす");
  assert.equal(FL.pageName("note", "html"), "note.html");
  assert.equal(FL.pageName("p.md", "html"), "p.md");
  // 新規フォルダの名前: ページの字に加えて / も不可
  assert.ok(FL.folderNameProblem("a/b"));
  assert.ok(FL.folderNameProblem("a|b"));
  assert.equal(FL.folderNameProblem("2026 読書"), null);
  assert.ok(FL.nameProblem(".hidden"), ". 始まりを通した");
  assert.ok(FL.nameProblem("dir/.x"), "区画の . 始まりを通した");
  calls.length = 0;
  await FL.previewRename("a.md", "x:y");
  assert.ok(!calls.some((c) => c.name === "wiki_rename_preview"), "使えない文字の名前で core を呼んだ");

  // 新規ページの流れ: wiki_create に Untitled.<形式> → 名前があれば wiki_rename {rewrite: false} → 編集で開く
  S.app.wikiOn = true;
  tab.wiki = { path: "d/f.md", scrollTop: 0, editing: false };
  handlers.wiki_list = () => ({ total: 0, count: 0, pages: [] });
  handlers.wiki_create = (a) => ({ path: (a.dir ? a.dir + "/" : "") + a.name });
  handlers.wiki_rename = (a) => ({ path: a.to, rewritten: [] });
  const made = () => calls.filter((c) => c.name === "wiki_create" || c.name === "wiki_rename").map((c) => [c.name, c.args]);
  calls.length = 0;
  W.wk.dialog = { kind: "new", dir: "d" };
  assert.equal(await FL.createPage("d", "memo", "html"), true);
  assert.deepEqual(made(), [
    ["wiki_create", { dir: "d", name: "Untitled.html" }],
    ["wiki_rename", { from: "d/Untitled.html", to: "d/memo.html", rewrite: false }],
  ]);
  assert.equal(W.wk.dialog, null);
  assert.deepEqual([tab.wiki.path, tab.wiki.editing], ["d/memo.html", true], "作ったページを編集で開く");
  calls.length = 0;
  await FL.createPage("", "p.md.", "html"); // 名前の拡張子が優先（末尾の . は落とす）= 作る形式も md
  assert.deepEqual(made(), [
    ["wiki_create", { dir: "", name: "Untitled.md" }],
    ["wiki_rename", { from: "Untitled.md", to: "p.md", rewrite: false }],
  ]);
  calls.length = 0;
  await FL.createPage("", "  ", "md"); // 空の名前 = Untitled のまま・改名しない
  assert.deepEqual(made(), [["wiki_create", { dir: "", name: "Untitled.md" }]]);
  assert.equal(tab.wiki.path, "Untitled.md");
  calls.length = 0;
  assert.equal(await FL.createPage("", ".html", "md"), false);
  assert.deepEqual(made(), [], ". 始まりの名前で core を呼んだ（Untitled が残る）");

  // 新規フォルダ: wiki_mkdir → 木に出すために覚え、祖先を開く。空・/ 入りは core を呼ばない
  handlers.wiki_mkdir = (a) => ({ path: (a.dir ? a.dir + "/" : "") + a.name });
  calls.length = 0;
  W.wk.dialog = { kind: "newdir", dir: "d/e" };
  assert.equal(await FL.createFolder("d/e", " 新しい "), true);
  assert.deepEqual(calls.filter((c) => c.name === "wiki_mkdir").map((c) => c.args), [{ dir: "d/e", name: "新しい" }]);
  assert.ok(W.wk.newDirs.has("d/e/新しい"), "作ったフォルダを覚えていない");
  for (const p of ["d", "d/e", "d/e/新しい"]) assert.ok(W.wk.open.has(p), `開いていない: ${p}`);
  assert.equal(W.wk.dialog, null);
  calls.length = 0;
  assert.equal(await FL.createFolder("", ""), false);
  assert.equal(await FL.createFolder("", "a/b"), false);
  assert.equal(calls.filter((c) => c.name === "wiki_mkdir").length, 0);

  // 改名の確認: 書き換えない参照がチャットのログ（kind "chat-log" = core は書き換えない）だけなら確認を出さずに改名・他の参照があれば確認
  const chatOnly = [{ path: "chat-log/x.md", kind: "chat-log" }];
  handlers.wiki_rename_preview = (a) => ({ from: a.from, to: a.to, links: 0, files: [], other: chatOnly });
  calls.length = 0;
  W.wk.dialog = { kind: "rename", path: "n.md" };
  await FL.previewRename("n.md", "m");
  assert.deepEqual(calls.filter((c) => c.name === "wiki_rename").map((c) => c.args), [{ from: "n.md", to: "m.md", rewrite: false }], "ログだけの参照で確認を出した");
  handlers.wiki_rename_preview = (a) => ({ from: a.from, to: a.to, links: 0, files: [], other: [...chatOnly, { path: "p.html", kind: "wiki" }] });
  calls.length = 0;
  W.wk.dialog = { kind: "rename", path: "n.md" };
  await FL.previewRename("n.md", "m");
  assert.equal(calls.filter((c) => c.name === "wiki_rename").length, 0, "html の参照があるのに確認せずに改名した");
  assert.equal(W.wk.dialog?.preview?.other?.length, 2, "確認を出さない");
  W.wk.dialog = null;
  tests++;
}

// ---------- ⑮ WikiPage の配線（実部品: ⌘E の往復・ゴミ箱の失敗・root の変化・Wiki OFF） ----------
{
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const { EditorView } = await import("@codemirror/view");
  okWrite("W1");
  const files = { "w.md": "# W\n\n本文\n" };
  handlers.wiki_read_gui = (a) => page(a.path, files[a.path], "W0");
  handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
  S.app.wikiOn = true;
  W.wk.status = { enabled: true, root: "/v" };
  W.wk.page = null;
  tab.wiki = { path: "w.md", scrollTop: 0, editing: false };
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  const has = (sel) => !!target.querySelector(sel);
  const cmView = () => EditorView.findFromDOM(target.querySelector(".cm-editor"));
  assert.ok(await waitFor(() => W.wk.page?.path === "w.md" && has(".wk-seg")), "ページを読んで切替を出す");
  const cmdE = () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "e", metaKey: true, bubbles: true }));
  cmdE();
  assert.ok(await waitFor(() => has(".cm-editor"), 3000), "⌘E でエディタを開かない");
  assert.equal(E.ed.path, "w.md");
  // 打鍵（実部品のエディタへ）→ ⌘E で表示へ = 保存してから
  cmView().dispatch({ changes: { from: cmView().state.doc.length, insert: "追記" }, userEvent: "input" });
  cmdE();
  assert.ok(await waitFor(() => !has(".cm-editor")), "⌘E で表示へ戻らない");
  assert.equal(writes().at(-1).args.content, "# W\n\n本文\n追記", "表示へ戻す前に保存しない");
  assert.equal(W.wk.page.content, "# W\n\n本文\n追記", "表示へ戻した wk.page が古い");
  cmdE();
  assert.ok(await waitFor(() => has(".cm-editor"), 3000), "⌘E の往復で再び開かない");
  assert.equal(cmView().state.doc.toString(), "# W\n\n本文\n追記", "開き直したエディタの本文が保存した内容でない");
  // ゴミ箱の失敗 = エディタも本文も残る
  cmView().dispatch({ changes: { from: 0, insert: "X" }, userEvent: "input" });
  handlers.wiki_trash = () => {
    throw new Error("no");
  };
  await FL.doTrash("w.md");
  await tick();
  assert.ok(has(".cm-editor"), "ゴミ箱に移せなかったのにエディタを閉じた");
  assert.ok(E.editText().startsWith("X# W"), "ゴミ箱に移せなかったのに本文を捨てた");
  // root の変化 = 書かずに止める（エディタを閉じ・本文を残す）
  calls.length = 0;
  handlers.wiki_status = () => ({ enabled: true, root: "/other", pages: 0, inbox: 0 });
  await W.loadWikiStatus();
  assert.ok(await waitFor(() => !has(".cm-editor")), "root が変わったのにエディタが残った");
  assert.equal(writes().length, 0, "root が変わったのに書いた");
  assert.ok(E.ed.stranded.at(-1)?.content.startsWith("X# W"), "root が変わった本文を残していない");
  E.ed.stranded.length = 0;
  // Wiki OFF = 本文を残して畳む
  tab.wiki.editing = true;
  assert.ok(await waitFor(() => has(".cm-editor"), 3000), "編集に戻れない");
  cmView().dispatch({ changes: { from: 0, insert: "OFF" }, userEvent: "input" });
  failWrite();
  calls.length = 0;
  handlers.wiki_status = () => ({ enabled: false, root: "/other" });
  await W.loadWikiStatus();
  assert.equal(S.app.wikiOn, false);
  assert.ok(E.ed.stranded.at(-1)?.content.startsWith("OFF"), "Wiki OFF で本文を捨てた");
  await sleep(50);
  assert.equal(writes().length, 1, `Wiki OFF で二重に書きに行った: ${writes().length}`);
  assert.equal(E.ed.stranded.filter((x) => x.content.startsWith("OFF")).length, 1, "同じ本文を二重に残した");
  unmount(cmp);
  target.remove();
  tests++;
}

// ---------- ⑯ 起動時に参照フォルダの一覧を読む ----------
// 起動 = loadWikiEnabled（wiki_settings）が先に app.wikiOn = true にするので、loadWikiStatus の「無効 → 有効」の読みは起動では走らない。
// 以前はこれで設定に残っているのに一覧が空のまま（リンクが外部で開いた）→ loadWikiEnabled が読む
{
  handlers.wiki_settings = () => ({ enabled: true, root: "/v" });
  handlers.wiki_ref_dirs_get = () => ({ dirs: ["/ref"] });
  S.app.wikiOn = false;
  W.wk.refDirs = [];
  await W.loadWikiEnabled();
  await new Promise((r) => setTimeout(r, 20));
  assert.equal(S.app.wikiOn, true);
  assert.deepEqual(W.wk.refDirs, ["/ref"], "起動時に参照フォルダを読まない");
  // 無効なら読まない
  handlers.wiki_settings = () => ({ enabled: false, root: "/v" });
  calls.length = 0;
  W.wk.refDirs = [];
  await W.loadWikiEnabled();
  await new Promise((r) => setTimeout(r, 20));
  assert.ok(!calls.some((c) => c.name === "wiki_ref_dirs_get"), "無効なのに参照フォルダを読んだ");
  tests++;
}

// ---------- ⑰ 「/」メニュー ----------
{
  const AC = await import("@codemirror/autocomplete");
  const S = await import("./editor/slash.ts");
  const labels = (v) => AC.currentCompletions(v.state).map((c) => c.label);
  const open = async (h, text, at) => {
    // 打鍵のように caret も進める（type は changes だけ = caret は元の位置に残る）
    const from = at ?? h.view.state.doc.length;
    h.view.dispatch({ changes: { from, insert: text }, selection: { anchor: from + text.length }, userEvent: "input.type" });
    AC.startCompletion(h.view);
    // 候補源は 50ms の debounce の後に走り、結果は updateSyncTime(100ms)の後に反映される = 待つ
    for (let i = 0; i < 40 && AC.completionStatus(h.view.state) === "pending"; i++) await new Promise((r) => setTimeout(r, 25));
    await new Promise((r) => setTimeout(r, 100)); // interactionDelay(75ms)= 開いた直後の Enter は受け付けない
  };
  const picked = [];
  let pickFiles = null;
  const mk = (doc) => createEditor(document.createElement("div"), { doc, mode: "md", onChange: () => {}, onPickFiles: () => void pickFiles?.() });

  // 開く条件: 行の頭・空白の後。URL の `//` や語の途中では開かない
  let h = mk("");
  await open(h, "/");
  const all = labels(h.view);
  assert.ok(all.length >= 14 && all.includes("表") && all.includes("目次") && all.includes("画像・PDF を埋め込む…"), `全候補: ${all}`);
  assert.ok(!all.some((l) => /色|color/i.test(l)), "文字色は入れない（見送り）");
  h = mk("https:/");
  await open(h, "/");
  assert.equal(AC.completionStatus(h.view.state), null, "URL の // では開かない");
  h = mk("a");
  await open(h, "/");
  assert.equal(AC.completionStatus(h.view.state), null, "語の途中では開かない");
  h = mk("文 ");
  await open(h, "/");
  assert.ok(labels(h.view).length > 0, "空白の後では開く");

  // 絞り込み: 日本語のラベルの部分一致・英語の別名の前方一致
  h = mk("");
  await open(h, "/tab");
  assert.deepEqual(labels(h.view), ["表"]);
  h = mk("");
  await open(h, "/見出し");
  assert.deepEqual(labels(h.view), ["見出し 1", "見出し 2", "見出し 3"]);
  h = mk("");
  await open(h, "/zzz");
  assert.equal(AC.completionStatus(h.view.state), null, "当たりが無ければ閉じる");

  // 雛形の挿入: `/…` を消して置く・caret は雛形の欄
  h = mk("");
  await open(h, "/h2");
  assert.ok(AC.acceptCompletion(h.view));
  assert.equal(h.getText(), "## ");
  assert.equal(h.view.state.selection.main.head, 3, "caret は見出しの本文の位置");
  h = mk("");
  await open(h, "/table");
  AC.acceptCompletion(h.view);
  assert.equal(h.getText(), "| 列 1 | 列 2 |\n| --- | --- |\n|  |  |\n");
  assert.deepEqual([h.view.state.selection.main.from, h.view.state.selection.main.to], [2, 5], "最初の欄「列 1」を選択（Tab で次の欄）");
  h = mk("");
  await open(h, "/toc");
  AC.acceptCompletion(h.view);
  assert.equal(h.getText(), "[toc]\n");
  h = mk("");
  await open(h, "/折");
  AC.acceptCompletion(h.view);
  assert.equal(h.getText(), "> [!note]- \n> ");
  h = mk("");
  await open(h, "/code");
  AC.acceptCompletion(h.view);
  assert.equal(h.getText(), "```\n\n```\n");
  assert.equal(h.view.state.selection.main.head, 4, "caret は本文（2 行目）。Tab で開始行の言語へ");
  assert.ok(AC.nextSnippetField(h.view));
  assert.equal(h.view.state.selection.main.head, 3, "Tab で言語の欄");
  // 段落の直後の `---` は setext 見出しになる・`[toc]` は段落の続きになる → 空行を挟む
  h = mk("前\n");
  await open(h, "/hr");
  AC.acceptCompletion(h.view);
  assert.equal(h.getText(), "前\n\n---\n");
  h = mk("前 ");
  await open(h, "/toc");
  AC.acceptCompletion(h.view);
  assert.equal(h.getText(), "前 \n\n[toc]\n");
  h = mk("前\n\n");
  await open(h, "/toc");
  AC.acceptCompletion(h.view);
  assert.equal(h.getText(), "前\n\n[toc]\n", "既に空行があれば足さない");
  // 記号だけの行（Enter でリスト / 引用が続いた直後）は記号を置き換える
  h = mk("- a\n- ");
  await open(h, "/todo");
  AC.acceptCompletion(h.view);
  assert.equal(h.getText(), "- a\n- [ ] ");
  h = mk("> x\n> ");
  await open(h, "/table");
  AC.acceptCompletion(h.view);
  assert.equal(h.getText(), "> x\n\n| 列 1 | 列 2 |\n| --- | --- |\n|  |  |\n", "引用の記号を置き換え、引用の続きに読まれないよう空行を挟む");
  {
    const { renderMarkdown } = await import("./render/md.ts");
    assert.match(renderMarkdown(h.getText()), /<\/blockquote>\n<table>/, "描画では引用の外の表");
  }
  h = mk("- a\n  - ");
  await open(h, "/todo");
  AC.acceptCompletion(h.view);
  assert.equal(h.getText(), "- a\n  - [ ] ", "入れ子の字下げは残す");
  h = mk("> - a\n> - ");
  await open(h, "/todo");
  AC.acceptCompletion(h.view);
  assert.equal(h.getText(), "> - a\n> - [ ] ", "引用の中のリストは `> ` を残す");
  h = mk("1. a\n2. ");
  await open(h, "/h2");
  AC.acceptCompletion(h.view);
  assert.equal(h.getText(), "1. a\n## ");
  // 速く `/h2⏎` と打つ（候補が開いた後に 1 字足して即 Enter）= 問い直しを待たせない（activateOnTypingDelay 0）
  h = mk("");
  await open(h, "/h");
  h.view.dispatch({ changes: { from: 2, insert: "2" }, selection: { anchor: 3 }, userEvent: "input.type" });
  await new Promise((r) => setTimeout(r, 30));
  assert.ok(AC.acceptCompletion(h.view), "1 字足した直後の Enter が候補の確定になる");
  assert.equal(h.getText(), "## ");
  // コード・frontmatter の中では開かない
  h = mk("```\ncd ");
  await open(h, "/");
  assert.equal(AC.completionStatus(h.view.state), null, "fence の中");
  h = mk("`a  b`");
  await open(h, "/", 3); // 閉じたインラインコードの中（閉じていない backtick は普通の文字 = 開いてよい）
  assert.equal(AC.completionStatus(h.view.state), null, "インラインコードの中");
  h = mk("---\npath: \n---\n");
  await open(h, "/", 10); // 閉じた frontmatter の中（閉じていない `---` は普通の段落 = 開いてよい）
  assert.equal(AC.completionStatus(h.view.state), null, "frontmatter の中");
  h = mk("```\nx\n```\n");
  await open(h, "/");
  assert.ok(labels(h.view).length > 0, "fence を閉じた後は開く");
  // 全角の「／」（日本語入力のまま）でも開く
  h = mk("");
  await open(h, "／h1");
  AC.acceptCompletion(h.view);
  assert.equal(h.getText(), "# ");
  // 文の途中（前に文字がある）の block は改行してから・inline（リンク）はその場
  h = mk("前 ");
  await open(h, "/h1");
  AC.acceptCompletion(h.view);
  assert.equal(h.getText(), "前 \n# ", "行に文字があれば改行して行の頭から");
  h = mk("前 ");
  await open(h, "/link");
  AC.acceptCompletion(h.view);
  assert.equal(h.getText(), "前 [[]]");
  assert.equal(h.view.state.selection.main.head, 4, "caret は [[ の中");
  // 字下げだけの行は改行しない（リストの入れ子の中）
  h = mk("  ");
  await open(h, "/todo");
  AC.acceptCompletion(h.view);
  assert.equal(h.getText(), "  - [ ] ");

  // 埋め込み = `/…` を消してファイル選択。選んだファイルは wiki_import_file → caret に `![[…]]`（dropPaths と同じ道）
  calls.length = 0;
  handlers.wiki_import_file = (a) => ({ path: "attachments/2026-11/" + a.abs.split("/").pop(), embed: `![[${a.abs.split("/").pop()}]]`, copied: true });
  E.startEdit(page("s.md", "", "S0"));
  const he = editor("");
  globalThis.__dialogOpen = async (o) => {
    picked.push(o);
    return ["/Users/x/fig.png", "/Users/x/paper.pdf"];
  };
  pickFiles = E.pickFiles;
  // createEditor の onPickFiles は mk のもの = editor() は WikiEditor の配線を通らないので、ここでは pickFiles を直接
  assert.equal(await E.pickFiles(), true);
  assert.equal(picked.length, 1);
  assert.equal(picked[0].multiple, true);
  assert.ok(picked[0].filters[0].extensions.includes("pdf") && picked[0].filters[0].extensions.includes("png"));
  assert.deepEqual(calls.filter((c) => c.name === "wiki_import_file").map((c) => c.args.abs), ["/Users/x/fig.png", "/Users/x/paper.pdf"]);
  assert.equal(he.getText(), "![[fig.png]]\n![[paper.pdf]]");
  // 取り消し（null）= 何も入れない・エラーにしない
  globalThis.__dialogOpen = async () => null;
  calls.length = 0;
  assert.equal(await E.pickFiles(), false);
  assert.equal(calls.filter((c) => c.name === "wiki_import_file").length, 0);
  // メニューの候補を選ぶと `/…` が消えて選択が始まる（onPickFiles）
  let asked = 0;
  pickFiles = async () => void asked++;
  h = mk("x ");
  await open(h, "/pdf");
  assert.deepEqual(labels(h.view), ["画像・PDF を埋め込む…"]);
  AC.acceptCompletion(h.view);
  assert.equal(h.getText(), "x ", "`/pdf` は消える");
  assert.equal(asked, 1);
  // ダイアログの失敗は知らせる（黙って `/…` だけ消えない）
  globalThis.__dialogOpen = async () => {
    throw new Error("no dialog");
  };
  E.setNotice((m) => notices.push(m)); // ⑤ で読んだ wiki.svelte.ts が受け口を toast に差し替えている = 戻す
  const n0 = notices.length;
  assert.equal(await E.pickFiles(), false);
  assert.equal(notices.length, n0 + 1, "失敗を知らせる");
  assert.match(notices.at(-1), /ファイルを選べません/);
  he.destroy();
  E.stopEdit();
  delete globalThis.__dialogOpen;
  // 候補源の純関数
  assert.ok(S.slashMatches({ label: "表", keys: ["table"] }, "TAB") && !S.slashMatches({ label: "表", keys: ["table"] }, "x"));
  tests++;
}

// ---------- ⑱ PICK_EXT（「/」メニューと OS ドロップで取り込める拡張子）= core の media_kind（crates/openref-core/src/wiki/scan.rs）----------
// 一覧の一致は注記だけの約束だった → Rust のソースの match の腕を読んで固定する（links fixture と同じ「共有の規則を両側で読む」型）
{
  const rs = readFileSync(new URL("../../../../crates/openref-core/src/wiki/scan.rs", import.meta.url), "utf8");
  const fn = rs.match(/pub fn media_kind\(ext: &str\)[\s\S]*?\n\}/)?.[0];
  assert.ok(fn, "scan.rs に media_kind が無い（場所か名前が変わった = このテストを追随させる）");
  const arms = {};
  for (const m of fn.matchAll(/^\s*((?:"[a-z0-9]+"\s*\|\s*)*"[a-z0-9]+")\s*=>\s*"(\w+)"/gm)) {
    arms[m[2]] = [...m[1].matchAll(/"([a-z0-9]+)"/g)].map((x) => x[1]);
  }
  assert.deepEqual(Object.keys(arms), ["image", "video", "audio", "pdf"], `media_kind の腕: ${JSON.stringify(arms)}`);
  const core = Object.values(arms).flat();
  assert.ok(core.length >= 20, `拡張子が読めていない: ${core}`);
  assert.equal(new Set(core).size, core.length, "media_kind に重複");
  assert.equal(new Set(E.PICK_EXT).size, E.PICK_EXT.length, "PICK_EXT に重複");
  assert.deepEqual([...E.PICK_EXT].sort(), [...core].sort(), "PICK_EXT と core の media_kind の拡張子が一致しない（片側だけ変えない）");
  tests++;
}

// ---------- ⑲ WikiInfo: 保存のたびにバックリンクと文献カードを読み直さない ----------
// 保存は wk.page を新しい物（本文・sha256・大きさだけが違う）に置き換える。右ペインの読み直しの鍵は path と items の中身
{
  const { default: WikiInfo } = await import("./WikiInfo.svelte");
  const n = (name) => calls.filter((c) => c.name === name).length;
  handlers.wiki_backlinks = (a) => ({ total: 1, pages: [{ path: `from-${a.path}`, title: `from ${a.path}`, kind: "md", link_kinds: ["wiki"], excerpt: null }] });
  handlers.get_item = (a) => ({ key: a.key, csl: { title: `T ${a.key}`, author: [] }, attachments: [], deleted: false });
  okWrite("I1");
  W.wk.status = { enabled: true, root: "/v" };
  W.wk.page = { ...page("i.md", "本文", "I0"), items: ["K1", "K2"] };
  calls.length = 0;
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiInfo, { target });
  const backRows = () => target.querySelectorAll(".wk-link").length;
  const cards = () => [...target.querySelectorAll(".wk-card-title")].map((e) => e.textContent);
  assert.ok(await waitFor(() => backRows() === 1 && cards().length === 2), "バックリンクと文献カードを出さない");
  assert.deepEqual(cards(), ["T K1", "T K2"]);
  assert.equal(n("wiki_backlinks"), 1);
  assert.equal(n("get_item"), 2);
  // 保存の合図 ×2（実セッション: 打鍵 → flush → setOnSaved が wk.page を置き換える）
  const h = session("i.md", "本文", "");
  type(h, "+1");
  assert.equal(await E.flush(), true);
  assert.equal(W.wk.page.content, "本文+1", "保存で wk.page が揃っていない（合図が出ていない）");
  flushSync();
  await tick();
  type(h, "+2");
  assert.equal(await E.flush(), true);
  assert.equal(W.wk.page.content, "本文+1+2");
  flushSync();
  await tick();
  assert.equal(n("wiki_backlinks"), 1, "保存のたびにバックリンクを読み直した");
  assert.equal(n("get_item"), 2, "保存のたびに文献カードを取り直した");
  assert.equal(backRows(), 1, "保存でバックリンクの表示が消えた");
  assert.deepEqual(cards(), ["T K1", "T K2"], "保存で文献カードが消えた");
  close(h);
  // 別のページ = 両方を読み直す（鍵が path を見ている側）
  W.wk.page = { ...page("j.md", "x", "J0"), items: ["K3"] };
  assert.ok(await waitFor(() => n("wiki_backlinks") === 2 && n("get_item") === 3), "別のページで読み直さない");
  assert.ok(await waitFor(() => cards().join() === "T K3" && target.querySelector(".wk-link-title")?.textContent === "from j.md"));
  // items だけが変わった（編集中の読み直し = adopt）= カードだけ取り直す・バックリンクは触らない
  W.wk.page = { ...W.wk.page, items: ["K3", "K4"] };
  assert.ok(await waitFor(() => n("get_item") === 5), "items が変わったのにカードを取り直さない");
  assert.equal(n("wiki_backlinks"), 2, "items だけの変化でバックリンクを読み直した");
  // 中身が同じ新しい配列（wiki_read_gui の読み直し = 毎回新しい配列）= 取り直さない（鍵は配列の同一性ではなく中身）
  W.wk.page = { ...W.wk.page, items: [...W.wk.page.items] };
  flushSync();
  await tick();
  assert.equal(n("get_item"), 5, "中身が同じ items の新しい配列でカードを取り直した");
  // 参照フォルダのファイル（external）= バックリンクは読まない
  W.wk.page = { ...page("/ref/r.md", "x", "R0"), external: "/ref", items: [] };
  await tick();
  assert.equal(n("wiki_backlinks"), 2, "参照フォルダのファイルでバックリンクを読んだ");
  unmount(cmp);
  target.remove();
  W.wk.page = null;
  tests++;
}

// ---------- ⑳-0 ShadowPage: 描画の計測は旗（__wkMeasure）のときだけ残す・`![[ページ]]` の読みは世代（wk.embedGen = 索引の変更 + 保存の成功）に 1 回 ----------
{
  const { default: MdView } = await import("./MdView.svelte");
  const { proxy } = await import("svelte/internal/client");
  const target = document.createElement("div");
  document.body.append(target);
  const reads = [];
  const readPage = async (p) => {
    reads.push(p);
    return { path: p, abs_path: "/v/" + p, kind: "md", title: p, size: 0, mtime: 0, content: `埋め込み本文 ${reads.length}`, links: [] };
  };
  const links = [{ kind: "embed", target: "inner", path: "inner.md", ambiguous: false }];
  const mk = (n) => ({ ...page("e.md", `前 ${n}\n\n![[inner]]\n`, `E${n}`), links });
  const props = proxy({ page: mk(1), root: "/v", refs: [], onNavigate: () => {}, readPage });
  const measures = () => performance.getEntriesByType("measure").filter((e) => e.name === "wk-render e.md").length;
  const embedded = () => target.querySelector(".wk-md")?.shadowRoot?.querySelector(".wk-embed-page-body")?.textContent ?? "";
  const shown = () => target.querySelector(".wk-md")?.shadowRoot?.textContent ?? "";
  assert.notEqual(globalThis.__wkMeasure, true, "旗が最初から立っている");
  const cmp = mount(MdView, { target, props });
  flushSync();
  assert.ok(await waitFor(() => embedded().includes("埋め込み本文 1")), "埋め込みを描かない");
  assert.deepEqual(reads, ["inner.md"]);
  // 描き直し（本文が変わる = 分割のプレビューの打鍵）×2: 計測は残らない・埋め込みは読み直さない（同じ世代）
  for (const n of [2, 3]) {
    props.page = mk(n);
    flushSync();
    assert.ok(await waitFor(() => shown().includes(`前 ${n}`)), `描き直さない (${n})`);
    await sleep(20);
  }
  assert.equal(measures(), 0, "旗が無いのに計測が溜まる");
  assert.deepEqual(reads, ["inner.md"], "同じ世代で埋め込みを読み直した");
  assert.ok(embedded().includes("埋め込み本文 1"), "覚えた埋め込みを使っていない");
  // 索引が変わった（wiki-changed = embedGen が進む）= 次の描画で読み直す
  W.wk.embedGen++;
  props.page = mk(4);
  flushSync();
  assert.ok(await waitFor(() => embedded().includes("埋め込み本文 2")), "世代が進んでも埋め込みを読み直さない");
  assert.deepEqual(reads, ["inner.md", "inner.md"]);
  // GUI 自身の保存の成功でも世代が進む（監視の event が届く前の描き直しに前の埋め込みを出さない）
  E.startEdit(page("inner.md", "x", "I0"));
  const hs = editor("x");
  type(hs, "y");
  okWrite("I1");
  assert.equal(await E.flush(), true, "保存できない");
  E.stopEdit();
  E.detach(hs);
  hs.destroy();
  props.page = mk(40);
  flushSync();
  assert.ok(await waitFor(() => embedded().includes("埋め込み本文 3")), "保存の成功で埋め込みを読み直さない");
  assert.deepEqual(reads, ["inner.md", "inner.md", "inner.md"]);
  // 失敗した読みは覚えない（次の描画でもう一度読む）
  let fail = true;
  props.readPage = async (p) => {
    reads.push(p);
    if (fail) throw new Error("no");
    return { path: p, abs_path: "/v/" + p, kind: "md", title: p, size: 0, mtime: 0, content: "復帰", links: [] };
  };
  W.wk.stamp++;
  props.page = mk(5);
  flushSync();
  assert.ok(await waitFor(() => (target.querySelector(".wk-md")?.shadowRoot?.querySelector(".wk-embed-error")?.textContent ?? "").includes("no")), "失敗を出さない");
  fail = false;
  props.page = mk(6);
  flushSync();
  assert.ok(await waitFor(() => embedded().includes("復帰")), "失敗した読みを覚えて読み直さない");
  assert.equal(reads.length, 5);
  // 旗を立てると計測が残る（受入の計測・以下のテストが描き直しの回数に使う）
  globalThis.__wkMeasure = true;
  props.page = mk(7);
  flushSync();
  assert.ok(await waitFor(() => measures() === 1), "旗を立てても計測が残らない");
  unmount(cmp);
  target.remove();
  tests++;
}

// ---------- ⑳ MdView: 参照フォルダ（refs）を足すと表示中のリンクの分類が変わる ----------
// `file://` のリンクは refs の中の pdf なら ref（内蔵の表示）・外なら open（ホストで開く）。refs は描画 effect の依存に入っていなければ古い分類のまま
{
  const { default: MdView } = await import("./MdView.svelte");
  const { proxy } = await import("svelte/internal/client");
  const target = document.createElement("div");
  document.body.append(target);
  const props = proxy({
    page: { ...page("m.md", "[x](file:///Users/u/kb/journals/x.pdf) と [y](file:///Users/u/other/y.pdf)", "M0"), links: [] },
    root: "/v",
    refs: [],
    onNavigate: () => {},
    readPage: async () => {
      throw new Error("no");
    },
  });
  const cmp = mount(MdView, { target, props });
  flushSync();
  const links = () => [...(target.querySelector(".wk-md")?.shadowRoot?.querySelectorAll("a") ?? [])].map((a) => a.className);
  assert.ok(await waitFor(() => links().length === 2), "描かない");
  assert.deepEqual(links(), ["wk-l-open", "wk-l-open"], "参照フォルダが無いのに ref にした");
  props.refs = ["/Users/u/kb"];
  flushSync();
  assert.ok(await waitFor(() => links()[0] === "wk-l-ref"), `参照フォルダを足しても古い分類のまま: ${links()}`);
  assert.deepEqual(links(), ["wk-l-ref", "wk-l-open"]);
  // 中身が同じ新しい配列（wiki-state ごとの loadRefDirs は毎回新しい配列を入れる）= 描き直さない
  const renders = () => performance.getEntriesByType("measure").filter((e) => e.name === "wk-render m.md").length;
  const r1 = renders();
  props.refs = ["/Users/u/kb"];
  flushSync();
  await sleep(20);
  assert.equal(renders(), r1, "中身が同じ refs の新しい配列で描き直した");
  // 参照フォルダを外す = 戻る
  props.refs = [];
  flushSync();
  assert.ok(await waitFor(() => links()[0] === "wk-l-open"), "参照フォルダを外しても ref のまま");
  unmount(cmp);
  target.remove();
  tests++;
}

// ---------- ㉑ 分割のプレビュー（WikiPage）: 保存で同じ本文を描き直さない ----------
// 保存は wk.page を置き換える → shown も新しくなる。previewPage が shown から組まれていると ShadowPage が同じ本文を描き直す（wk-render の計測で数える）
{
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const { EditorView } = await import("@codemirror/view");
  okWrite("P1");
  const delay0 = E.cfg.delay;
  E.cfg.delay = 100000; // 自動保存は走らせない（flush が本当に書くことを確かめる = 先に自動保存が済むと flush が空振りして何も比べていない）
  handlers.wiki_read_gui = (a) => page(a.path, "# S\n\n本文\n", "P0");
  handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
  handlers.wiki_status = () => ({ enabled: true, root: "/v", pages: 1, inbox: 0 });
  S.app.wikiOn = true;
  W.wk.status = { enabled: true, root: "/v" };
  W.wk.page = null;
  W.wk.split = true;
  tab.wiki = { path: "s.md", scrollTop: 0, editing: true };
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  const renders = () => performance.getEntriesByType("measure").filter((e) => e.name === "wk-render s.md").length;
  const preview = () => target.querySelector(".wk-preview .wk-md")?.shadowRoot?.textContent ?? "";
  const cmView = () => EditorView.findFromDOM(target.querySelector(".cm-editor"));
  assert.ok(await waitFor(() => preview().includes("本文") && !!target.querySelector(".cm-editor"), 3000), "分割のプレビューを描かない");
  const r0 = renders();
  assert.ok(r0 >= 1);
  // 打鍵 = プレビューの debounce の後に描き直す（これは正しい描き直し）
  cmView().dispatch({ changes: { from: cmView().state.doc.length, insert: "追記" }, userEvent: "input" });
  assert.ok(await waitFor(() => preview().includes("追記")), "打鍵でプレビューが変わらない");
  const r1 = renders();
  assert.ok(r1 > r0, "打鍵でプレビューを描き直していない");
  await sleep(30);
  const r2 = renders();
  // 保存（⌘S 相当）= ここで初めて書く → wk.page が置き換わる。プレビューは同じ本文 = 描き直さない
  assert.equal(E.ed.state, "dirty", "flush の前に自動保存が走った（比べる相手が無い）");
  const w0 = writes().length;
  assert.equal(await E.flush(), true);
  assert.equal(writes().length, w0 + 1, "flush で書いていない");
  assert.equal(W.wk.page.content, "# S\n\n本文\n追記", "保存で wk.page が揃っていない");
  flushSync();
  await sleep(40);
  assert.equal(renders(), r2, "保存のたびに分割のプレビューが同じ本文を描き直した");
  assert.ok(preview().includes("追記"));
  // 見出しの色の切替（描画部品の page 以外の props）= 描き直さない（spread の props の全部に依存していると全描画になる）
  W.wk.headColors = !W.wk.headColors;
  flushSync();
  await sleep(20);
  assert.equal(renders(), r2, "page 以外の props（見出しの色）の変化でプレビューを描き直した");
  W.wk.headColors = !W.wk.headColors;
  unmount(cmp);
  target.remove();
  E.cfg.delay = delay0;
  W.wk.split = false;
  tab.wiki = { path: null, scrollTop: 0, editing: false };
  tests++;
}

// ---------- ㉒ Wiki のモーダルが開いている間はタブ操作を受け付けない（⌘1〜9・⌘⇧]・⌘T・⌘W = state の tabOpsAllowed） ----------
const C = await import("./chat.svelte.ts");
{
  E.ed.stranded.length = 0; // 前の節が残した救出の本文（救出が出ている間はタブ操作を受け付けない = ここで確かめる対象）
  S.app.wikiOn = true;
  W.wk.status = { enabled: true, root: "/v" };
  tab.wiki = { path: null, scrollTop: 0, editing: false };
  S.app.tabs.push({ ...tab, id: 777, wiki: null, viewer: null });
  const before = S.app.tab;
  const tabOps = async (label) => {
    calls.length = 0;
    assert.equal(S.tabOpsAllowed(), false, `${label}: tabOpsAllowed`);
    assert.equal(S.modalOpen(), true, `${label}: modalOpen`);
    assert.equal(W.infoKeyAllowed(), false, `${label}: ⌘⌥I が効く`);
    S.activateTabAt(2); // ⌘2
    S.cycleTab(1); // ⌘⇧]
    void S.openTab({ kind: "all" }); // ⌘T
    void S.closeTab(S.app.tab); // ⌘W
    await sleep(20);
    assert.equal(S.app.tab, before, `${label}: タブが替わった`);
    assert.equal(S.app.tabs.length, 2, `${label}: タブが増減した`);
    assert.equal(calls.length, 0, `${label}: invoke が走った`);
  };
  const why = () =>
    JSON.stringify({ busy: S.app.busy, editorOpen: S.app.editorOpen, modal: S.app.modal, importJob: S.app.importJob, dialog: W.wk.dialog, conflict: E.ed.dialog, consent: C.chat.consent, stranded: E.ed.stranded.length });
  assert.equal(S.tabOpsAllowed(), true, `前のテストの残り: ${why()}`);
  assert.equal(S.modalOpen(), false);
  W.wk.dialog = { kind: "new", dir: "" };
  await tabOps("新規ページのダイアログ");
  W.wk.dialog = null;
  E.ed.dialog = true;
  await tabOps("conflict");
  E.ed.dialog = false;
  C.chat.consent = true;
  await tabOps("チャットの説明");
  C.chat.consent = false;
  E.ed.stranded.push({ path: "s.md", content: "x", reason: "r" });
  await tabOps("救出");
  E.ed.stranded.length = 0;
  assert.equal(S.tabOpsAllowed(), true, "モーダルを閉じたのに受け付けない");
  assert.equal(S.modalOpen(), false);
  assert.equal(W.infoKeyAllowed(), true, "モーダルを閉じたのに ⌘⌥I が効かない");
  tab.wiki = null;
  assert.equal(W.infoKeyAllowed(), false, "前面が Wiki でないのに ⌘⌥I が効く");
  tab.wiki = { path: null, scrollTop: 0, editing: false };
  // 本体のモーダル（app.modal）の規則はそのまま
  S.app.modal = { kind: "x" };
  assert.equal(S.tabOpsAllowed(), false);
  S.app.modal = null;
  tests++;
}

// ---------- ㉓ Wiki OFF でダイアログとメニューを畳む・待っていた「破棄して移る」の確認は「移らない」で終わる ----------
{
  calls.length = 0;
  failWrite();
  W.wk.status = { enabled: true, root: "/v" };
  const h = session("off3.md", "x", "y"); // 前面 = 編集中・未保存あり（2 枚目のタブ 777 が居る）
  const p = S.closeTab(S.app.tab).then(() => "resolved");
  assert.ok(await waitFor(() => W.wk.dialog?.kind === "discard"), "保存できないとき破棄して移るかを尋ねる");
  W.wk.menu = { kind: "new", path: "", x: 0, y: 0 };
  handlers.wiki_status = () => ({ enabled: false, root: "/v" });
  await W.loadWikiStatus();
  assert.equal(S.app.wikiOn, false);
  assert.equal(W.wk.dialog, null, "OFF にしたのにダイアログが残った");
  assert.equal(W.wk.menu, null, "OFF にしたのにメニューが残った");
  assert.equal(await Promise.race([p, sleep(300).then(() => "hung")]), "resolved", "破棄の確認の約束が宙に残った（閉じるが終わらない）");
  assert.equal(S.app.tabs.length, 2, "「移らない」のはずのタブが閉じた");
  assert.equal(S.app.tab, tab.id);
  assert.equal(tab.wiki, null, "OFF なのに Wiki タブの中身が残った");
  assert.equal(E.ed.stranded.at(-1)?.content, "xy", "書けなかった本文を残していない");
  E.ed.stranded.length = 0;
  E.detach(h);
  h.destroy();
  handlers.wiki_status = () => ({ enabled: true, root: "/v", pages: 0, inbox: 0 });
  okWrite();
  // 保存が通る変種（OFF の時点で書ける = 救出なし）: 「移らない」で終わるのは答えが false だから（true なら閉じていた）
  S.app.wikiOn = true;
  W.wk.status = { enabled: true, root: "/v" };
  failWrite();
  const h2 = session("off4.md", "x", "y");
  const p2 = S.closeTab(S.app.tab).then(() => "resolved");
  assert.ok(await waitFor(() => W.wk.dialog?.kind === "discard"));
  okWrite();
  handlers.wiki_status = () => ({ enabled: false, root: "/v" });
  await W.loadWikiStatus();
  assert.equal(await Promise.race([p2, sleep(300).then(() => "hung")]), "resolved");
  assert.equal(E.ed.stranded.length, 0, "書けたのに救出に残した");
  assert.equal(S.app.tabs.length, 2, "保存が通ったあと「破棄して移る」を true で答えてタブを閉じた");
  assert.equal(W.wk.dialog, null);
  E.detach(h2);
  h2.destroy();
  // 編集していないときの OFF: 新規ページのダイアログ・メニュー・チャットの説明（WikiChatConsent は OFF で描かれない）も畳む
  S.app.wikiOn = true;
  W.wk.status = { enabled: true, root: "/v" };
  tab.wiki = { path: null, scrollTop: 0, editing: false };
  W.wk.dialog = { kind: "new", dir: "" };
  W.wk.menu = { kind: "new", path: "", x: 0, y: 0 };
  C.chat.consent = true;
  assert.equal(S.tabOpsAllowed(), false);
  await W.loadWikiStatus();
  assert.equal(S.app.wikiOn, false);
  assert.equal(W.wk.dialog, null, "OFF にしたのに新規のダイアログが残った");
  assert.equal(W.wk.menu, null);
  assert.equal(C.chat.consent, false, "OFF にしたのにチャットの説明が残った（見えないモーダルがタブ操作を止め続ける）");
  assert.equal(S.tabOpsAllowed(), true, "OFF の後もタブ操作を受け付けない");
  S.app.tabs.pop();
  handlers.wiki_status = () => ({ enabled: true, root: "/v", pages: 0, inbox: 0 });
  tests++;
}

// ---------- ㉔ [＋ 新規] の再押下で閉じる（実部品 WikiHeader + WikiDialogs） ----------
{
  S.app.wikiOn = true;
  W.wk.status = { enabled: true, root: "/v" };
  tab.wiki = { path: null, scrollTop: 0, editing: false };
  FL.openNewMenu(10, 20);
  assert.equal(W.wk.menu?.kind, "new");
  FL.openNewMenu(10, 20);
  assert.equal(W.wk.menu, null, "再押下で閉じない");
  const { default: Header } = await import("./WikiHeader.svelte");
  const { default: Dialogs } = await import("./WikiDialogs.svelte");
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(Dialogs, { target });
  const hd = mount(Header, { target });
  flushSync();
  const btn = target.querySelector(".wk-head-new");
  assert.ok(btn, "[＋ 新規] が無い");
  const press = (el) => {
    el.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
    flushSync();
    el.dispatchEvent(new MouseEvent("click", { bubbles: true, button: 0 }));
    flushSync();
  };
  press(btn);
  assert.equal(W.wk.menu?.kind, "new", "1 回目の押下で開かない");
  assert.equal(btn.getAttribute("aria-expanded"), "true");
  press(btn);
  assert.equal(W.wk.menu, null, "2 回目の押下で閉じない（mousedown で閉じて click で開き直した）");
  assert.equal(btn.getAttribute("aria-expanded"), "false");
  press(btn);
  assert.equal(W.wk.menu?.kind, "new", "3 回目の押下で開かない");
  // 外側の mousedown では閉じる（従来どおり）・Esc も
  document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
  flushSync();
  assert.equal(W.wk.menu, null, "外側の押下で閉じない");
  press(btn);
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  flushSync();
  assert.equal(W.wk.menu, null, "Esc で閉じない");
  unmount(hd);
  unmount(cmp);
  target.remove();
  tests++;
}

host.remove();
console.log(`wiki edit: ok (${tests} tests)`);
process.exit(0); // CodeMirror・toast のタイマーを待たない
