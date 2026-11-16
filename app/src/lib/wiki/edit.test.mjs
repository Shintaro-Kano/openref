/* OpenNote の編集の部品テスト（node app/src/lib/wiki/edit.test.mjs・Svelte 5 の実部品 + 実 CodeMirror + jsdom）。
   Tauri の invoke は差し替え（Rust の command は契約 types.ts の形で返す）。確かめること:
   ① 自動保存の debounce（入力停止まで書かない・続けて打っても 1 回・if_sha256 = 読んだ / 書いた hash）
   ② conflict の分岐（自動保存を止める・離れられない・ダイアログの「読み直す」= wiki_read_gui で置き換え /「上書き」= force）
   ③ 外部の書換え（未保存が無ければ黙って読み直す・あれば触らない）
   ④ caret への挿入（OS ドロップの wiki_import_file・貼付画像の wiki_save_blob・書式キー）
   ⑤ 保存に失敗したままタブを閉じようとすると止まる（state.svelte の closeTab → wiki.svelte の leave guard）
   ⑥〜⑮ 本文を失わない終わり方・ワークスペース（編集は始めたときの root に固定・タブの行き来と明示の切替）・ゴミ箱・改行・終了の要求・OS ドロップ・WikiPage の配線（各節の見出しに書く）
   ⑯ 起動時に参照フォルダの一覧を読む（設定に残っているのに一覧が空のまま = リンクが外部で開く、を防ぐ）
   ⑰ 「/」メニュー（editor/slash.ts）: 開く条件・絞り込み・雛形の挿入（行の頭 / 文の途中）・埋め込みはファイル選択 → wiki_import_file
   ⑱ PICK_EXT = core の media_kind（取り込める拡張子の一覧の一致を Rust のソースを読んで固定）
   ⑲ WikiInfo（右ペイン）は保存のたびにバックリンクと文献カードを読み直さない（wk.page の置換 ≠ ページの変化。別のページ・items の変化では読む）
   ⑳-0 ShadowPage の描画の計測（performance.measure）は旗 __wkMeasure のときだけ残る・`![[ページ]]` の読みは世代（wk.embedGen = 索引の変更 + 保存の成功）に 1 回（失敗は覚えない）
   ⑳ ShadowPage（md）は参照フォルダ（refs）を足すと表示中のリンクの分類を描き直す（refs は描画の依存）
   ㉑ 分割のプレビューは保存・page 以外の props の変化で描き直さない（previewPage は path と ed.preview から組む。打鍵では描き直す）
   ㉒ Wiki のモーダル（wk.dialog・conflict・チャットの説明・救出）が開いている間はタブの切替・開く・閉じる（⌘1〜9・⌘⇧]・⌘T・⌘W）を受け付けない
   ㉓ Wiki OFF でダイアログ・メニュー・チャットの説明を畳む・「破棄して移る」の確認を待っていた閉じるは「移らない」で終わる（約束を宙に残さない・保存が通ってもタブは閉じない）
   ㉔ [＋ 新規] の再押下で閉じる（実部品 WikiHeader + WikiDialogs: ボタンの mousedown では閉じず click が閉じる・外側の mousedown では閉じる）
   ㉕ 複製（実部品 WikiDialogs: ファイルのメニューの 2 行目・押すと wiki_duplicate → 同じタブで開く・編集中は先に保存・保存できなければ複製しない・待つ間にタブ / ページ / ワークスペースが変わったら開かない）
   ㉖ 編集中の目次（実部品 WikiPage + WikiInfo: 編集・分割で目次を押すとエディタの caret がその見出しの行へ・目次は入力中の本文から = 保存を待たない・id は表示と同じ・
      エディタの読込の前に来た移動は開いた後に効く・分割のプレビューの描き直しは目次を上書きしない）
   ㉗ 編集に入った直後に前のページの目次を出さない（DOM に入った目次の行を MutationObserver で全部記録）
   ㉘ html のページの分割でも目次の受け手は 1 つ（プレビューの描き直しが目次を上書きしない）
   ㉚ 救出の窓・取込の窓は開いたときに焦点を自分へ移す: エディタ（実 CodeMirror）に焦点がある状態で出す → 焦点は窓の枠
      （ボタンでない = Enter / Space で何も起きない・[捨てて閉じる] も [クリップボードにコピー] も既定の焦点でない）・エディタは焦点を失う
      = 続けて打った字・⌘F が裏のエディタに入らない
   ㉛ 救出の窓・取込の窓を閉じたら、開く前の焦点へ戻す: 元の焦点（エディタ = カーソルの位置のまま / 一覧の欄 = 選択のまま / 焦点なし）×
      閉じ方（救出 = ボタン・状態の側 / 取込 = Esc（App の連鎖の closeImport）・ボタン・状態の側）。戻さない = 元の要素が消えた・隠れた・無効・
      開いている間に窓の外へ移した・閉じるのと同時に別の窓が開いた（自分で焦点を取る窓 / 取らない窓）・前面のタブが替わった
   ㉜ ページのツールバーは [編集]（トグル）・[⋯]・[集中] の 3 個: [編集] = ⌘E と同じ（押し込み = 編集中・編集へは前回の配置）・
      [⋯] = 分割（チェック = 分割で編集中）・見出しの色（md の表示中・チェック = 今の設定）・外部で開く・名前の変更・複製・ゴミ箱・
      html = [ソース]・編集できないページ = [編集] の代わりに「編集不可」・参照フォルダのページ = [⋯] が無く [外部で開く] のボタン
   ㉝ [⋯] のメニューはキーボードで操作できる: [⋯] で Enter / Space / ↓ = 開いて最初の項目へ・↑↓ で移る（区切りを飛ばし端で回る）・
      Home / End・Enter / Space で実行して [⋯] へ戻る・Esc で閉じて [⋯] へ・Tab で閉じる（[⋯] から次へ）・マウスで開いたときは焦点を動かさない
   ㉞ キーで [⋯] から窓を開く項目を選んだら、焦点は窓の中（覆いの下の [⋯] でない）・覆いの間は [⋯] のキー / クリックでメニューを開かない・
      窓が取った焦点を奪わない・ゴミ箱の窓は開いたら [キャンセル] に焦点（削除の側でない・エディタに焦点があっても）、閉じたら元へ（カーソルのまま）
   ㉟ 覆いが出たら [⋯] のメニューを閉じる・ダイアログ（新規ページ・新規フォルダ・名前の変更・破棄の確認・ゴミ箱）を閉じたら開く前の焦点へ
      戻す・名前の変更の 2 段目は押せるようになってから [書き換えて変更] へ焦点・[⋯] の全項目の字の頭が揃う（チェックの欄）・[編集] の押し込み
   ㊱ 右ペインの目次は一番上・全レベル: 節の順（目次 → ページ情報 → リンク先の文献 → バックリンク）・h1〜h6・字下げ = 一番浅い見出しから 12px ずつ・
      飛んだレベル・見出しなし = 節ごと出さない・押すと移る（表示 / 編集）・編集中に足した h6 が追う
   ㊲ 右ペインの目次を畳む / 開く: 見出しは <button>（aria-expanded・aria-controls）・件数・畳むと行を描かない・
      焦点は見出しのボタンのまま・ui_wiki_toc に書く・覆いの下では動かない・別のページへ移っても畳んだまま・畳んでいる間の編集で件数が追う
   ㉙ 窓の外へドラッグしても閉じない（実部品 WikiDialogs の新規ページ / 新規フォルダ）: 覆いで押して覆いで離したときだけ閉じる・
      名前の欄で押して外で離す = 開いたまま（打った字が残る）・覆いで押して中で離す = 開いたまま・覆いで押したまま閉じて開き直した直後の離し = 開いたまま・
      窓の形（.overlay > .modal・role・aria-labelledby）は前のまま
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
const M = await import("../mode.svelte.ts"); // セグメントの切替（wiki.svelte から分けた）
const FL = await import("./files.svelte.ts"); // 新規 / 改名 / ゴミ箱（wiki.svelte.ts から分けた）
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
  // LIBRARY への切替（switchMode）も止める
  const p4 = M.switchMode("library");
  assert.ok(await waitFor(() => W.wk.dialog?.kind === "discard"), "switchMode が guard を通らない");
  W.answerDiscard(false);
  await p4;
  assert.ok(tab.wiki, "保存に失敗したのに LIBRARY へ切り替えた");
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
  // 保存の後の live() を外す変異を捕まえる: LIBRARY を押して保存を待つ間に、後から押した物が勝つ
  // （NOTE = 今のモードに留まる意思 → 保存が済んでも LIBRARY へ移らない）
  let release = null;
  handlers.wiki_save = (a) => new Promise((r) => (release = () => r({ ok: true, path: a.path, sha256: "P3", size: a.content.length, mtime: 7, created: false })));
  const h = session("p3.md", "a", "b");
  const p = M.switchMode("library");
  assert.ok(await waitFor(() => !!release), "前提: 保存を待っている");
  assert.equal(S.tabOpsAllowed(), true, "前提: OpenNote の保存の間も押せる（覆い・処理中でない）");
  await M.switchMode("wiki");
  release();
  await p;
  assert.ok(tab.wiki, "保存を待つ間に NOTE を押したのに、先に押した LIBRARY が勝った");
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

// ---------- ⑦ ワークスペース: 編集は始めたときの root に固定（args.root = ed.root）----------
// (a) 編集中に別の root のタブへ = 先に元の root へ保存してから移る・移った後に終えても元の root へ書く・戻って続きを編集できる（元の root へ）
// (b) 明示の切替（wiki_switch_root の戻り値）= 先に元の root へ保存 → 前面のタブだけホームへ・編集を終える（新しい root へは書かない）
// (c) root の分からない編集（仮の状態のまま始めた = ed.root が null）は切替の後に新しい root へ書かない = 待つ間の打鍵は本文を残す
{
  S.app.wikiOn = true;
  okWrite();
  handlers.list_items = () => [];
  handlers.wiki_ref_dirs_get = () => ({ dirs: [] });
  handlers.wiki_front_root = (a) => ({ root: a.root, restart: false });
  const WS = [
    { root: "/A", last_used: 2, exists: true, current: true },
    { root: "/B", last_used: 1, exists: true, current: false },
  ];
  handlers.wiki_status = (a) => ({ enabled: true, root: a?.root ?? "/A", pages: 0, inbox: 0, workspaces: WS });
  // 画面の root = /A（仮の状態から root 無しで読んで採る）
  tab.wiki = { path: "a.md", root: "/A", scrollTop: 0, editing: true };
  await W.loadWikiStatus();
  assert.equal(W.shownRoot(), "/A", "画面の root を採らない");
  // (a) タブ 2 = /B のページ。タブ 1 で /A の a.md を編集中（未保存あり）→ タブ 2 へ
  const t2 = { ...tab, id: 701, wiki: { path: "b.md", root: "/B", scrollTop: 0, editing: false }, viewer: null };
  S.app.tabs.push(t2);
  E.startEdit(page("a.md", "A", "A0"), "/A");
  const h = editor("A");
  type(h, "+1");
  calls.length = 0;
  await S.activateTab(701);
  assert.equal(S.app.tab, 701, "タブ 2 へ移れない");
  assert.equal(writes().length, 1, "移る前に保存しない");
  assert.deepEqual(writes()[0].args, { path: "a.md", content: "A+1", if_sha256: "A0", root: "/A" }, "保存が編集を始めたときの root でない");
  assert.ok(await waitFor(() => calls.some((c) => c.name === "wiki_front_root")), "移った先の root へ設定を揃えない");
  assert.deepEqual(calls.find((c) => c.name === "wiki_front_root").args, { root: "/B" });
  const iSave = calls.findIndex((c) => c.name === "wiki_save");
  const iFront = calls.findIndex((c) => c.name === "wiki_front_root");
  assert.ok(iSave < iFront, `元の root へ保存する前に設定を移した: ${calls.map((c) => c.name)}`);
  assert.equal(W.shownRoot(), "/B");
  // 画面の root が /B でも、編集のセッションの書込は /A（WikiPage が無い試験 = セッションはまだ生きている）
  type(h, "+2");
  assert.equal(await E.flush(), true);
  assert.equal(writes().at(-1).args.root, "/A", "移った後の保存が画面の root（/B）へ行った");
  assert.equal(E.ed.root, "/A");
  // 戻って続きを編集（同じ /A のセッション）
  await S.activateTab(tab.id);
  assert.ok(await waitFor(() => W.shownRoot() === "/A"), "戻っても画面の root が /A に戻らない");
  type(h, "+3");
  assert.equal(await E.flush(), true);
  assert.deepEqual([writes().at(-1).args.root, writes().at(-1).args.content], ["/A", "A+1+2+3"], "戻った後の続きが /A へ書かれない");
  // 別の root の同じ相対パスは別のセッション（startEdit が前のセッションを始めたときの root へ書いて終える）
  type(h, "+4");
  E.detach(h);
  calls.length = 0;
  E.startEdit(page("a.md", "B の a", "BA0"), "/B");
  assert.ok(await waitFor(() => writes().length === 1), "前のセッションの未保存を書かない");
  assert.deepEqual([writes()[0].args.root, writes()[0].args.content], ["/A", "A+1+2+3+4"], "前のセッションの未保存を /A へ書かない");
  assert.equal(E.ed.root, "/B");
  E.discardEdit();
  h.destroy();

  // (b) 明示の切替（メニュー / ホームの [開く]）= 前面のタブだけ。先に元の root へ保存 → wiki_switch_root → 前面のタブを戻り値の root のホームへ・編集を終える
  tab.wiki = { path: "m.md", root: "/A", scrollTop: 0, editing: true };
  E.startEdit(page("m.md", "m", "m.md-0"), "/A");
  const h2 = editor("m");
  type(h2, "+edit");
  calls.length = 0;
  handlers.wiki_switch_root = (a) => ({ root: a.root, restart: false });
  await W.switchWorkspace("/B");
  assert.deepEqual(calls.filter((c) => c.name !== "wiki_status" && c.name !== "wiki_ref_dirs_get" && c.name !== "wiki_list" && c.name !== "wiki_tags" && c.name !== "wiki_inbox").map((c) => c.name), ["wiki_save", "wiki_switch_root"], `保存の前に切り替えた: ${calls.map((c) => c.name)}`);
  assert.equal(writes()[0].args.root, "/A", "切替の前の保存が元の root でない");
  assert.equal(E.ed.path, null, "切り替えたのに編集が残った");
  assert.deepEqual({ ...tab.wiki }, { path: null, root: "/B", scrollTop: 0, editing: false }, "前面のタブが戻り値の root のホームにならない");
  assert.deepEqual(S.app.tabs.find((x) => x.id === 701).wiki.root, "/B", "他のタブ（もともと /B）を触った");
  assert.ok(!calls.some((c) => c.name === "wiki_front_root"), "明示の切替の後に wiki_front_root を呼んだ（Rust が済ませた）");
  assert.equal(writes().length, 1, "新しい root へ書いた");
  E.detach(h2);
  h2.destroy();

  // (c) root の分からない編集 = 切替を待つ間の打鍵は新しい root へ書かずに残す
  const h3 = session("n.md", "n", "+x");
  calls.length = 0;
  handlers.wiki_switch_root = (a) => {
    type(h3, "+待つ間"); // 切替を待つ間の打鍵
    return { root: a.root, restart: false };
  };
  await W.switchWorkspace("/A");
  assert.equal(writes().length, 1, "切替の前に保存しない / 切替の後に書いた");
  assert.equal(writes()[0].args.root, undefined, "root の分からない編集に root を付けた");
  assert.equal(E.ed.path, null, "編集が残った");
  assert.equal(E.ed.stranded.at(-1)?.content, "n+x+待つ間", "待つ間の打鍵を残していない");
  assert.ok(S.app.toast.includes("フォルダが変わった"), S.app.toast);
  E.ed.stranded.length = 0;
  E.detach(h3);
  h3.destroy();
  S.app.tabs.splice(S.app.tabs.findIndex((x) => x.id === 701), 1);
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
  // 別のワークスペースの同じ相対パスは別のファイル = 閉じない（ゴミ箱は画面の root の物）
  S.app.tabs.push({ ...tab, id: 996, wiki: { path: "d/other.md", root: "/elsewhere-in-history", scrollTop: 0, editing: false }, viewer: null });
  calls.length = 0;
  // 移せなかった: 中のページの編集はそのまま・自動保存は再開（打鍵で書きに行く）・タブも newDirs も変わらない
  handlers.wiki_trash = () => {
    throw new Error("trash failed");
  };
  assert.ok(FL.editingUnder("d", true) && !FL.editingUnder("dx", true), "editingUnder");
  await FL.doTrash("d", true);
  assert.ok(S.app.toast.includes("ゴミ箱に移せません"), `toast: ${S.app.toast}`);
  assert.equal(E.ed.path, "d/sub/t.md", "移せなかったのに編集を捨てた");
  assert.equal(S.app.tabs.length, 4);
  assert.equal(W.wk.newDirs.size, 3);
  type(h, "!");
  await sleep(1400);
  assert.ok(writes().length >= 1, "移せなかった後、自動保存が再開していない");
  calls.length = 0;
  handlers.wiki_trash = (a) => ({ path: a.path, method: "trash", dir: true, files: 2 });
  handlers.list_items = () => []; // タブを閉じると次のタブへ切り替わり、下の蔵書の一覧を読み直す
  await FL.doTrash("d", true);
  assert.ok(S.app.toast.includes("ゴミ箱に移しました"), `toast: ${S.app.toast}`);
  assert.deepEqual(calls.filter((c) => c.name === "wiki_trash").map((c) => c.args), [{ path: "d", dir: true, root: W.shownRoot() }], "wiki_trash はフォルダの path と dir（と画面の root）で 1 回");
  assert.equal(E.ed.path, null, "中のページの編集を捨てていない");
  assert.ok(!S.app.tabs.some((x) => x.wiki?.path === "d/other.md" && x.wiki.root !== "/elsewhere-in-history"), "中のページのタブが残った");
  assert.ok(S.app.tabs.some((x) => x.id === 996), "別のワークスペースの同じ相対パスのタブを閉じた");
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

// ---------- ⑩b 複数ファイルの取込・貼付の途中でタブを移った = 残りは編集を始めたときの root 以外へ書かない ----------
// 1 本目を待つ間に別のワークスペース（B）のタブで編集が始まる / 表示だけのタブへ移る（ed.root = null）= 2 本目以降は取り込まずに止めて知らせる。
// 取り込んだが本文に入れなかったファイルも知らせに並べる（最後の 1 本の最中に離れた場合も）。移らなければ全部が始めたときの root（A）へ
{
  const imports = () => calls.filter((c) => c.name === "wiki_import_file").map((c) => [c.args.abs, c.args.page, c.args.root ?? null]);
  const blobs = () => calls.filter((c) => c.name === "wiki_save_blob").map((c) => [c.args.page, c.args.root ?? null]);
  // 移らない = 全部 A（root は始めたときの物を渡す）
  calls.length = 0;
  handlers.wiki_import_file = (a) => ({ path: "attachments/" + a.abs.split("/").pop(), embed: `![[${a.abs.split("/").pop()}]]`, copied: true });
  E.startEdit(page("a.md", "A", "A0"), "/A");
  let h = editor("A");
  assert.equal(await E.dropPaths(["/x/one.png", "/x/two.png", "/x/three.png"]), true);
  assert.deepEqual(imports(), [["/x/one.png", "a.md", "/A"], ["/x/two.png", "a.md", "/A"], ["/x/three.png", "a.md", "/A"]]);
  close(h);
  // 1 本目を待つ間に B のタブで編集が始まった = 2 本目以降は B へも A へも書かない・知らせる
  for (const toView of [false, true]) {
    calls.length = 0;
    E.startEdit(page("a.md", "A", "A0"), "/A");
    h = editor("A");
    let n = 0;
    handlers.wiki_import_file = (a) => {
      if (++n === 1) {
        E.stopEdit();
        if (!toView) E.startEdit(page("b.md", "B", "B0"), "/B"); // 行き先のタブで編集が始まった / 表示だけのタブ（ed.root = null）
      }
      return { path: "attachments/" + a.abs.split("/").pop(), embed: `![[${a.abs}]]`, copied: true };
    };
    S.app.toast = ""; // 知らせ（setNotice）は wiki.svelte.ts を読んだ後は toast
    assert.equal(await E.dropPaths(["/x/one.png", "/x/two.png", "/x/three.png"]), true);
    assert.deepEqual(imports(), [["/x/one.png", "a.md", "/A"]], `移った後の取込を書いた（${toView ? "表示へ" : "B の編集へ"}）: ${JSON.stringify(imports())}`);
    assert.ok(S.app.toast.includes("残りのファイルを取り込みませんでした") && S.app.toast.includes("two.png, three.png"), `知らせない: ${S.app.toast}`);
    assert.ok(S.app.toast.includes("取り込んだファイルを本文に入れませんでした: attachments/one.png"), `取り込んだが入れなかった分を知らせない: ${S.app.toast}`);
    E.detach(h);
    h.destroy();
    E.discardEdit();
  }
  // 最後の 1 本の最中に離れた（残りは無い） = 取り込んだ分を知らせる・1 本だけでも同じ
  for (const files of [["/x/one.png", "/x/two.png"], ["/x/solo.png"]]) {
    calls.length = 0;
    E.startEdit(page("a.md", "A", "A0"), "/A");
    h = editor("A");
    let m = 0;
    handlers.wiki_import_file = (a) => {
      if (++m === files.length) E.stopEdit();
      return { path: "attachments/" + a.abs.split("/").pop(), embed: `![[${a.abs}]]`, copied: true };
    };
    S.app.toast = "";
    await E.dropPaths(files);
    const want = files.map((f) => "attachments/" + f.split("/").pop()).join(", ");
    assert.ok(S.app.toast.includes(`取り込んだファイルを本文に入れませんでした: ${want}`), `最後の 1 本の最中に離れたのに知らせない: ${JSON.stringify(S.app.toast)}`);
    assert.ok(!S.app.toast.includes("残りのファイル"), "残りが無いのに「残り」を知らせた");
    assert.equal(h.getText(), "A", "離れたのに本文に入れた");
    E.detach(h);
    h.destroy();
    E.discardEdit();
  }
  // 移らなければ知らせない
  calls.length = 0;
  E.startEdit(page("a.md", "A", "A0"), "/A");
  h = editor("A");
  handlers.wiki_import_file = (a) => ({ path: "attachments/" + a.abs.split("/").pop(), embed: "![[ok]]", copied: true });
  S.app.toast = "";
  await E.dropPaths(["/x/a.png", "/x/b.png"]);
  assert.equal(S.app.toast, "", `移らないのに知らせた: ${S.app.toast}`);
  close(h);
  // 貼付（wiki_save_blob）も同じ = 始めたときの root・移ったら止める
  calls.length = 0;
  E.startEdit(page("a.md", "A", "A0"), "/A");
  h = editor("A");
  let k = 0;
  handlers.wiki_save_blob = (a) => {
    if (++k === 1) {
      E.stopEdit();
      E.startEdit(page("b.md", "B", "B0"), "/B");
    }
    return { path: "attachments/" + a.name, embed: `![[${a.name}]]` };
  };
  const png = (name) => new File([new Uint8Array([137, 80, 78, 71])], name, { type: "image/png" });
  S.app.toast = "";
  await E.pasteImages([png("p1.png"), png("p2.png")]);
  assert.deepEqual(blobs(), [["a.md", "/A"]], `移った後の貼付を書いた: ${JSON.stringify(blobs())}`);
  assert.ok(S.app.toast.includes("残りのファイルを取り込みませんでした") && S.app.toast.includes("p2.png"), `貼付の残りを知らせない: ${S.app.toast}`);
  assert.ok(S.app.toast.includes("取り込んだファイルを本文に入れませんでした: attachments/"), `貼付で書いたが入れなかった分を知らせない: ${S.app.toast}`);
  E.detach(h);
  h.destroy();
  E.discardEdit();
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
  /** 画面の root で呼ぶ（wikiArgs が args.root を足す。分からなければ付けない） */
  const R = (o) => (W.shownRoot() ? { ...o, root: W.shownRoot() } : o);
  calls.length = 0;
  W.wk.dialog = { kind: "new", dir: "d" };
  assert.equal(await FL.createPage("d", "memo", "html"), true);
  assert.deepEqual(made(), [
    ["wiki_create", R({ dir: "d", name: "Untitled.html" })],
    ["wiki_rename", R({ from: "d/Untitled.html", to: "d/memo.html", rewrite: false })],
  ]);
  assert.equal(W.wk.dialog, null);
  assert.deepEqual([tab.wiki.path, tab.wiki.editing], ["d/memo.html", true], "作ったページを編集で開く");
  calls.length = 0;
  await FL.createPage("", "p.md.", "html"); // 名前の拡張子が優先（末尾の . は落とす）= 作る形式も md
  assert.deepEqual(made(), [
    ["wiki_create", R({ dir: "", name: "Untitled.md" })],
    ["wiki_rename", R({ from: "Untitled.md", to: "p.md", rewrite: false })],
  ]);
  calls.length = 0;
  await FL.createPage("", "  ", "md"); // 空の名前 = Untitled のまま・改名しない
  assert.deepEqual(made(), [["wiki_create", R({ dir: "", name: "Untitled.md" })]]);
  assert.equal(tab.wiki.path, "Untitled.md");
  calls.length = 0;
  assert.equal(await FL.createPage("", ".html", "md"), false);
  assert.deepEqual(made(), [], ". 始まりの名前で core を呼んだ（Untitled が残る）");

  // 新規フォルダ: wiki_mkdir → 木に出すために覚え、祖先を開く。空・/ 入りは core を呼ばない
  handlers.wiki_mkdir = (a) => ({ path: (a.dir ? a.dir + "/" : "") + a.name });
  calls.length = 0;
  W.wk.dialog = { kind: "newdir", dir: "d/e" };
  assert.equal(await FL.createFolder("d/e", " 新しい "), true);
  assert.deepEqual(calls.filter((c) => c.name === "wiki_mkdir").map((c) => c.args), [R({ dir: "d/e", name: "新しい" })]);
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
  assert.deepEqual(calls.filter((c) => c.name === "wiki_rename").map((c) => c.args), [R({ from: "n.md", to: "m.md", rewrite: false })], "ログだけの参照で確認を出した");
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
  tab.wiki = { path: "w.md", root: "/v", scrollTop: 0, editing: false };
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  const has = (sel) => !!target.querySelector(sel);
  const cmView = () => EditorView.findFromDOM(target.querySelector(".cm-editor"));
  assert.ok(await waitFor(() => W.wk.page?.path === "w.md" && has(".wk-edit-btn")), "ページを読んで切替（[編集]）を出す");
  const cmdE = () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "e", metaKey: true, bubbles: true }));
  cmdE();
  assert.ok(await waitFor(() => has(".cm-editor"), 3000), "⌘E でエディタを開かない");
  assert.equal(E.ed.path, "w.md");
  assert.equal(E.ed.root, "/v", "WikiPage が編集をタブの root で始めない（書込の root の固定）");
  assert.equal(calls.filter((c) => c.name === "wiki_read_gui").at(-1)?.args.root, "/v", "ページをタブの root で読まない");
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
  // ワークスペースの明示の切替（戻り値の root で前面のタブだけ） = 先に元の root（このタブの root）へ保存 → エディタを閉じる → タブは新しい root のホームへ。
  // 新しい root へは書かない（編集は始めたときの root に固定 = ed.root）
  calls.length = 0;
  handlers.wiki_switch_root = (a) => ({ root: a.root, restart: false });
  handlers.wiki_status = (a) => ({ enabled: true, root: a?.root ?? "/other", pages: 0, inbox: 0 });
  handlers.wiki_ref_dirs_get = () => ({ dirs: [] });
  assert.equal(E.ed.root, tab.wiki.root ?? null, "編集の root がタブの root でない");
  const edRoot = E.ed.root;
  await W.switchWorkspace("/other");
  assert.ok(await waitFor(() => !has(".cm-editor")), "切り替えたのにエディタが残った");
  assert.equal(writes().length, 1, `切替の前に 1 回だけ保存する: ${writes().length}`);
  assert.equal(writes()[0].args.root, edRoot ?? undefined, "切替の前の保存が元の root でない");
  assert.ok(writes()[0].args.content.startsWith("X# W"), "切替の前に未保存を保存しない");
  assert.equal(E.ed.stranded.length, 0, "保存できた本文を残した");
  // 前面のタブだけ新しい root のホームへ（前の root の相対パスを新しい root で開かない）
  assert.deepEqual({ ...tab.wiki }, { path: null, root: "/other", scrollTop: 0, editing: false }, "前面のタブが新しい root のホームに戻らない");
  assert.equal(W.shownRoot(), "/other");
  // Wiki OFF = 本文を残して畳む（新しい root でページを開き直して編集）
  tab.wiki = { path: "w.md", scrollTop: 0, editing: true };
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
  const { default: ShadowPage } = await import("./render/ShadowPage.svelte");
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
  const props = proxy({ mode: "md", page: mk(1), root: "/v", refs: [], onNavigate: () => {}, readPage });
  const measures = () => performance.getEntriesByType("measure").filter((e) => e.name === "wk-render e.md").length;
  const embedded = () => target.querySelector(".wk-md")?.shadowRoot?.querySelector(".wk-embed-page-body")?.textContent ?? "";
  const shown = () => target.querySelector(".wk-md")?.shadowRoot?.textContent ?? "";
  assert.notEqual(globalThis.__wkMeasure, true, "旗が最初から立っている");
  const cmp = mount(ShadowPage, { target, props });
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
  // 旗を立てると計測が残る（以下のテストが描き直しの回数に使う）
  globalThis.__wkMeasure = true;
  props.page = mk(7);
  flushSync();
  assert.ok(await waitFor(() => measures() === 1), "旗を立てても計測が残らない");
  unmount(cmp);
  target.remove();
  tests++;
}

