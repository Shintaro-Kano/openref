<script lang="ts">
  /* OpenNote の右ペインの [チャット]。状態と動詞は chat.svelte.ts、行の解釈は chat.ts。
     上から: 状態と [新しい会話] / アカウント（ログイン・ログアウト）とモデルの選択 / 使えないときの案内 / 会話（発言・応答の逐次・道具の要約・ターンごとの変更の一覧）/ 診断（stderr）/
     文脈のチップ（今のページ、または PDF のタブなら開いている文献・選択範囲 = × で外す）と入力欄（Enter で送信・Shift+Enter で改行・IME の変換中は送らない）。
     右ペイン（WikiRight）と内蔵の PDF viewer の右バー（PdfViewer）の両方から同じ会話を出す。
     スクロールは末尾に追従し、user が上へ戻したら追従しない */
  import { tick } from "svelte";
  import { t } from "../i18n.svelte";
  import { composing } from "../input";
  import { reveal } from "../state.svelte";
  import WikiChatMd from "./WikiChatMd.svelte";
  import { changeView, clipSelection, codePoints, groupByTurn, toolName, type ChatEntry } from "./chat";
  import { authAction, busy, chat, chatHidden, claimFocus, currentSource, floatChat, interrupt, loadChatStatus, newConversation, send, setModel, sourceKey, undoChange, usable } from "./chat.svelte";
  import type { WikiChatChange } from "./types";
  import { openWikiPage, workspaceName } from "./wiki.svelte";

  let list: HTMLDivElement | undefined = $state();
  let input: HTMLTextAreaElement | undefined = $state();
  let follow = true;

  const st = $derived(chat.status);
  const groups = $derived(groupByTurn(chat.conv.entries));
  const isBusy = $derived(busy());
  const ok = $derived(usable(st));
  /** 会話が別のワークスペースの物 = 隠して 1 行（送ると前面の root で新しい会話） */
  const hidden = $derived(chatHidden());
  // 文脈の元 = 前面のタブ（Wiki のページ / PDF のタブの文献）。チップの文 = ページは path・文献は題名（無ければ key）
  const src = $derived(currentSource());
  const srcKey = $derived(sourceKey(src));
  const srcLabel = $derived(!src ? "" : src.kind === "page" ? src.path : src.title || src.key);
  const selText = $derived(chat.sel && chat.sel.path === srcKey ? chat.sel.text : "");
  const selClip = $derived(selText ? clipSelection(selText) : null);

  const stateLabel = $derived(
    st?.state === "starting" ? t("起動中…")
    : st?.state === "busy" ? t("応答中…")
    : st?.state === "stopping" ? t("停止中…")
    : st?.state === "idle" ? t("待機中")
    : t("未起動")
  );

  // タブを押して開いた = 入力欄へ明示 focus（WKWebView は autofocus が効かない）。
  // タブの切替で mount された直後でも拾う（setTab が立てた focusPending を mount の後に読む）。消費は見えている入力欄が focus できたときだけ
  // （隠れた PDF タブの WikiChat が先に消費すると見えている方に来ない = claimFocus）
  $effect(() => {
    if (!chat.focusPending || !input) return;
    const el = input;
    queueMicrotask(() => void claimFocus(el));
  });

  // 会話が伸びた = 追従中なら末尾へ
  $effect(() => {
    void chat.conv;
    void chat.changes;
    if (!follow) return;
    void tick().then(() => {
      if (list && follow) list.scrollTop = list.scrollHeight;
    });
  });

  function onScroll() {
    if (!list) return;
    follow = list.scrollHeight - list.scrollTop - list.clientHeight < 32;
  }

  async function submit() {
    follow = true;
    if (await send()) queueMicrotask(() => input?.focus());
  }

  function onKey(e: KeyboardEvent) {
    if (e.key !== "Enter" || composing(e)) return;
    if (e.shiftKey) return; // 改行
    e.preventDefault();
    if (!isBusy) void submit();
  }

  const lastLine = (s: string) => s.split("\n").filter((l) => l.trim()).pop() ?? "";
  /** モデルの選択肢（`claude --model` の別名）。設定が別の表記（model id 等）ならそれも出す */
  const MODELS = ["fable", "opus", "sonnet", "haiku"];
  const modelLabel = (m: string) => m.charAt(0).toUpperCase() + m.slice(1);
  /** 変えられなかったとき <select> を描き直す鍵（status.model が同じ値のままでも設定の値に戻す） */
  let modelKey = $state(0);
  async function onModel(e: Event) {
    if (!(await setModel((e.currentTarget as HTMLSelectElement).value))) modelKey++;
  }
  /** [ログアウト] は 1 段の確認（子プロセスが止まり、ターミナルの Claude Code からもログアウトされる） */
  let confirmLogout = $state(false);
  async function logout() {
    confirmLogout = false;
    await authAction("logout");
  }
  const accountText = $derived.by(() => {
    const a = st?.auth;
    if (!a) return "";
    const parts = [a.email, a.method, a.subscription].filter((x): x is string => !!x);
    return parts.join(" · ");
  });
  const clipText = (s: string, n: number) => (s.length > n ? `${s.slice(0, n)}…` : s);

  function kindLabel(k: WikiChatChange["kind"]): string {
    return k === "write" ? t("上書き") : k === "create" ? t("新規") : k === "move" ? t("移動") : t("蔵書に登録");
  }

  function openChange(c: WikiChatChange, e: MouseEvent) {
    const v = changeView(c);
    if (!v.open) return;
    if ("page" in v.open) void openWikiPage(v.open.page, e.metaKey);
    else void reveal(v.open.item);
  }

  function noteText(e: Extract<ChatEntry, { k: "note" }>): string {
    if (e.code === "exit") return `${t("Claude Code が終了しました")} (${e.detail}) · ${t("次の送信で起動し直します")}`;
    if (e.code === "send-failed") return `${t("送信できませんでした")}: ${e.detail}`;
    return e.detail;
  }

  function resultText(e: Extract<ChatEntry, { k: "result" }>): string {
    if (e.turn != null && chat.stopped.includes(e.turn)) return t("停止しました");
    return `${t("エラーで終了しました")}${e.subtype ? ` (${e.subtype})` : ""}${e.text ? `: ${clipText(e.text, 600)}` : ""}`;
  }
