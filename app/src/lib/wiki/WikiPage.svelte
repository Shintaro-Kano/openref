<script lang="ts">
  /* OpenNote の中央 = 前面のタブのページ。ツールバー + kind で描画部品（MdView / HtmlView / MediaView）を出し分け、
     部品の遷移の要求（WikiNav）は wiki.svelte.ts の navigate が実行する。ホーム（path = null）は vault の概要だけ。
     読み直し = 前面の path が変わった・索引の変更（wiki-changed）にこのページが入った。スクロール位置はタブ（TabWiki.scrollTop）に持つ。
     編集: [表示 / 編集 / 分割]（⌘E = 表示 ⇄ 編集・⌘S = 保存）。編集はエディタ（WikiEditor = CodeMirror の動的 import）、
     分割は左にエディタ・右にプレビュー（入力中の本文を debounce で描き直す・スクロール同期なし）。保存の規則は edit.svelte.ts。
     ワークスペース: ページはそのタブの root で読む（root と path の組が変われば読み直す）・設定を前面の root に揃え終えてから読む
     （rootReady = wiki_front_root が asset scope に足し終えた = 初めて前面にした root の画像が 403 のまま残らない）・編集はその root に固定 */
  import { onDestroy, tick, untrack } from "svelte";
  import { t } from "../i18n.svelte";
  import { composing } from "../input";
  import { app, frontWiki } from "../state.svelte";
  import HtmlView from "./HtmlView.svelte";
  import MdView from "./MdView.svelte";
  import MediaView from "./MediaView.svelte";
  import WikiResults from "./WikiResults.svelte";
  import WikiEditor from "./WikiEditor.svelte";
  import WikiConflict from "./WikiConflict.svelte";
  import WikiDialogs from "./WikiDialogs.svelte";
  import type { EditorHandle } from "./editor/cm";
  import type { WikiLink, WikiNav, WikiPageData } from "./types";
  import { absOf, baseName, refViewable, relInRoot } from "./render/asset";
  import {
    hasQuery,
    navigate,
    noteTitle,
    openPath,
    readPage,
    requestFind,
    rootReady,
    setEditing,
    setHeadings,
    toggleHeadColors,
    touched,
    vaultName,
    wk,
  } from "./wiki.svelte";
  import { clearSearch } from "./search.svelte";
  import { ed, editable, flush, refreshFromDisk, startEdit, stopEdit } from "./edit.svelte";
  import { setChatEditor } from "./selection";

  /** links の無いページの代わり（毎回新しい [] を作ると previewPage が shown の置換のたびに組み直される） */
  const NO_LINKS: WikiLink[] = [];
  const tw = $derived(frontWiki());
  const path = $derived(tw?.path ?? null);
  /** 前面のタブのワークスペース（分からなければ null = 画面の root で読む） */
  const root = $derived(tw?.root ?? null);
  let body = $state<HTMLDivElement | null>(null);
  let seq = 0;

  async function load(p: string | null, r: string | null, keepScroll: boolean) {
    const my = ++seq;
    const jump = wk.scrollTo?.nonce;
    wk.headings = [];
    if (!p) {
      wk.page = null;
      wk.pageError = null;
      return;
    }
    await rootReady(); // 前面の root を揃え終えてから（起動の門・タブの行き来の wiki_front_root）
    if (my !== seq) return;
    try {
      const page = await readPage(p, r);
      if (my !== seq) return;
      wk.page = page;
      wk.pageError = null;
    } catch (e) {
      if (my !== seq) return;
      wk.page = null;
      wk.pageError = String(e);
    }
    // 描画の後にタブの位置へ（遷移で開いた = 0・タブの切替で戻った = 前の位置）。ただし描画の間に見出しへ飛んだ
    // （`[[page#見出し]]` の着地 = onHeadings → jumpToHeading → scrollTo）ならその位置を残す（0 で上書きしない）
    await tick();
    if (wk.scrollTo?.nonce !== jump) return;
    const want = frontWiki();
    if (body && want && want.path === p) body.scrollTop = keepScroll ? want.scrollTop : 0;
  }

  // 前面の path か root が変わった（タブの切替・遷移・明示の切替）
  // load の同期部分が読む状態（wk.scrollTo 等）を依存にしない = path / root が変わったときだけ（untrack）
  // path が変わった = 前のページの見出しへの scrollTo を消す（タブの切替で戻った html ページが前のページの id へ着地しない。
  // `[[page#見出し]]` の着地は読込の後の onHeadings → jumpToHeading が新しく立てる）
  $effect(() => {
    const p = path;
    const r = root;
    untrack(() => {
      wk.scrollTo = null;
      void load(p, r, true);
    });
  });

  // 索引が変わった = このページが入っていれば読み直す（位置は保つ）。編集中は本文を edit.svelte の規則で
  // （未保存が無ければ黙って読み直す・あれば触らない = 次の保存が conflict）・題名やリンクだけ wk.page に反映
  let seenStamp = wk.stamp;
  $effect(() => {
    const s = wk.stamp;
    if (s === seenStamp) return;
    seenStamp = s;
    const p = frontWiki()?.path ?? null;
    if (!p || !touched(p)) return;
    untrack(() => {
      const r = frontWiki()?.root ?? null;
      if (ed.path === p && ed.root === r) void refreshFromDisk().then((pg) => pg && ed.path === p && adopt(pg));
      else void load(p, r, true);
    });
  });

  function onScroll() {
    const w = frontWiki();
    if (w && body && wk.page && w.path === wk.page.path) w.scrollTop = body.scrollTop;
  }

  const searching = $derived(wk.showResults && hasQuery());

  function onKey(e: KeyboardEvent) {
    if (!frontWiki()) return;
    const cmd = e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey;
    // ⌘S = 保存（編集していなければ何もしない。既定の動作も止める）
    if (cmd && e.key.toLowerCase() === "s") {
      e.preventDefault();
      if (!e.repeat && ed.path) void flush();
      return;
    }
    if (wk.dialog || ed.dialog || app.modal) return;
    // 結果一覧の Esc = 検索を消して木（ページ）に戻る（検索欄の中の Esc は検索欄が先に扱う）。
    // IME の取消 Esc は入力欄が stopPropagation しない = ここで弾く（App.svelte の Esc と同じ形・欄ごとの手当てに頼らない）
    if (searching && e.key === "Escape" && !composing(e) && !app.editorOpen) {
      clearSearch();
      return;
    }
    if (!wk.page || searching) return;
    // ⌘E = 表示 ⇄ 編集（編集へは前回の配置 = エディタだけ / 分割）
    if (cmd && e.key.toLowerCase() === "e" && canEdit) {
      e.preventDefault();
      if (!e.repeat && !composing(e)) void setEditing(!editing);
      return;
    }
    if (cmd && e.key.toLowerCase() === "f") {
      if (e.defaultPrevented) return; // CodeMirror が自分の検索を開いた
      e.preventDefault();
      if (editing) edHandle?.openSearch(); // 編集中は CodeMirror の検索（置換つき）
      else requestFind();
    }
  }

  const shown = $derived(wk.page && wk.page.path === path ? wk.page : null);
  /** 参照フォルダのファイル: 描画部品には root = そのフォルダ・path = その相対で渡す（相対リンク・CSS・画像の解決がワークスペースと同じ式で効く）。
      部品が返す「root 相対のページ」は絶対パスに戻し、内蔵で開ける種類なら ref・それ以外（md 等）はホストの open へ */
  const external = $derived(shown?.external ?? null);
  const viewPage = $derived(shown && external ? { ...shown, path: relInRoot(external, shown.abs_path) ?? shown.abs_path } : shown);
  function navigateRef(nav: WikiNav, newTab?: boolean) {
    if (nav.kind === "page" && external) {
      const abs = absOf(external, nav.path);
      // 参照フォルダが vault を含むとき（vault の親を参照フォルダにできる）: vault の中のファイルは vault のページとして開く（ref の別タブに二重に出さない）
      const ws = root ?? wk.status?.root;
      const inVault = ws ? relInRoot(ws, abs) : null;
      if (inVault) return navigate({ kind: "page", path: inVault, heading: nav.heading }, newTab);
      return navigate(refViewable(abs) ? { kind: "ref", abs } : { kind: "open", abs }, newTab);
    }
    return navigate(nav, newTab);
  }
  /** 描画部品の props のうち page 以外（page は表示 = viewPage・分割のプレビュー = previewPage を別に渡す）。
      shown の同一性には依存させない（保存のたびに wk.page が置き換わる → spread の props が新しくなると部品の描画 effect が走る）
      = 依存は有無（viewOn）・external・root・参照フォルダ・scrollTo・findRequest・見出しの色だけ */
  const viewOn = $derived(!!shown && !!viewPage);
  const viewBase = $derived(
    viewOn
      ? {
          root: external ?? root ?? wk.status?.root ?? "",
          refs: wk.refDirs,
          onNavigate: external ? navigateRef : navigate,
          readPage,
          onHeadings: setHeadings,
          scrollTo: wk.scrollTo,
          findRequest: wk.findRequest,
          headingColors: wk.headColors,
        }
      : null
  );

  /** 編集中に読み直したページを wk.page に（題名・tags・リンク = 右ペインとプレビューのリンクの解決） */
  function adopt(pg: WikiPageData) {
    noteTitle(pg);
    if (frontWiki()?.path === pg.path) wk.page = pg;
  }

  // ---- 編集 ----
  // 編集中のページは編集を続ける（外部の書換えで読みが途中まで・U+FFFD 入りになっても、エディタの本文を捨てない）
  const canEdit = $derived(!!shown && (ed.path === shown.path || editable(shown)));
  const editing = $derived(!!tw?.editing && canEdit);
  // 編集のセッション = 前面のタブの編集中のページ（タブの切替・ページの移動は先に保存してから = wiki.svelte / state.svelte の leave）
  $effect(() => {
    const p = shown;
    const on = editing;
    const r = root;
    untrack(() => {
      if (on && p) startEdit(p, r); // 編集はこのタブの root に固定（ed.root）
      else if (ed.path && (!p || ed.path !== p.path || ed.root !== r || !on)) stopEdit();
    });
  });
  onDestroy(() => {
    stopEdit();
    setChatEditor(null);
  });
  // 編集を閉じた = チャットの選択の読み取りにエディタを使わない
  $effect(() => {
    if (!editing) untrack(() => setChatEditor(null));
  });

  let edHandle: EditorHandle | null = null;
  // 分割のプレビュー: 保存のたびに wk.page が新しくなっても同じ本文を描き直さない = shown の同一性ではなく path と ed.preview だけから組む
  // （描画部品が finish に渡す root 相対の path と links は viewPage から・本文は ed.preview。保存で変わる sha256 / size / mtime は描画に無関係）
  const previewOn = $derived(!!shown && editing);
  const previewPath = $derived(viewPage?.path ?? null);
  const previewLinks = $derived(shown?.links ?? NO_LINKS);
  const previewPage = $derived.by((): WikiPageData | null => {
    if (!previewOn || previewPath == null) return null;
    const path = previewPath;
    const links = previewLinks;
    const content = ed.preview;
    const base = untrack(() => viewPage);
    return base ? { ...base, path, links, content, truncated: false } : null;
  });
  const saveLabel = $derived(
    ed.state === "saving" ? t("保存中…")
    : ed.state === "saved" ? t("保存済み::wiki")
    : ed.state === "dirty" ? t("未保存")
    : ed.state === "error" ? t("⚠ 保存に失敗")
    : ed.state === "conflict" ? t("⚠ 競合")
    : ""
  );

  function setMode(m: "view" | "edit" | "split") {
    void setEditing(m !== "view", m === "split" ? true : m === "edit" ? false : undefined);
  }

  function openMenu(e: MouseEvent) {
    if (!shown) return;
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    wk.menu = { kind: "file", path: shown.path, x: Math.max(8, r.right - 180), y: r.bottom + 4 };
  }