// ---------- ⑳ ShadowPage（md）: 参照フォルダ（refs）を足すと表示中のリンクの分類が変わる ----------
// `file://` のリンクは refs の中の pdf なら ref（内蔵の表示）・外なら open（ホストで開く）。refs は描画 effect の依存に入っていなければ古い分類のまま
{
  const { default: ShadowPage } = await import("./render/ShadowPage.svelte");
  const { proxy } = await import("svelte/internal/client");
  const target = document.createElement("div");
  document.body.append(target);
  const props = proxy({
    mode: "md",
    page: { ...page("m.md", "[x](file:///Users/u/ref/papers/x.pdf) と [y](file:///Users/u/other/y.pdf)", "M0"), links: [] },
    root: "/v",
    refs: [],
    onNavigate: () => {},
    readPage: async () => {
      throw new Error("no");
    },
  });
  const cmp = mount(ShadowPage, { target, props });
  flushSync();
  const links = () => [...(target.querySelector(".wk-md")?.shadowRoot?.querySelectorAll("a") ?? [])].map((a) => a.className);
  assert.ok(await waitFor(() => links().length === 2), "描かない");
  assert.deepEqual(links(), ["wk-l-open", "wk-l-open"], "参照フォルダが無いのに ref にした");
  props.refs = ["/Users/u/ref"];
  flushSync();
  assert.ok(await waitFor(() => links()[0] === "wk-l-ref"), `参照フォルダを足しても古い分類のまま: ${links()}`);
  assert.deepEqual(links(), ["wk-l-ref", "wk-l-open"]);
  // 中身が同じ新しい配列（wiki-state ごとの loadRefDirs は毎回新しい配列を入れる）= 描き直さない
  const renders = () => performance.getEntriesByType("measure").filter((e) => e.name === "wk-render m.md").length;
  const r1 = renders();
  props.refs = ["/Users/u/ref"];
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
const C = await import("../chat/chat.svelte.ts");
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
  // 前面が一覧 = 一覧の右ペインの開閉を受ける（togglePane が振り分ける）。PDF のタブ = 右バーの開閉を受ける。エディタ・取込の下・モーダルでは受けない
  assert.equal(W.infoKeyAllowed(), true, "前面が一覧なのに ⌘⌥I が効かない");
  tab.viewer = { itemKey: "K", attachmentId: 1, title: "T" };
  assert.equal(W.infoKeyAllowed(), true, "前面が PDF のタブなのに ⌘⌥I（右バーの開閉）が効かない");
  S.app.editorOpen = true;
  assert.equal(W.infoKeyAllowed(), false, "エディタの下の PDF のタブで ⌘⌥I が効く");
  S.app.editorOpen = false;
  S.app.importJob = { running: true };
  assert.equal(W.infoKeyAllowed(), false, "取込の下の PDF のタブで ⌘⌥I が効く");
  S.app.importJob = null;
  W.wk.dialog = { kind: "new", dir: "" };
  assert.equal(W.infoKeyAllowed(), false, "モーダルの下の PDF のタブで ⌘⌥I が効く");
  W.wk.dialog = null;
  tab.viewer = null;
  S.app.editorOpen = true;
  assert.equal(W.infoKeyAllowed(), false, "エディタの下の一覧で ⌘⌥I が効く");
  S.app.editorOpen = false;
  S.app.importJob = { running: true };
  assert.equal(W.infoKeyAllowed(), false, "取込の下の一覧で ⌘⌥I が効く");
  S.app.importJob = null;
  W.wk.dialog = { kind: "new", dir: "" };
  assert.equal(W.infoKeyAllowed(), false, "モーダルの下の一覧で ⌘⌥I が効く");
  W.wk.dialog = null;
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
  // 編集していないときの OFF: 新規ページのダイアログ・メニュー・チャットの説明（ChatConsent は OFF で描かれない）も畳む
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

// ---------- ㉕ 複製（実部品 WikiDialogs + files.svelte の duplicateFile） ----------
{
  S.app.wikiOn = true;
  W.wk.status = { enabled: true, root: "/v" };
  tab.wiki = { path: "d/f.md", root: "/v", scrollTop: 40, editing: false };
  S.app.tab = tab.id;
  const R = (o) => (W.shownRoot() ? { ...o, root: W.shownRoot() } : o);
  handlers.wiki_list = () => ({ total: 0, count: 0, pages: [] });
  handlers.wiki_duplicate = (a) => ({ path: a.path.replace(/(\.[^.]+)$/, " 1$1") });
  const dups = () => calls.filter((c) => c.name === "wiki_duplicate").map((c) => c.args);
  const { default: Dialogs } = await import("./WikiDialogs.svelte");
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(Dialogs, { target });
  const items = () => [...target.querySelectorAll(".wk-menu button")];
  const labels = () => items().map((b) => b.textContent.trim());

  // ファイルのメニュー = 「名前を変更…」の下・フォルダのメニューには無い
  W.wk.menu = { kind: "dir", path: "d", x: 0, y: 0 };
  flushSync();
  assert.ok(!labels().includes("複製する"), `フォルダのメニューに出た: ${labels()}`);
  W.wk.menu = { kind: "file", path: "d/f.md", x: 0, y: 0 };
  flushSync();
  assert.deepEqual(labels(), ["名前を変更…", "複製する", "ゴミ箱に移す…"]);

  // 押す = メニューを閉じて複製・同じタブで開く（新しいタブにしない・表示で）・木を読み直す・toast は複製の名前
  calls.length = 0;
  const tabs = S.app.tabs.length;
  items()[1].click();
  flushSync();
  assert.equal(W.wk.menu, null, "メニューが閉じない");
  assert.ok(await waitFor(() => tab.wiki.path === "d/f 1.md"), `複製を開かない: ${tab.wiki.path}`);
  assert.deepEqual(dups(), [R({ path: "d/f.md" })]);
  assert.deepEqual([S.app.tabs.length, tab.wiki.editing, tab.wiki.scrollTop], [tabs, false, 0], "同じタブで頭から表示で開かない");
  assert.ok(S.app.toast.includes("複製しました: f 1.md"), `toast: ${S.app.toast}`);
  assert.ok(calls.some((c) => c.name === "wiki_list"), "木を読み直さない");

  // 開いていないファイル（画像）の複製も同じタブ
  tab.wiki.path = "d/f.md";
  await FL.duplicateFile("img/図.png");
  assert.equal(tab.wiki.path, "img/図 1.png");
  assert.ok(S.app.toast.includes("複製しました: 図 1.png"), S.app.toast);

  // core の拒否 = 理由を出す・開かない
  handlers.wiki_duplicate = () => { throw new Error("書込権がありません（読み取り専用のフォルダ）: /v/ro"); };
  tab.wiki.path = "d/f.md";
  await FL.duplicateFile("ro/a.md");
  assert.ok(S.app.toast.includes("複製できません") && S.app.toast.includes("読み取り専用"), S.app.toast);
  assert.equal(tab.wiki.path, "d/f.md");

  // 索引のエラー = 複製はできている = 開いて知らせる
  handlers.wiki_duplicate = (a) => ({ path: "d/f 2.md", index_error: "ix" });
  await FL.duplicateFile("d/f.md");
  assert.equal(tab.wiki.path, "d/f 2.md");
  assert.ok(S.app.toast.includes("索引を更新できませんでした") && S.app.toast.includes("f 2.md") && S.app.toast.includes("ix"), S.app.toast);

  // 編集中: 未保存を先に保存してから写す（複製に入る）
  handlers.wiki_duplicate = (a) => ({ path: a.path.replace(/(\.[^.]+)$/, " 1$1") });
  handlers.wiki_save = (a) => ({ ok: true, path: a.path, sha256: "S9", size: a.content.length, mtime: 1, created: false });
  tab.wiki = { path: "e.md", root: "/v", scrollTop: 0, editing: true };
  let h = session("e.md", "x", "y");
  calls.length = 0;
  await FL.duplicateFile("e.md");
  const order = calls.map((c) => c.name).filter((n) => n === "wiki_save" || n === "wiki_duplicate");
  assert.deepEqual(order, ["wiki_save", "wiki_duplicate"], "保存より先に写した");
  assert.equal(calls.find((c) => c.name === "wiki_save").args.content, "xy");
  assert.deepEqual([tab.wiki.path, tab.wiki.editing], ["e 1.md", false], "複製を表示で開かない");
  close(h);

  // 編集中で保存できない = 複製しない（破棄して移るを断ったら元のページのまま）
  failWrite();
  tab.wiki = { path: "e.md", root: "/v", scrollTop: 0, editing: true };
  h = session("e.md", "x", "y");
  calls.length = 0;
  const p = FL.duplicateFile("e.md");
  assert.ok(await waitFor(() => W.wk.dialog?.kind === "discard"), "保存できないのに尋ねない");
  W.answerDiscard(false);
  await p;
  assert.deepEqual(dups(), [], "保存できないのに複製した");
  assert.equal(tab.wiki.path, "e.md");
  // 破棄して移るを選んだ = 編集を捨てて保存済みの内容を写す（他の遷移と同じ canLeave）
  const pd = FL.duplicateFile("e.md");
  assert.ok(await waitFor(() => W.wk.dialog?.kind === "discard"));
  W.answerDiscard(true);
  await pd;
  assert.deepEqual(dups(), [R({ path: "e.md" })]);
  assert.deepEqual([tab.wiki.path, E.ed.path], ["e 1.md", null]);
  E.detach(h);
  h.destroy();

  // 大きなコピーを待つ間に画面が変わった = 開かない（複製はできている = 知らせる）。finish = コピーの終わり
  let finish = null;
  handlers.wiki_duplicate = () => new Promise((r) => (finish = () => r({ path: "v 1.mp4" })));
  /** 複製を待たせて move（画面を変える操作）を挟み、終わらせる */
  const during = async (move) => {
    finish = null;
    S.app.toast = "";
    const q = FL.duplicateFile("v.mp4");
    await waitFor(() => !!finish);
    await move();
    finish();
    await q;
    assert.ok(S.app.toast.includes("複製しました: v 1.mp4"), S.app.toast);
  };
  // 別のタブへ切り替えた（同じページのタブ = ページの比較では止まらない。読むのは proxy = S.app.tabs の中。push した生のオブジェクトは書込みが届かない）
  tab.wiki = { path: "v.mp4", root: "/v", scrollTop: 0, editing: false };
  S.app.tabs.push({ ...tab, id: 998, wiki: { path: "v.mp4", root: "/v", scrollTop: 0, editing: false }, viewer: null });
  await during(() => (S.app.tab = 998));
  assert.equal(S.app.tabs.find((x) => x.id === 998).wiki.path, "v.mp4", "待つ間に切り替えたタブの中身を変えた");
  assert.equal(tab.wiki.path, "v.mp4");
  S.app.tabs.pop();
  S.app.tab = tab.id;
  // 同じタブの中で別のページへ移った
  await during(() => W.openWikiPage("読んでいる.md"));
  assert.equal(tab.wiki.path, "読んでいる.md", "読み始めたページから複製へ飛ばした");
  // 同じタブのまま LIBRARY へ切り替えた = OpenNote へ引き戻さない・タブを増やさない
  tab.wiki.path = "v.mp4";
  const before = S.app.tabs.length;
  await during(() => M.switchMode("library"));
  assert.equal(S.app.tabs.length, before, "LIBRARY から複製のタブを開いた");
  assert.equal(S.frontWiki(), null);
  tab.wiki = { path: null, root: "/v", scrollTop: 0, editing: false };
  // 同じタブのままワークスペースが変わった（ホームからホームへ = ページは同じ null）。provisional を抜けないと shownRoot が null で比べられない
  handlers.wiki_front_root = () => ({});
  handlers.wiki_status = (a) => ({ enabled: true, root: a?.root ?? "/v", workspaces: [] });
  W.enterRoot("/v", true);
  await waitFor(() => W.shownRoot() === "/v");
  assert.equal(W.shownRoot(), "/v");
  await during(() => {
    W.enterRoot("/other", true);
    tab.wiki = { path: null, root: "/other", scrollTop: 0, editing: false }; // 明示の切替 = 前面のタブをその root のホームへ
  });
  assert.equal(tab.wiki.path, null, "別のワークスペースで元の root の相対パスを開いた");
  unmount(cmp);
  target.remove();
  tests++;
}

// ---------- ㉖ 編集中の目次（実部品 WikiPage + WikiInfo）: 押すとエディタの caret がその見出しの行へ・中身は編集中の本文から（保存を待たない） ----------
// 前は目次の押下（jumpToHeading → wk.scrollTo）を受けるのが表示の部品（ShadowPage）だけ = 編集だけの画面では何も起きず、
// 目次は表示で描いたときの見出しのまま（書き足した見出しが出ない）。分割はプレビューが動くだけで caret は動かなかった
{
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const { default: WikiInfo } = await import("./WikiInfo.svelte");
  const { EditorView } = await import("@codemirror/view");
  okWrite("T1");
  const delay0 = E.cfg.delay;
  E.cfg.delay = 100000; // 自動保存は走らせない = 目次が保存を待たずに変わることを確かめる
  // frontmatter（行がずれる）・同名の見出し（id の連番）・記号と和文・インラインの記法と wikilink の別名・コードの中の # は見出しでない
  const src = "---\ntitle: t\n---\n# 表題\n\n本文\n\n## 所見: CT\n\na\n\n## 所見: CT\n\nb\n\n```\n# 見出しでない\n```\n\n### `code` と **強調** & [[p|別名]]\n\nc\n";
  handlers.wiki_read_gui = (a) => page(a.path, src, "T0");
  handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
  handlers.wiki_status = () => ({ enabled: true, root: "/v", pages: 1, inbox: 0 });
  S.app.wikiOn = true;
  S.app.tab = tab.id;
  W.wk.status = { enabled: true, root: "/v" };
  W.wk.page = null;
  W.wk.split = false;
  tab.wiki = { path: "toc.md", root: "/v", scrollTop: 0, editing: false };
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  const info = mount(WikiInfo, { target });
  const toc = () => [...target.querySelectorAll(".wk-toc")];
  const tocText = () => toc().map((b) => b.textContent);
  const cmView = () => EditorView.findFromDOM(target.querySelector(".cm-editor"));
  const caretLine = () => cmView().state.doc.lineAt(cmView().state.selection.main.head).number;
  const lineOf = (text, nth = 0) => cmView().state.doc.toString().split("\n").map((l, i) => [l, i + 1]).filter(([l]) => l === text)[nth][1];
  // 表示 / 編集（エディタだけ）/ 分割（[編集] のトグル = ⌘E と同じ・分割は [⋯] の中 = ここは WikiDialogs を置かないので setEditing で同じ切替）
  const editBtn = () => target.querySelector(".wk-edit-btn");
  const toEdit = () => editBtn().click(); // W.wk.split = false（上）= エディタだけで開く
  const toSplit = () => void W.setEditing(true, true); // [⋯] の「分割」と同じ呼び（WikiDialogs の toggleSplit = 編集中は split の反転）
  const toView = () => editBtn().click(); // 編集中の [編集] = 表示へ
  // 表示 = 目次は描画した見出し（ShadowPage の onHeadings）
  assert.ok(await waitFor(() => toc().length === 4, 3000), `表示で目次が出ない: ${tocText()}`);
  const viewHeads = W.wk.headings.map((h) => ({ ...h }));
  assert.deepEqual(tocText(), ["表題", "所見: CT", "所見: CT", "code と 強調 & 別名"]);
  // 編集（エディタだけ）。エディタの読込（動的 import）の前に来た移動は、開いた後に効く（`[[page#見出し]]` の着地と同じ道）
  toEdit();
  flushSync();
  assert.ok(!target.querySelector(".cm-editor"), "エディタが同期で開いた（読込の前の移動を試せない）");
  assert.equal(W.jumpToHeading("所見: CT"), true, "編集に入った時点の目次（入力中の本文から）に見出しが無い");
  assert.ok(await waitFor(() => !!target.querySelector(".cm-editor") && !target.querySelector(".wk-preview"), 3000), "編集のエディタが開かない");
  assert.ok(await waitFor(() => caretLine() === lineOf("## 所見: CT")), `エディタの読込の前に来た移動が効かない: ${caretLine()}`);
  await tick();
  // 編集中の目次の id は表示の描画と同じ（同名の見出しの連番・記号・和文・インラインの記法）
  assert.deepEqual(W.wk.headings.map(({ level, text, id }) => ({ level, text, id })), viewHeads.map(({ level, text, id }) => ({ level, text, id })), "編集中の目次の id が表示とずれる");
  // 2 つ目の「所見: CT」を押す → caret がその行へ（同名の 1 つ目へ行かない・frontmatter の分ずらす）
  cmView().dispatch({ selection: { anchor: 0 } });
  toc()[2].click();
  assert.ok(await waitFor(() => caretLine() === lineOf("## 所見: CT", 1)), `目次を押しても caret が見出しの行へ動かない: ${caretLine()}`);
  toc()[3].click();
  assert.ok(await waitFor(() => caretLine() === lineOf("### `code` と **強調** & [[p|別名]]")), `記法つきの見出しへ動かない: ${caretLine()}`);
  // 書き足した見出しが目次に出る（保存を待たない）→ 押すとその行へ
  cmView().dispatch({ changes: { from: cmView().state.doc.length, insert: "\n## 足した見出し\n\nd\n" }, userEvent: "input" });
  assert.ok(await waitFor(() => tocText().includes("足した見出し")), `書き足した見出しが目次に出ない: ${tocText()}`);
  assert.equal(writes().filter((w) => w.args.path === "toc.md").length, 0, "目次のために保存した");
  cmView().dispatch({ selection: { anchor: 0 } });
  toc().at(-1).click();
  assert.ok(await waitFor(() => caretLine() === lineOf("## 足した見出し")), `書き足した見出しへ動かない: ${caretLine()}`);
  // 消した見出しは目次から消える
  const t0 = cmView().state.doc.toString().indexOf("# 表題");
  cmView().dispatch({ changes: { from: t0, to: t0 + "# 表題".length, insert: "表題（見出しでなくした）" }, userEvent: "input" });
  assert.ok(await waitFor(() => !tocText().includes("表題")), `消した見出しが目次に残る: ${tocText()}`);
  // 分割 = エディタの caret が動く（プレビューも同じ id の見出しへ = 表示と同じ id の規則）
  toSplit();
  assert.ok(await waitFor(() => !!target.querySelector(".wk-preview .wk-md")?.shadowRoot?.textContent.includes("足した見出し"), 3000), "分割のプレビューが出ない");
  cmView().dispatch({ selection: { anchor: 0 } });
  toc()[1].click();
  assert.ok(await waitFor(() => caretLine() === lineOf("## 所見: CT", 1)), `分割で目次を押しても caret が動かない: ${caretLine()}`);
  // 受け手は 1 つ: 分割のプレビューの描画（onHeadings）が目次を上書きしない（目次は入力中の本文から = 行つき）。
  // 打鍵 → プレビューの描き直しを待ってから見る（描画の側が書けば line の無い見出しになる）
  cmView().dispatch({ changes: { from: cmView().state.doc.length, insert: "\n## 分割で足した\n" }, userEvent: "input" });
  assert.ok(await waitFor(() => !!target.querySelector(".wk-preview .wk-md")?.shadowRoot?.textContent.includes("分割で足した")), "プレビューが描き直さない");
  await sleep(30);
  assert.ok(W.wk.headings.length > 0 && W.wk.headings.every((h) => typeof h.line === "number"), `分割のプレビューが目次を上書きした: ${JSON.stringify(W.wk.headings)}`);
  // 打鍵によらないプレビューの描き直し（参照フォルダの変化 = 描画の依存）でも上書きしない（打鍵なら同じ flush で目次の側も書き直すので見えない）
  const renders0 = performance.getEntriesByType("measure").filter((e) => e.name === "wk-render toc.md").length;
  const refs0 = W.wk.refDirs;
  W.wk.refDirs = ["/ref-目次"];
  assert.ok(await waitFor(() => performance.getEntriesByType("measure").filter((e) => e.name === "wk-render toc.md").length > renders0), "参照フォルダの変化でプレビューを描き直さない（試験の前提）");
  await sleep(10);
  assert.ok(W.wk.headings.every((h) => typeof h.line === "number"), `プレビューの描き直しが目次を上書きした: ${JSON.stringify(W.wk.headings)}`);
  W.wk.refDirs = refs0;
  const sr = target.querySelector(".wk-preview .wk-md").shadowRoot;
  const id = W.wk.headings.find((h) => h.text === "足した見出し").id;
  assert.ok(sr.getElementById(id), `プレビューに目次と同じ id の見出しが無い: ${id}`);
  // 表示へ戻す（保存してから）= 目次は表示の描画（ShadowPage）の物に戻る
  toView();
  assert.ok(await waitFor(() => !target.querySelector(".cm-editor") && !!target.querySelector(".wk-body .wk-md")), "表示へ戻らない");
  assert.ok(await waitFor(() => tocText().join("|") === "所見: CT|所見: CT|code と 強調 & 別名|足した見出し|分割で足した"), `表示へ戻した目次: ${tocText()}`);
  unmount(info);
  unmount(cmp);
  target.remove();
  E.cfg.delay = delay0;
  W.wk.page = null;
  tests++;
}

// ---------- ㉗ 編集に入った直後に前のページの目次を出さない（DOM に一度でも入った目次の行を全部記録して見る） ----------
// ed.preview は編集を終えても前のページの本文のまま（stopEdit は消さない）。編集に入った直後、startEdit が ed.preview を
// 新しいページの本文にする前に目次の effect が走ると、前のページの見出しが 1 拍出うる = 起きないことを固定する
{
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const { default: WikiInfo } = await import("./WikiInfo.svelte");
  const { EditorView } = await import("@codemirror/view");
  okWrite("Q1");
  const files = { "qa.md": "# A の見出し\n\n本文\n", "qb.md": "# B の見出し\n\n本文\n" };
  handlers.wiki_read_gui = (a) => page(a.path, files[a.path], "Q0");
  handlers.wiki_save = (a) => ((files[a.path] = a.content), { ok: true, path: a.path, sha256: "Q1", size: a.content.length, mtime: 1, created: false });
  handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
  S.app.wikiOn = true;
  S.app.tab = tab.id;
  W.wk.status = { enabled: true, root: "/v" };
  W.wk.page = null;
  W.wk.split = false;
  tab.wiki = { path: "qa.md", root: "/v", scrollTop: 0, editing: true };
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  const info = mount(WikiInfo, { target });
  const cmView = () => EditorView.findFromDOM(target.querySelector(".cm-editor"));
  const tocText = () => [...target.querySelectorAll(".wk-toc")].map((b) => b.textContent);
  assert.ok(await waitFor(() => !!target.querySelector(".cm-editor") && tocText().includes("A の見出し"), 3000), "qa.md の編集が開かない");
  // A だけにある見出しを書き足す（未保存のまま ed.preview に入る）
  cmView().dispatch({ changes: { from: cmView().state.doc.length, insert: "\n## A だけの見出し\n" }, userEvent: "input" });
  assert.ok(await waitFor(() => tocText().includes("A だけの見出し")));
  const seen = [];
  const mo = new MutationObserver((rs) => {
    for (const r of rs) for (const n of r.addedNodes) if (n.nodeType === 1) for (const b of [n, ...n.querySelectorAll(".wk-toc")]) if (b.classList?.contains("wk-toc")) seen.push(b.textContent);
  });
  // ① 編集のまま同じタブで別のページへ（編集で開く）
  mo.observe(target, { childList: true, subtree: true, characterData: true });
  await W.openWikiPage("qb.md", false, undefined, true);
  assert.ok(await waitFor(() => !!target.querySelector(".cm-editor") && tocText().join() === "B の見出し", 3000), `qb.md の編集の目次: ${tocText()}`);
  await sleep(20);
  mo.takeRecords();
  assert.ok(!seen.some((x) => x.startsWith("A ")), `別のページの編集に入った直後に前のページの目次が出た: ${seen}`);
  // ② 表示 → 別のページ → 編集（ed.preview は前のページの本文のまま）
  cmView().dispatch({ changes: { from: cmView().state.doc.length, insert: "\n## B だけの見出し\n" }, userEvent: "input" });
  assert.ok(await waitFor(() => tocText().includes("B だけの見出し")));
  await W.setEditing(false);
  assert.ok(await waitFor(() => !target.querySelector(".cm-editor")));
  await W.openWikiPage("qa.md");
  assert.ok(await waitFor(() => tocText().join() === "A の見出し,A だけの見出し", 3000), `qa.md の表示の目次: ${tocText()}`);
  seen.length = 0;
  await W.setEditing(true);
  assert.ok(await waitFor(() => !!target.querySelector(".cm-editor"), 3000));
  await sleep(20);
  mo.takeRecords();
  mo.disconnect();
  assert.ok(!seen.some((x) => x.startsWith("B ")), `表示から編集に入った直後に前のページの目次が出た: ${seen}`);
  unmount(info);
  unmount(cmp);
  target.remove();
  W.wk.page = null;
  tests++;
}

// ---------- ㉘ html のページの分割: 目次の受け手は 1 つ（プレビュー = ShadowPage の html の描き直しが目次を上書きしない） ----------
{
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const { EditorView } = await import("@codemirror/view");
  okWrite("H1");
  const delay0 = E.cfg.delay;
  E.cfg.delay = 100000;
  const html = "<h1>表題 一</h1>\n<p>a</p>\n<h2>節 二</h2>\n<p>b</p>\n";
  handlers.wiki_read_gui = (a) => ({ ...page(a.path, html, "H0"), kind: "html" });
  handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
  S.app.wikiOn = true;
  S.app.tab = tab.id;
  W.wk.status = { enabled: true, root: "/v" };
  W.wk.page = null;
  W.wk.split = true;
  tab.wiki = { path: "h.html", root: "/v", scrollTop: 0, editing: true };
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  const cmView = () => EditorView.findFromDOM(target.querySelector(".cm-editor"));
  const preview = () => target.querySelector(".wk-preview .wk-md")?.shadowRoot?.textContent ?? "";
  assert.ok(await waitFor(() => !!target.querySelector(".cm-editor") && preview().includes("節 二"), 3000), "html の分割が開かない");
  assert.deepEqual(W.wk.headings.map((h) => [h.text, h.line]), [["表題 一", 1], ["節 二", 3]], "html の編集中の目次（原文から・行つき）");
  // 打鍵 → 目次に出る
  cmView().dispatch({ changes: { from: cmView().state.doc.length, insert: "<h3>足した 三</h3>\n" }, userEvent: "input" });
  assert.ok(await waitFor(() => preview().includes("足した 三") && W.wk.headings.some((h) => h.text === "足した 三")), "html の打鍵が目次に出ない");
  // 打鍵によらない描き直し（参照フォルダ）で、プレビューの onHeadings が目次を上書きしない（受け手は 1 つ）
  const renders = () => performance.getEntriesByType("measure").filter((e) => e.name === "wk-render h.html").length;
  const r0 = renders();
  const refs0 = W.wk.refDirs;
  W.wk.refDirs = ["/ref-html"];
  assert.ok(await waitFor(() => renders() > r0), "参照フォルダの変化で html のプレビューを描き直さない（試験の前提）");
  await sleep(10);
  assert.ok(W.wk.headings.length === 3 && W.wk.headings.every((h) => typeof h.line === "number"), `html のプレビューの描き直しが目次を上書きした: ${JSON.stringify(W.wk.headings)}`);
  W.wk.refDirs = refs0;
  unmount(cmp);
  target.remove();
  E.cfg.delay = delay0;
  W.wk.split = false;
  W.wk.page = null;
  tests++;
}

// ---------- ㉙ 窓の外へドラッグしても閉じない（実部品 WikiDialogs） ----------
{
  const { default: Dialogs } = await import("./WikiDialogs.svelte");
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(Dialogs, { target });
  const ov = () => target.querySelector(".overlay");
  // 実機の順 = pointerdown（押した所）→ pointerup（離した所）→ click（押しと離しの共通の祖先 = ここでは覆い）
  const press = (down, up) => {
    down?.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, cancelable: true, button: 0 }));
    up?.dispatchEvent(new PointerEvent("pointerup", { bubbles: true, cancelable: true, button: 0 }));
    ov()?.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
    flushSync();
  };
  for (const kind of ["new", "newdir"]) {
    const open = () => {
      W.wk.dialog = { kind, dir: "" };
      flushSync();
    };
    open();
    // 窓の形（載せ替えの前後で同じ = app.css の .overlay / .modal・Esc と focus は窓の部品のまま）
    const frame = ov()?.querySelector(":scope > .modal");
    assert.ok(ov() && frame, `${kind}: .overlay > .modal でない`);
    assert.equal(ov().parentElement, target, `${kind}: 覆いの置き場が変わった`);
    assert.deepEqual([ov().getAttribute("role"), frame.getAttribute("role"), frame.getAttribute("aria-labelledby"), frame.className], ["presentation", "dialog", "wk-dlg-h", "modal"], `${kind}: 窓の属性が変わった`);
    const input = frame.querySelector(".modal-body input:not([type=radio])");
    input.value = "メモ";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    // 名前の欄で押して外で離す（字を選んでドラッグが外へ出た）= 開いたまま・打った字が残る
    press(input, ov());
    assert.equal(W.wk.dialog?.kind, kind, `${kind}: 窓の中で押して外で離すと閉じた`);
    assert.equal(frame.querySelector(".modal-body input:not([type=radio])")?.value, "メモ", `${kind}: 打った字が消えた`);
    // 覆いで押して窓の中で離す = 開いたまま
    press(ov(), frame.querySelector(".modal-head h3"));
    assert.equal(W.wk.dialog?.kind, kind, `${kind}: 覆いで押して中で離すと閉じた`);
    // 覆いで押したまま Esc で閉じ、開き直した直後の最初の離し（押しは覆いの出る前）= 開いたまま
    ov().dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, cancelable: true, button: 0 }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    flushSync();
    assert.equal(W.wk.dialog, null, `${kind}: Esc で閉じない`);
    open();
    press(null, ov());
    assert.equal(W.wk.dialog?.kind, kind, `${kind}: 開き直した直後の離しで閉じた`);
    // 覆いで押して覆いで離す = 今までどおり閉じる
    press(ov(), ov());
    assert.equal(W.wk.dialog, null, `${kind}: 覆いを押して離しても閉じない`);
  }
  unmount(cmp);
  target.remove();
  tests++;
}