</script>

<div class="wk-ch">
  <div class="wk-ch-head">
    <span class="wk-ch-dot wk-ch-dot-{st?.state ?? 'off'}" aria-hidden="true"></span>
    <span class="wk-ch-state">{stateLabel}</span>
    <span class="wk-ch-sp"></span>
    {#if chat.confirmNew}
      <span class="wk-ch-ask">{t("今の会話を閉じますか")}</span>
      <button class="wk-ch-btn" onclick={() => (chat.confirmNew = false)}>{t("キャンセル")}</button>
      <button class="wk-ch-btn wk-ch-primary" onclick={() => void newConversation()}>{t("新しい会話")}</button>
    {:else}
      <button
        class="wk-ch-btn"
        disabled={!ok || chat.sending || st?.state === "stopping"}
        title={t("今の会話を閉じて新しい会話を始める（会話記録は Claude Code に残ります）")}
        onclick={() => (chat.conv.entries.length ? (chat.confirmNew = true) : void newConversation())}>{t("新しい会話")}</button>
    {/if}
    {#if !chat.float}
      <button class="wk-ch-btn wk-ch-float" title={t("チャットを浮かせる（アプリの窓の中で動かせます）")} aria-label={t("チャットを浮かせる（アプリの窓の中で動かせます）")} onclick={floatChat}>⧉</button>
    {/if}
  </div>

  {#if st?.enabled && st.cli}
    <div class="wk-ch-acct">
      {#if confirmLogout}
        <span class="wk-ch-ask">{t("Claude Code からログアウトしますか（ターミナルの claude も未ログインになります）")}</span>
        <button class="wk-ch-btn" onclick={() => (confirmLogout = false)}>{t("キャンセル")}</button>
        <button class="wk-ch-btn wk-ch-primary" onclick={() => void logout()}>{t("ログアウト")}</button>
      {:else}
        {#if st.logged_in}
          <span class="wk-ch-acct-in" title={accountText}>{t("ログイン済み")}{accountText ? `: ${accountText}` : ""}</span>
          <button class="wk-ch-btn" disabled={isBusy || chat.sending} title={t("claude auth logout を実行します")} onclick={() => (confirmLogout = true)}>{t("ログアウト")}</button>
        {:else}
          <span class="wk-ch-acct-out">{st.logged_in === false ? t("未ログイン") : t("ログイン状態を確認できません")}</span>
          <button class="wk-ch-btn wk-ch-primary" title={t("ターミナルに claude auth login を開きます")} onclick={() => void authAction("login")}>{t("ログイン")}</button>
        {/if}
        <button class="wk-ch-btn" title={t("ログイン状態を読み直します")} onclick={() => void loadChatStatus({ force: true })}>{t("再確認")}</button>
        {#if st.cli}<span class="wk-ch-dim wk-ch-cli" title={st.cli}>Claude Code {st.version ?? "?"}</span>{/if}
        <span class="wk-ch-sp"></span>
        <label class="wk-ch-model">
          <span>{t("モデル")}</span>
          {#key modelKey}
            <select value={st.model ?? ""} disabled={isBusy || chat.sending} aria-label={t("モデル")} onchange={onModel}>
              <option value="">{t("既定（Claude Code の設定）")}</option>
              {#each MODELS as m (m)}<option value={m}>{modelLabel(m)}</option>{/each}
              {#if st.model && !MODELS.includes(st.model)}<option value={st.model}>{st.model}</option>{/if}
            </select>
          {/key}
        </label>
        {#if chat.model}<span class="wk-ch-dim wk-ch-model-now" title={t("今の会話で動いているモデル")}>{chat.model}</span>{/if}
      {/if}
    </div>
  {/if}

  {#if chat.statusError}
    <p class="wk-ch-err wk-ch-pad">{t("チャットの状態を読めません")}: {chat.statusError}</p>
  {/if}

  {#if st && !ok}
    <div class="wk-ch-guide">
      {#if !st.cli}
        <p>{t("Claude Code（CLI）が見つかりません。")}</p>
        <p class="wk-ch-dim">{t("ターミナルで Claude Code をインストールし、claude を一度起動してログインしてから [再確認] を押してください。OpenRef はログインを代行しません")}</p>
      {:else if st.logged_in === false}
        <p>{t("Claude Code にログインしていません。")}</p>
        <p class="wk-ch-dim">{t("[ログイン] でターミナルに claude auth login を開きます。ブラウザでログインを終えたら [再確認] を押してください")}</p>
      {:else if !st.mcp}
        <p>{t("openref-mcp が見つからないか、古い版です（wiki_write がありません）。")}</p>
        <p class="wk-ch-dim">{t("OpenRef を最新の版に更新してください")}</p>
      {/if}
      {#if st.reason}<p class="wk-ch-dim wk-ch-reason">{st.reason}</p>{/if}
      {#if st.cli}<p class="wk-ch-dim">{st.cli}{st.version ? ` · ${st.version}` : ""}</p>{/if}
      {#if !st.cli}<button class="wk-ch-btn" onclick={() => void loadChatStatus({ force: true })}>{t("再確認")}</button>{/if}
    </div>
  {/if}

  <div class="wk-ch-list" bind:this={list} onscroll={onScroll}>
    {#if hidden}
      <p class="wk-ch-note wk-ch-other" title={chat.root ?? ""}>{t("この会話は {name} のワークスペースのものです。ここで送ると新しい会話になります").replace("{name}", workspaceName(chat.root ?? "") || (chat.root ?? ""))}</p>
    {:else}
    {#if !chat.conv.entries.length && chat.resumed}
      <p class="wk-ch-dim">{t("前の会話の続きです（過去の発言はここには表示されません）")}</p>
    {/if}
    {#if chat.conv.dropped}<p class="wk-ch-note">{t("古い発言を省きました")} ({chat.conv.dropped})</p>{/if}
    {#if chat.notice}<p class="wk-ch-note">{chat.notice}</p>{/if}

    {#each groups as g, gi (g.entries[0].id)}
      {#each g.entries as e (e.id)}
        {#if e.k === "user"}
          <div class="wk-ch-user">
            <div class="wk-ch-user-text">{e.text}</div>
            {#if e.ctx}
              <div class="wk-ch-user-ctx">
                {#if e.ctx.path}<span class="wk-ch-chip wk-ch-chip-ro" title={e.ctx.path}>{e.ctx.path}</span>{/if}
                {#if e.ctx.item}<span class="wk-ch-chip wk-ch-chip-ro wk-ch-chip-item" title={e.ctx.item}>{e.ctx.title || e.ctx.item}</span>{/if}
                {#if e.ctx.selection}<span class="wk-ch-chip wk-ch-chip-ro">{t("選択範囲")} {codePoints(e.ctx.selection)} {t("字")}</span>{/if}
              </div>
            {/if}
          </div>
        {:else if e.k === "text"}
          {#if e.final}
            <WikiChatMd text={e.text} />
          {:else}
            <div class="wk-ch-stream">{e.text}<span class="wk-ch-caret" aria-hidden="true"></span></div>
          {/if}
        {:else if e.k === "tool"}
          <details class="wk-ch-tool" class:wk-ch-tool-err={e.error}>
            <summary>
              <span class="wk-ch-tool-name">{toolName(e.name)}</span>
              <span class="wk-ch-tool-arg" title={e.arg}>{e.arg}</span>
              {#if !e.done}<span class="wk-ch-spin" aria-label={t("実行中")}></span>{/if}
            </summary>
            {#if e.result != null}
              <pre class="wk-ch-tool-out">{e.result || t("（結果なし）")}</pre>
            {:else}
              <p class="wk-ch-dim">{e.done ? t("（結果なし）") : t("実行中")}</p>
            {/if}
          </details>
        {:else if e.k === "result"}
          {#if !e.ok}<p class="wk-ch-note" class:wk-ch-err={!(e.turn != null && chat.stopped.includes(e.turn))}>{resultText(e)}</p>{/if}
        {:else if e.k === "note"}
          <p class="wk-ch-note" class:wk-ch-err={e.tone === "error"}>{noteText(e)}</p>
        {/if}
      {/each}
      {#if g.turn != null && chat.changes[g.turn]?.length}
        {@const turn = g.turn}
        <div class="wk-ch-changes">
          <p class="wk-ch-changes-h">{t("このターンで変わったもの")}</p>
          {#each chat.changes[turn] as c, i (`${turn}:${i}`)}
            {@const v = changeView(c)}
            {@const u = chat.undo[`${turn}:${i}`]}
            <div class="wk-ch-change">
              <span class="wk-ch-kind wk-ch-kind-{c.kind}">{kindLabel(c.kind)}</span>
              <span class="wk-ch-change-label" title={v.label}>{v.label}</span>
              <button class="wk-ch-btn" disabled={!v.open || !!u?.ok} onclick={(ev) => openChange(c, ev)}>{t("開く")}</button>
              <button class="wk-ch-btn" disabled={u?.busy || u?.ok} onclick={() => void undoChange(turn, i)}>
                {u?.ok ? t("戻しました") : u?.busy ? t("戻しています…") : t("元に戻す")}
              </button>
              {#if u && u.ok === false}<p class="wk-ch-err wk-ch-undo-why">{t("戻せません")}: {u.reason}</p>{/if}
            </div>
          {/each}
        </div>
      {/if}
      {#if gi === groups.length - 1 && isBusy && st?.state === "busy" && g.entries[g.entries.length - 1].k === "user"}
        <p class="wk-ch-dim wk-ch-wait"><span class="wk-ch-spin"></span>{t("応答を待っています…")}</p>
      {/if}
    {/each}
    {/if}
  </div>

  {#if chat.stderr.length}
    <details class="wk-ch-diag">
      <summary>{t("診断")} ({chat.stderr.length}) <span class="wk-ch-diag-last">{clipText(lastLine(chat.stderr.join("\n")), 80)}</span></summary>
      <pre>{chat.stderr.join("\n")}</pre>
      <button class="wk-ch-btn" onclick={() => (chat.stderr = [])}>{t("消去")}</button>
    </details>
  {/if}

  <div class="wk-ch-compose">
    {#if chat.error}<p class="wk-ch-err">{chat.error}</p>{/if}
    {#if (srcKey && chat.ctxOff !== srcKey) || selText}
      <div class="wk-ch-ctx" aria-label={t("発言に添える文脈")}>
        {#if src && srcKey && chat.ctxOff !== srcKey}
          {@const off = src.kind === "page" ? t("このページを添えない") : t("この文献を添えない")}
          <span class="wk-ch-chip" class:wk-ch-chip-item={src.kind === "item"} title={src.kind === "page" ? srcLabel : `${srcLabel} (${src.key})`}>
            <span class="wk-ch-chip-text">{srcLabel}</span>
            <button class="wk-ch-x" aria-label={off} title={off} onclick={() => (chat.ctxOff = srcKey)}>×</button>
          </span>
        {/if}
        {#if selText && selClip}
          <span class="wk-ch-chip" title={clipText(selText, 400)}>
            <span class="wk-ch-chip-text">{t("選択範囲")} {selClip.length} {t("字")}{selClip.cut ? ` (${t("先頭だけ")})` : ""}</span>
            <button class="wk-ch-x" aria-label={t("選択範囲を添えない")} title={t("選択範囲を添えない")} onclick={() => (chat.sel = null)}>×</button>
          </span>
        {/if}
      </div>
    {/if}
    <textarea
      bind:this={input}
      bind:value={chat.draft}
      rows="3"
      disabled={!ok}
      placeholder={ok ? t("Claude に頼む（Enter で送信・Shift+Enter で改行）") : ""}
      spellcheck="false"
      aria-label={t("Claude への発言")}
      onkeydown={onKey}></textarea>
    <div class="wk-ch-actions">
      {#if isBusy}
        <button class="wk-ch-btn wk-ch-stop" disabled={st?.state !== "busy"} onclick={() => void interrupt()}>{t("停止")}</button>
      {:else}
        <button class="wk-ch-btn wk-ch-primary" disabled={!ok || chat.sending || !chat.draft.trim()} onclick={() => void submit()}>{t("送信")}</button>
      {/if}
    </div>
  </div>
</div>

<style>
  .wk-ch { display: flex; flex-direction: column; min-height: 0; flex: 1; }
  .wk-ch-head { display: flex; align-items: center; gap: 6px; padding: 6px 12px; border-bottom: 1px solid var(--border); flex-shrink: 0; flex-wrap: wrap; }
  .wk-ch-dot { width: 8px; height: 8px; border-radius: 50%; background: var(--dim); opacity: 0.5; flex-shrink: 0; }
  .wk-ch-dot-idle { background: #3aa76d; opacity: 1; }
  .wk-ch-dot-busy, .wk-ch-dot-starting { background: var(--accent); opacity: 1; animation: wk-ch-pulse 1.2s ease-in-out infinite; }
  .wk-ch-dot-stopping { background: var(--danger); opacity: 0.8; }
  @keyframes wk-ch-pulse { 50% { opacity: 0.35; } }
  .wk-ch-state { font-size: 12px; color: var(--dim); }
  .wk-ch-sp { flex: 1; }
  .wk-ch-ask { font-size: 12px; }
  .wk-ch-btn {
    border: 1px solid var(--border); border-radius: 6px; padding: 2px 8px; background: var(--bg); font-size: 11.5px; cursor: pointer;
    color: var(--text); white-space: nowrap;
  }
  .wk-ch-btn:hover:not(:disabled) { border-color: var(--accent); }
  .wk-ch-btn:disabled { opacity: 0.45; cursor: default; }
  .wk-ch-primary { background: var(--accent); border-color: var(--accent); color: #fff; }
  .wk-ch-stop { color: var(--danger); }
  .wk-ch-float { padding: 1px 7px; font-size: 13px; line-height: 1.2; }
  .wk-ch-pad { padding: 0 12px; }
  .wk-ch-dim { margin: 2px 0; color: var(--dim); font-size: 12px; overflow-wrap: anywhere; }
  .wk-ch-err { margin: 4px 0; color: var(--danger); font-size: 12px; white-space: pre-wrap; overflow-wrap: anywhere; }
  .wk-ch-guide { margin: 10px 12px; padding: 10px; border: 1px solid var(--border); border-radius: 8px; font-size: 12.5px; }
  .wk-ch-guide p { margin: 0 0 6px; }
  .wk-ch-reason { white-space: pre-wrap; }
  .wk-ch-list { flex: 1; min-height: 0; overflow-y: auto; padding: 10px 12px; display: flex; flex-direction: column; gap: 8px; }
  .wk-ch-acct { display: flex; align-items: center; gap: 6px; padding: 5px 12px; border-bottom: 1px solid var(--border); flex-shrink: 0; flex-wrap: wrap; font-size: 11.5px; }
  .wk-ch-acct-in { color: var(--dim); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; max-width: 100%; }
  .wk-ch-acct-out { color: var(--danger); }
  .wk-ch-model { display: inline-flex; align-items: center; gap: 4px; color: var(--dim); }
  .wk-ch-model select {
    font: inherit; font-size: 11.5px; padding: 1px 4px; border: 1px solid var(--border); border-radius: 5px; background: var(--bg); color: var(--text);
  }
  .wk-ch-model-now { font-family: ui-monospace, "SF Mono", Menlo, monospace; font-size: 11px; }
  .wk-ch-user { align-self: flex-end; max-width: 92%; background: var(--accent-soft); border-radius: 10px 10px 2px 10px; padding: 6px 10px; }
  .wk-ch-user-text { white-space: pre-wrap; overflow-wrap: anywhere; font-size: 13px; }
  .wk-ch-user-ctx { display: flex; flex-wrap: wrap; gap: 4px; margin-top: 4px; }
  .wk-ch-stream { white-space: pre-wrap; overflow-wrap: anywhere; font-size: 13px; line-height: 1.6; }
  .wk-ch-caret { display: inline-block; width: 6px; height: 12px; margin-left: 2px; background: var(--dim); vertical-align: -1px; animation: wk-ch-pulse 1s steps(2) infinite; }
  .wk-ch-tool { border: 1px solid var(--border); border-radius: 6px; font-size: 12px; }
  .wk-ch-tool summary { display: flex; align-items: center; gap: 6px; padding: 3px 8px; cursor: pointer; min-width: 0; }
  .wk-ch-tool-name { font-family: ui-monospace, "SF Mono", Menlo, monospace; font-weight: 600; flex-shrink: 0; }
  .wk-ch-tool-arg { color: var(--dim); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; flex: 1; }
  .wk-ch-tool-err .wk-ch-tool-name { color: var(--danger); }
  .wk-ch-tool-out {
    margin: 0; padding: 6px 8px; border-top: 1px solid var(--border); max-height: 220px; overflow: auto; font-size: 11px;
    white-space: pre-wrap; overflow-wrap: anywhere;
  }
  .wk-ch-tool p { padding: 0 8px 4px; }
  .wk-ch-spin {
    display: inline-block; width: 10px; height: 10px; border: 2px solid var(--border); border-top-color: var(--accent); border-radius: 50%;
    animation: wk-ch-rot 0.8s linear infinite; flex-shrink: 0;
  }
  @keyframes wk-ch-rot { to { transform: rotate(360deg); } }
  .wk-ch-wait { display: flex; align-items: center; gap: 6px; }
  .wk-ch-note { margin: 0; font-size: 12px; color: var(--dim); white-space: pre-wrap; overflow-wrap: anywhere; }
  .wk-ch-changes { border: 1px solid var(--border); border-radius: 8px; padding: 6px 8px; background: var(--bg); }
  .wk-ch-changes-h { margin: 0 0 4px; font-size: 11px; font-weight: 700; letter-spacing: 0.3px; color: var(--dim); }
  .wk-ch-change { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; padding: 2px 0; font-size: 12px; }
  .wk-ch-kind { font-size: 10.5px; padding: 0 5px; border-radius: 4px; background: var(--accent-soft); color: var(--accent); flex-shrink: 0; }
  .wk-ch-kind-create { background: transparent; box-shadow: inset 0 0 0 1px var(--accent); }
  .wk-ch-kind-item { color: var(--text); }
  .wk-ch-change-label { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wk-ch-undo-why { flex-basis: 100%; margin: 0 0 2px; }
  .wk-ch-diag { margin: 0 12px 4px; font-size: 11.5px; color: var(--dim); flex-shrink: 0; }
  .wk-ch-diag summary { cursor: pointer; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wk-ch-diag-last { opacity: 0.8; }
  .wk-ch-diag pre { max-height: 160px; overflow: auto; margin: 4px 0; font-size: 11px; white-space: pre-wrap; overflow-wrap: anywhere; }
  .wk-ch-compose { border-top: 1px solid var(--border); padding: 8px 12px 10px; flex-shrink: 0; display: flex; flex-direction: column; gap: 6px; }
  .wk-ch-compose textarea {
    width: 100%; box-sizing: border-box; resize: vertical; min-height: 54px; max-height: 40vh; font: inherit; font-size: 13px;
    padding: 6px 8px; border: 1px solid var(--border); border-radius: 6px; background: var(--bg); color: var(--text);
  }
  .wk-ch-compose textarea:focus { outline: none; border-color: var(--accent); }
  .wk-ch-actions { display: flex; justify-content: flex-end; }
  .wk-ch-ctx { display: flex; flex-wrap: wrap; gap: 4px; }
  .wk-ch-chip {
    display: inline-flex; align-items: center; gap: 2px; max-width: 100%; font-size: 11px; padding: 1px 2px 1px 6px; border-radius: 4px;
    border: 1px solid var(--border); background: var(--bg); color: var(--dim);
  }
  .wk-ch-chip-ro { padding-right: 6px; background: transparent; }
  .wk-ch-chip-item::before { content: "📄"; font-size: 10px; margin-right: 3px; }
  .wk-ch-chip-text { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
  .wk-ch-cli { font-size: 11px; white-space: nowrap; }
  .wk-ch-x { border: 0; background: transparent; color: var(--dim); cursor: pointer; padding: 0 4px; font-size: 12px; line-height: 1; }
  .wk-ch-x:hover { color: var(--text); }
</style>
