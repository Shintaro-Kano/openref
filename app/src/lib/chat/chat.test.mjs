/* OpenNote のチャットの試験（node app/src/lib/chat/chat.test.mjs・Svelte 5 の実部品 + jsdom）。
   invoke は差し替え・event "wiki-chat" は listen の差し替えで直接流す（契約 = types.ts の「チャットの契約」）。実物の行は chat-fixtures/。確かめること:
   ① reducer（chat.ts）: stream-json の逐次（text_delta）→ 確定の assistant で置き換え・1 ブロックずつ / 全ブロックの両方の形・
      道具の要約（tool_use + input_json_delta）と結果（tool_result）・result で確定・system / 未知 / 壊れた行は無視・下位の agent は出さない
   ② 送信: 未起動なら wiki_chat_start → wiki_chat_send {text, context}・IME の変換中の Enter は送らない・文脈のチップの × で外す
   ③ changes の表示（種類・[開く]）と [元に戻す]（wiki_chat_undo {turn, index}・ok:false の reason を出す）
   ④ stderr は診断に畳む・異常終了は知らせる（自動で起こし直さない）・停止 = wiki_chat_interrupt
   ⑤ 面はアプリ右端の柱 1 本（ChatDock）: チャットが無効ならタブ列のボタンを出さない・押すと開く・ui_chat_dock に覚える・
      幅はつまみを離したときに 1 回だけ確定する・開いた / 前面のタブが変わったときに状態を読み直す（柱の門と ⌘⌥C は App の配線 = app.test）
   ⑥ 一覧の右ペイン: 開閉は ui_detail・⌘⌥I は前面のモードの右ペインだけ（チャットとは別）・一覧が前面のときの柱の文脈 = 選んでいる文献 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { setupHarness } from "../wiki/test-harness.mjs";

// 土台（test-harness.mjs）: loader・jsdom の global 化・Tauri の偽物（invoke = handlers / event = emit で流す）
const { calls, handlers, emit, listeners, sleep, waitFor } = setupHarness();
const L = (o) => JSON.stringify(o);
let tests = 0;

// ---------- ① reducer ----------
const C = await import("./chat.ts");
{
  const fold = (lines, turn = 1, c = C.emptyConv()) => lines.reduce((acc, line) => C.reduceChat(acc, { t: "line", turn, line: typeof line === "string" ? line : L(line) }), c);
  const se = (event) => ({ type: "stream_event", event, session_id: "S" });

  // system/init・未知の type・JSON でない・壊れた JSON・配列 = 同じ Conv（描き直さない）
  const c0 = C.emptyConv();
  for (const l of [L({ type: "system", subtype: "init", tools: ["Read"] }), "not json", "{broken", "[1,2]", L({ type: "zzz" }), L({ type: "control_response", response: {} }), ""]) {
    assert.equal(C.reduceChat(c0, { t: "line", turn: 1, line: l }), c0, `無視しない: ${l}`);
  }
  tests++;

  // 逐次の本文 → 確定で置き換え（1 ブロックずつ届く形）・道具（input_json_delta）→ 結果 → 2 つ目の本文 → result
  let c = fold([
    { type: "system", subtype: "init" },
    se({ type: "message_start", message: { id: "m1" } }),
    se({ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } }),
    se({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "読み" } }),
    se({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "ます" } }),
  ]);
  assert.equal(c.entries.length, 1);
  assert.deepEqual([c.entries[0].k, c.entries[0].text, c.entries[0].final], ["text", "読みます", false], "逐次の本文");
  const firstId = c.entries[0].id;
  c = fold(
    [
      se({ type: "content_block_stop", index: 0 }),
      { type: "assistant", message: { id: "m1", content: [{ type: "text", text: "読みます。" }] } },
      se({ type: "content_block_start", index: 1, content_block: { type: "tool_use", id: "tu1", name: "mcp__openref__call", input: {} } }),
      se({ type: "content_block_delta", index: 1, delta: { type: "input_json_delta", partial_json: '{"name":"wiki_write","ar' } }),
      se({ type: "content_block_delta", index: 1, delta: { type: "input_json_delta", partial_json: 'gs":{"path":"notes/a.md","content":"x"}}' } }),
    ],
    1,
    c
  );
  assert.equal(c.entries[0].id, firstId, "確定で entry を作り直した（置き換えのはず）");
  assert.deepEqual([c.entries[0].text, c.entries[0].final], ["読みます。", true], "確定の本文で置き換え");
  assert.equal(c.entries[1].k, "tool");
  assert.equal(c.entries[1].arg, "", "引数は content_block_stop まで読まない");
  c = fold(
    [
      se({ type: "content_block_stop", index: 1 }),
      { type: "assistant", message: { id: "m1", content: [{ type: "tool_use", id: "tu1", name: "mcp__openref__call", input: { name: "wiki_write", args: { path: "notes/a.md", content: "x" } } }] } },
    ],
    1,
    c
  );
  assert.equal(c.entries.length, 2, "確定の tool_use で道具を二重に出した");
  assert.deepEqual([c.entries[1].name, c.entries[1].arg, c.entries[1].done], ["mcp__openref__call", "wiki_write notes/a.md", false]);
  assert.equal(C.toolName(c.entries[1].name), "call");
  c = fold(
    [
      { type: "user", message: { role: "user", content: [{ type: "tool_result", tool_use_id: "tu1", content: [{ type: "text", text: '{"ok":true}' }] }] } },
      se({ type: "message_start", message: { id: "m2" } }),
      se({ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } }),
      se({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "書きました" } }),
      { type: "assistant", message: { id: "m2", content: [{ type: "text", text: "書きました。" }] } },
      { type: "result", subtype: "success", is_error: false, duration_ms: 1234, total_cost_usd: 0.01, result: "書きました。" },
    ],
    1,
    c
  );
  assert.deepEqual([c.entries[1].result, c.entries[1].done, c.entries[1].error], ['{"ok":true}', true, false], "道具の結果");
  assert.deepEqual(
    c.entries.map((e) => e.k),
    ["text", "tool", "text", "result"]
  );
  assert.deepEqual([c.entries[2].text, c.entries[2].final], ["書きました。", true]);
  assert.deepEqual([c.entries[3].ok, c.entries[3].ms, c.entries[3].text], [true, 1234, null], "成功の result は本文を持たない（cost も持たない）");
  tests++;

  // 全ブロックが届く形（同じ message.id の assistant が累積）= 二重に足さない
  c = fold([
    se({ type: "message_start", message: { id: "k" } }),
    se({ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } }),
    se({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "A" } }),
    { type: "assistant", message: { id: "k", content: [{ type: "text", text: "A!" }] } },
    se({ type: "content_block_start", index: 1, content_block: { type: "text", text: "" } }),
    se({ type: "content_block_delta", index: 1, delta: { type: "text_delta", text: "B" } }),
    { type: "assistant", message: { id: "k", content: [{ type: "text", text: "A!" }, { type: "text", text: "B!" }] } },
    { type: "assistant", message: { id: "k", content: [{ type: "text", text: "A!" }, { type: "text", text: "B!" }] } },
  ]);
  assert.deepEqual(c.entries.map((e) => [e.text, e.final]), [["A!", true], ["B!", true]], "累積の形で二重に足した");
  // 逐次なし（--include-partial-messages が効かない）= 確定だけで足す・1 ブロックずつの 2 つ目は前を上書きしない
  c = fold([
    { type: "assistant", message: { id: "n", content: [{ type: "text", text: "一" }] } },
    { type: "assistant", message: { id: "n", content: [{ type: "tool_use", id: "t", name: "Read", input: { file_path: "/v/a.md" } }] } },
    { type: "assistant", message: { id: "n", content: [{ type: "text", text: "二" }] } },
  ]);
  assert.deepEqual(c.entries.map((e) => (e.k === "text" ? e.text : `${e.name}:${e.arg}`)), ["一", "Read:/v/a.md", "二"], "逐次なしの 1 ブロックずつ");
  tests++;

  // 結果の無いまま result（停止）= 逐次の本文・道具を確定に・エラーの result は本文を持つ・下位の agent の行は出さない
  c = fold([
    se({ type: "message_start", message: { id: "x" } }),
    se({ type: "content_block_start", index: 0, content_block: { type: "text", text: "途中" } }),
    se({ type: "content_block_start", index: 1, content_block: { type: "tool_use", id: "g", name: "Grep", input: { pattern: "PI-RADS", path: "notes" } } }),
    { type: "assistant", parent_tool_use_id: "zz", message: { id: "sub", content: [{ type: "text", text: "下位" }] } },
    { type: "result", subtype: "error_during_execution", is_error: true, result: "interrupted" },
  ]);
  assert.deepEqual(c.entries.map((e) => e.k), ["text", "tool", "result"], "下位の agent を出した");
  assert.equal(c.entries[0].final, true);
  assert.equal(c.entries[1].done, true);
  assert.equal(c.entries[1].arg, "PI-RADS (notes)");
  assert.deepEqual([c.entries[2].ok, c.entries[2].subtype, c.entries[2].text], [false, "error_during_execution", "interrupted"]);
  // 未知の delta・形の合わない値は無視
  const before = c;
  for (const ev of [{ type: "content_block_delta", index: 9, delta: { type: "text_delta", text: "z" } }, { type: "content_block_delta", index: "0" }, { type: "content_block_start" }, { type: "ping" }]) {
    assert.equal(C.reduceChat(before, { t: "line", turn: 1, line: L(se(ev)) }), before, `無視しない: ${L(ev)}`);
  }
  // tool_result の文字列・画像・長さの上限
  assert.equal(C.resultText("abc"), "abc");
  assert.equal(C.resultText([{ type: "image", source: {} }, { type: "text", text: "t" }]), "[image]\nt");
  assert.equal(C.resultText("x".repeat(C.RESULT_MAX + 10)).length, C.RESULT_MAX + 1);
  tests++;

  // user / turn / note / end
  let u = C.reduceChat(C.emptyConv(), { t: "user", text: "要約して", ctx: { path: "a.md" } });
  const uid = u.entries[0].id;
  u = C.reduceChat(u, { t: "turn", id: uid, turn: 3 });
  assert.deepEqual([u.entries[0].turn, u.entries[0].ctx], [3, { path: "a.md" }]);
  assert.deepEqual(C.reduceChat(C.emptyConv(), { t: "user", text: "x", ctx: {} }).entries[0].ctx, undefined, "空の文脈を持った");
  u = fold([se({ type: "message_start", message: { id: "q" } }), se({ type: "content_block_start", index: 0, content_block: { type: "text", text: "a" } })], 3, u);
  u = C.reduceChat(u, { t: "end" });
  assert.equal(u.entries[1].final, true, "end で確定しない");
  assert.equal(C.reduceChat(u, { t: "end" }), u, "変化の無い end で作り直した");
  const groups = C.groupByTurn([...u.entries, ...C.reduceChat(u, { t: "user", text: "次" }).entries.slice(-1)]);
  assert.deepEqual(groups.map((g) => [g.turn, g.entries.length]), [[3, 2], [null, 1]], "ターンの束");
  tests++;

  // 道具の要約・変更の表示・[[リンク]] の引き当て
  assert.equal(C.summarize("mcp__openref__call", { name: "wiki_move", args: { from: "ingest/a.pdf", to: "sources/2026/a.pdf" } }), "wiki_move ingest/a.pdf → sources/2026/a.pdf");
  assert.equal(C.summarize("mcp__openref__sql", { sql: "SELECT  key\n FROM items" }), "SELECT key FROM items");
  assert.equal(C.summarize("Glob", { pattern: "**/*.md" }), "**/*.md");
  assert.ok(C.summarize("Read", { file_path: "x".repeat(300) }).length <= 140);
  assert.deepEqual(C.changeView({ kind: "write", path: "a.md", sha256_after: "s", backup: "b" }), { kind: "write", label: "a.md", open: { page: "a.md" } });
  assert.deepEqual(C.changeView({ kind: "move", from: "a.md", to: "b/a.md" }).open, { page: "b/a.md" });
  assert.deepEqual(C.changeView({ kind: "item", key: "K1", title: "T" }), { kind: "item", label: "T (K1)", open: { item: "K1" } });
  const links = C.chatLinks("[[a]] [[b|別名]] ![[dir/c]] [[dup]] [[none]] [[a]]", [{ path: "x/a.md" }, { path: "y/b.md" }, { path: "dir/c.md" }, { path: "p/dup.md" }, { path: "q/dup.md" }]);
  assert.deepEqual(
    links.map((l) => [l.kind, l.target, l.path, l.ambiguous]),
    [
      ["wiki", "a", "x/a.md", false],
      ["wiki", "b", "y/b.md", false],
      ["embed", "dir/c", "dir/c.md", false],
      ["wiki", "dup", null, true],
      ["wiki", "none", null, false],
    ],
    "[[リンク]] の引き当て（決まらなければ未解決）"
  );
  // 切り出しは描画と同じ走査（md.ts の wikiLinksOf）: code の中は拾わない・末尾の `\` は全部落とす・`]` を含む名前・内側の `[[`
  assert.deepEqual(
    C.chatLinks("`[[no]]` [[a\\\\]] [[x]y]] [[out [[in]]\n```\n[[no2]]\n```", []).map((l) => [l.kind, l.target]),
    [["wiki", "a"], ["wiki", "x]y"], ["wiki", "in"]],
    "chatLinks の切り出しが描画と違う"
  );
  // 選択範囲の切り詰め = code point 単位（サロゲートペアの途中で切らない）・切った印
  const emoji = "😀".repeat(C.SELECTION_MAX + 3);
  const cl = C.clipSelection(emoji);
  assert.deepEqual([cl.length, cl.cut, cl.text.length, C.codePoints(cl.text)], [C.SELECTION_MAX, true, C.SELECTION_MAX * 2, C.SELECTION_MAX], "code point で切らない");
  assert.ok(!/[\uD800-\uDBFF]$/.test(cl.text), "サロゲートペアの途中で切った");
  assert.deepEqual(C.clipSelection("x".repeat(C.SELECTION_MAX)), { text: "x".repeat(C.SELECTION_MAX), length: C.SELECTION_MAX, cut: false }, "ちょうど上限で切った");
  assert.equal(C.clipSelection("x".repeat(C.SELECTION_MAX + 1)).text.length, C.SELECTION_MAX);
  tests++;
}

// 実物の行（Claude Code 2.1.197・2026-11-01 の stdout・chat-fixtures/ = 個人の値・id・パスは置換済み）を畳む
{
  const fx = (name) => readFileSync(new URL(`./chat-fixtures/${name}.jsonl`, import.meta.url), "utf8").split("\n").filter(Boolean);
  const fold = (lines, c = C.emptyConv()) => lines.reduce((acc, line) => C.reduceChat(acc, { t: "line", turn: 1, line }), c);
  const show = (c) =>
    c.entries.map((e) =>
      e.k === "text" ? `text:${e.final}:${e.text}` : e.k === "tool" ? `${C.toolName(e.name)}:${e.arg}:${e.done ? "done" : "open"}${e.error ? ":error" : ""}` : e.k === "result" ? `result:${e.ok}:${e.subtype}` : e.k
    );
  const tidy = (c, name) => assert.deepEqual([Object.keys(c.blocks).length, Object.keys(c.json).length, Object.keys(c.fin).length, c.msg], [0, 0, 0, null], `${name}: 突き合わせの表が残った`);
  const cases = {
    plain: ["text:true:PINEAPPLE", "result:true:success"],
    read: ["Read:/Users/user/vault/chat-test/sample.md:done", "text:true:# Sample note", "result:true:success"],
    create: ["call:wiki_write chat-test/chat-new.md:done", "text:true:done", "result:true:success"],
    move: ["call:wiki_move:done:error", "call:wiki_move chat-test/chat-new.md → chat-test/chat-moved.md:done", "text:true:moved", "result:true:success"],
    twice: ["call:wiki_write chat-test/twice.md:done", "call:wiki_write chat-test/twice.md:done", "text:true:ok", "result:true:success"],
    interrupt: ["result:false:error_during_execution"],
  };
  for (const [name, want] of Object.entries(cases)) {
    const c = fold(fx(name));
    assert.deepEqual(show(c), want, name);
    tidy(c, name);
  }
  // (a) 要約して追記 = wiki_read → wiki_write（if_sha256）→ 本文
  let c = fold(fx("append_sha"));
  assert.deepEqual(show(c).slice(0, 2), ["call:wiki_read chat-test/sample.md:done", "call:wiki_write chat-test/sample.md:done"]);
  assert.ok(c.entries[2].text.startsWith("「## 要約」節を末尾に追記しました"), "本文");
  assert.ok(JSON.parse(c.entries[1].result).backup.includes("/wiki-history/"), "上書きの結果に backup");
  // 道具のエラー（例 = 拒否された Read の記録・MCP のエラー）は道具のエラーとして出す（文字列の content）
  c = fold(fx("outside2"));
  assert.deepEqual(show(c)[0], "Read:/Users/user/outside/secret.txt:done:error");
  assert.ok(c.entries[0].result.startsWith("Permission to use Read has been denied"));
  c = fold(fx("mcp_error"));
  assert.deepEqual(show(c)[0], "call:wiki_read chat-test/does-not-exist.md:done:error");
  assert.ok(c.entries[0].result.startsWith("not found:"));
  // 中断の result の errors（Claude Code の診断）は手がかりとして持つ（止めたターンは画面が「停止しました」にする）
  assert.ok(fold(fx("interrupt")).entries[0].text.startsWith("[ede_diagnostic]"), "errors を落とした");
  // 存在しない会話の resume の result（Rust は起動の確認より前の行として流さない・流れても落ちない）
  assert.deepEqual(show(fold(fx("bad_resume"))), ["result:false:error_during_execution"]);
  // 逐次: 確定の assistant より前は final でない本文が 1 つ（text_delta まで）・確定で同じ entry を置き換える
  const lines = fx("plain");
  const iDelta = lines.findIndex((l) => l.includes('"text_delta"'));
  const mid = fold(lines.slice(0, iDelta + 1));
  assert.deepEqual(show(mid), ["text:false:PINEAPPLE"], "逐次の本文");
  assert.equal(fold(lines.slice(iDelta + 1), mid).entries[0].id, mid.entries[0].id, "確定で作り直した");
  // thinking のブロック（index 0）の後の本文・道具も index で突き合う = 二重に出さない（read は thinking → tool_use の順）
  assert.ok(fx("read").some((l) => l.includes('"type":"thinking"')), "フィクスチャに thinking がある");
  // init（system）・rate_limit_event・control_response・system/status・thinking_tokens は描き直さない
  const c0 = C.emptyConv();
  for (const l of [...fx("init"), ...fx("plain").filter((l) => /"type":"(system|rate_limit_event)"/.test(l))]) assert.equal(C.reduceChat(c0, { t: "line", turn: 1, line: l }), c0, l.slice(0, 80));
  tests++;
}

// reducer の突き合わせ = message の中のブロックの index（同じ本文が続いても落とさない・置き換える）・順の入れ替わり・上限
{
  const fold = (lines, turn = 1, c = C.emptyConv()) => lines.reduce((acc, line) => C.reduceChat(acc, { t: "line", turn, line: L(line) }), c);
  const se = (event) => ({ type: "stream_event", event });
  const show = (c) => c.entries.map((e) => (e.k === "text" ? `${e.text}${e.final ? "" : "~"}` : e.k === "tool" ? `@${e.name}` : e.k));
  // 逐次なし・1 ブロックずつ・同じ本文が 2 回
  let c = fold([
    { type: "assistant", message: { id: "n", content: [{ type: "text", text: "OK" }] } },
    { type: "assistant", message: { id: "n", content: [{ type: "tool_use", id: "t", name: "Read", input: { file_path: "a" } }] } },
    { type: "assistant", message: { id: "n", content: [{ type: "text", text: "OK" }] } },
  ]);
  assert.deepEqual(show(c), ["OK", "@Read", "OK"], "同じ本文の 2 つ目を落とした");
  // 逐次あり・同じ本文の 2 つ目のブロック（逐次の途中 "O"）を確定で置き換える
  c = fold([
    se({ type: "message_start", message: { id: "m" } }),
    se({ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } }),
    se({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "OK" } }),
    { type: "assistant", message: { id: "m", content: [{ type: "text", text: "OK" }] } },
    se({ type: "content_block_start", index: 1, content_block: { type: "text", text: "" } }),
    se({ type: "content_block_delta", index: 1, delta: { type: "text_delta", text: "O" } }),
    { type: "assistant", message: { id: "m", content: [{ type: "text", text: "OK" }] } },
  ]);
  assert.deepEqual(show(c), ["OK", "OK"], "逐次の 2 つ目を確定で置き換えない");
  // 確定が逐次より先に届いた = 二重にしない
  c = fold([
    se({ type: "message_start", message: { id: "m" } }),
    { type: "assistant", message: { id: "m", content: [{ type: "text", text: "Hello" }] } },
    se({ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } }),
    se({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "Hello" } }),
    { type: "result", subtype: "success" },
  ]);
  assert.deepEqual(show(c), ["Hello", "result"], "確定が先に届いたら二重に出した");
  // 累積の形（2 ブロック以上）は index で置き換え・同じものの再送は増やさない
  c = fold([
    { type: "assistant", message: { id: "k", content: [{ type: "text", text: "A" }] } },
    { type: "assistant", message: { id: "k", content: [{ type: "text", text: "A" }, { type: "text", text: "A" }] } },
    { type: "assistant", message: { id: "k", content: [{ type: "text", text: "A" }, { type: "text", text: "A" }] } },
  ]);
  assert.deepEqual(show(c), ["A", "A"], "累積の形");
  // 逐次にだけ出るブロック（thinking = index 0）で index がずれても、種類の違う entry を本文として扱わない
  c = fold([
    se({ type: "message_start", message: { id: "m" } }),
    se({ type: "content_block_start", index: 0, content_block: { type: "thinking", thinking: "" } }),
    se({ type: "content_block_start", index: 1, content_block: { type: "text", text: "" } }),
    se({ type: "content_block_delta", index: 1, delta: { type: "text_delta", text: "streamA" } }),
    { type: "assistant", message: { id: "m", content: [{ type: "text", text: "FINAL-A" }] } },
    se({ type: "content_block_start", index: 2, content_block: { type: "tool_use", id: "tu", name: "Read", input: {} } }),
    { type: "assistant", message: { id: "m", content: [{ type: "tool_use", id: "tu", name: "Read", input: { file_path: "x" } }] } },
    se({ type: "content_block_start", index: 3, content_block: { type: "text", text: "" } }),
    se({ type: "content_block_delta", index: 3, delta: { type: "text_delta", text: "streamB" } }),
    { type: "assistant", message: { id: "m", content: [{ type: "text", text: "FINAL-B" }] } },
  ]);
  assert.deepEqual(show(c), ["FINAL-A", "@Read", "FINAL-B"], "index のずれで本文を確定しない");
  // content_block_stop が来なかった道具の json・突き合わせ表は result で捨てる
  c = fold([
    se({ type: "message_start", message: { id: "j" } }),
    se({ type: "content_block_start", index: 0, content_block: { type: "tool_use", id: "tj", name: "Read", input: {} } }),
    se({ type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: '{"file_pa' } }),
  ]);
  assert.equal(Object.keys(c.json).length, 1);
  c = fold([{ type: "result", subtype: "success" }], 1, c);
  assert.deepEqual([Object.keys(c.json).length, Object.keys(c.blocks).length, Object.keys(c.fin).length], [0, 0, 0], "ターンの後に突き合わせ表を残した");
  // 上限（ターン単位）: 発言は直近 MAX_TURNS・entry が MAX_ENTRIES を超えたらさらに古いターン・直近 KEEP_TURNS は必ず残す。刈るのは prune のときだけ
  const turnOf = (c, i, n = 1) => {
    c = C.reduceChat(c, { t: "user", text: `q${i}` });
    for (let j = 0; j < n; j++) c = fold([{ type: "assistant", message: { id: `a${i}_${j}`, content: [{ type: "text", text: `r${i}` }] } }], i, c);
    return c;
  };
  c = C.emptyConv();
  for (let i = 1; i <= C.MAX_TURNS + 5; i++) c = turnOf(c, i);
  assert.equal(c.entries.filter((e) => e.k === "user").length, C.MAX_TURNS + 5, "prune の前に刈った（user の追加で刈った）");
  c = C.reduceChat(c, { t: "prune" });
  assert.equal(c.entries.filter((e) => e.k === "user").length, C.MAX_TURNS, "古い発言を刈らない");
  assert.equal(c.entries[0].text, "q6", "ターンの境で切らない");
  assert.equal(c.dropped, 5, "刈った数が発言の数でない");
  assert.equal(C.reduceChat(c, { t: "prune" }), c, "刈るものが無いのに作り直した");
  // entry の数: 古いターンから刈る・ターンの途中では切らない
  c = C.emptyConv();
  for (let i = 1; i <= 20; i++) c = turnOf(c, i, 60); // 1 ターン 61 entry × 20 = 1220
  c = C.reduceChat(c, { t: "prune" });
  assert.ok(c.entries.length <= C.MAX_ENTRIES, `entry の上限: ${c.entries.length}`);
  assert.equal(c.entries[0].k, "user", "ターンの途中で切った");
  assert.equal(c.entries.length % 61, 0, "ターンの途中で切った");
  // 直近 KEEP_TURNS は entry が多くても残す
  c = C.emptyConv();
  for (let i = 1; i <= 15; i++) c = turnOf(c, i, 200);
  c = C.reduceChat(c, { t: "prune" });
  assert.equal(c.entries.filter((e) => e.k === "user").length, C.KEEP_TURNS, "直近 KEEP_TURNS を刈った");
  assert.equal(c.dropped, 15 - C.KEEP_TURNS);
  // drop = 送れなかった発言を外す
  c = C.reduceChat(C.emptyConv(), { t: "user", text: "x" });
  assert.equal(C.reduceChat(c, { t: "drop", id: c.entries[0].id }).entries.length, 0);
  assert.equal(C.reduceChat(c, { t: "drop", id: "none" }), c);
  tests++;
}

// ---------- ②〜⑤ 実部品 ----------
const { mount, unmount, flushSync } = await import("svelte");
const S = await import("../state.svelte.ts");
const W = await import("../wiki/wiki.svelte.ts");
const CH = await import("./chat.svelte.ts");
const V = await import("../viewing.svelte.ts"); // 見ている物と選択範囲（chat.svelte.ts から分けた）
const SE = await import("../wiki/selection.ts"); // 選択の読み取り（chat.svelte.ts から分けた）
const { default: WikiChat } = await import("./ChatPanel.svelte");
const { default: ChatDock } = await import("./ChatDock.svelte");
const { default: TabStrip } = await import("../TabStrip.svelte");

// Rust の擬似: 子プロセスの状態と会話の id を持ち、wiki_chat_status はそれを返す。start は返る前に state を出す（契約）
const mock = { enabled: true, state: "off", session: null, over: {} };
const status = (over = {}) => ({ enabled: true, cli: "/usr/local/bin/claude", version: "2.1.197", logged_in: true, mcp: "/x/openref-mcp", state: "off", session_id: null, ...over });
handlers.wiki_chat_status = () => status({ enabled: mock.enabled, state: mock.state, session_id: mock.session, ...mock.over });
const stateEv = (state, session = mock.session) => {
  mock.state = state;
  mock.session = session;
  emit("wiki-chat", { type: "state", state, session_id: session });
};
const startOk = (a) => {
  const sid = a.new ? `N${++startOk.n}` : (mock.session ?? "S1");
  stateEv("idle", sid);
  return { session_id: sid, resumed: false };
};
startOk.n = 0;
const prefs = { ui_chat_dock: JSON.stringify({ open: true, w: 420 }) }; // 起動時の読みで柱の開閉と幅が戻る（下の起動の節）
handlers.get_ui_pref = (a) => prefs[a.key] ?? null;
handlers.set_ui_pref = (a) => {
  prefs[a.key] = a.value;
  return null;
};
handlers.wiki_backlinks = () => ({ total: 0, pages: [] });
W.wk.status = { enabled: true, root: "/v" };
W.wk.page = { path: "notes/a.md", abs_path: "/v/notes/a.md", kind: "md", title: "A", size: 1, mtime: 0, content: "本文", sha256: "s", truncated: false };
// 前面のタブ = Wiki のページ（文脈 = ページ。一覧が前面なら文脈は選んでいる文献 = 下の柱の節で確認）
{
  const t0 = S.app.tabs.find((t) => t.id === S.app.tab);
  if (t0) t0.wiki = { path: "notes/a.md", scrollTop: 0, editing: false };
}
// 本番の起動順: initChat（onMount）→ wiki_settings が返って app.wikiOn = true
S.app.wikiOn = false;
calls.length = 0;
const stopViewing = V.initViewing(); // App の onMount と同じ順（initViewing → initChat）
const stop = CH.initChat();
await sleep(10);
assert.equal(calls.filter((c) => c.name === "wiki_chat_status").length, 0, "Wiki OFF で状態を読んだ");
S.app.wikiOn = true;

// 起動時: 柱（ChatDock）の mount を待たずに状態を読む（タブ列のボタンが起動直後から出る）・1 回だけ・柱の開閉と幅（ui_chat_dock）が戻る
assert.ok(await waitFor(() => CH.chat.status != null), "起動時（柱の前）に状態を読まない");
assert.ok(CH.chatShown(), "有効なのに柱を出す判定が false");
assert.equal(calls.filter((c) => c.name === "wiki_chat_status").length, 1, "起動時の読みが 1 回でない");
assert.ok(await waitFor(() => CH.chat.dock.open === true), "柱の開閉（ui_chat_dock）が戻らない");
assert.equal(CH.chat.dock.w, 420, "柱の幅（ui_chat_dock）が戻らない");
CH.chat.dock = { open: false, w: C.DOCK_W };
delete prefs.ui_chat_dock;
tests++;

// ⑤ 柱の入口（タブ列の右端のボタン）: 無効 = ボタンなし・有効 = ボタン・押すと開いて覚える（ui_chat_dock）・開いた柱の入力欄へ focus・
// もう一度で閉じる・Settings で OFF（enabled event）= ボタンが消える・state event では status を読まない・応答中は点。
// 柱そのものの門（chatShown() && chat.dock.open で mount）と ⌘⌥C は App の配線 = app.test。ここでは App と同じ条件で ChatDock を mount する
{
  mock.enabled = false;
  const target = document.createElement("div");
  document.body.append(target);
  const strip = mount(TabStrip, { target });
  emit("wiki-state", null); // 有効 / 無効の変化（Rust の wiki-state）= 読み直す
  await waitFor(() => CH.chat.status?.enabled === false);
  flushSync();
  assert.equal(target.querySelector(".tabs-chat"), null, "チャットが無効なのに柱のボタンを出した");
  assert.equal(CH.chatShown(), false, "チャットが無効なのに柱を出す判定が true");
  mock.enabled = true;
  emit("wiki-state", null);
  assert.ok(await waitFor(() => (flushSync(), !!target.querySelector(".tabs-chat"))), "有効になってもボタンが出ない");
  const btn = () => target.querySelector(".tabs-chat");
  assert.equal(btn().getAttribute("aria-pressed"), "false", "閉じているのに押された絵");
  btn().click(); // 開く（App は chatShown() && chat.dock.open で ChatDock を mount する = 下で同じ条件で mount）
  flushSync();
  assert.deepEqual([CH.chat.dock.open, JSON.parse(prefs.ui_chat_dock)], [true, { open: true, w: C.DOCK_W }], "ボタンで開かない / 開閉を覚えない");
  const dockAt = document.createElement("div");
  document.body.append(dockAt);
  let dock = mount(ChatDock, { target: dockAt });
  flushSync();
  await sleep(10);
  assert.equal(document.activeElement, dockAt.querySelector(".wk-ch textarea"), "柱を開いても入力欄へ focus しない");
  assert.equal(btn().getAttribute("aria-pressed"), "true", "開いているのに押された絵にならない");
  btn().click(); // 閉じる
  flushSync();
  unmount(dock);
  assert.deepEqual([CH.chat.dock.open, JSON.parse(prefs.ui_chat_dock).open], [false, false], "もう一度押しても閉じない / 閉じたことを覚えない");
  // 子の状態の event（state: busy / idle）では status を読み直さない（claude --version / auth status を毎ターン起こさない）・応答中はボタンに点
  calls.length = 0;
  stateEv("busy");
  flushSync();
  assert.ok(target.querySelector(".tabs-chat .tabs-chat-dot"), "応答中の点がボタンに出ない");
  stateEv("idle");
  flushSync();
  assert.equal(target.querySelector(".tabs-chat-dot"), null, "応答が終わっても点が残った");
  await sleep(20);
  assert.ok(!calls.some((c) => c.name === "wiki_chat_status"), "state event で status を読み直した");
  // Settings で OFF = Rust は wiki-chat {type: enabled, enabled: false} を出す → ボタンを消して読み直す
  mock.enabled = false;
  emit("wiki-chat", { type: "enabled", enabled: false });
  flushSync();
  assert.equal(target.querySelector(".tabs-chat"), null, "enabled:false でボタンが消えない");
  assert.ok(await waitFor(() => calls.some((c) => c.name === "wiki_chat_status")), "enabled event で読み直さない");
  mock.enabled = true;
  emit("wiki-chat", { type: "enabled", enabled: true });
  assert.ok(await waitFor(() => (flushSync(), !!target.querySelector(".tabs-chat"))), "enabled:true でボタンが戻らない");
  // 空きのダブルクリック = 新しいタブ（TabStrip の dblclickNew）はボタンの上では効かない
  const nTabs = S.app.tabs.length;
  btn().dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  await sleep(10);
  assert.equal(S.app.tabs.length, nTabs, "ボタンのダブルクリックで新しいタブを開いた");
  CH.chat.dock = { open: false, w: C.DOCK_W };
  unmount(strip);
  target.remove();
  dockAt.remove();
  tests++;
}

const target = document.createElement("div");
document.body.append(target);
const cmp = mount(WikiChat, { target });
flushSync();
const $ = (s) => target.querySelector(s);
const $$ = (s) => [...target.querySelectorAll(s)];
const ta = () => $(".wk-ch-compose textarea");
const typeDraft = (s) => {
  ta().value = s;
  ta().dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
};
const enter = (o = {}) => ta().dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true, ...o }));

// start の後に状態を読み直す（Rust の state が来なくても starting で固まらない）・失敗したら off に戻す
{
  handlers.wiki_chat_start = () => {
    mock.state = "idle"; // state event を出さない Rust
    mock.session = "Q1";
    return { session_id: "Q1", resumed: false };
  };
  await CH.newConversation();
  assert.equal(CH.chat.status.state, "idle", "start の後に starting のまま");
  assert.equal(CH.busy(), false);
  handlers.wiki_chat_start = () => {
    throw new Error("claude が起動しません");
  };
  stateEv("off");
  await waitFor(() => CH.chat.status.state === "off");
  await CH.newConversation();
  assert.equal(CH.chat.status.state, "off", "start に失敗しても starting のまま");
  assert.equal(CH.chat.error, "Error: claude が起動しません");
  flushSync();
  assert.ok($(".wk-ch-compose .wk-ch-err")?.textContent.includes("claude が起動しません"), "起動の失敗を出さない");
  tests++;
}

// ② 送信: IME の変換中は送らない → 未起動なら start → send（文脈 = ページ・選択範囲）・× で外す・失敗したら戻す・送信中 / 応答中は送らない
{
  mock.session = null;
  mock.state = "off";
  await CH.loadChatStatus();
  calls.length = 0;
  handlers.wiki_chat_start = startOk;
  let turn = 0;
  handlers.wiki_chat_send = () => ({ turn: ++turn });
  V.viewing.sel = { key: "notes/a.md", text: "選んだ文" };
  flushSync();
  assert.deepEqual($$(".wk-ch-ctx .wk-ch-chip-text").map((e) => e.textContent), ["notes/a.md", "選択範囲 4 字"], "文脈のチップ");
  typeDraft("このページを要約して");
  enter({ isComposing: true });
  enter({ keyCode: 229 });
  enter({ shiftKey: true });
  await sleep(10);
  assert.equal(calls.filter((c) => c.name.startsWith("wiki_chat_s") && c.name !== "wiki_chat_status").length, 0, "IME の変換中・Shift+Enter で送った");
  enter();
  assert.ok(await waitFor(() => calls.some((c) => c.name === "wiki_chat_send")), "送らない");
  assert.deepEqual(
    calls.filter((c) => c.name !== "wiki_chat_status").map((c) => c.name),
    ["wiki_chat_start", "wiki_chat_send"],
    "未起動なら起こしてから送る"
  );
  // 子を起こす・送る root = 前面の root（画面の root）
  assert.deepEqual(calls.find((c) => c.name === "wiki_chat_start").args, { new: false, root: "/v" }, "保存済みの会話を続ける");
  assert.deepEqual(calls.find((c) => c.name === "wiki_chat_send").args, { text: "このページを要約して", context: { path: "notes/a.md", selection: "選んだ文" }, root: "/v" });
  await waitFor(() => CH.chat.conv.entries[0]?.turn === 1);
  flushSync();
  assert.equal(ta().value, "", "送った後に入力欄を空にしない");
  assert.equal($(".wk-ch-user-text").textContent, "このページを要約して");
  assert.equal(V.viewing.sel, null, "選択範囲を次の発言にも付ける");
  // × でページを外す → 文脈なしで送る
  stateEv("idle");
  flushSync();
  $(".wk-ch-x").click();
  flushSync();
  assert.equal($(".wk-ch-ctx"), null, "× で外れない");
  calls.length = 0;
  typeDraft("次");
  enter();
  assert.ok(await waitFor(() => calls.some((c) => c.name === "wiki_chat_send")));
  assert.deepEqual(calls.filter((c) => c.name !== "wiki_chat_status").map((c) => c.name), ["wiki_chat_send"], "起動済みなのに起こし直した");
  assert.deepEqual(calls.find((c) => c.name === "wiki_chat_send").args, { text: "次", root: "/v" }, "外した文脈を添えた");
  assert.ok(await waitFor(() => !CH.chat.sending), "送信が終わらない"); // invoke の後の後始末（wiki/ipc.ts の包みの分だけ遅れる）を待つ
  CH.pageChanged("other.md");
  assert.equal(CH.chat.ctxOff, null, "ページが変わっても外したまま");
  // 送れなかった = 入力欄の文章と選択のチップを戻す・発言は会話に残さない・失敗の表示は 1 か所
  handlers.wiki_chat_send = () => {
    throw new Error("boom");
  };
  const n = CH.chat.conv.entries.length;
  V.viewing.sel = { key: "notes/a.md", text: "残す選択" };
  typeDraft("長い依頼文");
  assert.equal(await CH.send(), false);
  flushSync();
  assert.equal(CH.chat.draft, "長い依頼文", "送れなかった文章を捨てた");
  assert.deepEqual(V.viewing.sel, { key: "notes/a.md", text: "残す選択" }, "送れなかった選択を捨てた");
  assert.equal(CH.chat.conv.entries.length, n, "送れなかった発言を会話に残した");
  assert.equal($$(".wk-ch-err").filter((e) => e.textContent.includes("boom")).length, 1, "失敗を 2 か所に出した");
  // 送信中・応答中は送らない
  handlers.wiki_chat_send = () => ({ turn: ++turn });
  CH.chat.sending = true;
  calls.length = 0;
  assert.equal(await CH.send(), false, "送信中に送った");
  CH.chat.sending = false;
  stateEv("busy");
  assert.equal(await CH.send(), false, "応答中に送った");
  assert.ok(!calls.some((c) => c.name === "wiki_chat_send"));
  stateEv("idle");
  CH.chat.draft = "";
  V.viewing.sel = null;
  CH.chat.error = null;
  tests++;
}

// 応答の逐次 → 確定の md は MdView（shadow DOM）で描く・道具は畳む・停止
{
  stateEv("busy");
  const line = (o) => emit("wiki-chat", { type: "line", session_id: mock.session, turn: 2, line: L(o) });
  line({ type: "system", subtype: "init" });
  line({ type: "stream_event", event: { type: "message_start", message: { id: "r1" } } });
  line({ type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } } });
  line({ type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "**太字**" } } });
  flushSync();
  assert.equal($(".wk-ch-stream")?.textContent, "**太字**", "逐次の本文を素の文字で出さない");
  await waitFor(() => !!$(".wk-ch-stop"));
  flushSync();
  assert.ok($(".wk-ch-stop"), "応答中に停止ボタンが無い");
  line({ type: "assistant", message: { id: "r1", content: [{ type: "text", text: "**太字** <img src=x onerror=alert(1)> [[a]]" }] } });
  line({ type: "assistant", message: { id: "r1", content: [{ type: "tool_use", id: "T1", name: "Read", input: { file_path: "/v/notes/a.md" } }] } });
  flushSync();
  assert.equal($(".wk-ch-stream"), null);
  const host = (await waitFor(() => $(".wk-ch-md .wk-md")?.shadowRoot?.querySelector("strong"))) && $(".wk-ch-md .wk-md");
  assert.ok(host, "確定の本文を MdView で描かない");
  assert.equal(host.shadowRoot.querySelector("img[onerror]"), null, "sanitize を通っていない");
  assert.equal($(".wk-ch-tool summary .wk-ch-tool-name").textContent, "Read");
  assert.equal($(".wk-ch-tool").open, false, "道具の結果を畳まない");
  // 木（wk.pages）が変わっても [[…]] の解決が同じなら描き直さない
  const box = host.shadowRoot.firstElementChild;
  W.wk.pages = [{ path: "x/zzz.md", kind: "md", title: "z", tags: [], items: [], mtime: 0, size: 0 }];
  flushSync();
  await sleep(10);
  assert.equal(host.shadowRoot.firstElementChild, box, "解決が同じなのに描き直した");
  W.wk.pages = [{ path: "x/a.md", kind: "md", title: "a", tags: [], items: [], mtime: 0, size: 0 }];
  flushSync();
  await sleep(10);
  assert.notEqual(host.shadowRoot.firstElementChild, box, "解決が変わったのに描き直さない");
  // 停止 = interrupt（会話は続く）・停止したターンの result は「停止しました」
  calls.length = 0;
  handlers.wiki_chat_interrupt = () => null;
  $(".wk-ch-stop").click();
  assert.ok(await waitFor(() => calls.some((c) => c.name === "wiki_chat_interrupt")), "停止で interrupt を呼ばない");
  line({ type: "result", subtype: "error_during_execution", is_error: true });
  stateEv("idle");
  flushSync();
  assert.ok($$(".wk-ch-note").some((e) => e.textContent === "停止しました"), "停止したターンを知らせない");
  assert.equal($$(".wk-ch-note.wk-ch-err").length, 0, "停止をエラーとして出した");
  // turn が付く前に停止を押しても停止として扱う
  let release;
  handlers.wiki_chat_send = () => new Promise((r) => (release = r));
  CH.chat.draft = "遅い送信";
  const p = CH.send();
  await waitFor(() => CH.chat.conv.entries.at(-1)?.k === "user" && CH.chat.conv.entries.at(-1).turn == null);
  await CH.interrupt();
  release({ turn: 9 });
  assert.equal(await p, true);
  assert.ok(CH.chat.stopped.includes(9), "turn の前の停止を忘れた");
  tests++;
}

// ③ changes と [元に戻す]（戻したものは [開く] を止める）
{
  emit("wiki-chat", {
    type: "changes",
    session_id: mock.session,
    turn: 2,
    changes: [
      { kind: "write", path: "notes/a.md", sha256_after: "s2", backup: "/h/1" },
      { kind: "create", path: "notes/new.md", sha256_after: "s3" },
      { kind: "move", from: "ingest/x.pdf", to: "sources/2026/x.pdf" },
      { kind: "item", key: "ABC", title: "Paper" },
    ],
  });
  emit("wiki-chat", { type: "changes", session_id: "OLD", turn: 2, changes: [] }); // 前の会話のものは無視
  flushSync();
  const rows = $$(".wk-ch-change");
  assert.equal(rows.length, 4, "変更の一覧");
  assert.deepEqual(rows.map((r) => r.querySelector(".wk-ch-kind").textContent), ["上書き", "新規", "移動", "蔵書に登録"]);
  assert.deepEqual(rows.map((r) => r.querySelector(".wk-ch-change-label").textContent), ["notes/a.md", "notes/new.md", "ingest/x.pdf → sources/2026/x.pdf", "Paper (ABC)"]);
  calls.length = 0;
  handlers.wiki_chat_undo = (a) =>
    a.index === 0 ? { ok: false, turn: a.turn, index: a.index, reason: "その後に編集されています" } : { ok: true, turn: a.turn, index: a.index };
  const undoBtn = (i) => $$(".wk-ch-change")[i].querySelectorAll("button")[1];
  const openBtn = (i) => $$(".wk-ch-change")[i].querySelectorAll("button")[0];
  for (const i of [0, 1, 2]) undoBtn(i).click();
  await waitFor(() => CH.chat.undo["2:0"]?.ok === false && CH.chat.undo["2:1"]?.ok && CH.chat.undo["2:2"]?.ok);
  flushSync();
  assert.deepEqual(calls.map((c) => [c.name, c.args]), [0, 1, 2].map((i) => ["wiki_chat_undo", { turn: 2, index: i }]));
  assert.equal($$(".wk-ch-change")[0].querySelector(".wk-ch-undo-why").textContent, "戻せません: その後に編集されています", "ok:false の reason を出さない");
  assert.equal(undoBtn(1).textContent.trim(), "戻しました");
  assert.equal(undoBtn(1).disabled, true, "戻した後も押せる");
  assert.deepEqual([0, 1, 2, 3].map((i) => openBtn(i).disabled), [false, true, true, false], "戻した変更（新規・移動）を開ける");
  calls.length = 0;
  await CH.undoChange(2, 1);
  assert.equal(calls.length, 0, "戻した後にもう一度戻した");
  handlers.wiki_chat_undo = () => {
    throw new Error("IPC 失敗");
  };
  undoBtn(3).click();
  await waitFor(() => CH.chat.undo["2:3"]?.ok === false);
  flushSync();
  assert.ok($$(".wk-ch-change")[3].querySelector(".wk-ch-undo-why").textContent.includes("IPC 失敗"));
  tests++;
}

// ④ stderr（行数・長さの上限）・異常終了を知らせる（起こし直さない）・新しい会話・前の子プロセスがあったときだけ終了を抑える
{
  emit("wiki-chat", { type: "stderr", text: "warn: a\n\nerror: b" });
  flushSync();
  assert.ok($(".wk-ch-diag"), "stderr を出さない");
  assert.equal($(".wk-ch-diag").open, false, "診断を畳まない");
  assert.ok($(".wk-ch-diag summary").textContent.includes("(2)"));
  emit("wiki-chat", { type: "stderr", text: Array.from({ length: CH.STDERR_MAX + 50 }, (_, i) => `l${i}`).join("\n") });
  assert.equal(CH.chat.stderr.length, CH.STDERR_MAX, "stderr の行数の上限");
  assert.equal(CH.chat.stderr.at(-1), `l${CH.STDERR_MAX + 49}`, "新しい行を捨てた");
  emit("wiki-chat", { type: "stderr", text: "x".repeat(CH.STDERR_LINE_MAX + 100) });
  assert.equal(CH.chat.stderr.at(-1).length, CH.STDERR_LINE_MAX + 1, "stderr の 1 行の長さの上限");
  calls.length = 0;
  mock.state = "off";
  emit("wiki-chat", { type: "exit", code: 1, signal: null });
  flushSync();
  assert.ok($$(".wk-ch-note.wk-ch-err").some((e) => e.textContent.includes("Claude Code が終了しました (code 1)")), "異常終了を知らせない");
  assert.equal(CH.chat.status.state, "off");
  await sleep(20);
  assert.ok(!calls.some((c) => c.name === "wiki_chat_start"), "自動で起こし直した");
  // 前の子プロセスが無い（off）ときの新しい会話 = その後の異常終了は知らせる
  handlers.wiki_chat_start = startOk;
  const newBtn = () => $$(".wk-ch-head button").find((b) => b.textContent.trim() === "新しい会話");
  newBtn().click();
  flushSync();
  assert.ok($(".wk-ch-ask"), "会話があるのに確認しない");
  newBtn().click();
  assert.ok(await waitFor(() => CH.chat.session?.startsWith("N") && !CH.chat.sending));
  flushSync();
  assert.deepEqual(calls.find((c) => c.name === "wiki_chat_start").args, { new: true, root: "/v" });
  assert.equal(CH.chat.conv.entries.length, 0, "新しい会話で表示を空にしない");
  assert.deepEqual(CH.chat.changes, {});
  mock.state = "off";
  emit("wiki-chat", { type: "exit", code: 2, signal: null });
  flushSync();
  assert.equal($$(".wk-ch-note.wk-ch-err").length, 1, "前の子プロセスが無いのに異常終了を抑えた");
  // 動いている子プロセスを止める新しい会話 = その終了は知らせない・SIGTERM も知らせない
  stateEv("idle");
  await waitFor(() => CH.chat.status.state === "idle");
  await CH.newConversation();
  emit("wiki-chat", { type: "exit", code: 143, signal: null });
  emit("wiki-chat", { type: "exit", code: null, signal: "SIGTERM" });
  flushSync();
  assert.equal($$(".wk-ch-note.wk-ch-err").length, 0, "新しい会話で止めた前の子プロセスの終了をエラーにした: " + $$(".wk-ch-note.wk-ch-err").map((e) => e.textContent).join("/"));
  // 別の session_id の行 = 表示を新しくする
  emit("wiki-chat", { type: "line", session_id: "S3", turn: 1, line: L({ type: "assistant", message: { id: "z", content: [{ type: "text", text: "hi" }] } }) });
  assert.equal(CH.chat.session, "S3");
  assert.equal(CH.chat.conv.entries.length, 1);
  mock.session = "S3";
  tests++;
}

// ワークスペース: 前面の root（画面の root）が会話の root（子の root = wiki_chat_status.root）と違う間は会話を隠して 1 行
// （消さない・状態の読み直しでも消さない）・戻れば見える・隠している間に送る = wiki_chat_stop → wiki_chat_start {root: 前面} → wiki_chat_send {…, root: 前面}
// = 新しい会話。会話が無くなった（状態の session_id が null・off・隠していない）= 表示を消す
{
  mock.state = "idle";
  mock.session = "S3";
  mock.over = { root: "/v" }; // 子の root
  await CH.loadChatStatus();
  emit("wiki-chat", { type: "changes", session_id: "S3", turn: 1, changes: [{ kind: "create", path: "z.md", sha256_after: "x" }] });
  CH.chat.undo["1:0"] = { ok: true };
  CH.chat.stopped = [1];
  assert.equal(CH.chat.conv.entries.length, 1);
  assert.equal(CH.chat.root, "/v", "会話の root（子の root）を覚えない");
  assert.equal(CH.chatHidden(), false);
  // 前面が別のワークスペースのタブ（画面の root = /other）= 隠す・消さない
  W.wk.status = { enabled: true, root: "/other" };
  flushSync();
  assert.equal(CH.chatHidden(), true, "別の root なのに会話を隠さない");
  assert.deepEqual([CH.chat.conv.entries.length, Object.keys(CH.chat.changes), Object.keys(CH.chat.undo), [...CH.chat.stopped]], [1, ["1"], ["1:0"], [1]], "別の root へ移っただけで会話を消した");
  const other = $(".wk-ch-list .wk-ch-other");
  assert.ok(other, "隠した 1 行が無い");
  assert.equal(other.textContent, "この会話は v のワークスペースのものです。ここで送ると新しい会話になります");
  assert.equal($(".wk-ch-list").children.length, 1, "隠しているのに会話を出した");
  // 状態の読み直し（wiki-state）でも消さない（子は生きている = タブの行き来で止めない）
  emit("wiki-state", null);
  await sleep(10);
  assert.equal(CH.chat.conv.entries.length, 1, "隠している間の状態の読み直しで会話を消した");
  // 子がいなくても（状態の session_id は前面の root の物）隠している会話は消さない
  mock.over = {};
  mock.state = "off";
  mock.session = "OTHER-SAVED";
  emit("wiki-state", null);
  await sleep(10);
  assert.equal(CH.chat.conv.entries.length, 1, "子がいない間の状態の読み直しで隠している会話を消した");
  assert.equal(CH.chat.session, "S3");
  // 戻る = 会話がそのまま見える
  W.wk.status = { enabled: true, root: "/v" };
  flushSync();
  assert.equal(CH.chatHidden(), false);
  assert.equal($(".wk-ch-other"), null, "戻っても 1 行が残った");
  assert.ok($(".wk-ch-list").children.length >= 1 && CH.chat.conv.entries.length === 1, "戻っても会話が見えない");
  // 隠している間に送る = 前の子を止めて前面の root で起こし直し、その root で送る（新しい会話）
  mock.state = "idle";
  mock.session = "S3";
  mock.over = { root: "/v" };
  await CH.loadChatStatus();
  W.wk.status = { enabled: true, root: "/other" };
  flushSync();
  handlers.wiki_chat_stop = () => {
    mock.state = "off";
    return {};
  };
  handlers.wiki_chat_start = (a) => {
    mock.over = { root: a.root };
    stateEv("idle", "O1");
    return { session_id: "O1", resumed: false };
  };
  handlers.wiki_chat_send = () => ({ turn: 1 });
  calls.length = 0;
  typeDraft("別の root で");
  enter();
  assert.ok(await waitFor(() => calls.some((c) => c.name === "wiki_chat_send")), "隠している間に送れない");
  const seq = calls.filter((c) => c.name !== "wiki_chat_status").map((c) => c.name);
  assert.deepEqual(seq, ["wiki_chat_stop", "wiki_chat_start", "wiki_chat_send"], `止めて起こし直さない: ${seq}`);
  assert.deepEqual(calls.find((c) => c.name === "wiki_chat_start").args, { new: false, root: "/other" }, "前面の root で起こさない");
  assert.equal(calls.find((c) => c.name === "wiki_chat_send").args.root, "/other", "前面の root で送らない");
  assert.equal(CH.chat.root, "/other");
  assert.equal(CH.chat.session, "O1");
  assert.equal(CH.chatHidden(), false);
  assert.deepEqual(CH.chat.conv.entries.map((e) => [e.k, e.text]), [["user", "別の root で"]], "前の root の会話を新しい会話に混ぜた");
  assert.deepEqual([CH.chat.changes, CH.chat.undo, CH.chat.stopped], [{}, {}, []], "前の root の changes・undo・stopped が残った");
  flushSync();
  assert.equal($(".wk-ch-other"), null);
  // 会話が無くなった（session_id が null・off）= 表示を消す（隠していないとき）
  mock.session = null;
  mock.state = "off";
  mock.over = {};
  emit("wiki-state", null);
  assert.ok(await waitFor(() => CH.chat.conv.entries.length === 0 && !Object.keys(CH.chat.changes).length), "会話が無くなっても前の表示を残した");
  W.wk.status = { enabled: true, root: "/v" };
  flushSync();
  tests++;
}

// 選択範囲: 中央のページ（.wk-page）の中だけ・外（入力欄）の選択では前の選択を残す・畳んだら消す・切ったらチップに印
{
  const pageEl = document.createElement("section");
  pageEl.className = "wk-page";
  pageEl.innerHTML = "<p>中央の本文です</p>";
  const out = document.createElement("div");
  out.textContent = "外の文字";
  document.body.append(pageEl, out);
  const sel = document.getSelection();
  const pick = (node, a, b) => {
    const r = document.createRange();
    r.setStart(node, a);
    r.setEnd(node, b);
    sel.removeAllRanges();
    sel.addRange(r);
    document.dispatchEvent(new Event("selectionchange"));
  };
  V.viewing.sel = null;
  pick(pageEl.querySelector("p").firstChild, 0, 2);
  await sleep(0);
  assert.deepEqual(V.viewing.sel, { key: "notes/a.md", text: "中央" }, "ページの選択を拾わない");
  pick(out.firstChild, 0, 2);
  await sleep(0);
  assert.equal(V.viewing.sel?.text, "中央", "ページの外の選択で前の選択を捨てた・外の選択を拾った");
  sel.collapse(pageEl.querySelector("p").firstChild, 1);
  document.dispatchEvent(new Event("selectionchange"));
  await sleep(0);
  assert.equal(V.viewing.sel, null, "ページの中で畳んでも残した");
  assert.equal(SE.inPage(pageEl.querySelector("p").firstChild), true, "テキストノードを判定できない");
  assert.equal(SE.inPage(out), false);
  // shadow DOM の中 = host の祖先で判定
  const host = document.createElement("div");
  pageEl.append(host);
  const sr = host.attachShadow({ mode: "open" });
  sr.innerHTML = "<p>影</p>";
  assert.equal(SE.inPage(sr.querySelector("p").firstChild), true, "shadow DOM の中を判定できない");
  const host2 = document.createElement("div");
  out.append(host2);
  assert.equal(SE.inPage(host2.attachShadow({ mode: "open" })), false);
  // 20000 字を超える選択 = 切って送る・チップは切った後の長さと印
  V.viewing.sel = { key: "notes/a.md", text: "あ".repeat(20005) };
  flushSync();
  assert.equal($(".wk-ch-ctx .wk-ch-chip:last-child .wk-ch-chip-text").textContent, "選択範囲 20000 字 (先頭だけ)", "切ったことをチップに出さない");
  assert.equal(CH.currentContext().selection.length, 20000, "選択を切らずに送る");
  // 別の元の選択（effect を待たずに置いた = タブを替えた直後と同じ）はチップにも文脈にも出さない
  V.viewing.sel = { key: "item:K9", text: "別の元の選択" };
  flushSync();
  assert.deepEqual($$(".wk-ch-ctx .wk-ch-chip-text").map((e) => e.textContent), ["notes/a.md"], "別の元の選択をチップに出した");
  assert.deepEqual(CH.currentContext(), { path: "notes/a.md" }, "別の元の選択を文脈に添えた");
  // 選択のチップの × = 選択だけ外す（ページは残る）
  V.viewing.sel = { key: "notes/a.md", text: "外す選択" };
  flushSync();
  $(".wk-ch-ctx .wk-ch-chip:last-child .wk-ch-x").click();
  flushSync();
  assert.equal(V.viewing.sel, null, "選択のチップの × で外れない");
  assert.deepEqual(CH.currentContext(), { path: "notes/a.md" }, "選択の × でページまで外した");
  V.viewing.sel = null;
  pageEl.remove();
  out.remove();
  tests++;
}

// PDF のタブが前面 = 文脈は文献（item / title）・選択は PdfViewer が viewing の setViewingSelection へ書く・× で外す・OpenNote のタブへ戻ると元に戻る
{
  const wikiTab = S.app.tab;
  const base = S.app.tabs.find((t) => t.id === wikiTab);
  S.app.tabs.push({ ...base, id: 9001, wiki: null, viewer: { itemKey: "K1", attachmentId: 1, title: "題名 T" } });
  S.app.tab = 9001;
  flushSync();
  assert.deepEqual(V.viewingSource(), { kind: "item", key: "K1", title: "題名 T" });
  assert.deepEqual(CH.currentContext(), { item: "K1", title: "題名 T" }, "PDF のタブでページを添えた");
  assert.equal($(".wk-ch-ctx .wk-ch-chip-item .wk-ch-chip-text")?.textContent, "題名 T", "文献のチップが題名でない");
  V.setViewingSelection("item:K1", "PDF の選択");
  flushSync();
  assert.deepEqual(CH.currentContext(), { item: "K1", title: "題名 T", selection: "PDF の選択" });
  assert.equal($(".wk-ch-ctx .wk-ch-chip:last-child .wk-ch-chip-text").textContent, "選択範囲 7 字");
  V.setViewingSelection("item:K9", null);
  assert.ok(V.viewing.sel, "別の文献の選択解除で消した");
  V.setViewingSelection("item:K1", null);
  assert.equal(V.viewing.sel, null, "選択解除で消えない");
  // 送ると item / title が context に（path は無い）
  stateEv("idle");
  flushSync();
  handlers.wiki_chat_send = () => ({ turn: 77 });
  calls.length = 0;
  typeDraft("この文献は");
  enter();
  assert.ok(await waitFor(() => calls.some((c) => c.name === "wiki_chat_send")));
  assert.deepEqual(calls.find((c) => c.name === "wiki_chat_send").args, { text: "この文献は", context: { item: "K1", title: "題名 T" }, root: "/v" }, "文献の文脈を添えない");
  assert.ok(await waitFor(() => !CH.chat.sending), "送信が終わらない");
  assert.equal(CH.chat.error, null, "送信が失敗した");
  flushSync();
  assert.equal($$(".wk-ch-user .wk-ch-chip-item").at(-1)?.textContent, "題名 T", "発言に文献のチップが残らない");
  // × で外す → 文脈なし。Wiki のタブへ戻る = 文脈はページ・外した印は戻る
  stateEv("idle");
  flushSync();
  $(".wk-ch-x").click();
  flushSync();
  assert.equal(CH.chat.ctxOff, "item:K1");
  assert.equal(CH.currentContext(), undefined, "外したのに添えた");
  S.app.tab = wikiTab;
  flushSync();
  assert.equal(CH.chat.ctxOff, null, "タブが変わっても外したまま");
  assert.deepEqual(CH.currentContext(), { path: "notes/a.md" });
  // Wiki のタブの選択は PDF のタブへ移ると捨てる
  V.viewing.sel = { key: "notes/a.md", text: "ページの選択" };
  S.app.tab = 9001;
  flushSync();
  assert.equal(V.viewing.sel, null, "別の元の選択を残した");
  S.app.tab = wikiTab;
  S.app.tabs.pop();
  flushSync();
  CH.chat.draft = "";
  tests++;
}

// 本棚のタブ・本のタブが前面 = 見ている物なし（一覧で選んでいた文献を添えない = 取り違えない）・チップも送信の context の item も無い
{
  const wikiTab = S.app.tab;
  const base = S.app.tabs.find((t) => t.id === wikiTab);
  // 一覧で文献を選んでいた（本棚へ移る前の状態）。一覧のタブなら文脈になる物
  S.app.selected = "K5";
  S.app.detail = { key: "K5", csl: { title: "題名 5" }, attachments: [], collections: [], tags: [], notes: [], dateAdded: "2026-01-01" };
  stateEv("idle");
  let sent = null;
  handlers.wiki_chat_send = (a) => {
    sent = a;
    return { turn: 88 };
  };
  for (const [label, over] of [
    ["本棚", { shelf: { root: "/books", dir: null, query: "", sort: "name", scrollTop: 0 } }],
    ["本", { shelfBook: { root: "/books", id: "b1", path: "/books/b1.pdf", title: "本 1", size: 1, mtime: 0 } }],
  ]) {
    S.app.tabs.push({ ...base, id: 9101, wiki: null, viewer: null, shelf: null, shelfBook: null, ...over });
    S.app.tab = 9101;
    flushSync();
    assert.equal(V.viewingSource(), null, `${label}のタブで見ている物があった（一覧の選択を返した）`);
    assert.equal($(".wk-ch-ctx"), null, `${label}のタブで文脈のチップが出た`);
    assert.equal(CH.currentContext(), undefined, `${label}のタブで文脈を組んだ`);
    sent = null;
    CH.chat.draft = `${label}で`;
    assert.equal(await CH.send(), true, `${label}のタブから送れない`);
    assert.equal(sent.context, undefined, `${label}のタブの送信に文脈（item）が入った`);
    stateEv("idle");
    S.app.tab = wikiTab;
    S.app.tabs.pop();
    flushSync();
  }
  // 戻した一覧のタブでは今までどおり選んでいる文献（本棚の分岐が一覧を壊していない）
  S.app.tabs.push({ ...base, id: 9102, wiki: null, viewer: null, shelf: null, shelfBook: null });
  S.app.tab = 9102;
  flushSync();
  assert.deepEqual(V.viewingSource(), { kind: "item", key: "K5", title: "題名 5" }, "一覧のタブで選んでいる文献が見ている物にならない");
  S.app.tab = wikiTab;
  S.app.tabs.pop();
  S.app.selected = null;
  S.app.detail = null;
  flushSync();
  tests++;
}

// 読み手の登録（wanted）は関数 = チャットが有効になった直後の最初の選択も拾う（状態の写しの遅れが無い）・読み手がいなければ拾わない
{
  const pageEl = document.createElement("section");
  pageEl.className = "wk-page";
  pageEl.innerHTML = "<p>有効化の直後</p>";
  document.body.append(pageEl);
  const sel = document.getSelection();
  const pick = () => {
    const r = document.createRange();
    r.setStart(pageEl.querySelector("p").firstChild, 0);
    r.setEnd(pageEl.querySelector("p").firstChild, 3);
    sel.removeAllRanges();
    sel.addRange(r);
    document.dispatchEvent(new Event("selectionchange"));
  };
  const saved = CH.chat.status;
  V.viewing.sel = null;
  CH.chat.status = { ...saved, enabled: false };
  pick();
  await sleep(0);
  assert.equal(V.viewing.sel, null, "チャットが無効なのにページの選択を拾った");
  V.setViewingSelection("item:K1", "無効の間");
  assert.equal(V.viewing.sel, null, "チャットが無効なのに PDF の選択を書いた");
  // 有効にした直後（flushSync も effect も挟まない）の最初の選択
  CH.chat.status = { ...saved, enabled: true };
  pick();
  await sleep(0);
  assert.deepEqual(V.viewing.sel, { key: "notes/a.md", text: "有効化" }, "有効にした直後の最初の選択を取りこぼした");
  V.viewing.sel = null;
  V.setViewingSelection("item:K1", "直後");
  assert.deepEqual(V.viewing.sel, { key: "item:K1", text: "直後" }, "有効にした直後の PDF の選択を取りこぼした");
  // 読み手を外す = 拾わない（登録が無ければ viewing は何も集めない）・登録し直すと拾う
  V.viewing.sel = null;
  V.setViewingWanted(null);
  pick();
  await sleep(0);
  V.setViewingSelection("item:K1", "外した後");
  assert.equal(V.viewing.sel, null, "読み手がいないのに拾った");
  V.setViewingWanted(CH.chatShown);
  pick();
  await sleep(0);
  assert.equal(V.viewing.sel?.text, "有効化", "登録し直しても拾わない");
  V.viewing.sel = null;
  sel.removeAllRanges();
  CH.chat.status = saved;
  pageEl.remove();
  tests++;
}

// viewing.svelte.ts はチャットもホストの部品も import しない・module の評価時に wk / app に触らない（chat → viewing → wiki.svelte → chat の環）
{
  const src = readFileSync(new URL("../viewing.svelte.ts", import.meta.url), "utf8");
  const imports = [...src.matchAll(/^import [^;]* from "([^"]+)";/gm)].map((m) => m[1]);
  assert.deepEqual(imports.sort(), ["./state.svelte", "./wiki/selection", "./wiki/wiki.svelte", "svelte"], `viewing の import が想定外: ${imports}`);
  // top-level（字下げなし）の文で wk / app に触れる行が無い（宣言・コメント・関数の見出しだけ）
  const top = src.split("\n").filter((l) => /^[^\s/}]/.test(l) && !l.startsWith("import ") && !l.startsWith("export function ") && !l.startsWith("function ") && !l.startsWith("export type "));
  assert.deepEqual(top.filter((l) => /\b(wk|app)\b/.test(l)), [], "viewing が module の評価時に wk / app に触れる");
  tests++;
}

// ホストの書き手（PdfViewer が使う followItemSelection・偽ホスト = chat-fixtures/host.svelte.ts）: 前面で選択があるときだけ `item:<key>` へ書く・
// 子の state event（busy / idle = chat.status の置き換え）で書き戻さない = 送信 / × で消した選択を蘇らせない・
// 前面でないホストは書かない・読み手が無効の間は書かず、有効になったら選び直す（従来の chatTab = 真偽値の derived と同じ依存）
{
  const { host, startHost, startRawWriter } = await import("./chat-fixtures/host.svelte.ts");
  const wikiTab = S.app.tab;
  const base = S.app.tabs.find((t) => t.id === wikiTab);
  S.app.tabs.push({ ...base, id: 9201, wiki: null, viewer: { itemKey: "K1", attachmentId: 1, title: "題名 T" } });
  S.app.tab = 9201;
  flushSync();
  V.viewing.sel = null;
  host.front = true;
  host.text = null;
  const stopHost = startHost("K1");
  flushSync();
  host.text = "PDF で選んだ一節";
  flushSync();
  assert.deepEqual(V.viewing.sel, { key: "item:K1", text: "PDF で選んだ一節" }, "ホストの選択を item:<key> で書かない");
  assert.equal(CH.currentContext()?.selection, "PDF で選んだ一節", "ホストの選択が文脈に乗らない");
  // 送る = 添えて消す → 子の state event（busy / idle）が来ても書き戻さない → 次の発言には付かない
  stateEv("idle");
  const sent = [];
  handlers.wiki_chat_send = (a) => {
    sent.push(a.context);
    return { turn: 300 + sent.length };
  };
  CH.chat.draft = "この一節を説明して";
  assert.equal(await CH.send(), true, "送れない");
  stateEv("busy");
  stateEv("idle");
  flushSync();
  assert.equal(V.viewing.sel, null, "送った選択が state event で戻った");
  CH.chat.draft = "別の質問";
  assert.equal(await CH.send(), true, "送れない");
  assert.deepEqual(sent, [{ item: "K1", title: "題名 T", selection: "PDF で選んだ一節" }, { item: "K1", title: "題名 T" }], "送った選択が次の発言にも付いた");
  // × で外す → state event でも戻らない
  stateEv("idle");
  host.text = "二つ目の一節";
  flushSync();
  assert.equal(V.viewing.sel?.text, "二つ目の一節", "ホストの新しい選択を書かない");
  $(".wk-ch-ctx .wk-ch-chip:last-child .wk-ch-x").click();
  flushSync();
  stateEv("busy");
  stateEv("idle");
  flushSync();
  assert.equal(V.viewing.sel, null, "× で外した選択が state event で戻った");
  // 前面でないホストは書かない
  host.front = false;
  host.text = "裏の選択";
  flushSync();
  assert.equal(V.viewing.sel, null, "前面でないホストが書いた");
  // 読み手が無効の間は書かない・有効になったら選び直す
  const saved = CH.chat.status;
  host.front = true;
  CH.chat.status = { ...saved, enabled: false };
  host.text = "無効の間の選択";
  flushSync();
  assert.equal(V.viewing.sel, null, "読み手が無効なのにホストの選択を書いた");
  CH.chat.status = { ...saved, enabled: true };
  flushSync();
  assert.deepEqual(V.viewing.sel, { key: "item:K1", text: "無効の間の選択" }, "読み手が有効になってもホストの選択を選び直さない");
  stopHost();
  // setViewingSelection を effect から直に呼んでも、その effect に chat.status を追わせない（state event のたびに走らない）
  const raw = startRawWriter("item:K1", "直に書く");
  flushSync();
  const runs0 = raw.runs();
  CH.chat.status = { ...CH.chat.status, state: "busy" };
  flushSync();
  CH.chat.status = { ...CH.chat.status, state: "idle" };
  flushSync();
  assert.equal(raw.runs(), runs0, "setViewingSelection が呼び手の effect に chat.status を追わせた");
  raw.stop();
  V.viewing.sel = null;
  CH.chat.draft = "";
  S.app.tab = wikiTab;
  S.app.tabs.pop();
  flushSync();
  tests++;
}

// 使えない = 案内（claude が無い / 未ログイン / openref-mcp が古い）・入力欄は使えない
{
  for (const [over, want] of [
    [{ cli: null, version: null }, "Claude Code（CLI）が見つかりません。"],
    [{ logged_in: false }, "Claude Code にログインしていません。"],
    [{ mcp: null, reason: "openref-mcp に wiki_write がありません" }, "openref-mcp が見つからないか、古い版です（wiki_write がありません）。"],
  ]) {
    CH.chat.status = status(over);
    flushSync();
    assert.equal($(".wk-ch-guide p")?.textContent, want, `案内: ${JSON.stringify(over)}`);
    assert.equal(ta().disabled, true, "使えないのに入力できる");
    if (over.reason) assert.equal($(".wk-ch-reason").textContent, over.reason, "reason をそのまま出さない");
  }
  CH.chat.draft = "x";
  assert.equal(await CH.send(), false, "使えないのに送った");
  CH.chat.draft = "";
  tests++;
}

// Major A: 状態の session_id が null でも、子がいる・送信中なら表示を消さない（消すのは off・送信中でない・null のときだけ）
// 状態の読みが入れ替わって返っても、古い応答で上書きしない / 刈ったターンの changes・undo・stopped を捨てる
{
  const mockStatus = handlers.wiki_chat_status;
  const { emptyConv, MAX_TURNS } = await import("./chat.ts");
  mock.enabled = true;
  mock.session = "S9";
  mock.state = "idle";
  CH.chat.status = status({ state: "idle", session_id: "S9" });
  CH.chat.session = "S9";
  CH.chat.conv = emptyConv();
  let tn = 100;
  handlers.wiki_chat_send = () => {
    mock.state = "busy";
    mock.session = null; // Rust がまだ保存していない状態を返す（契約違反でも表示は守る）
    return { turn: tn++ };
  };
  CH.chat.draft = "保存前";
  const sending = CH.send();
  await CH.loadChatStatus(); // 送信中の読み
  assert.equal(await sending, true);
  stateEv("busy", null);
  emit("wiki-chat", { type: "enabled", enabled: true });
  await sleep(20);
  assert.ok(CH.chat.conv.entries.some((e) => e.k === "user" && e.text === "保存前"), "子がいる・送信中なのに session_id が null で表示を消した");
  mock.state = "off";
  await CH.loadChatStatus();
  assert.equal(CH.chat.conv.entries.length, 0, "off・session_id null で表示を消さない");
  // 読みの入れ替わり
  let release;
  let n = 0;
  handlers.wiki_chat_status = () => (++n === 1 ? new Promise((r) => (release = r)) : status({ enabled: true, state: "idle", session_id: "S9" }));
  const p1 = CH.loadChatStatus();
  await CH.loadChatStatus();
  release(status({ enabled: false, state: "off", session_id: null }));
  await p1;
  assert.equal(CH.chat.status.enabled, true, "古い状態の応答で新しい状態を上書きした");
  handlers.wiki_chat_status = mockStatus;
  // 刈ったターン
  mock.session = "S9";
  mock.state = "idle";
  await CH.loadChatStatus();
  CH.chat.session = "S9";
  CH.chat.conv = emptyConv();
  CH.chat.changes = {};
  CH.chat.undo = {};
  CH.chat.stopped = [];
  tn = 200;
  handlers.wiki_chat_send = () => ({ turn: tn++ });
  handlers.wiki_chat_undo = (a) => ({ ok: true, turn: a.turn, index: a.index });
  for (let i = 0; i < MAX_TURNS + 5; i++) {
    CH.chat.draft = `t${i}`;
    assert.equal(await CH.send(), true);
    const turn = tn - 1;
    emit("wiki-chat", { type: "changes", session_id: "S9", turn, changes: [{ kind: "create", path: `p${turn}.md`, sha256_after: "x" }] });
    await CH.undoChange(turn, 0);
    CH.chat.stopped = [...CH.chat.stopped, turn];
    stateEv("idle", "S9");
  }
  assert.equal(CH.chat.conv.dropped, 5);
  assert.equal(CH.chat.changes[204], undefined, "刈ったターンの changes を残した");
  assert.equal(CH.chat.undo["204:0"], undefined, "刈ったターンの undo を残した");
  assert.ok(!CH.chat.stopped.includes(204), "刈ったターンの stopped を残した");
  assert.ok(CH.chat.changes[205] && CH.chat.undo["205:0"]?.ok && CH.chat.stopped.includes(205), "残したターンの changes・undo を捨てた");
  flushSync();
  assert.ok($$(".wk-ch-note").some((e) => e.textContent === "古い発言を省きました (5)"), "刈った発言の数を出さない");
  // 送れなければ刈らない
  handlers.wiki_chat_send = () => {
    throw new Error("x");
  };
  const before = CH.chat.conv.entries.length;
  CH.chat.draft = "fail";
  assert.equal(await CH.send(), false);
  assert.deepEqual([CH.chat.conv.entries.length, CH.chat.conv.dropped], [before, 5], "送れなかったのに刈った");
  CH.chat.draft = "";
  CH.chat.error = null;
  tests++;
}

// 失敗を toast に出す（状態の読込・有効化）・consent は Wiki が有効なときだけ・初期 focus は [キャンセル]
{
  const { default: Consent } = await import("./ChatConsent.svelte");
  const t2 = document.createElement("div");
  document.body.append(t2);
  const c2 = mount(Consent, { target: t2 });
  emit("wiki-chat-consent", null);
  flushSync();
  assert.ok(t2.querySelector(".modal"), "consent を出さない");
  await sleep(0);
  assert.equal(document.activeElement?.textContent, "キャンセル", "初期 focus が [キャンセル] でない");
  handlers.wiki_chat_set_enabled = () => {
    throw new Error("設定に書けません");
  };
  t2.querySelector(".modal-foot .primary").click();
  await waitFor(() => S.app.toast?.includes("設定に書けません"));
  assert.ok(S.app.toast.includes("チャットを有効にできません"), `有効化の失敗を知らせない: ${S.app.toast}`);
  handlers.wiki_chat_status = () => {
    throw new Error("status 失敗");
  };
  await CH.loadChatStatus();
  assert.ok(S.app.toast.includes("status 失敗"), "状態の読込の失敗を知らせない");
  unmount(c2);
  t2.remove();
  tests++;
}

// Wiki OFF の間は event を積まない・invoke しない・consent を出さない
{
  S.app.wikiOn = false;
  CH.chat.conv = (await import("./chat.ts")).emptyConv();
  CH.chat.stderr = [];
  CH.chat.consent = false;
  calls.length = 0;
  emit("wiki-chat", { type: "state", state: "idle", session_id: "X" });
  emit("wiki-chat", { type: "enabled", enabled: false });
  emit("wiki-chat-consent", null);
  emit("wiki-chat", { type: "line", session_id: "X", turn: 1, line: L({ type: "assistant", message: { id: "a", content: [{ type: "text", text: "hi" }] } }) });
  emit("wiki-chat", { type: "stderr", text: "e" });
  emit("wiki-chat", { type: "changes", session_id: "X", turn: 1, changes: [{ kind: "item", key: "K" }] });
  await sleep(10);
  assert.deepEqual(calls.map((c) => c.name), [], "Wiki OFF で invoke した");
  assert.deepEqual([CH.chat.consent, CH.chat.conv.entries.length, CH.chat.stderr.length, CH.chat.changes[1]], [false, 0, 0, undefined], "Wiki OFF で積んだ");
  assert.equal(CH.chat.status, null);
  tests++;
}

// 柱の幅: 320〜窓幅の半分（clampDockW）・ui_chat_dock の読み（壊れた値は既定）・確定（setDockWidth）だけが覚える・表示は窓に合わせて収める
// （覚えた値は書き換えない）・見出しの × で閉じる・一覧が前面なら見えていない Wiki のページを文脈にしない
{
  mock.enabled = true;
  mock.state = "idle";
  mock.over = {};
  handlers.wiki_chat_status = () => status({ enabled: mock.enabled, state: mock.state, session_id: mock.session, ...mock.over });
  S.app.wikiOn = true;
  CH.chat.status = status({ enabled: true, state: "idle", session_id: null });
  // 純関数（chat.ts）
  assert.deepEqual([C.clampDockW(100, 1200), C.clampDockW(500, 1200), C.clampDockW(900, 1200), C.clampDockW(500, 600), C.clampDockW(NaN, 1200)], [320, 500, 600, 320, 380], "幅を 320〜窓幅の半分に収めない");
  assert.deepEqual(
    [null, "", "x", "[]", "null", '{"open":true}', '{"open":"yes","w":500}', '{"open":true,"w":-5}', '{"open":false,"w":444.4}'].map(C.parseDockPref),
    [{ open: false, w: 380 }, { open: false, w: 380 }, { open: false, w: 380 }, { open: false, w: 380 }, { open: false, w: 380 }, { open: true, w: 380 }, { open: false, w: 500 }, { open: true, w: 380 }, { open: false, w: 444 }],
    "ui_chat_dock の読みが既定に倒れない"
  );
  CH.chat.dock = { open: true, w: C.DOCK_W };
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(ChatDock, { target }); // mount = 状態を読み直す（mock は有効）
  assert.ok(await waitFor(() => !!target.querySelector(".cd .wk-ch")), "柱にチャットが出ない");
  const aside = () => target.querySelector("aside.cd");
  assert.equal(aside().style.width, "380px", "既定の幅が 380 でない");
  // 確定 = 収めて覚える（窓幅 1024 の半分 = 512 まで）
  delete prefs.ui_chat_dock;
  CH.setDockWidth(900);
  await sleep(5);
  flushSync();
  assert.deepEqual(JSON.parse(prefs.ui_chat_dock), { open: true, w: 512 }, "確定した幅を窓幅の半分に収めて覚えない");
  assert.equal(aside().style.width, "512px");
  const kept = prefs.ui_chat_dock;
  delete prefs.ui_chat_dock;
  CH.setDockWidth(600); // 収めると同じ 512 = 覚え直さない
  await sleep(5);
  assert.equal(prefs.ui_chat_dock, undefined, "同じ幅で覚え直した");
  prefs.ui_chat_dock = kept;
  // 窓が狭くなった = 表示だけ収める（覚えた値は書き換えない）・広がれば戻る
  const iw = window.innerWidth;
  window.innerWidth = 700;
  window.dispatchEvent(new Event("resize"));
  flushSync();
  assert.deepEqual([aside().style.width, JSON.parse(prefs.ui_chat_dock).w], ["350px", 512], "窓に合わせて収めない / 収めただけで覚え直した");
  window.innerWidth = iw;
  window.dispatchEvent(new Event("resize"));
  flushSync();
  assert.equal(aside().style.width, "512px", "窓が広がっても覚えた幅に戻らない");
  // 一覧の上に柱を開いたとき = 見えていない Wiki のページを文脈にしない
  {
    const wikiTab = S.app.tab;
    const base = S.app.tabs.find((t) => t.id === wikiTab);
    S.app.tabs.push({ ...base, id: 9002, wiki: null, viewer: null });
    S.app.tab = 9002;
    flushSync();
    assert.equal(V.viewingSource(), null, "一覧が前面なのに Wiki のページを文脈にした");
    S.app.tab = wikiTab;
    S.app.tabs.pop();
    flushSync();
    assert.deepEqual(V.viewingSource(), { kind: "page", path: "notes/a.md" });
  }
  // 見出しの × = 柱を閉じる（覚える）
  target.querySelector(".wk-ch-close").click();
  await sleep(5);
  assert.deepEqual([CH.chat.dock.open, JSON.parse(prefs.ui_chat_dock).open], [false, false], "× で柱が閉じない / 覚えない");
  unmount(cmp);
  target.remove();
  CH.chat.dock = { open: false, w: C.DOCK_W };
  tests++;
}

// 柱のつまみ: 動かしている間は線だけ動かし柱の幅（= レイアウト）は変えない・覚えない（読み手が描き直さない）・離したときに 1 回だけ確定して覚える・
// 掴んで動かしても入力欄やエディタの focus を奪わない・つまみの最中に柱が消えても後始末する
{
  CH.chat.dock = { open: true, w: C.DOCK_W };
  const target = document.createElement("div");
  document.body.append(target);
  const cmp = mount(ChatDock, { target });
  flushSync();
  const aside = target.querySelector("aside.cd");
  aside.getBoundingClientRect = () => ({ left: 644, right: 1024, top: 0, bottom: 700, width: 380, height: 700 });
  const outside = document.createElement("input");
  document.body.append(outside);
  outside.focus();
  let writes = 0;
  const setPref = handlers.set_ui_pref;
  handlers.set_ui_pref = (a) => {
    if (a.key === "ui_chat_dock") writes++;
    return setPref(a);
  };
  const grip = target.querySelector(".cd-grip");
  const pe = (type, x, buttons) => grip.dispatchEvent(new window.PointerEvent(type, { bubbles: true, cancelable: true, button: 0, buttons, clientX: x, clientY: 100, pointerId: 7 }));
  pe("pointerdown", 644, 1);
  pe("pointermove", 600, 1);
  pe("pointermove", 520, 1);
  flushSync();
  assert.equal(aside.style.width, "380px", "動かしている間に柱の幅（レイアウト）を変えた");
  assert.equal(document.querySelector(".cd-guide")?.style.left, "520px", "動かしている間に線が指に追わない");
  assert.ok(document.body.classList.contains("dragging"), "動かしている間に選択を止めない");
  await sleep(5);
  assert.equal(writes, 0, "動かしている間に幅を覚えた");
  pe("pointerup", 520, 0);
  await sleep(5);
  flushSync();
  assert.deepEqual([aside.style.width, writes, JSON.parse(prefs.ui_chat_dock).w], ["504px", 1, 504], "離したときに 1 回だけ確定しない");
  assert.equal(document.querySelector(".cd-guide"), null, "離しても線が残った");
  assert.equal(document.body.classList.contains("dragging"), false, "離しても選択を止めたまま");
  assert.equal(document.activeElement, outside, "つまみで入力欄から focus を奪った");
  // 動かさずに離した = 覚え直さない
  pe("pointerdown", 520, 1);
  pe("pointerup", 520, 0);
  await sleep(5);
  assert.equal(writes, 1, "動かさずに離しただけで覚え直した");
  // つまみの最中に柱が消えた（⌘⌥C・チャット OFF）= リスナーと body の印を残さない
  pe("pointerdown", 520, 1);
  pe("pointermove", 480, 1);
  flushSync();
  unmount(cmp);
  flushSync();
  assert.equal(document.body.classList.contains("dragging"), false, "柱が消えても選択を止めたまま");
  await sleep(5);
  assert.equal(writes, 1, "柱が消えたときに途中の幅を確定した");
  handlers.set_ui_pref = setPref;
  outside.remove();
  target.remove();
  CH.chat.dock = { open: false, w: C.DOCK_W };
  tests++;
}

// ⑥ 一覧の右ペイン: 開閉の永続と ⌘⌥I の振り分け（前面のモードの右ペインだけ）・行の選択で開かない・閉じても詳細は読む
{
  const wikiTab = S.app.tab;
  const base = S.app.tabs.find((t) => t.id === wikiTab);
  S.app.tabs.push({ ...base, id: 9003, wiki: null, viewer: null });
  S.app.tab = 9003;
  flushSync();
  const info = W.wk.infoOpen;
  const side0 = S.app.viewerSideOpen;
  assert.equal(S.app.detailOpen, true, "既定は開く");
  W.togglePane(); // 一覧が前面
  await sleep(5);
  assert.deepEqual([S.app.viewerSideOpen, prefs.ui_viewer_side], [side0, undefined], "一覧で ⌘⌥I が PDF の右バーを変えた");
  assert.deepEqual([S.app.detailOpen, prefs.ui_detail], [false, "0"], "一覧で ⌘⌥I が一覧の右ペインを閉じない");
  assert.equal(W.wk.infoOpen, info, "一覧で ⌘⌥I が OpenNote の右ペインを変えた");
  assert.equal(prefs.ui_wiki_info, undefined, "一覧で ⌘⌥I が ui_wiki_info を書いた");
  // 閉じたまま行を選ぶ = 開かない・詳細は読む（reveal / ★ / メモ・チャットの文脈が app.detail を読む）
  const items = S.app.items;
  S.app.items = [{ key: "K7" }];
  handlers.get_item = (a) => ({ key: a.key, csl: { title: "題名 7" }, attachments: [], collections: [], tags: [], notes: [], dateAdded: "2026-01-01" });
  await S.select("K7");
  assert.equal(S.app.detailOpen, false, "行の選択で右ペインが開いた");
  assert.equal(S.app.detail?.key, "K7", "閉じたら詳細を読まなくなった");
  assert.deepEqual(V.viewingSource(), { kind: "item", key: "K7", title: "題名 7" }, "閉じていても柱の文脈は選んでいる文献");
  S.app.selected = null;
  S.app.detail = null;
  S.app.checked.clear();
  S.app.items = items;
  S.toggleDetail();
  await sleep(5);
  assert.deepEqual([S.app.detailOpen, prefs.ui_detail], [true, "1"], "ボタンで開かない");
  // OpenNote が前面 = 従来どおり OpenNote の右ペイン
  S.app.tab = wikiTab;
  flushSync();
  W.togglePane();
  await sleep(5);
  assert.deepEqual([W.wk.infoOpen, prefs.ui_wiki_info], [!info, info ? "0" : "1"], "OpenNote で ⌘⌥I が OpenNote の右ペインを開閉しない");
  assert.deepEqual([S.app.viewerSideOpen, prefs.ui_viewer_side], [side0, undefined], "OpenNote で ⌘⌥I が PDF の右バーを変えた");
  assert.deepEqual([S.app.detailOpen, prefs.ui_detail], [true, "1"], "OpenNote で ⌘⌥I が一覧の右ペインを変えた");
  W.togglePane();
  await sleep(5);
  S.app.tabs.pop();
  flushSync();
  // 起動時の読み（main.ts が mount の前に待つ）: "0" だけが閉じる・無ければ開く・読めなければ既定のまま知らせる
  for (const [v, open] of [["0", false], ["1", true], [undefined, true]]) {
    if (v === undefined) delete prefs.ui_detail;
    else prefs.ui_detail = v;
    S.app.detailOpen = !open;
    await S.loadDetailPref();
    assert.equal(S.app.detailOpen, open, `ui_detail = ${v}`);
  }
  const getPref = handlers.get_ui_pref;
  handlers.get_ui_pref = () => {
    throw new Error("読めない");
  };
  await S.loadDetailPref();
  assert.equal(S.app.detailOpen, true, "読めないのに既定でない");
  assert.match(S.app.toast, /表示設定を読めません/, "読めないことを知らせない");
  handlers.get_ui_pref = getPref;
  tests++;
}

// ⑥ 一覧が前面のときの柱: 門が閉じていればボタンなし・invoke なし（右ペインは ItemDetail のまま = チャットと無関係）・開いていればボタン →
// 柱を開くと状態を読む・選んだ文献が文脈のチップに出て送れる（× で外す・別の文献で戻る）・同じ会話・PDF のタブへ移っても同じ柱（作り直さない）でチップは
// その文献・応答中の点・Settings で OFF = ボタンが消える
{
  const wikiTab = S.app.tab;
  const base = S.app.tabs.find((t) => t.id === wikiTab);
  S.app.tabs.push({ ...base, id: 9004, wiki: null, viewer: null });
  S.app.tab = 9004;
  const detail = (key, title) => ({ key, csl: { title }, attachments: [], collections: [], tags: [], notes: [], dateAdded: "2026-01-01" });
  const stripAt = document.createElement("div");
  const target = document.createElement("div");
  document.body.append(stripAt, target);
  const $ = (q) => target.querySelector(q);
  const btn = () => stripAt.querySelector(".tabs-chat");

  // OpenNote OFF: invoke しない・ボタンなし（覚えた開閉が「開」でも出さない = 柱の門は App が同じ chatShown() を見る）
  S.app.wikiOn = false;
  CH.chat.status = null;
  CH.chat.dock = { open: true, w: C.DOCK_W };
  calls.length = 0;
  const strip = mount(TabStrip, { target: stripAt });
  flushSync();
  await sleep(10);
  assert.deepEqual([btn(), CH.chatShown()], [null, false], "OpenNote OFF でボタンが出た / 柱を出す判定が true");
  assert.deepEqual(calls.filter((c) => c.name.startsWith("wiki_chat")).map((c) => c.name), [], "OpenNote OFF で invoke した");
  // OpenNote ON・チャット OFF も同じ
  S.app.wikiOn = true;
  mock.enabled = false;
  CH.chat.status = status({ enabled: false });
  await sleep(5);
  flushSync();
  assert.deepEqual([btn(), CH.chatShown()], [null, false], "チャット OFF でボタンが出た / 柱を出す判定が true");
  await sleep(10);
  calls.length = 0;
  {
    // タブ列が出ただけでは読まない（読むのは柱を開いたとき。上の読みは initChat = OpenNote が ON になった分）
    const again = document.createElement("div");
    document.body.append(again);
    const s2 = mount(TabStrip, { target: again });
    flushSync();
    await sleep(10);
    assert.deepEqual(calls.filter((c) => c.name.startsWith("wiki_chat")).map((c) => c.name), [], "チャット OFF で invoke した");
    unmount(s2);
    again.remove();
  }
  // Settings で ON = ボタンが出る・柱を開くと状態を読む・入力欄へ focus
  mock.enabled = true;
  CH.chat.dock = { open: false, w: C.DOCK_W };
  emit("wiki-chat", { type: "enabled", enabled: true });
  assert.ok(await waitFor(() => (flushSync(), !!btn())), "チャット ON でボタンが出ない");
  calls.length = 0;
  btn().click();
  flushSync();
  const dock = mount(ChatDock, { target }); // App と同じ条件（chatShown() && chat.dock.open）
  flushSync();
  await sleep(10);
  assert.ok($(".cd .wk-ch"), "柱に WikiChat が出ない");
  assert.equal(document.activeElement?.tagName, "TEXTAREA", "柱を開いても入力欄へ focus しない");
  assert.ok(calls.some((c) => c.name === "wiki_chat_status"), "柱を開いても状態を読まない");
  const box = $(".cd .wk-ch textarea"); // 作り直されないことを下で見る

  // 文脈: 選んでいる文献がチップに出る・読込の途中（app.detail が前の行）は添えない・× で外す・別の文献で戻る
  assert.equal(V.viewingSource(), null, "選んでいないのに文脈があった（Wiki のページを添えない）");
  S.app.selected = "K1";
  S.app.detail = detail("K1", "題名 1");
  flushSync();
  assert.equal($(".wk-ch-ctx .wk-ch-chip-item .wk-ch-chip-text")?.textContent, "題名 1", "選んだ文献がチップに出ない");
  assert.deepEqual(CH.currentContext(), { item: "K1", title: "題名 1" });
  S.app.selected = "K2";
  flushSync();
  assert.equal(V.viewingSource(), null, "読込の途中で前の文献を文脈にした");
  S.app.detail = detail("K2", "題名 2");
  flushSync();
  assert.deepEqual(CH.currentContext(), { item: "K2", title: "題名 2" });
  $(".wk-ch-ctx .wk-ch-x").click();
  flushSync();
  assert.equal(CH.currentContext(), undefined, "× で外れない");
  assert.equal($(".wk-ch-ctx"), null, "× の後もチップが残った");
  S.app.detail = detail("K2", "題名 2（再読込）"); // reloadDetail = 同じ元 = 外したまま
  flushSync();
  assert.equal(CH.currentContext(), undefined, "再読込で外した文脈が戻った");
  S.app.selected = "K3";
  S.app.detail = detail("K3", "");
  await sleep(5);
  flushSync();
  assert.deepEqual(CH.currentContext(), { item: "K3" }, "別の文献で文脈が戻らない（題名なしは item だけ）");
  assert.equal($(".wk-ch-ctx .wk-ch-chip-item .wk-ch-chip-text")?.textContent, "K3", "題名なしのチップが key でない");
  S.app.detail = detail("K3", "題名 3");
  flushSync();
  // 送る = Rust の契約は PDF のタブと同じ {item, title}（path は無い）
  mock.state = "idle";
  CH.chat.status = status({ enabled: true, state: "idle", session_id: "S1" });
  let sent = null;
  handlers.wiki_chat_send = (a) => {
    sent = a;
    return { turn: 501 };
  };
  CH.chat.draft = "要約して";
  assert.equal(await CH.send(), true, "一覧のタブから送れない");
  assert.deepEqual(sent.context, { item: "K3", title: "題名 3" }, "選んだ文献が context に乗らない");
  flushSync();

  // 同じ会話: 柱は同じ chat を映す
  assert.ok(target.textContent.includes("要約して"), "送った発言が出ない");

  // PDF のタブが前面 = 同じ柱（入力欄の DOM が同じ = 作り直さない）・チップは PDF の文献・戻れば一覧の文献
  S.app.tabs.push({ ...base, id: 9005, wiki: null, viewer: { itemKey: "K1", attachmentId: 1, title: "T" } });
  S.app.tab = 9005;
  flushSync();
  assert.equal($(".cd .wk-ch textarea"), box, "PDF のタブへ移ると柱を作り直した");
  assert.equal($(".wk-ch-ctx .wk-ch-chip-item .wk-ch-chip-text")?.textContent, "T", "PDF のタブで柱のチップがその文献でない");
  S.app.tabs.pop();
  S.app.tab = 9004;
  flushSync();
  assert.equal($(".cd .wk-ch textarea"), box, "一覧へ戻ると柱を作り直した");
  assert.equal($(".wk-ch-ctx .wk-ch-chip-item .wk-ch-chip-text")?.textContent, "題名 3", "一覧へ戻ってもチップが選んでいる文献に戻らない");

  // 応答中の点・Settings で OFF = ボタンが消える（柱は App が chatShown() で外す）
  stateEv("busy");
  flushSync();
  assert.ok(stripAt.querySelector(".tabs-chat-dot"), "応答中の点が出ない");
  stateEv("idle");
  mock.enabled = false;
  emit("wiki-chat", { type: "enabled", enabled: false });
  flushSync();
  assert.deepEqual([btn(), CH.chatShown()], [null, false], "OFF でボタンが消えない / 柱を出す判定が残った");
  await sleep(10);
  mock.enabled = true;
  emit("wiki-chat", { type: "enabled", enabled: true });
  assert.ok(await waitFor(() => (flushSync(), !!btn())), "ON でボタンが戻らない");
  unmount(dock);
  unmount(strip);
  target.remove();
  stripAt.remove();
  S.app.selected = null;
  S.app.detail = null;
  S.app.tabs.pop();
  S.app.tab = wikiTab;
  CH.chat.dock = { open: false, w: C.DOCK_W };
  CH.chat.conv = (await import("./chat.ts")).emptyConv();
  flushSync();
  tests++;
}

// md 以外（pdf / html / 画像 / 動画 / その他）へのリンクは新しいタブ・md と拡張子なしは同じタブの中で遷移
{
  assert.deepEqual(["a/b.pdf", "x.html", "y.HTM", "img.png", "v.mp4", "z.docx"].map(W.opensInNewTab), [true, true, true, true, true, true]);
  assert.deepEqual(["notes/a.md", "notes/a"].map(W.opensInNewTab), [false, false]);
  tests++;
}

unmount(cmp);
target.remove();

// 状態の読み直し: 自動（柱を開いた = mount・開いたまま前面のタブが変わった）は force なし = Rust が claude auth status を 30 秒覚えて省く（wiki_chat.rs の
// auth_status_is_cached_for_ttl）・読み中とは重ねない・[再確認]（アカウントの行・claude が無いときの案内）は force: true
{
  mock.enabled = true;
  mock.state = "off";
  mock.over = {};
  handlers.wiki_chat_status = () => status({ enabled: mock.enabled, state: mock.state, session_id: mock.session, ...mock.over });
  CH.chat.dock = { open: true, w: C.DOCK_W };
  await waitFor(() => !CH.statusInFlight());
  const reads = () => calls.filter((c) => c.name === "wiki_chat_status");
  calls.length = 0;
  const t1 = document.createElement("div");
  document.body.append(t1);
  const began = Date.now();
  let c = mount(ChatDock, { target: t1 });
  assert.ok(await waitFor(() => reads().length === 1 && !CH.statusInFlight()), "柱を開いても読み直さない");
  unmount(c);
  c = mount(ChatDock, { target: t1 });
  assert.ok(await waitFor(() => reads().length === 2 && !CH.statusInFlight()), "柱を開き直しても読み直さない");
  // 開いたまま前面のタブが変わった = 読み直す
  const wikiTab = S.app.tab;
  const base = S.app.tabs.find((t) => t.id === wikiTab);
  S.app.tabs.push({ ...base, id: 9006, wiki: null, viewer: null });
  S.app.tab = 9006;
  flushSync();
  assert.ok(await waitFor(() => reads().length === 3 && !CH.statusInFlight()), "柱を開いたまま前面のタブが変わっても読み直さない");
  // 読み中に前面のタブが変わった = 重ねない
  let release;
  handlers.wiki_chat_status = () => new Promise((r) => (release = r));
  const slow = CH.loadChatStatus();
  S.app.tab = wikiTab;
  flushSync();
  await sleep(10);
  assert.equal(reads().length, 4, "読み中に前面のタブが変わって読みを重ねた");
  release(status({ enabled: true, state: "off" }));
  await slow;
  handlers.wiki_chat_status = () => status({ enabled: mock.enabled, state: mock.state, session_id: mock.session, ...mock.over });
  S.app.tabs.pop();
  assert.ok(Date.now() - began < 10_000, "時間待ちで読み直している");
  assert.deepEqual(reads().map((r) => r.args), [{}, {}, {}, {}], "自動の読み直しに force を付けた（Rust の覚えを捨てる）");
  flushSync();
  calls.length = 0;
  const again = t1.querySelector('.wk-ch-acct .wk-ch-btn[title="ログイン状態を読み直します"]');
  assert.ok(again, "アカウントの行に [再確認] が無い");
  again.click();
  assert.ok(await waitFor(() => reads().length === 1 && !CH.statusInFlight()));
  assert.deepEqual(reads().map((r) => r.args), [{ force: true }], "[再確認] が force: true で読まない");
  mock.over = { cli: null, version: null, logged_in: null };
  await CH.loadChatStatus();
  flushSync();
  calls.length = 0;
  const guide = t1.querySelector(".wk-ch-guide .wk-ch-btn");
  assert.ok(guide, "claude が無いときの案内に [再確認] が無い");
  guide.click();
  assert.ok(await waitFor(() => reads().length === 1 && !CH.statusInFlight()));
  assert.deepEqual(reads().map((r) => r.args), [{ force: true }], "案内の [再確認] が force: true で読まない");
  mock.over = {};
  await CH.loadChatStatus();
  unmount(c);
  t1.remove();
  CH.chat.dock = { open: false, w: C.DOCK_W };
  tests++;
}

// 入力欄への focus（focusPending）: 見えている入力欄が focus できたときだけ消費する（面は柱 1 つになったが、見えない入力欄が
// 先に消費しない規則と明示の focus は残す = WKWebView は autofocus が効かない）・disabled（使えない）なら消費だけする。jsdom に配置は無い = checkVisibility を差し替えて判定だけを見る（実機の確認は別）
{
  const el = document.createElement("textarea");
  document.body.append(el);
  CH.chat.focusPending = false;
  assert.equal(CH.claimFocus(el), false, "立っていないのに消費した");
  CH.chat.focusPending = true;
  el.checkVisibility = () => false;
  assert.equal(CH.claimFocus(el), false, "見えていないのに受けた");
  assert.equal(CH.chat.focusPending, true, "見えていない入力欄が消費した");
  assert.notEqual(document.activeElement, el);
  el.checkVisibility = () => true;
  assert.equal(CH.claimFocus(el), true);
  assert.deepEqual([CH.chat.focusPending, document.activeElement === el], [false, true], "見えている入力欄が受けない");
  // focus が移らない（文書の外の要素）= 消費しない
  const loose = document.createElement("textarea");
  CH.chat.focusPending = true;
  assert.equal(CH.claimFocus(loose), false);
  assert.equal(CH.chat.focusPending, true, "focus が移らないのに消費した");
  // checkVisibility の無い WebView = 見えている扱い（従来どおり focus する）
  delete el.checkVisibility;
  el.blur();
  assert.equal(CH.claimFocus(el), true);
  assert.equal(CH.chat.focusPending, false);
  // 見えているが disabled（使えない）= focus せずに消費する
  el.blur();
  el.disabled = true;
  CH.chat.focusPending = true;
  assert.equal(CH.claimFocus(el), false);
  assert.deepEqual([CH.chat.focusPending, document.activeElement === el], [false, false], "disabled の入力欄で focusPending が残った");
  el.disabled = false;
  el.remove();

  // 部品: 使えない（未ログイン = 入力欄が disabled）で柱を開く → login → 使える → 柱を開き直す（mount のし直し）で、
  // 別の入力欄で作業中の focus を奪わない
  {
    const saved = CH.chat.status;
    CH.chat.status = status({ enabled: true, state: "off", logged_in: false });
    const box = document.createElement("div");
    document.body.append(box);
    let w = mount(WikiChat, { target: box });
    flushSync();
    assert.ok(box.querySelector("textarea").disabled, "未ログインなのに入力欄が使える");
    CH.chat.focusPending = true;
    flushSync();
    await sleep(10);
    assert.equal(CH.chat.focusPending, false, "disabled の入力欄で focusPending が残った（後の mount で focus を奪う）");
    CH.chat.status = status({ enabled: true, state: "off", logged_in: true });
    unmount(w);
    const other = document.createElement("input");
    document.body.append(other);
    other.focus();
    w = mount(WikiChat, { target: box });
    flushSync();
    await sleep(10);
    assert.equal(document.activeElement, other, "mount のし直しで別の入力欄から focus を奪った");
    unmount(w);
    box.remove();
    other.remove();
    CH.chat.status = saved;
  }
  tests++;

  // 部品: 隠れた方（DOM 順が先）と見えている方の WikiChat が両方 mount されている → 見えている方の入力欄に来る
  const proto = window.HTMLTextAreaElement.prototype;
  proto.checkVisibility = function () {
    return !this.closest(".v-hidden");
  };
  const hid = document.createElement("div");
  hid.className = "v-hidden";
  const vis = document.createElement("div");
  document.body.append(hid, vis);
  const a = mount(WikiChat, { target: hid });
  const b = mount(WikiChat, { target: vis });
  flushSync();
  CH.chat.focusPending = true;
  flushSync();
  await sleep(10);
  assert.equal(document.activeElement, vis.querySelector("textarea"), "隠れた WikiChat が focusPending を消費した（見えている方に来ない）");
  assert.equal(CH.chat.focusPending, false, "見えている方が受けたのに残った");
  unmount(a);
  unmount(b);
  hid.remove();
  vis.remove();
  delete proto.checkVisibility;
  tests++;
}

// ---------- 監視の通知（wiki-changed = {all, added, removed, modified}）: 種別で読む物を選ぶ ----------
// 木（wiki_list）は増減（all / added / removed）と「知らない modified」（木に無いパス = core が同期で索引に入れた新規ページ）のときだけ・
// Tags 箱は毎回（ログの `#tag` も索引に入る）・Ingest / Chat log 箱はそのフォルダに変更があるときだけ
{
  S.app.wikiOn = true;
  W.wk.showResults = false;
  const pg = (path, title = "") => ({ path, kind: "md", title, tags: [], items: [], mtime: 0, size: 0 });
  const known = ["chat-log/x.md", "notes/a.md", "ingest/x.pdf", "notes/b.md", "img/x.png"].map((p) => pg(p));
  handlers.wiki_list = () => ({ total: known.length, count: known.length, pages: known.map((p) => ({ ...p })) });
  handlers.wiki_tags = () => ({ tags: [], total: 0 });
  W.wk.pages = known.map((p) => ({ ...p }));
  handlers.wiki_inbox = () => ({ files: [], total: 0 });
  const offWiki = W.initWiki();
  await sleep(10);
  const count = () => ({
    list: calls.filter((c) => c.name === "wiki_list").length,
    tags: calls.filter((c) => c.name === "wiki_tags").length,
    ingest: calls.filter((c) => c.name === "wiki_inbox" && c.args.dir !== "chat-log").length,
    log: calls.filter((c) => c.name === "wiki_inbox" && c.args.dir === "chat-log").length,
  });
  const ev = (o) => ({ all: false, added: [], removed: [], modified: [], ...o });
  const changed = async (...batches) => {
    calls.length = 0;
    for (const b of batches) emit("wiki-changed", b);
    await sleep(1150); // 画面の 1 秒まとめ
    return count();
  };
  // (a) modified だけ = 木を読まない・Tags は読む・箱はそのフォルダのときだけ。ログの追記（1 秒の間に 2 回）= Chat log 箱 1 回
  assert.deepEqual(await changed(ev({ modified: ["chat-log/x.md"] }), ev({ modified: ["chat-log/x.md"] })), { list: 0, tags: 1, ingest: 0, log: 1 }, "ログの追記");
  assert.deepEqual(await changed(ev({ modified: ["notes/a.md"] })), { list: 0, tags: 1, ingest: 0, log: 0 }, "ページの上書き");
  assert.deepEqual(await changed(ev({ modified: ["ingest/x.pdf"] })), { list: 0, tags: 1, ingest: 1, log: 0 }, "ingest/ の上書き");
  // (b) added = 木（行が増える）+ Tags + そのフォルダの箱
  assert.deepEqual(await changed(ev({ added: ["chat-log/new.md"] })), { list: 1, tags: 1, ingest: 0, log: 1 }, "新しいログ");
  assert.deepEqual(await changed(ev({ added: ["notes/n.md"] })), { list: 1, tags: 1, ingest: 0, log: 0 }, "新しいページ");
  // (c) removed = 木（行が減る）+ Tags + そのフォルダの箱
  assert.deepEqual(await changed(ev({ removed: ["chat-log/x.md"] })), { list: 1, tags: 1, ingest: 0, log: 1 }, "消えたログ");
  assert.deepEqual(await changed(ev({ removed: ["ingest/x.pdf"] })), { list: 1, tags: 1, ingest: 1, log: 0 }, "消えた素材");
  // (d) all（全走査の後）= 全部
  assert.deepEqual(await changed(ev({ all: true })), { list: 1, tags: 1, ingest: 1, log: 1 }, "全走査");
  // (e) 1 秒の間の混在 = 和（ログの追記 + 新しいページ → 木 1・Chat log 箱 1・各 1 回ずつ）
  assert.deepEqual(await changed(ev({ modified: ["chat-log/x.md"] }), ev({ added: ["notes/b.md"] }), ev({ modified: ["chat-log/x.md"] })), { list: 1, tags: 1, ingest: 0, log: 1 }, "混在");
  // (f) 古い形（string[]）・欠けた欄 = 空の変更として扱う（投げない・木は読まない・Tags だけ）
  assert.deepEqual(await changed(["a.md"]), { list: 0, tags: 1, ingest: 0, log: 0 }, "古い形の payload");
  assert.deepEqual(await changed({ added: ["x.md"] }), { list: 1, tags: 1, ingest: 0, log: 0 }, "欠けた欄");
  // (f2) 木が知らない modified（core が同期で索引に入れた新規ページ = 監視では modified で届く）= 木を読む。全部既知なら読まない
  assert.deepEqual(await changed(ev({ modified: ["notes/a.md", "notes/fresh.md"] })), { list: 1, tags: 1, ingest: 0, log: 0 }, "知らない modified で木を読まない");
  assert.deepEqual(await changed(ev({ modified: ["notes/a.md", "notes/b.md"] })), { list: 0, tags: 1, ingest: 0, log: 0 }, "既知の modified で木を読んだ");
  assert.deepEqual(await changed(ev({ modified: ["chat-log/fresh.md"] })), { list: 1, tags: 1, ingest: 0, log: 1 }, "知らないログの modified");
  // (g) wk.changed / stamp / touched / touchedAny（種別を問わず・ログは touchedAny から除く・all は全部）
  const st0 = W.wk.stamp;
  emit("wiki-changed", ev({ modified: ["chat-log/x.md"] }));
  assert.equal(W.wk.stamp, st0 + 1, "stamp が進まない");
  assert.deepEqual(W.wk.changed, ev({ modified: ["chat-log/x.md"] }));
  assert.equal(W.touched("chat-log/x.md"), true, "touched（単一）にログが入らない");
  assert.equal(W.touched("notes/a.md"), false);
  assert.equal(W.touchedAny(["notes/a.md"]), false, "ログの追記でバックリンクを読み直す");
  assert.equal(W.touchedAny(["chat-log/x.md"]), false, "参照元のログの追記でバックリンクを読み直す");
  emit("wiki-changed", ev({ removed: ["notes/b.md"] }));
  assert.equal(W.touchedAny(["notes/a.md"]), true, "md の削除でバックリンクを読み直さない");
  emit("wiki-changed", ev({ added: ["chat-log.md"] })); // 名前が前方一致だけのページはログでない
  assert.equal(W.touchedAny(["notes/a.md"]), true, "chat-log.md をログとみなした");
  emit("wiki-changed", ev({ added: ["img/x.png"] }));
  assert.equal(W.touchedAny(["notes/a.md"]), false, "画像だけの変更で読み直した");
  assert.equal(W.touched("img/x.png"), true);
  emit("wiki-changed", ev({ all: true }));
  assert.equal(W.touchedAny(["notes/a.md"]), true, "全走査で読み直さない");
  assert.equal(W.touched("anything.md"), true, "全走査で touched でない");
  await sleep(1150);
  // (h) reloadPlan（純関数）
  assert.deepEqual(W.reloadPlan(ev({ modified: ["a.md"] })), { tree: false, tags: true, ingest: false, chatLog: false });
  assert.deepEqual(W.reloadPlan(ev({ added: ["a.md"] })), { tree: true, tags: true, ingest: false, chatLog: false });
  assert.deepEqual(W.reloadPlan(ev({ removed: ["ingest/a.pdf"] })), { tree: true, tags: true, ingest: true, chatLog: false });
  assert.deepEqual(W.reloadPlan(ev({ modified: ["chat-log/a.md", "ingest/b.md"] })), { tree: false, tags: true, ingest: true, chatLog: true });
  assert.deepEqual(W.reloadPlan(ev({ all: true })), { tree: true, tags: true, ingest: true, chatLog: true });
  assert.deepEqual(W.reloadPlan(ev({ modified: ["Chat-Log/a.md", "ingestx/b.md"] })), { tree: false, tags: true, ingest: false, chatLog: false }, "前方一致だけのフォルダ");
  assert.deepEqual(W.reloadPlan(ev({ modified: ["a.md", "n.md"] }), (p) => p === "a.md"), { tree: true, tags: true, ingest: false, chatLog: false }, "known を見ない");
  assert.deepEqual(W.reloadPlan(ev({ modified: ["a.md"] }), (p) => p === "a.md"), { tree: false, tags: true, ingest: false, chatLog: false });
  tests++;
  // (i) 画面からの作成 / 改名 / ゴミ箱の後の loadAll = 木・Tags・両方の箱
  calls.length = 0;
  await W.loadAll();
  await sleep(20);
  assert.deepEqual(count(), { list: 1, tags: 1, ingest: 1, log: 1 }, "loadAll");
  offWiki();
  tests++;
}

// ---------- 索引の再利用: タブの題名（titleIndex）とチャットの [[…]]（pageIndex）は木の配列ごとに 1 回・wk.pages は raw（要素を proxy に包まない） ----------
{
  const pages = [
    { path: "x/a.md", kind: "md", title: "A", tags: [], items: [], mtime: 0, size: 0 },
    { path: "b.md", kind: "md", title: "", tags: [], items: [], mtime: 0, size: 0 },
  ];
  const t1 = W.titleIndex(pages);
  assert.equal(W.titleIndex(pages), t1, "同じ配列で題名の索引を作り直した");
  assert.notEqual(W.titleIndex([...pages]), t1, "別の配列で古い索引を返した");
  W.wk.pages = pages;
  assert.equal(W.wk.pages, pages, "wk.pages が置いた配列そのものでない（proxy に包まれた）");
  assert.equal(W.wk.pages[0], pages[0], "wk.pages の要素が proxy に包まれた");
  assert.equal(W.titleIndex(W.wk.pages), t1, "wk.pages 経由で索引を作り直した");
  assert.equal(W.wikiTabTitle({ path: "x/a.md", scrollTop: 0, editing: false }), "A");
  assert.equal(W.wikiTabTitle({ path: "b.md", scrollTop: 0, editing: false }), "b", "題名が空ならファイル名");
  assert.equal(W.wikiTabTitle({ path: "zz/no.md", scrollTop: 0, editing: false }), "no", "索引に無いページはファイル名");
  assert.equal(W.wikiTabTitle({ path: null, scrollTop: 0, editing: false }), "OpenNote");
  // 読んだページの題名（titleOverrides）が先: 題名だけの変更は modified = 木を読まないので、読み直したページの題名で引く。木を読んだら索引の題名に戻る
  calls.length = 0;
  handlers.wiki_read_gui = (a) => ({ path: a.path, abs_path: "/v/" + a.path, kind: "md", title: "New", size: 0, mtime: 0, content: "# New\n" });
  await W.readPage("x/a.md");
  assert.equal(W.wikiTabTitle({ path: "x/a.md", scrollTop: 0, editing: false }), "New", "読み直したページの題名で引かない");
  assert.equal(calls.filter((c) => c.name === "wiki_list").length, 0, "題名のために木を読んだ");
  handlers.wiki_read_gui = (a) => ({ path: a.path, abs_path: "/v/" + a.path, kind: "md", title: "", size: 0, mtime: 0, content: "" });
  await W.readPage("b.md");
  assert.equal(W.wikiTabTitle({ path: "b.md", scrollTop: 0, editing: false }), "b", "題名が空になったページはファイル名");
  handlers.wiki_read_ref = (a) => ({ path: a.abs, abs_path: a.abs, kind: "md", title: "Ref", size: 0, mtime: 0, content: "", external: "/ref" });
  W.wk.refDirs = ["/ref"];
  await W.readPage("/ref/r.md");
  assert.equal(W.wk.titleOverrides.has("/ref/r.md"), false, "参照フォルダのファイルの題名を覚えた");
  W.wk.refDirs = [];
  handlers.wiki_list = () => ({ total: pages.length, count: pages.length, pages: pages.map((p) => ({ ...p })) });
  await W.loadTree();
  assert.equal(W.wikiTabTitle({ path: "x/a.md", scrollTop: 0, editing: false }), "A", "木を読んでも覚えた題名が残る");
  assert.equal(W.wk.titleOverrides.size, 0);
  const p1 = C.pageIndex(pages);
  assert.equal(C.pageIndex(pages), p1, "同じ配列で名前の索引を作り直した");
  C.chatLinks("[[a]] と [[x/a.md]] と [[b]]", pages);
  assert.equal(C.pageIndex(pages), p1, "chatLinks が索引を作り直した");
  assert.notEqual(C.pageIndex([...pages]), p1, "別の配列で古い索引を返した");
  assert.deepEqual(
    C.chatLinks("[[a]] [[b]] [[none]]", pages).map((l) => l.path),
    ["x/a.md", "b.md", null],
    "索引の再利用で解決が変わった"
  );
  // results / tags / inbox / chatLog も raw（置き換えで派生が組み直る = 置いた配列そのもの）
  for (const k of ["results", "tags", "inbox", "chatLog"]) {
    const v = [{ path: `${k}.md`, tag: k, count: 1 }];
    W.wk[k] = v;
    assert.equal(W.wk[k], v, `wk.${k} が raw でない`);
    W.wk[k] = [];
  }
  W.wk.pages = [];
  tests++;
}

// ---------- 見出しへの着地（`[[p#見出し]]` → jumpToHeading）は描画の headingKey と同じ規則（埋め込みの節・同じページの #… と同じ組） ----------
{
  const heads = [
    { level: 1, text: "表題", id: "h-表題" },
    { level: 2, text: "A: B", id: "h-A:-B" },
    { level: 2, text: "所見 - CT", id: "h-所見---CT" },
  ];
  W.setHeadings(heads);
  const land = (x) => (W.jumpToHeading(x) ? W.wk.scrollTo.id : null);
  assert.equal(land("A B"), "h-A:-B", "記号を空白に");
  assert.equal(land("a-b"), "h-A:-B", "`-` を空白に（同じページの #a-b と同じ）");
  assert.equal(land("所見 ct"), "h-所見---CT");
  assert.equal(land("h-所見---CT"), "h-所見---CT", "id の完全一致");
  assert.equal(land("Ａ B"), null, "全角は畳まない");
  W.setHeadings([]);
  W.wk.scrollTo = null;
  tests++;
}

// ---- initWiki / initChat: 解除が listen の完了より先でも off を取りこぼさない（解除の後に届いた off はその場で呼ぶ）----
{
  const total = () => Object.values(listeners).reduce((n, a) => n + a.length, 0);
  for (const [name, init] of [["initWiki", W.initWiki], ["initChat", CH.initChat]]) {
    const before = total();
    const off = init();
    assert.ok(total() > before, `${name}: listen を登録していない`);
    off(); // listen の Promise が解ける前（同じ tick）に解除
    await sleep(10);
    assert.equal(total(), before, `${name}: listen の完了前に解除すると受け口が残る`);
  }
  tests++;
}

stop();
stopViewing();
console.log(`wiki chat: ok (${tests} tests)`);
process.exit(0);