// ---------- ㉚ 救出の窓・取込の窓は開いたときに焦点を自分へ移す（実部品 WikiRescue / ImportModal + 実 CodeMirror） ----------
{
  const { default: Rescue } = await import("./WikiRescue.svelte");
  const { default: Import } = await import("../ImportModal.svelte");
  const { EditorView } = await import("@codemirror/view");
  const host2 = document.createElement("div");
  document.body.append(host2);
  const h = createEditor(host2, { doc: "本文", mode: "md", onChange: () => {} });
  const target = document.createElement("div");
  document.body.append(target);
  const cmps = [mount(Rescue, { target }), mount(Import, { target })];
  flushSync();
  const cases = [
    // [名前, 出す, 片付ける, 焦点の先（窓の中の要素）]
    ["救出の窓", () => E.ed.stranded.push({ path: "r.md", content: "書けなかった本文", reason: "test" }), () => E.ed.stranded.splice(0),
      () => target.querySelector(".modal.wk-rs")],
    ["取込の窓", () => (S.app.importJob = { files: [{ path: "/x.pdf", name: "x.pdf", status: "running" }], running: true, cancel: false }), () => (S.app.importJob = null),
      () => target.querySelector(".modal.im-modal")],
  ];
  for (const [name, show, hide, want] of cases) {
    h.view.focus();
    assert.ok(EditorView.findFromDOM(host2.querySelector(".cm-editor")).hasFocus || document.activeElement === h.view.contentDOM, `${name}: 前提 = エディタに焦点がある`);
    show();
    flushSync();
    await sleep(0); // 焦点は queueMicrotask の後（窓の部品の描画が済んでから）
    const el = want();
    assert.ok(el, `${name}: 焦点の先が無い`);
    // 要素どうしの assert.equal は失敗の差分で jsdom の物を展開して止まる = 真偽値で比べる
    assert.ok(document.activeElement === el, `${name}: 開いても焦点が窓に移らない（${document.activeElement?.className || document.activeElement?.tagName}）`);
    assert.ok(target.querySelector(".modal")?.contains(document.activeElement), `${name}: 焦点が窓の外`);
    assert.equal(h.view.hasFocus, false, `${name}: 裏のエディタが焦点を持ったまま（打った字・⌘F が入る）`);
    // 焦点はボタンでない = Enter / Space で窓の操作（捨てる・クリップボードの上書き・中止・閉じる）が起きない
    assert.ok(document.activeElement.tagName !== "BUTTON", `${name}: 焦点がボタンにある（Enter / Space で押される）`);
    assert.equal(document.activeElement.getAttribute("tabindex"), "-1", `${name}: 焦点の先の枠が tabindex=-1 でない`);
    hide();
    flushSync();
  }
  for (const c of cmps) unmount(c);
  h.destroy();
  target.remove();
  host2.remove();
  // 枠に焦点を置いても輪は出さない（jsdom は :focus の輪を描かない = 規則の字面で見る。フォルダの窓 .nf-modal / .mf-modal と同じ）
  const appCss = readFileSync(new URL("../../app.css", import.meta.url), "utf8");
  assert.match(appCss, /\.modal\.im-modal:focus[^{]*\{ outline: none; \}/, "取込の窓の枠の焦点の輪を消していない");
  assert.match(readFileSync(new URL("./WikiRescue.svelte", import.meta.url), "utf8"), /\.wk-rs:focus \{ outline: none; \}/, "救出の窓の枠の焦点の輪を消していない");
  tests++;
}

// ---------- ㉛ 救出の窓・取込の窓を閉じたら、開く前の焦点へ戻す（実部品 WikiRescue / ImportModal / WikiDialogs + 実 CodeMirror） ----------
{
  const { default: Rescue } = await import("./WikiRescue.svelte");
  const { default: Import } = await import("../ImportModal.svelte");
  const { default: Dialogs } = await import("./WikiDialogs.svelte");
  const host2 = document.createElement("div");
  document.body.append(host2);
  const h = createEditor(host2, { doc: "一二三四五六", mode: "md", onChange: () => {} });
  // エディタへは CodeMirror 自身の戻し方（view.focus = 選択を状態から描き直し・スクロールさせない）で戻すこと = 呼ばれた回数を数える
  // （素の element.focus() でも jsdom ではカーソルが同じに見える = 配置が無くスクロールを測れない。実入力では選択・スクロールまで見た）
  let cmFocus = 0;
  let selInsideAtFocus = null; // CodeMirror の焦点を戻す瞬間に、DOM の選択範囲がエディタの中にあったか
  const viewFocus = h.view.focus.bind(h.view);
  h.view.focus = () => {
    cmFocus++;
    const s = window.getSelection();
    selInsideAtFocus = !!s?.anchorNode && h.view.dom.contains(s.anchorNode);
    return viewFocus();
  };
  const input = document.createElement("input");
  input.value = "abcdef";
  const input2 = document.createElement("input");
  document.body.append(input, input2);
  const target = document.createElement("div");
  document.body.append(target);
  const cmps = [mount(Rescue, { target }), mount(Import, { target }), mount(Dialogs, { target })];
  flushSync();
  // App の Esc の連鎖（取込の窓 = closeImport）の代わり
  const appEsc = (e) => e.key === "Escape" && S.closeImport();
  window.addEventListener("keydown", appEsc);
  const settle2 = async () => {
    flushSync();
    await sleep(0);
    await sleep(0);
    flushSync();
  };
  const WINDOWS = {
    救出: {
      open: () => E.ed.stranded.push({ path: "r.md", content: "書けなかった本文", reason: "test" }),
      ways: {
        ボタン: () => [...target.querySelectorAll(".modal-foot button")].find((b) => b.textContent.includes("捨てて閉じる")).click(),
        状態: () => E.ed.stranded.splice(0),
      },
    },
    取込: {
      open: () => (S.app.importJob = { files: [{ path: "/x.pdf", name: "x.pdf", status: "added" }], running: false, cancel: false }),
      ways: {
        Esc: () => document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true })),
        ボタン: () => [...target.querySelectorAll(".modal-foot button")].find((b) => b.textContent.includes("閉じる")).click(),
        状態: () => (S.app.importJob = null),
      },
    },
  };
  const ORIGINS = {
    エディタ: {
      put: () => {
        h.view.dispatch({ selection: { anchor: 3 } });
        h.view.focus();
      },
      back: () => h.view.hasFocus && h.view.state.selection.main.head === 3,
    },
    欄: {
      put: () => {
        input.focus();
        input.setSelectionRange(2, 4);
      },
      back: () => document.activeElement === input && input.selectionStart === 2 && input.selectionEnd === 4,
    },
    焦点なし: {
      put: () => document.activeElement?.blur?.(),
      back: () => document.activeElement === document.body,
    },
  };
  for (const [wname, w] of Object.entries(WINDOWS)) {
    for (const [way, close] of Object.entries(w.ways)) {
      for (const [oname, o] of Object.entries(ORIGINS)) {
        o.put();
        w.open();
        await settle2();
        assert.ok(target.querySelector(".modal")?.contains(document.activeElement), `${wname}×${oname}: 開いても焦点が窓に移らない`);
        cmFocus = 0;
        close();
        await settle2();
        if (oname === "エディタ") assert.equal(cmFocus, 1, `${wname}×${way}: エディタへ CodeMirror の戻し方（view.focus）で戻していない`);
        assert.equal(target.querySelector(".modal"), null, `${wname}（${way}）: 閉じない`);
        assert.ok(o.back(), `${wname}×${oname}×${way}: 閉じた後に元の焦点へ戻らない（${document.activeElement?.className || document.activeElement?.tagName}）`);
      }
    }
  }
  // 覆いの上の押下で閉じる場面: ブラウザは覆いの押下で DOM の選択範囲を覆いへ移す。そのまま CodeMirror へ焦点を戻すと、画面の外の
  // 選択を DOM に書けず、描かれていない区間に残った DOM の選択範囲を読んで選択が 0 へ飛んだ（jsdom は全行を描く = 飛ぶところは再現しない）
  // → 戻す瞬間の DOM の選択範囲がエディタの中（開く前の物を置き直した）であることを見る
  for (const [wname, w] of Object.entries(WINDOWS)) {
    ORIGINS.エディタ.put();
    w.open();
    await settle2();
    const ov = target.querySelector(".overlay");
    window.getSelection().collapse(ov, 0); // 覆いの押下（ブラウザの既定）を擬す
    selInsideAtFocus = null;
    w.ways.状態();
    await settle2();
    assert.equal(selInsideAtFocus, true, `${wname}: 覆いの押下の後、DOM の選択範囲を開く前の物に置き直さずに CodeMirror の焦点を戻した`);
    assert.ok(ORIGINS.エディタ.back(), `${wname}: 覆いの押下の後にエディタへ（カーソルのまま）戻らない`);
  }
  // 戻さない場合（元の焦点 = 欄・閉じ方 = 状態の側）
  const noReturn = async (label, w, during, want) => {
    ORIGINS.欄.put();
    w.open();
    await settle2();
    during();
    flushSync();
    w.ways.状態();
    await settle2();
    assert.ok(want(), `${label}: 戻した / 焦点が違う（${document.activeElement?.className || document.activeElement?.tagName}）`);
  };
  for (const [wname, w] of Object.entries(WINDOWS)) {
    await noReturn(`${wname}: 元の要素が消えた`, w, () => input.remove(), () => document.activeElement === document.body);
    document.body.append(input);
    // 消えた・display:none・disabled は jsdom もブラウザも focus() で焦点を移さない = 守りを外しても結果は同じ（守りは重ねとして残す）。
    // visibility:hidden と inert は jsdom では焦点が移る = 守り（shown / inert の検査）を単独で外すと落ちる
    await noReturn(`${wname}: 元の要素が隠れた（display:none）`, w, () => (input.style.display = "none"), () => document.activeElement === document.body);
    input.style.display = "";
    await noReturn(`${wname}: 元の要素が隠れた（visibility:hidden）`, w, () => (input.style.visibility = "hidden"), () => document.activeElement === document.body);
    input.style.visibility = "";
    await noReturn(`${wname}: 元の要素が無効（disabled）`, w, () => (input.disabled = true), () => document.activeElement === document.body);
    input.disabled = false;
    await noReturn(`${wname}: 元の要素が無効（inert）`, w, () => input.setAttribute("inert", ""), () => document.activeElement === document.body);
    input.removeAttribute("inert");
    await noReturn(`${wname}: 開いている間に窓の外へ移した`, w, () => input2.focus(), () => document.activeElement === input2);
    // 2 つの検査を 1 つずつ区別する:
    // (a) 閉じる時点の焦点が窓の外 → 戻さない（その外の要素が、戻す前の microtask までに消えて body になっても戻さない）
    ORIGINS.欄.put();
    w.open();
    await settle2();
    input2.focus();
    w.ways.状態();
    flushSync(); // 窓が閉じる（焦点は窓の外の input2）
    input2.remove();
    await settle2();
    assert.ok(document.activeElement === document.body, `${wname}: 閉じる時点で窓の外にあった焦点が消えた後に、元の欄へ戻した`);
    document.body.append(input2);
    // (b) 閉じた後、戻す前にほかの要素が焦点を取った → 戻さない（閉じる時点の焦点は窓の中）
    ORIGINS.欄.put();
    w.open();
    await settle2();
    w.ways.状態();
    flushSync(); // 窓が閉じる（焦点は窓の中 → body）
    input2.focus();
    await settle2();
    assert.ok(document.activeElement === input2, `${wname}: 閉じた後に焦点を取った要素から、元の欄へ奪い返した`);
    const tab0 = S.app.tab;
    await noReturn(`${wname}: 前面のタブが替わった`, w, () => (S.app.tab = "x-other"), () => document.activeElement === document.body);
    S.app.tab = tab0;
    // 閉じるのと同時に別の窓（OpenNote の新規ページの窓）が開く = その窓の焦点を奪わない
    ORIGINS.欄.put();
    w.open();
    await settle2();
    w.ways.状態();
    W.wk.dialog = { kind: "new", dir: "" };
    await settle2();
    const dlgInput = target.querySelector(".overlay .modal-body input:not([type=radio])");
    assert.ok(dlgInput && document.activeElement === dlgInput, `${wname}: 続けて開いた窓の焦点を奪った（${document.activeElement?.className || document.activeElement?.tagName}）`);
    W.wk.dialog = null;
    flushSync();
    // 続けて開く窓が自分で焦点を取らない（参照エディタ・移行・完全に削除 = 開いたとき focus が body のままの窓）= 戻すと裏の欄へ打ててしまう → 戻さない
    ORIGINS.欄.put();
    w.open();
    await settle2();
    w.ways.状態();
    const other = document.createElement("div");
    other.className = "overlay";
    document.body.append(other);
    await settle2();
    assert.ok(document.activeElement === document.body, `${wname}: 焦点を取らない窓が続けて開いたのに、裏の欄へ戻した（${document.activeElement?.tagName}）`);
    other.remove();
  }
  window.removeEventListener("keydown", appEsc);
  for (const c of cmps) unmount(c);
  h.destroy();
  for (const el of [target, host2, input, input2]) el.remove();
  tests++;
}