</script>

<svelte:window onkeydown={onKey} />

<section class="wk-page">
  {#if searching}
    <WikiResults />
  {:else if !path}
    <div class="wk-home">
      <h2>{vaultName() || "OpenNote"}</h2>
      <p class="wk-dim">{wk.status?.root ?? ""}</p>
      {#if wk.status?.pages != null}<p>{t("ページ数")}: {wk.status.pages}</p>{/if}
      {#if wk.status?.inbox}<p>{t("ingest/ の素材")}: {wk.status.inbox}</p>{/if}
      <p class="wk-dim">{t("左の木からページを選ぶか、検索欄で探してください")}</p>
    </div>
  {:else}
    <div class="wk-bar">
      <div class="wk-bar-title" title={path}>
        <span class="wk-bar-name">{shown?.title ?? baseName(path)}</span>
        <span class="wk-bar-path">{path}</span>
      </div>
      {#if editing && saveLabel}
        <button
          class="wk-save wk-save-{ed.state}"
          title={ed.error ?? (ed.state === "conflict" ? t("外部で書き換えられています（⌘S で選ぶ）") : t("⌘S で今すぐ保存"))}
          onclick={() => void flush()}>{saveLabel}</button>
      {/if}
      {#if canEdit}
        <div class="wk-seg" role="group" aria-label={t("表示の切替")}>
          <button class:wk-seg-on={!editing} aria-pressed={!editing} title={t("表示（⌘E で切替）")} onclick={() => setMode("view")}>{t("表示")}</button>
          <button class:wk-seg-on={editing && !wk.split} aria-pressed={editing && !wk.split} title={t("編集（⌘E で切替）")} onclick={() => setMode("edit")}>
            {shown?.kind === "html" ? t("ソース") : t("編集")}
          </button>
          <button class:wk-seg-on={editing && wk.split} aria-pressed={editing && wk.split} title={t("左に編集・右にプレビュー")} onclick={() => setMode("split")}>{t("分割")}</button>
        </div>
      {:else if shown && !external && (shown.kind === "md" || shown.kind === "html")}
        <span class="wk-ro" title={shown.truncated ? t("長いページのため途中までしか読めていません（全体を書き戻せないので編集できません）") : t("UTF-8 でない文字を含むため編集できません（書き戻すと文字が壊れます）")}>{t("編集不可")}</span>
      {/if}
      {#if shown?.kind === "md" && !editing}
        <button
          class="wk-btn wk-hc-btn"
          class:wk-hc-on={wk.headColors}
          aria-pressed={wk.headColors}
          title={t("見出しをレベル（h1〜h6）ごとに色分けする")}
          onclick={toggleHeadColors}
        >
          <span class="wk-hc-swatch" aria-hidden="true"><i></i><i></i><i></i></span>{t("見出しの色")}
        </button>
      {/if}
      {#if external}
        <span class="wk-ro" title={external}>{t("参照フォルダ（読み取り専用）")}</span>
      {/if}
      {#if shown}
        <button class="wk-btn" title={t("既定のアプリで開く")} onclick={() => void openPath(shown.abs_path)}>{t("外部で開く")}</button>
        {#if !external}
          <button class="wk-btn wk-more" title={t("名前の変更・複製・ゴミ箱")} aria-label={t("名前の変更・複製・ゴミ箱")} onclick={openMenu}>⋯</button>
        {/if}
      {/if}
    </div>
    {#if editing && shown && previewPage && viewBase}
      <div class="wk-edit" class:wk-split={wk.split}>
        <div class="wk-edit-pane">
          {#key shown.path}
            <WikiEditor mode={shown.kind === "html" ? "html" : "md"} onReady={(h) => {
              edHandle = h;
              setChatEditor(h);
            }} />
          {/key}
        </div>
        {#if wk.split}
          <div class="wk-preview wk-body">
            <div class="wk-sheet">
              {#if shown.kind === "md"}
                <MdView {...viewBase} page={previewPage} />
              {:else}
                <HtmlView {...viewBase} page={previewPage} />
              {/if}
            </div>
          </div>
        {/if}
      </div>
    {:else}
    <div class="wk-body" bind:this={body} onscroll={onScroll}>
      {#if wk.pageError && !shown}
        <p class="wk-err">{wk.pageError}</p>
      {:else if !shown}
        <p class="wk-dim wk-pad">{t("読込中…")}</p>
      {:else}
        {#if shown.truncated}
          <p class="wk-warn">{t("長いページのため途中までを表示しています")} · {shown.chars} {t("字")}</p>
        {/if}
        {#if viewBase && viewPage && shown.kind === "md"}
          <div class="wk-sheet"><MdView {...viewBase} page={viewPage} /></div>
        {:else if viewBase && viewPage && shown.kind === "html"}
          <div class="wk-sheet"><HtmlView {...viewBase} page={viewPage} /></div>
        {:else if viewBase && viewPage && shown.kind === "media"}
          <MediaView {...viewBase} page={viewPage} />
        {:else}
          <div class="wk-other">
            <p>{t("このファイルは外部のアプリで開きます")}</p>
            <button class="wk-btn" onclick={() => void openPath(shown.abs_path)}>{t("外部で開く")}</button>
          </div>
        {/if}
      {/if}
    </div>
    {/if}
  {/if}
</section>
{#if ed.dialog && ed.path}<WikiConflict onReloaded={adopt} />{/if}
<WikiDialogs />

<style>
  .wk-page { display: flex; flex-direction: column; min-width: 0; min-height: 0; background: var(--panel); }
  .wk-bar {
    display: flex; align-items: center; gap: 8px; padding: 6px 14px; border-bottom: 1px solid var(--border); flex-shrink: 0;
  }
  .wk-bar-title { flex: 1; min-width: 0; display: flex; align-items: baseline; gap: 8px; overflow: hidden; }
  .wk-bar-name { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .wk-bar-path { color: var(--dim); font-size: 11px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .wk-btn {
    border: 1px solid var(--border); border-radius: 6px; padding: 3px 10px; background: var(--bg); font-size: 12px; cursor: pointer;
    color: var(--text); white-space: nowrap;
  }
  .wk-btn:hover { border-color: var(--accent); }
  .wk-btn.wk-hc-btn { display: inline-flex; align-items: center; gap: 5px; color: var(--dim); }
  .wk-btn.wk-hc-on { color: var(--text); }
  .wk-hc-swatch { display: inline-flex; gap: 1px; }
  .wk-hc-swatch i { width: 3px; height: 10px; border-radius: 1px; background: var(--dim); opacity: 0.5; }
  .wk-hc-on .wk-hc-swatch i { opacity: 1; }
  .wk-hc-on .wk-hc-swatch i:nth-child(1) { background: var(--wk-h1); }
  .wk-hc-on .wk-hc-swatch i:nth-child(2) { background: var(--wk-h3); }
  .wk-hc-on .wk-hc-swatch i:nth-child(3) { background: var(--wk-h4); }
  .wk-body { flex: 1; min-height: 0; overflow: auto; container-type: inline-size; }
  /* md / html の外周の余白（中央の幅に応じて縮む = cqi は .wk-body の幅）。MediaView は自前の余白のまま */
  .wk-sheet { padding: clamp(14px, 3cqi, 26px) clamp(16px, 5cqi, 40px) 8px; }
  .wk-home { padding: 32px; }
  .wk-home h2 { margin: 0 0 4px; }
  .wk-dim { color: var(--dim); font-size: 12px; overflow-wrap: anywhere; }
  .wk-pad { padding: 16px 20px; }
  .wk-err { margin: 16px 20px; color: var(--danger); white-space: pre-wrap; overflow-wrap: anywhere; }
  .wk-warn { margin: 8px clamp(16px, 5cqi, 40px) 0; font-size: 12px; color: var(--dim); }
  .wk-other { padding: 32px 20px; display: flex; flex-direction: column; align-items: flex-start; gap: 8px; }
  .wk-btn.wk-more { padding: 3px 8px; letter-spacing: 1px; }
  .wk-seg { display: inline-flex; border: 1px solid var(--border); border-radius: 6px; overflow: hidden; flex-shrink: 0; }
  .wk-seg button { padding: 3px 10px; font-size: 12px; color: var(--dim); background: var(--bg); white-space: nowrap; }
  .wk-seg button + button { border-left: 1px solid var(--border); }
  .wk-seg button.wk-seg-on { color: var(--text); background: var(--accent-soft); }
  .wk-save { font-size: 11.5px; color: var(--dim); white-space: nowrap; padding: 2px 4px; }
  .wk-save-error, .wk-save-conflict { color: var(--danger); }
  .wk-save-dirty { color: var(--text); }
  .wk-ro { font-size: 11.5px; color: var(--dim); white-space: nowrap; }
  .wk-edit { flex: 1; min-height: 0; display: grid; grid-template-columns: minmax(0, 1fr); }
  .wk-edit.wk-split { grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); }
  .wk-edit-pane { min-width: 0; min-height: 0; overflow: hidden; }
  .wk-split .wk-edit-pane { border-right: 1px solid var(--border); }
  .wk-preview { min-width: 0; }
</style>