// ---------- ㉜ ページのツールバーは [編集]・[⋯]・[集中] の 3 個（実部品 WikiPage + WikiDialogs） ----------
{
  const { default: WikiPage } = await import("./WikiPage.svelte"); // WikiDialogs（[⋯] のメニュー）は WikiPage の中に置かれている
  const files = {
    "p.md": page("p.md", "# 題\n\n本文\n", "P0"),
    "h.html": { ...page("h.html", "<h1>題</h1>", "H0"), kind: "html" },
    "t.md": { ...page("t.md", "# 長い\n", "T0"), truncated: true },
  };
  handlers.wiki_read_gui = (a) => files[a.path];
  handlers.wiki_read_ref = (a) => ({ ...page(a.abs, "# 参照\n", "R0"), abs_path: a.abs, external: "/ref" });
  handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
  handlers.wiki_save = (a) => ({ ok: true, path: a.path, sha256: "S1", size: a.content.length, mtime: 7, created: false });
  const opened = [];
  handlers.wiki_open_path = (a) => (opened.push(a.path), false);
  handlers.set_ui_pref = () => null;
  S.app.wikiOn = true;
  W.wk.status = { enabled: true, root: "/v" };
  W.wk.split = false;
  W.wk.headColors = true;
  tab.wiki = { path: "p.md", root: "/v", scrollTop: 0, editing: false };
  const target = document.createElement("div");
  document.body.append(target);
  const cmps = [mount(WikiPage, { target })];
  const bar = () => [...target.querySelectorAll(".wk-bar button")].map((b) => b.textContent.trim());
  const editBtn = () => target.querySelector(".wk-edit-btn");
  const more = () => target.querySelector(".wk-bar .wk-more");
  const items = () => [...target.querySelectorAll(".wk-menu [role^=menuitem]")].map((b) => (b.getAttribute("aria-checked") === "true" ? "✓" : "") + b.textContent.trim().replace(/^✓/, ""));
  const item = (label) => [...target.querySelectorAll(".wk-menu [role^=menuitem]")].find((b) => b.textContent.includes(label));
  const openMore = () => {
    more().click();
    flushSync();
  };
  const isEditing = () => !!target.querySelector(".cm-editor");
  // md の表示: 3 個（字数はボタンでない・[文献] と保存は編集中だけ = 今のまま）
  assert.ok(await waitFor(() => W.wk.page?.path === "p.md" && !!editBtn(), 3000), "md のページで [編集] が出ない");
  assert.deepEqual(bar(), ["編集", "⋯", "集中"], "md の表示のツールバーが 3 個でない");
  assert.equal(editBtn().getAttribute("aria-pressed"), "false");
  // [⋯]（表示中）= 分割・見出しの色（今の設定 = ON）・外部で開く・名前の変更・複製・ゴミ箱
  openMore();
  assert.deepEqual(items(), ["分割", "✓見出しの色", "外部で開く", "名前を変更…", "複製する", "ゴミ箱に移す…"], "md の表示の [⋯] の並び");
  // 見出しの色 = 今と同じ働き（wk.headColors を反転・ui_wiki_heading_colors に書く）
  const n0 = calls.filter((c) => c.name === "set_ui_pref" && c.args.key === "ui_wiki_heading_colors").length;
  item("見出しの色").click();
  flushSync();
  assert.equal(W.wk.headColors, false, "見出しの色が切り替わらない");
  assert.equal(calls.filter((c) => c.name === "set_ui_pref" && c.args.key === "ui_wiki_heading_colors").length, n0 + 1, "見出しの色の設定を書かない");
  assert.equal(W.wk.menu, null, "項目を押してもメニューが閉じない");
  openMore();
  assert.ok(items().includes("見出しの色") && !items().includes("✓見出しの色"), "見出しの色のチェックが今の設定を映さない");
  // 外部で開く = 今と同じ（既定のアプリでページの絶対パスを開く）
  item("外部で開く").click();
  await sleep(5);
  assert.deepEqual(opened, ["/v/p.md"], "外部で開くがページを開かない");
  W.wk.headColors = true;
  // [編集] = ⌘E と同じ（編集へ・押し込み・[文献] が出る）
  editBtn().click();
  assert.ok(await waitFor(isEditing, 3000), "[編集] で編集にならない");
  assert.equal(editBtn().getAttribute("aria-pressed"), "true", "編集中に [編集] が押し込まれていない");
  assert.ok(bar().includes("文献") && bar().includes("編集") && bar().includes("⋯") && bar().includes("集中"), `編集中のツールバー: ${bar()}`);
  assert.ok(!target.querySelector(".wk-preview"), "分割でないのにプレビューが出た");
  // [⋯]（編集中）= 分割（未チェック）・見出しの色は出ない（今の [見出しの色] のボタンと同じく表示中だけ）
  openMore();
  assert.deepEqual(items(), ["分割", "外部で開く", "名前を変更…", "複製する", "ゴミ箱に移す…"], "編集中の [⋯] の並び");
  item("分割").click();
  assert.ok(await waitFor(() => !!target.querySelector(".wk-preview"), 3000), "[⋯] の分割で分割にならない");
  assert.equal(W.wk.split, true);
  openMore();
  assert.equal(item("分割").getAttribute("aria-checked"), "true", "分割中に [⋯] の分割にチェックが無い");
  item("分割").click(); // 分割中にもう一度 = エディタだけ（編集のまま）
  assert.ok(await waitFor(() => !target.querySelector(".wk-preview") && isEditing(), 3000), "分割をもう一度押してもエディタだけに戻らない");
  // 分割中の [編集] = 表示へ戻る（前の [表示] と同じ）
  void W.setEditing(true, true);
  assert.ok(await waitFor(() => !!target.querySelector(".wk-preview"), 3000));
  editBtn().click();
  assert.ok(await waitFor(() => !isEditing(), 3000), "分割中の [編集] で表示へ戻らない");
  // 表示中の [⋯] の分割 = 分割で編集へ（前の [分割] のボタンと同じ）・⌘E = 前回の配置（分割）で編集へ
  W.wk.split = false;
  openMore();
  item("分割").click();
  assert.ok(await waitFor(() => isEditing() && !!target.querySelector(".wk-preview"), 3000), "表示中の [⋯] の分割で分割の編集にならない");
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "e", metaKey: true, bubbles: true }));
  assert.ok(await waitFor(() => !isEditing(), 3000), "⌘E で表示へ戻らない");
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "e", metaKey: true, bubbles: true }));
  assert.ok(await waitFor(() => isEditing() && !!target.querySelector(".wk-preview"), 3000), "⌘E で前回の配置（分割）で編集へ入らない");
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "e", metaKey: true, bubbles: true }));
  assert.ok(await waitFor(() => !isEditing(), 3000));
  // 表示中（前回の配置 = 分割を覚えている）: [⋯] の分割にチェックは付かない（チェック = 分割で編集中）・[編集] は ⌘E と同じく前回の配置で編集へ
  assert.equal(W.wk.split, true);
  openMore();
  assert.equal(item("分割").getAttribute("aria-checked"), "false", "表示中なのに [⋯] の分割にチェックが付いた");
  W.wk.menu = null;
  flushSync();
  editBtn().click();
  assert.ok(await waitFor(() => isEditing() && !!target.querySelector(".wk-preview"), 3000), "[編集] が前回の配置（分割）で編集へ入らない（⌘E と違う）");
  editBtn().click();
  assert.ok(await waitFor(() => !isEditing(), 3000));
  W.wk.split = false;
  // html = [ソース]・[⋯] に見出しの色は無い
  tab.wiki = { path: "h.html", root: "/v", scrollTop: 0, editing: false };
  assert.ok(await waitFor(() => W.wk.page?.path === "h.html" && !!editBtn(), 3000), "html のページで [ソース] が出ない");
  assert.deepEqual(bar(), ["ソース", "⋯", "集中"], "html の表示のツールバー");
  openMore();
  assert.deepEqual(items(), ["分割", "外部で開く", "名前を変更…", "複製する", "ゴミ箱に移す…"], "html の [⋯] の並び");
  W.wk.menu = null;
  // 編集できない md（途中までしか読めていない）= 「編集不可」・[⋯] に分割は無い
  tab.wiki = { path: "t.md", root: "/v", scrollTop: 0, editing: false };
  assert.ok(await waitFor(() => W.wk.page?.path === "t.md" && !!target.querySelector(".wk-ro"), 3000), "編集できないページで「編集不可」が出ない");
  assert.deepEqual(bar(), ["⋯", "集中"], "編集できないページのツールバー");
  openMore();
  assert.deepEqual(items(), ["✓見出しの色", "外部で開く", "名前を変更…", "複製する", "ゴミ箱に移す…"], "編集できないページの [⋯] の並び");
  W.wk.menu = null;
  // 参照フォルダのページ = [⋯] が無い → [外部で開く] はボタンのまま
  tab.wiki = { path: "/ref/doc.md", root: "/v", scrollTop: 0, editing: false };
  assert.ok(await waitFor(() => W.wk.page?.path === "/ref/doc.md", 3000), "参照フォルダのページを読まない");
  flushSync();
  assert.deepEqual(bar(), ["外部で開く", "集中"], "参照フォルダのページのツールバー");
  assert.equal(more(), null, "参照フォルダのページに [⋯] が出た");
  for (const c of cmps) unmount(c);
  target.remove();
  tab.wiki = null;
  tests++;
}

// ---------- ㉝ [⋯] のメニューはキーボードで操作できる（実部品 WikiPage + 中の WikiDialogs） ----------
{
  const { default: WikiPage } = await import("./WikiPage.svelte");
  handlers.wiki_read_gui = (a) => page(a.path, "# 題\n\n本文\n", "K0");
  handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
  handlers.set_ui_pref = () => null;
  S.app.wikiOn = true;
  W.wk.status = { enabled: true, root: "/v" };
  W.wk.split = false;
  W.wk.headColors = true;
  tab.wiki = { path: "k.md", root: "/v", scrollTop: 0, editing: false };
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  const more = () => target.querySelector(".wk-bar .wk-more");
  assert.ok(await waitFor(() => W.wk.page?.path === "k.md" && !!more(), 3000), "[⋯] が出ない");
  const menu = () => target.querySelector(".wk-menu");
  const label = () => document.activeElement?.textContent?.trim().replace(/^✓/, "") ?? "";
  const key = async (el, k, init = {}) => {
    const ev = new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init });
    el.dispatchEvent(ev);
    flushSync();
    await sleep(0);
    flushSync();
    return ev;
  };
  // 実機の順: 項目の実行（click）の中で予約した microtask は、Svelte の描き直し（メニューが消える）より先に走る = flushSync を挟まずに待つ
  const run = async (el, k) => {
    el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
    await sleep(0);
    flushSync();
    await sleep(0);
  };
  // [⋯] に焦点 → ↓ = 開いて最初の項目へ
  more().focus();
  const evDown = await key(more(), "ArrowDown");
  assert.ok(menu(), "↓ でメニューが開かない");
  assert.equal(menu().getAttribute("role"), "menu");
  assert.equal(evDown.defaultPrevented, true, "↓ を奪わない（ページがスクロールする）");
  assert.equal(label(), "分割", `開いても最初の項目に焦点が無い（${label()}）`);
  // ↑↓（区切りを飛ばす・端で回る）・Home / End
  const seq = [];
  for (const k of ["ArrowDown", "ArrowDown", "End", "ArrowDown", "ArrowUp", "Home", "ArrowUp"]) {
    await key(document.activeElement, k);
    seq.push(label());
  }
  assert.deepEqual(seq, ["見出しの色", "外部で開く", "ゴミ箱に移す…", "分割", "ゴミ箱に移す…", "分割", "ゴミ箱に移す…"], "↑↓ / Home / End の移り方");
  // チェック付きの項目の aria-checked
  const hc = [...menu().querySelectorAll("[role=menuitemcheckbox]")].find((b) => b.textContent.includes("見出しの色"));
  assert.equal(hc.getAttribute("aria-checked"), "true", "見出しの色の aria-checked が今の設定を映さない");
  // Esc = 閉じて [⋯] へ
  await key(document.activeElement, "Escape");
  assert.equal(menu(), null, "Esc で閉じない");
  assert.ok(document.activeElement === more(), `Esc の後に [⋯] へ焦点が戻らない（${label()}）`);
  // Enter = 開いて最初の項目へ → ↓ で見出しの色 → Enter で実行（切り替わる・閉じて [⋯] へ）
  const evEnter = await key(more(), "Enter");
  assert.equal(evEnter.defaultPrevented, true, "[⋯] の Enter を奪わない（click でもう一度開く）");
  assert.ok(menu() && label() === "分割", "Enter で開いて最初の項目へ行かない");
  await key(document.activeElement, "ArrowDown");
  assert.equal(label(), "見出しの色");
  await run(document.activeElement, "Enter");
  assert.equal(W.wk.headColors, false, "Enter で見出しの色が切り替わらない");
  assert.equal(menu(), null, "実行してもメニューが閉じない");
  assert.ok(document.activeElement === more(), `実行の後に [⋯] へ焦点が戻らない（${label()}）`);
  // Space = 開く → Space で実行（見出しの色をもう一度 = 戻る・1 回だけ）
  await key(more(), " ");
  assert.ok(menu() && label() === "分割", "Space で開いて最初の項目へ行かない");
  await key(document.activeElement, "ArrowDown");
  await run(document.activeElement, " ");
  assert.equal(W.wk.headColors, true, "Space で見出しの色が切り替わらない / 2 回切り替わった");
  assert.ok(document.activeElement === more());
  // Tab = 閉じる（焦点は [⋯] に置き、既定の Tab がそこから次へ進む = 奪わない）
  await key(more(), "ArrowDown");
  const evTab = await key(document.activeElement, "Tab");
  assert.equal(menu(), null, "Tab で閉じない");
  assert.equal(evTab.defaultPrevented, false, "Tab を奪った（次の要素へ進まない）");
  assert.ok(document.activeElement === more(), "Tab の前に [⋯] へ焦点を置いていない（メニューの次 = 文書の末尾へ飛ぶ）");
  // マウスで開いたとき（click の detail = 1）は焦点を動かさない（今のまま）
  const outside = document.createElement("input");
  document.body.append(outside);
  outside.focus();
  more().dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
  flushSync();
  await sleep(0);
  assert.ok(menu(), "マウスで開かない");
  assert.ok(document.activeElement === outside, "マウスで開いたのに焦点を動かした");
  W.wk.menu = null;
  flushSync();
  outside.remove();
  unmount(cmp);
  target.remove();
  tab.wiki = null;
  tests++;
}

// ---------- ㉞ [⋯] から窓を開く項目・ゴミ箱の窓の焦点（実部品 WikiPage + 中の WikiDialogs） ----------
{
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const { EditorView } = await import("@codemirror/view");
  handlers.wiki_read_gui = (a) => page(a.path, "# 題\n\n一二三四五六七八\n", "Q0");
  handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
  handlers.wiki_save = (a) => ({ ok: true, path: a.path, sha256: "S1", size: a.content.length, mtime: 7, created: false });
  S.app.wikiOn = true;
  W.wk.status = { enabled: true, root: "/v" };
  W.wk.split = false;
  tab.wiki = { path: "q.md", root: "/v", scrollTop: 0, editing: false };
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  const more = () => target.querySelector(".wk-bar .wk-more");
  assert.ok(await waitFor(() => W.wk.page?.path === "q.md" && !!more(), 3000), "[⋯] が出ない");
  const menu = () => target.querySelector(".wk-menu");
  const settle3 = async () => {
    await sleep(0);
    flushSync();
    await sleep(0);
    flushSync();
    await sleep(0);
  };
  const press = async (el, k) => {
    el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
    await settle3();
  };
  const where = () => `${document.activeElement?.tagName}.${document.activeElement?.className}:${document.activeElement?.textContent?.trim().slice(0, 10)}`;
  const inDialog = () => !!target.querySelector(".overlay .modal")?.contains(document.activeElement);
  // キーで [⋯] → 「ゴミ箱に移す…」→ Enter = ゴミ箱の窓・焦点は窓の [キャンセル]（[⋯] でない・削除の側でない）
  more().focus();
  await press(more(), "Enter");
  await press(document.activeElement, "End");
  assert.ok(document.activeElement?.textContent.includes("ゴミ箱に移す"), `End で最後の項目へ行かない（${where()}）`);
  await press(document.activeElement, "Enter");
  assert.equal(W.wk.dialog?.kind, "trash", "ゴミ箱の窓が開かない");
  assert.ok(inDialog(), `窓を開いたのに焦点が窓の中に無い（${where()}）`);
  assert.ok(document.activeElement !== more(), "焦点が覆いの下の [⋯] にある");
  assert.equal(document.activeElement?.textContent.trim(), "キャンセル", `ゴミ箱の窓の既定の焦点が [キャンセル] でない（${where()}）`);
  // 覆いの間: [⋯] の Enter / Space / ↓ とキーの click でメニューを開かない・窓は入れ替わらない
  for (const k of ["Enter", " ", "ArrowDown"]) {
    const ev = new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true });
    more().dispatchEvent(ev);
    await settle3();
    assert.equal(menu(), null, `覆いの下の [⋯] の ${JSON.stringify(k)} でメニューを開いた`);
    assert.equal(ev.defaultPrevented, false, `覆いの下で [⋯] の ${JSON.stringify(k)} を奪った`);
  }
  more().dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 0 }));
  await settle3();
  assert.equal(menu(), null, "覆いの下の [⋯] の click でメニューを開いた");
  assert.equal(W.wk.dialog?.kind, "trash", "窓が入れ替わった");
  // Esc で閉じる = 開く前の焦点（[⋯]）へ戻る
  await press(document.activeElement, "Escape");
  assert.equal(W.wk.dialog, null, "Esc でゴミ箱の窓が閉じない");
  assert.ok(document.activeElement === more(), `ゴミ箱の窓を閉じても [⋯] へ戻らない（${where()}）`);
  // キーで「名前を変更…」= 名前の欄に焦点（窓が取った焦点を、メニューの後始末が奪わない）
  await press(more(), "Enter");
  const rename = [...menu().querySelectorAll("[role^=menuitem]")].find((b) => b.textContent.includes("名前を変更"));
  rename.focus();
  await press(rename, "Enter");
  assert.equal(W.wk.dialog?.kind, "rename");
  await settle3();
  await sleep(5);
  assert.ok(document.activeElement?.matches?.(".overlay .modal-body input"), `名前の変更の窓の欄に焦点が無い / 奪われた（${where()}）`);
  W.wk.dialog = null;
  flushSync();
  // マウスで（押下で焦点を移さない WKWebView と同じ = エディタに焦点のまま）「ゴミ箱に移す…」→ 焦点は [キャンセル]・打った字は本文に入らない・
  // [キャンセル] で閉じたらエディタへ（カーソルのまま）
  more().blur();
  target.querySelector(".wk-edit-btn").click();
  assert.ok(await waitFor(() => !!target.querySelector(".cm-editor"), 3000), "編集にならない");
  const view = () => EditorView.findFromDOM(target.querySelector(".cm-editor"));
  view().dispatch({ selection: { anchor: 6 } });
  view().focus();
  assert.ok(view().hasFocus, "前提 = エディタに焦点");
  more().dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
  await settle3();
  assert.ok(view().hasFocus, "マウスで開いたのに焦点を動かした");
  [...menu().querySelectorAll("[role^=menuitem]")].find((b) => b.textContent.includes("ゴミ箱に移す")).click();
  await settle3();
  assert.equal(W.wk.dialog?.kind, "trash");
  assert.equal(view().hasFocus, false, "ゴミ箱の窓が開いてもエディタが焦点を持ったまま（打った字が窓の下の本文に入る）");
  assert.equal(document.activeElement?.textContent.trim(), "キャンセル", `ゴミ箱の窓の既定の焦点が [キャンセル] でない（${where()}）`);
  [...target.querySelectorAll(".overlay .modal-foot button")].find((b) => b.textContent.trim() === "キャンセル").click();
  await settle3();
  assert.equal(W.wk.dialog, null);
  assert.ok(view().hasFocus && view().state.selection.main.head === 6, `ゴミ箱の窓を閉じてもエディタへ（カーソルのまま）戻らない（${where()}）`);
  unmount(cmp);
  target.remove();
  tab.wiki = null;
  tests++;
}

// ---------- ㉟ 覆いでメニューを閉じる・ダイアログの焦点の戻り・2 段目の焦点・見た目（実部品 WikiPage + 中の WikiDialogs） ----------
{
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const CHS = await import("../chat/chat.svelte.ts");
  handlers.wiki_read_gui = (a) => page(a.path, "# 題\n\n本文\n", "L0");
  handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
  handlers.wiki_rename_preview = (a) => ({ from: a.from, to: a.to, links: 2, files: ["x.md"], other: [] });
  S.app.wikiOn = true;
  W.wk.status = { enabled: true, root: "/v" };
  W.wk.split = false;
  tab.wiki = { path: "l.md", root: "/v", scrollTop: 0, editing: false };
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  const more = () => target.querySelector(".wk-bar .wk-more");
  assert.ok(await waitFor(() => W.wk.page?.path === "l.md" && !!more(), 3000), "[⋯] が出ない");
  const menu = () => target.querySelector(".wk-menu");
  const settle4 = async () => {
    for (let i = 0; i < 3; i++) {
      await sleep(0);
      flushSync();
    }
  };
  const where = () => `${document.activeElement?.tagName}.${String(document.activeElement?.className).split(" ")[0]}:${document.activeElement?.textContent?.trim().slice(0, 10)}`;
  // 見た目: [⋯] の全項目にチェックの欄（字の頭が揃う）
  more().dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
  await settle4();
  const rows = [...menu().querySelectorAll("[role^=menuitem]")];
  assert.ok(rows.length >= 5 && rows.every((b) => b.classList.contains("wk-menu-chk") && b.querySelector(".wk-menu-check")), `[⋯] にチェックの欄の無い項目がある: ${rows.filter((b) => !b.querySelector(".wk-menu-check")).map((b) => b.textContent.trim())}`);
  // [⋯] を開いたまま、外から窓（取込・救出・同意）が出る = メニューを閉じる
  for (const [name, on, off] of [
    ["取込", () => (S.app.importJob = { files: [], running: false, cancel: false }), () => (S.app.importJob = null)],
    ["救出", () => E.ed.stranded.push({ path: "z.md", content: "x", reason: "t" }), () => E.ed.stranded.splice(0)],
    ["同意", () => (CHS.chat.consent = true), () => (CHS.chat.consent = false)],
  ]) {
    if (!menu()) {
      more().dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
      await settle4();
    }
    assert.ok(menu(), `${name}: 前提 = メニューが開いている`);
    on();
    await settle4();
    assert.equal(menu(), null, `${name}: 窓が出てもメニューが窓の上に残った`);
    off();
    await settle4();
  }
  // ダイアログを Esc / [キャンセル] で閉じたら開く前の焦点（[⋯]）へ
  const esc = async () => {
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await settle4();
  };
  const cancel = async () => {
    [...target.querySelectorAll(".overlay .modal-foot button")].find((b) => b.textContent.trim() === "キャンセル").click();
    await settle4();
  };
  for (const [name, d] of [
    ["新規ページ", { kind: "new", dir: "" }],
    ["新規フォルダ", { kind: "newdir", dir: "" }],
    ["名前の変更", { kind: "rename", path: "l.md", preview: null, to: "" }],
    ["破棄の確認", { kind: "discard", path: "l.md", error: "" }],
    ["ゴミ箱", { kind: "trash", path: "l.md", dir: false }],
  ]) {
    for (const [way, close] of [["Esc", esc], ["キャンセル", cancel]]) {
      more().focus();
      W.wk.dialog = d;
      await settle4();
      await sleep(5);
      assert.ok(target.querySelector(".overlay .modal")?.contains(document.activeElement), `${name}: 開いても焦点が窓に移らない（${where()}）`);
      await close();
      assert.equal(W.wk.dialog, null, `${name}（${way}）: 閉じない`);
      assert.ok(document.activeElement === more(), `${name}（${way}）: 閉じても開く前の焦点（[⋯]）へ戻らない（${where()}）`);
    }
  }
  // 名前の変更 → [次へ]（処理中）→ 2 段目（リンクの書き換えの確認）= 押せるようになってから [書き換えて変更] に焦点
  more().focus();
  W.wk.dialog = { kind: "rename", path: "l.md", preview: null, to: "" };
  await settle4();
  const nameInput = target.querySelector(".overlay .modal-body input");
  nameInput.value = "l2";
  nameInput.dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
  [...target.querySelectorAll(".overlay .modal-foot button")].find((b) => b.textContent.trim() === "次へ").click();
  assert.ok(await waitFor(() => !!W.wk.dialog?.preview, 2000), "名前の変更の 2 段目が出ない");
  assert.ok(await waitFor(() => document.activeElement?.textContent?.trim() === "書き換えて変更", 1000), `名前の変更の 2 段目で既定のボタンに焦点が無い（${where()}）`);
  W.wk.dialog = null;
  await settle4();
  // 見た目: [編集] の押し込み = [集中] の ON と同じ見せ方（アクセントの枠 + --accent-soft）= 規則の字面で見る（jsdom は scoped の CSS を計算しない）
  const pageSrc = readFileSync(new URL("./WikiPage.svelte", import.meta.url), "utf8");
  assert.match(pageSrc, /\.wk-btn\.wk-edit-on \{ background: var\(--accent-soft\); border-color: var\(--accent\); \}/, "[編集] の押し込みが [集中] の ON の見せ方でない");
  unmount(cmp);
  target.remove();
  tab.wiki = null;
  tests++;
}

// ---------- ㊱ 右ペインの目次は一番上・全レベル（実部品 WikiPage + WikiInfo） ----------
// 前は ページ情報 → リンク先の文献 → バックリンク → 目次 の順で、目次は h3 まで（h4〜h6 を使う記事では足りない）
{
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const { default: WikiInfo } = await import("./WikiInfo.svelte");
  const { EditorView } = await import("@codemirror/view");
  okWrite("T1");
  const delay0 = E.cfg.delay;
  E.cfg.delay = 100000;
  const files = {
    // h1〜h6 とレベルの飛び（h2 → h5）
    "all.md": "# 一\n\n## 二\n\n### 三\n\n#### 四\n\n##### 五\n\n###### 六\n\n## 二の二\n\n##### 飛んだ五\n\n本文\n",
    // h2 から始まる記事（字下げの起点は一番浅い見出し）・h1 → h4 の飛び
    "h2.md": "## 始まり\n\n### 下\n\n# 深い一\n\n#### 飛んだ四\n",
    "none.md": "見出しのない本文\n\nもう 1 段落\n",
  };
  handlers.wiki_read_gui = (a) => ({ ...page(a.path, files[a.path], "T0"), items: ["K1"] });
  handlers.get_item = (a) => ({ key: a.key, csl: { title: "文献の題" }, attachments: [], deleted: false });
  handlers.wiki_backlinks = () => ({ total: 1, pages: [{ path: "from.md", title: "参照元", excerpt: "" }] });
  S.app.wikiOn = true;
  S.app.tab = tab.id;
  W.wk.status = { enabled: true, root: "/v" };
  W.wk.page = null;
  W.wk.split = false;
  tab.wiki = { path: "all.md", root: "/v", scrollTop: 0, editing: false };
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  const info = mount(WikiInfo, { target });
  const aside = () => target.querySelector(".wk-info");
  // 節の名前（目次の見出しは畳むボタンの中 = .wk-sec-name・▸ と件数を除く）
  const secs = () => [...aside().querySelectorAll(":scope > .wk-sec > h3")].map((h) => (h.querySelector(".wk-sec-name") ?? h.childNodes[0]).textContent.trim());
  const toc = () => [...aside().querySelectorAll(".wk-toc")];
  const rows = () => toc().map((b) => [b.textContent, Number(b.dataset.level), b.style.paddingLeft]);
  const open = async (p) => {
    tab.wiki = { path: p, root: "/v", scrollTop: 0, editing: false };
    assert.ok(await waitFor(() => W.wk.page?.path === p && !!target.querySelector(".wk-body .wk-md"), 3000), `${p} を開けない`);
    await tick();
  };
  await open("all.md");
  assert.ok(await waitFor(() => toc().length === 8 && secs().includes("リンク先の文献") && secs().includes("バックリンク"), 3000), `目次・節が出ない: ${secs()} / ${rows()}`);
  // 節の順 = 目次が一番上
  assert.deepEqual(secs(), ["目次", "ページ情報", "リンク先の文献", "バックリンク"], "右ペインの節の順");
  // 全レベル（h4〜h6 も）・字下げ = 一番浅い見出しから 1 段 12px・飛んだレベルはレベルのとおり
  assert.deepEqual(
    rows(),
    [["一", 1, "4px"], ["二", 2, "16px"], ["三", 3, "28px"], ["四", 4, "40px"], ["五", 5, "52px"], ["六", 6, "64px"], ["二の二", 2, "16px"], ["飛んだ五", 5, "52px"]],
    "目次の行（レベル・字下げ）"
  );
  // 押す（表示）= その見出しへ（wk.scrollTo = 表示の部品が受ける）→ 表示の部品が本文の中のその見出しの要素を scrollIntoView する
  // （jsdom は配置を持たない = 位置は測れない。移る先の要素が正しいことまでを見る）
  const sivWas = HTMLElement.prototype.scrollIntoView; // ハーネスは HTMLElement に no-op を置く
  const sivTo = [];
  HTMLElement.prototype.scrollIntoView = function () {
    sivTo.push(this);
  };
  const body = () => target.querySelector(".wk-body .wk-md")?.shadowRoot;
  for (const i of [5, 7, 0]) {
    const n = sivTo.length;
    toc()[i].click();
    assert.equal(W.wk.scrollTo?.id, W.wk.headings[i].id, `表示で ${i} 番目の行を押してもその見出しへ移らない`);
    assert.ok(await waitFor(() => sivTo.length > n), `表示で ${i} 番目の行を押しても本文が動かない（scrollIntoView が呼ばれない）`);
    const el = sivTo.at(-1);
    assert.ok(body()?.contains(el) && /^H[1-6]$/.test(el.nodeName) && el.id === W.wk.headings[i].id && el.textContent.trim() === W.wk.headings[i].text,
      `表示で ${i} 番目の行を押して、別の要素へ移った: ${el?.nodeName}#${el?.id}`);
  }
  HTMLElement.prototype.scrollIntoView = sivWas;
  // h2 から始まる記事 = h2 が字下げなし・h1 は起点より浅い = 起点を h1 に取り直す（一番浅い見出し = h1）
  await open("h2.md");
  assert.ok(await waitFor(() => toc()[0]?.textContent === "始まり"), `h2.md の目次: ${rows()}`);
  assert.deepEqual(rows(), [["始まり", 2, "16px"], ["下", 3, "28px"], ["深い一", 1, "4px"], ["飛んだ四", 4, "40px"]], "h1 を含む記事の字下げ（起点 = h1）");
  files["h2.md"] = "## 始まり\n\n### 下\n\n#### 飛んだ四\n";
  await open("none.md");
  assert.ok(await waitFor(() => secs()[0] === "ページ情報"), `見出しのないページ: ${secs()}`);
  assert.equal(toc().length, 0, "見出しのないページに目次の行が出た");
  assert.ok(!secs().includes("目次"), "見出しのないページに目次の節が出た");
  W.wk.page = null;
  await open("h2.md");
  assert.ok(await waitFor(() => toc().length === 3), `h2 起点の目次: ${rows()}`);
  assert.deepEqual(rows(), [["始まり", 2, "4px"], ["下", 3, "16px"], ["飛んだ四", 4, "28px"]], "h2 から始まる記事の字下げ（起点 = h2）");
  // 編集中: 押すと caret がその行へ（h4 以下も）・見出しを足すと目次が追う（全レベル）
  await open("all.md");
  assert.ok(await waitFor(() => toc().length === 8));
  target.querySelector(".wk-edit-btn").click();
  assert.ok(await waitFor(() => !!target.querySelector(".cm-editor"), 3000), "編集のエディタが開かない");
  const cmView = () => EditorView.findFromDOM(target.querySelector(".cm-editor"));
  const caretLine = () => cmView().state.doc.lineAt(cmView().state.selection.main.head).number;
  const lineOf = (text) => cmView().state.doc.toString().split("\n").indexOf(text) + 1;
  await tick();
  cmView().dispatch({ selection: { anchor: 0 } });
  toc()[5].click();
  assert.ok(await waitFor(() => caretLine() === lineOf("###### 六")), `編集で h6 の行を押しても caret が動かない: ${caretLine()}`);
  cmView().dispatch({ changes: { from: cmView().state.doc.length, insert: "\n###### 足した六\n" }, userEvent: "input" });
  assert.ok(await waitFor(() => toc().at(-1)?.textContent === "足した六"), `編集中に足した h6 が目次に出ない: ${rows()}`);
  assert.deepEqual(rows().at(-1), ["足した六", 6, "64px"], "足した h6 の字下げ");
  assert.equal(secs()[0], "目次", "編集中も目次が一番上でない");
  cmView().dispatch({ selection: { anchor: 0 } });
  toc().at(-1).click();
  assert.ok(await waitFor(() => caretLine() === lineOf("###### 足した六")), `足した h6 へ動かない: ${caretLine()}`);
  target.querySelector(".wk-edit-btn").click();
  assert.ok(await waitFor(() => !target.querySelector(".cm-editor")), "表示へ戻らない");
  unmount(info);
  unmount(cmp);
  target.remove();
  E.cfg.delay = delay0;
  W.wk.page = null;
  tab.wiki = null;
  tests++;
}

// ---------- ㊲ 右ペインの目次を畳む / 開く（実部品 WikiPage + WikiInfo） ----------
// 見出しが約 16 を超えると下の節のボタンが画面の外・Tab は見出しの数だけ要った → 節の見出しを押すと行だけを畳む（件数は見出しに残す・覚える = toc.test）
{
  const { default: WikiPage } = await import("./WikiPage.svelte");
  const { default: WikiInfo } = await import("./WikiInfo.svelte");
  const { EditorView } = await import("@codemirror/view");
  okWrite("T1");
  const delay0 = E.cfg.delay;
  E.cfg.delay = 100000;
  const src = "# 一\n\n## 二\n\n### 三\n\n本文\n";
  const settle4 = async () => {
    for (let i = 0; i < 4; i++) {
      await tick();
      flushSync();
    }
  };
  handlers.wiki_read_gui = (a) => page(a.path, a.path === "plain.md" ? "見出しなし\n" : src, "T0");
  handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
  S.app.wikiOn = true;
  S.app.tab = tab.id;
  W.wk.status = { enabled: true, root: "/v" };
  W.wk.page = null;
  W.wk.split = false;
  W.wk.tocOpen = true;
  tab.wiki = { path: "fold.md", root: "/v", scrollTop: 0, editing: false };
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(WikiPage, { target });
  const info = mount(WikiInfo, { target });
  const head = () => target.querySelector(".wk-info .wk-sec-toggle");
  const toc = () => [...target.querySelectorAll(".wk-info .wk-toc")];
  const count = () => head()?.querySelector(".wk-n")?.textContent;
  const tocWrites = () => calls.filter((c) => c.name === "set_ui_pref" && c.args.key === "ui_wiki_toc").map((c) => c.args.value);
  const w0 = tocWrites().length;
  assert.ok(await waitFor(() => toc().length === 3, 3000), "目次が出ない");
  // 既定 = 開いている・見出しは <button>（aria-expanded・aria-controls = 行の入れ物）・件数
  const h = head();
  assert.equal(h.tagName, "BUTTON", "目次の見出しがボタンでない（Enter / Space で開閉できない）");
  assert.equal(h.getAttribute("aria-expanded"), "true", "既定が開いていない");
  const ctl = document.getElementById(h.getAttribute("aria-controls"));
  assert.ok(ctl && ctl.contains(toc()[0]), "aria-controls が目次の行の入れ物を指していない");
  assert.equal(count(), "3", "見出しの件数が無い");
  assert.ok(h.querySelector(".wk-chev.wk-chev-open"), "開いているのに ▸ が開いた向きでない（左バーの節と同じ型）");
  // 畳む: 行を描かない（Tab にも読み上げにも出ない）・件数は残る・焦点は見出しのボタンのまま（作り直さない）・ui_wiki_toc = "0"
  h.focus();
  h.click();
  await settle4();
  assert.ok(head() === h, "開閉で見出しのボタンを作り直した（焦点が飛ぶ）");
  assert.ok(document.activeElement === h, "畳んだら焦点が見出しのボタンから外れた");
  assert.equal(h.getAttribute("aria-expanded"), "false");
  assert.equal(toc().length, 0, "畳んだのに目次の行を描いた");
  assert.equal(target.querySelectorAll(".wk-info #" + h.getAttribute("aria-controls") + " button").length, 0, "畳んだ入れ物に押せる物が残った（Tab に入る）");
  assert.equal(count(), "3", "畳んでいる間に件数が無い");
  assert.ok(!h.querySelector(".wk-chev-open"), "畳んだのに ▸ が開いた向き");
  assert.deepEqual(tocWrites().slice(w0), ["0"], "畳んだ状態を ui_wiki_toc に書かない");
  // 覆いの下では見出しのボタンが動かない（ホームの行・新規ページと同じ）
  S.app.importJob = { files: [], running: false, cancel: false };
  await settle4();
  h.click();
  await settle4();
  assert.equal(h.getAttribute("aria-expanded"), "false", "覆いの下で目次の見出しが開閉した");
  assert.equal(tocWrites().length - w0, 1, "覆いの下で目次の開閉を書いた");
  S.app.importJob = null;
  await settle4();
  // 畳んだまま別のページへ = 畳んだまま（全ページ共通の 1 つ）・見出しのないページは節ごと無い（今のまま）
  tab.wiki = { path: "plain.md", root: "/v", scrollTop: 0, editing: false };
  assert.ok(await waitFor(() => W.wk.page?.path === "plain.md" && !head(), 3000), "見出しのないページに目次の節が出た");
  tab.wiki = { path: "other.md", root: "/v", scrollTop: 0, editing: false };
  assert.ok(await waitFor(() => W.wk.page?.path === "other.md" && count() === "3", 3000), "別のページへ移ったら目次の見出しが出ない");
  assert.equal(head().getAttribute("aria-expanded"), "false", "別のページへ移ったら畳んだ状態を忘れた");
  assert.equal(toc().length, 0);
  // 畳んでいる間の編集: 見出しを足す / 消すと件数が追う（行は描かない）
  target.querySelector(".wk-edit-btn").click();
  assert.ok(await waitFor(() => !!target.querySelector(".cm-editor"), 3000), "編集のエディタが開かない");
  const cm = () => EditorView.findFromDOM(target.querySelector(".cm-editor"));
  cm().dispatch({ changes: { from: cm().state.doc.length, insert: "\n#### 足した四\n" }, userEvent: "input" });
  assert.ok(await waitFor(() => count() === "4"), `畳んでいる間に足した見出しで件数が追わない: ${count()}`);
  assert.equal(toc().length, 0, "畳んでいる間の編集で行を描いた");
  const t0 = cm().state.doc.toString().indexOf("## 二");
  cm().dispatch({ changes: { from: t0, to: t0 + 2, insert: "" }, userEvent: "input" });
  assert.ok(await waitFor(() => count() === "3"), `畳んでいる間に消した見出しで件数が追わない: ${count()}`);
  // 見出しで畳んだ所の印（CodeMirror の .cm-foldPlaceholder）の aria-label / title は日本語の表示で日本語（前は CodeMirror の英語
  // "folded code" / "unfold" のまま）・英語の値は CodeMirror の原文と同じ（英語の表示は前のまま）
  {
    const { foldCode, unfoldAll } = await import("@codemirror/language");
    cm().dispatch({ selection: { anchor: 0 } });
    foldCode(cm());
    assert.ok(await waitFor(() => !!target.querySelector(".cm-foldPlaceholder")), "見出しで畳めない（試験の前提）");
    const ph = target.querySelector(".cm-foldPlaceholder");
    assert.deepEqual([ph.getAttribute("aria-label"), ph.title], ["畳んだ部分", "開く"], "日本語の表示で畳んだ所の印が英語のまま");
    const { en } = await import("../i18n-en.ts");
    unfoldAll(cm());
    await settle4();
    // CodeMirror の phrase の全部（使っている部品 = view / search / language / autocomplete / commands の原文の名前）が日本語の表示で訳される
    // （読み上げ・行へ移動の窓・補完の一覧の aria-label も = 画面に字で出ない物を含む）。英語の値は原文と同じ
    const NAMES = ["Find", "Replace", "next", "previous", "all", "match case", "regexp", "by word", "replace", "replace all", "close", "current match", "on line",
      "replaced match on line $", "replaced $ matches", "Control character", "folded code", "unfold", "Folded lines", "Unfolded lines", "to", "Go to line", "go",
      "Completions", "Selection deleted"];
    const left = NAMES.filter((n) => cm().state.phrase(n) === n);
    assert.deepEqual(left, [], `日本語の表示で CodeMirror の原文のままの phrase: ${left}`);
    // 訳を足した phrase の英語の値 = CodeMirror の原文（英語の表示は前のまま）。前からの phrase は共有の鍵の英語（例 all → 「すべて」= All Papers）
    // のまま（変えない）
    const ADDED = { "folded code": "畳んだ部分", unfold: "開く::fold", "Folded lines": "畳んだ行", "Unfolded lines": "開いた行", to: "〜::fold", "Go to line": "行へ移動",
      go: "移動::goto", Completions: "候補", "Selection deleted": "選択範囲を削除しました" };
    const enLeft = Object.entries(ADDED).filter(([n, k]) => en[k] !== n);
    // 検索と置換の窓の [all] は専用の鍵（前は LIBRARY の左バーの「すべて」= 英語の表示で「All Papers」）・ja は「すべて」のまま
    assert.equal(cm().state.phrase("all"), "すべて", "日本語の表示の検索と置換の [all]");
    assert.equal(en["すべて::search"], "All", "英語の表示の検索と置換の [all] が All でない");
    assert.notEqual(en["すべて"], "All", "前提: LIBRARY の左バーの「すべて」は別の英語（All Papers）");
    // 日本語の表示では鍵を戻しても字が同じ（すべて）= 使う鍵を字面で見る（英語の表示の字は下の言語の切替の節で実際に見る）
    assert.match(readFileSync(new URL("./WikiEditor.svelte", import.meta.url), "utf8"), /\ball: t\("すべて::search"\)/, "検索と置換の [all] が専用の鍵を使っていない");
    // 編集を開いたまま言語を切り替える = 開いているエディタの文言が今の言語になる（作り直さない）。未保存・選択あり・見出しを畳んだまま・
    // 検索の窓を開いたまま（問い合わせの字あり）で ja → en → ja。本文・選択・取消しの履歴・未保存の印・問い合わせ・焦点が変わらず、保存も確認の窓も出ない
    {
      const { i18n } = await import("../i18n.svelte.ts");
      const { undoDepth } = await import("@codemirror/commands");
      const { openSearchPanel: openSP, getSearchQuery, setSearchQuery, SearchQuery } = await import("@codemirror/search");
      const view0 = cm();
      const dom0 = target.querySelector(".cm-editor");
      cm().dispatch({ changes: { from: cm().state.doc.length, insert: "\n未保存の行\n" }, userEvent: "input" });
      await settle4();
      // 畳むのは最後の見出し（足した四 = 未保存の行を含む節）・選択はその外（本文 = 畳んだ範囲に入れると CodeMirror が開く）
      cm().dispatch({ selection: { anchor: cm().state.doc.toString().indexOf("#### 足した四") } });
      foldCode(cm());
      await settle4();
      openSP(cm());
      await settle4();
      cm().dispatch({ effects: setSearchQuery.of(new SearchQuery({ search: "一", replace: "壱" })) });
      await settle4();
      const at = cm().state.doc.toString().indexOf("本文");
      cm().dispatch({ selection: { anchor: at, head: at + 2 } });
      cm().focus();
      await settle4();
      const snap = () => ({ doc: cm().state.doc.toString(), sel: [cm().state.selection.main.anchor, cm().state.selection.main.head], undo: undoDepth(cm().state), state: E.ed.state, q: getSearchQuery(cm().state).search, r: getSearchQuery(cm().state).replace, panel: !!target.querySelector(".cm-search"), focus: cm().hasFocus });
      const before = snap();
      const writes0 = writes().length;
      const labels = () => ({ all: target.querySelector('.cm-search button[name="select"]')?.textContent, next: target.querySelector('.cm-search button[name="next"]')?.textContent, field: target.querySelector('.cm-search input[name="search"]')?.getAttribute("placeholder"), fold: target.querySelector(".cm-foldPlaceholder")?.title, foldLabel: target.querySelector(".cm-foldPlaceholder")?.getAttribute("aria-label") });
      assert.deepEqual(labels(), { all: "すべて", next: "次へ", field: "検索", fold: "開く", foldLabel: "畳んだ部分" }, "前提: 日本語の表示の検索の窓と畳んだ所の印");
      for (const [lang, want, phraseAll] of [["en", { all: "All", next: "Next", field: "Search", fold: "unfold", foldLabel: "folded code" }, "All"], ["ja", { all: "すべて", next: "次へ", field: "検索", fold: "開く", foldLabel: "畳んだ部分" }, "すべて"]]) {
        i18n.lang = lang;
        await settle4();
        assert.ok(cm() === view0 && target.querySelector(".cm-editor") === dom0, `${lang}: 言語の切替でエディタを作り直した`);
        assert.equal(cm().state.phrase("all"), phraseAll, `${lang}: 言語の切替で CodeMirror の文言が替わらない`);
        assert.equal(cm().state.phrase("Go to line"), lang === "en" ? "Go to line" : "行へ移動", `${lang}: 行へ移動の窓の文言`);
        assert.deepEqual(labels(), want, `${lang}: 開いている検索の窓・畳んだ所の印の字が今の言語でない`);
        assert.deepEqual(snap(), before, `${lang}: 言語の切替で本文・選択・取消しの履歴・未保存の印・問い合わせ・焦点が変わった`);
        assert.ok(!target.querySelector(".overlay"), `${lang}: 言語の切替で確認の窓が出た`);
      }
      assert.equal(writes().length, writes0, "言語の切替で保存した");
      // 検索の窓の欄に焦点（打ちかけ）のまま切り替える = 焦点はその欄のまま・字も残る
      const field = () => target.querySelector('.cm-search input[name="search"]');
      field().focus();
      for (const lang of ["en", "ja"]) {
        i18n.lang = lang;
        await settle4();
        assert.ok(document.activeElement === field() && field().value === "一", `${lang}: 検索の窓の欄に焦点を置いたまま切り替えると焦点 / 字が残らない（${document.activeElement?.className} / ${field()?.value}）`);
      }
      // 置換の欄に焦点・欄の中の選択ありで切り替える = 焦点は置換の欄のまま（作り直された窓は検索の欄へ焦点を置く → 元の欄へ戻す）・選択も残る
      // 実ブラウザの input.select() は焦点も移す（CodeMirror の検索の窓・行へ移動の窓は作られた時に欄を select() する）。jsdom は移さない → 同じにする
      const origSelect = HTMLInputElement.prototype.select;
      HTMLInputElement.prototype.select = function () {
        this.focus();
        return origSelect.call(this);
      };
      const rfield = () => target.querySelector('.cm-search input[name="replace"]');
      rfield().focus();
      rfield().setSelectionRange(0, 1);
      for (const lang of ["en", "ja"]) {
        i18n.lang = lang;
        await settle4();
        assert.ok(document.activeElement === rfield() && rfield().value === "壱" && rfield().selectionStart === 0 && rfield().selectionEnd === 1,
          `${lang}: 置換の欄に焦点を置いたまま切り替えると焦点 / 字 / 選択が残らない（${document.activeElement?.name} / ${rfield()?.value} / ${rfield()?.selectionStart}-${rfield()?.selectionEnd}）`);
      }
      // 打ちかけ（欄の字が問い合わせに未確定）のまま切り替える。実ブラウザは、字を変えた欄の blur と、焦点のある欄が DOM から外れる時に change を出す
      // （= CodeMirror の commit → dispatch）。jsdom は出さない → 同じ事を起こす: 欄の blur で change・外れる要素の中に焦点のある欄があれば change。
      // 先に欄を外さないと、作り直しの途中（更新の中）で commit が dispatch する = 例外（Chrome で見た）
      field().focus();
      field().value = "一二";
      const dirtyChange = () => field()?.dispatchEvent(new Event("change", { bubbles: true }));
      field().addEventListener("blur", dirtyChange, { once: true });
      const origRemove = Element.prototype.remove;
      Element.prototype.remove = function () {
        const a = document.activeElement;
        if (a instanceof HTMLInputElement && this.contains(a)) a.dispatchEvent(new Event("change", { bubbles: true }));
        return origRemove.call(this);
      };
      try {
        i18n.lang = "en";
        await settle4();
      } finally {
        Element.prototype.remove = origRemove;
      }
      assert.ok(getSearchQuery(cm().state).search === "一二" && field().value === "一二" && document.activeElement === field(),
        `打ちかけの字が問い合わせに確定しない / 欄の焦点が残らない（${getSearchQuery(cm().state).search} / ${field()?.value} / ${document.activeElement?.name}）`);
      i18n.lang = "ja";
      await settle4();
      cm().dispatch({ effects: setSearchQuery.of(new SearchQuery({ search: "一", replace: "壱" })) });
      await settle4();
      // 行へ移動の窓（⌘⌥G）を開いたまま・打ちかけの字ありで切り替える = 窓は開いたまま・字と焦点が残る（作り直しで閉じる → 開き直す）。
      // 焦点がエディタにあれば、窓を開き直した後も焦点はエディタ
      const { gotoLine: openGoto } = await import("@codemirror/search");
      const goto = () => target.querySelector(".cm-goto-line input");
      openGoto(cm());
      await settle4();
      goto().value = "3";
      goto().focus();
      for (const lang of ["en", "ja"]) {
        i18n.lang = lang;
        await settle4();
        await sleep(20);
        assert.ok(goto() && goto().value === "3" && document.activeElement === goto(),
          `${lang}: 行へ移動の窓を開いたまま切り替えると窓 / 打った字 / 焦点が残らない（${!!goto()} / ${goto()?.value} / ${document.activeElement?.tagName}.${document.activeElement?.className}）`);
        assert.ok(target.querySelector(".cm-goto-line").textContent.includes(lang === "en" ? "Go to line" : "行へ移動"), `${lang}: 行へ移動の窓の文言が今の言語でない`);
      }
      cm().focus();
      i18n.lang = "en";
      await settle4();
      await sleep(20);
      assert.ok(goto()?.value === "3" && cm().hasFocus, `焦点がエディタのとき、行へ移動の窓を開き直した後に焦点がエディタへ戻らない（${document.activeElement?.tagName}.${document.activeElement?.className}）`);
      i18n.lang = "ja";
      await settle4();
      await sleep(20);
      target.querySelector(".cm-goto-line .cm-dialog-close")?.click();
      await settle4();
      // 検索の窓を開いたまま、本文に焦点で切り替える = 焦点は本文のまま。作り直された窓は欄へ焦点を置き、CodeMirror 自身の戻し（hasFocus）は
      // 文書に焦点が無いと働かない（WebKit で見た）→ document.hasFocus() が false の時と同じにして確かめる
      assert.ok(target.querySelector(".cm-search"), "前提: 検索の窓が開いている");
      cm().focus();
      const hasFocus0 = document.hasFocus;
      document.hasFocus = () => false;
      try {
        i18n.lang = "en";
        await settle4();
      } finally {
        document.hasFocus = hasFocus0;
      }
      assert.ok(cm().contentDOM.contains(document.activeElement), `検索の窓を開いたまま本文に焦点で切り替えると、焦点が本文から外れた（${document.activeElement?.tagName}.${document.activeElement?.name ?? document.activeElement?.className}）`);
      i18n.lang = "ja";
      await settle4();
      // スクロールの位置: jsdom にはレイアウトが無い（scrollTop は常に 0）→ scrollDOM の scrollTop / scrollLeft を値として持たせ、作り直し（setState）で
      // ブラウザが位置を失う事（WebKit = 先頭へ・Chrome = 先へ）を起こす → 切替の後の測り直しで前の値へ書き戻される事を見る
      const sd = cm().scrollDOM;
      const pos = { top: 1234, left: 56 };
      Object.defineProperty(sd, "scrollTop", { configurable: true, get: () => pos.top, set: (v) => (pos.top = v) });
      Object.defineProperty(sd, "scrollLeft", { configurable: true, get: () => pos.left, set: (v) => (pos.left = v) });
      const view1 = cm();
      const setState0 = view1.setState;
      view1.setState = function (st) {
        const r = setState0.call(this, st);
        pos.top = 0;
        pos.left = 0;
        return r;
      };
      try {
        for (const lang of ["en", "ja"]) {
          pos.top = 1234;
          pos.left = 56;
          i18n.lang = lang;
          await settle4();
          await sleep(50); // 測り直し（requestAnimationFrame）
          assert.deepEqual({ ...pos }, { top: 1234, left: 56 }, `${lang}: 言語の切替（view の作り直し）でスクロールの位置が戻らない`);
        }
      } finally {
        delete view1.setState;
        delete sd.scrollTop;
        delete sd.scrollLeft;
      }
      HTMLInputElement.prototype.select = origSelect;
      unfoldAll(cm());
      target.querySelector('.cm-search button[name="close"]')?.click();
      await settle4();
    }
    assert.deepEqual(enLeft, [], `英語の表示で CodeMirror の原文と違う phrase（訳を足した物）: ${enLeft.map(([n, k]) => `${n} → ${en[k]}`)}`);
  }
  // 編集中に開く（キーで = 見出しのボタンに焦点を置いて）= 今の見出し（足した四を含む）が出る・焦点はボタンのまま・押すと caret がその行へ
  head().focus();
  head().click();
  await settle4();
  assert.deepEqual(toc().map((b) => b.textContent), ["一", "三", "足した四"], "編集中に開いた目次が今の見出しでない");
  assert.ok(document.activeElement === head(), `開いたら焦点が見出しのボタンから外れた（${document.activeElement?.tagName}.${document.activeElement?.className}）`);
  cm().dispatch({ selection: { anchor: 0 } });
  toc()[2].click();
  assert.ok(await waitFor(() => cm().state.doc.lineAt(cm().state.selection.main.head).text === "#### 足した四"), "開き直した目次を押しても caret が動かない");
  assert.deepEqual(tocWrites().slice(w0), ["0", "1"], "開いた状態を ui_wiki_toc に書かない");
  // 空白のない長い語の見出しは行の中で折り返す（jsdom は配置を持たない = 規則の字面で見る）
  const infoSrc = readFileSync(new URL("./WikiInfo.svelte", import.meta.url), "utf8");
  assert.match(infoSrc, /\.wk-link, \.wk-toc \{[^}]*overflow-wrap: anywhere;[^}]*\}/, "目次とバックリンクの行に overflow-wrap: anywhere が無い（長い語で右ペインが横にスクロールする）");
  // 目次の見出しのボタンの上下のはみ出しは −2px（前の 1px の余白と同じ高さ = 下の節の位置が変わらない）。
  // jsdom は配置を持たない（scoped の CSS も計算しない）= 位置は測れない → 規則の字面で守る
  assert.match(infoSrc, /\.wk-sec-toggle \{ flex: 1; margin: -2px -6px; \}/, "目次の見出しのボタンの上下のはみ出しが −2px でない（下の節の位置がずれる）");
  target.querySelector(".wk-edit-btn").click();
  assert.ok(await waitFor(() => !target.querySelector(".cm-editor")), "表示へ戻らない");
  unmount(info);
  unmount(cmp);
  target.remove();
  E.cfg.delay = delay0;
  W.wk.page = null;
  W.wk.tocOpen = true;
  tab.wiki = null;
  tests++;
}

// ---------- ㊳ 窓の中に Tab の焦点を留める（実部品）: 骨格 Modal の窓（新規ページ・ゴミ箱）と、自前の枠の窓（救出・外部の書き換え・文献の引用）の
// どれも、最後で Tab → 最初・最初で Shift+Tab → 最後・窓の外の焦点から Tab → 窓の中（前は窓の外の裏のボタン・本文へ出られた）。関数そのものは focustrap.test ----------
{
  const { default: WikiDialogs } = await import("./WikiDialogs.svelte");
  const { default: WikiRescue } = await import("./WikiRescue.svelte");
  const { default: WikiConflict } = await import("./WikiConflict.svelte");
  const { default: WikiCite } = await import("./WikiCite.svelte");
  const C = await import("./cite.svelte.ts");
  const { tabbables } = await import("../focusTrap.ts");
  const outside = document.createElement("button");
  outside.textContent = "裏のボタン";
  document.body.append(outside);
  const target = document.createElement("div");
  document.body.append(target);
  const settleT = async () => {
    for (let i = 0; i < 4; i++) {
      await tick();
      flushSync();
    }
  };
  const tabKey = (shift = false) => {
    const e = new KeyboardEvent("keydown", { key: "Tab", shiftKey: shift, bubbles: true, cancelable: true });
    (document.activeElement ?? document.body).dispatchEvent(e);
    return e;
  };
  const check = async (label, frameSel) => {
    await settleT();
    const frame = document.querySelector(frameSel);
    assert.ok(frame, `${label}: 窓が出ない（${frameSel}）`);
    const list = tabbables(frame);
    assert.ok(list.length >= 2, `${label}: 押せる物が 2 つ以上ない（試験の前提）: ${list.length}`);
    list.at(-1).focus();
    tabKey();
    assert.ok(document.activeElement === list[0], `${label}: 最後で Tab → 最初へ回らない（${document.activeElement?.textContent}）`);
    list[0].focus();
    tabKey(true);
    assert.ok(document.activeElement === list.at(-1), `${label}: 最初で Shift+Tab → 最後へ回らない（${document.activeElement?.textContent}）`);
    outside.focus();
    tabKey();
    assert.ok(frame.contains(document.activeElement), `${label}: 窓の外の焦点から Tab で窓の中へ入らない（${document.activeElement?.textContent}）`);
  };
  okWrite("T1");
  const dlg = mount(WikiDialogs, { target });
  // 覆いでないポップオーバー（木の右クリック・[⋯]・[＋ 新規]・名前のメニュー = wk.menu）は囲わない = Tab は既定のまま（メニューが閉じて opener から進む）
  W.wk.menu = { kind: "file", path: "a.md", x: 1, y: 1, keyboard: true };
  await settleT();
  const mi = [...document.querySelectorAll('.wk-menu [role^="menuitem"]:not(:disabled)')];
  assert.ok(mi.length >= 2, `メニューの項目が出ない: ${mi.length}`);
  mi.at(-1).focus();
  const pe = tabKey();
  assert.ok(!pe.defaultPrevented && document.activeElement !== mi[0], "覆いでないメニューで Tab を囲った（最後 → 最初へ回した）");
  W.wk.menu = null;
  await settleT();
  W.wk.dialog = { kind: "new", dir: "" };
  await check("新規ページ（骨格 Modal）", ".overlay .modal");
  W.wk.dialog = { kind: "trash", path: "a.md", dir: false };
  await check("ゴミ箱（骨格 Modal）", ".overlay .modal");
  W.wk.dialog = null;
  await settleT();
  unmount(dlg);
  E.ed.stranded = [{ path: "a.md", content: "x", reason: "書けない" }];
  const rs = mount(WikiRescue, { target });
  await check("救出（自前の枠）", ".wk-rs");
  unmount(rs);
  E.ed.stranded = [];
  const cf = mount(WikiConflict, { target });
  await check("外部の書き換え（自前の枠）", ".wk-conflict");
  unmount(cf);
  C.cite.open = true;
  C.cite.path = E.ed.path;
  C.cite.root = E.ed.root;
  const ct = mount(WikiCite, { target });
  await settleT();
  // 文献の引用の窓 = 欄だけ（押せる物 1 つ）なら欄に留まる・結果の行が無くても外へ出ない
  const citeFrame = document.querySelector(".wk-cite");
  assert.ok(citeFrame, "文献の引用の窓が出ない");
  const cl = tabbables(citeFrame);
  // jsdom の Tab は焦点を動かさない = 「中に残った」だけでは囲えていない時も通る → 既定の Tab を止めたこと（defaultPrevented）を見る。
  // 窓の受け手は stopPropagation する = window の受け手に届かない（実キーの試験で窓の外へ出た）ので、窓の受け手から moveTab を呼ぶ
  cl.at(-1).focus();
  for (const sh of [false, true]) {
    const ce = tabKey(sh);
    assert.ok(ce.defaultPrevented && citeFrame.contains(document.activeElement), `文献の引用（自前の枠）: ${sh ? "Shift+" : ""}Tab を止めない = 窓の外へ出る（${document.activeElement?.tagName}）`);
  }
  outside.focus();
  tabKey(true);
  assert.ok(citeFrame.contains(document.activeElement), "文献の引用（自前の枠）: 窓の外の焦点から Shift+Tab で窓の中へ入らない");
  // 結果の行（role=listbox の中のボタン）= 窓の順の一部: 最初の行で Shift+Tab → 検索欄・最後の行で Tab → 窓の最初（欄）= 行に閉じ込めない
  C.cite.results = [{ key: "R1", title: "一", firstAuthor: "A", year: 2001, container: "J" }, { key: "R2", title: "二", firstAuthor: "B", year: 2002, container: "J" }];
  await settleT();
  const rows = [...citeFrame.querySelectorAll(".wk-cite-row")];
  const citeInput = citeFrame.querySelector("input");
  assert.equal(rows.length, 2, "引用の結果の行が出ない（試験の前提）");
  rows[0].focus();
  tabKey(true);
  assert.ok(document.activeElement === citeInput, `引用の結果の最初の行で Shift+Tab → 検索欄へ戻らない（行に閉じ込めた）: ${document.activeElement?.className}`);
  rows[1].focus();
  tabKey();
  assert.ok(document.activeElement === tabbables(citeFrame)[0], `引用の結果の最後の行で Tab → 窓の最初へ回らない: ${document.activeElement?.className}`);
  C.cite.results = [];
  await settleT();
  unmount(ct);
  C.cite.open = false;
  outside.remove();
  target.remove();
  tests++;
}

host.remove();
console.log(`wiki edit: ok (${tests} tests)`);
process.exit(0); // CodeMirror・toast のタイマーを待たない
