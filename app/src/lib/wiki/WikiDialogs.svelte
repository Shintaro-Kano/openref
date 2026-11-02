<script lang="ts">
  /* OpenNote の小さなダイアログ: 新規ページの名前と形式（md / html）/ 新規フォルダの名前 / 名前の変更（[[旧名]] の書換えの確認）/ ゴミ箱へ。
     木の右クリック・ツールバーの「…」・ヘッダーの [＋ 新規] のメニューもここ（wk.menu）。状態は wiki.svelte.ts・動詞は files.svelte.ts（参照フォルダの追加は refdirs.svelte.ts） */
  import { t } from "../i18n.svelte";
  import { composing, focusMe, onEnter } from "../input";
  import { answerDiscard, openWikiPage, pickRoot, wk } from "./wiki.svelte";
  import { askNewFolder, askNewPage, askRename, askTrash, createFolder, createPage, doRename, doTrash, editingUnder, folderNameProblem, nameProblem, pageExt, pagesUnder, previewRename, type PageExt } from "./files.svelte";
  import { addRefDir } from "./refdirs.svelte";
  import { editText } from "./edit.svelte";
  import { baseName } from "./render/asset";
  import { toast } from "../state.svelte";

  let name = $state("");
  /** 新規ページの形式（ダイアログを開くたびに md に戻す） */
  let ext = $state<PageExt>("md");
  /** 新規ページの名前の欄（形式のラジオを押した後に戻す = WKWebView はクリックでフォーカスを移さない・Enter で作成できるように） */
  let nameInput: HTMLInputElement | undefined = $state();
  let busy = $state(false);
  let copied = $state(false);
  let seen: unknown = null;

  // ダイアログが変わったら入力欄を初期化（改名は今の名前・拡張子を除く）
  $effect(() => {
    const d = wk.dialog;
    if (d === seen) return;
    const prev = seen as typeof d;
    seen = d;
    copied = false;
    if (!d) return;
    if ((d.kind === "new" || d.kind === "newdir") && prev?.kind !== d.kind) {
      name = "";
      ext = "md";
    }
    if (d.kind === "rename" && !(prev?.kind === "rename" && prev.path === d.path)) name = baseName(d.path).replace(/\.(md|html?)$/i, "");
  });

  const stem = (p: string) => baseName(p).replace(/\.[^.]+$/, "");

  function close() {
    if (busy) return;
    if (wk.dialog?.kind === "discard") answerDiscard(false);
    else wk.dialog = null;
  }

  async function copyText() {
    try {
      await navigator.clipboard.writeText(editText());
      copied = true;
    } catch (e) {
      copied = false;
      toast(`✗ ${t("コピーできませんでした")}: ${String(e).slice(0, 160)}`);
    }
  }

  async function run(f: () => Promise<unknown>) {
    if (busy) return;
    busy = true;
    try {
      await f();
    } finally {
      busy = false;
    }
  }

  function onKey(e: KeyboardEvent) {
    if (composing(e)) return;
    if (e.key === "Escape" && (wk.dialog || wk.menu)) {
      e.preventDefault();
      e.stopPropagation();
      if (wk.menu) wk.menu = null;
      else close();
    }
  }

  /** 開いたダイアログの既定のボタンへフォーカス（WKWebView はクリックでフォーカスを移さない・CLAUDE.md §6） */
  function focusBtn(node: HTMLButtonElement) {
    queueMicrotask(() => node.focus());
  }

  function onDocClick(e: MouseEvent) {
    // 左ボタンだけ（右ボタンの mousedown が開いたばかりのメニューを閉じない）。メニューの中と、開いているメニューのボタン
    // （aria-expanded = ヘッダーの [＋ 新規]・vault 行）は除く = ボタンの click が閉じる / 開くを決める（mousedown で閉じて click で開き直さない）
    if (e.button !== 0 || !wk.menu) return;
    const el = e.target as Element | null;
    if (el?.closest?.('.wk-menu, [aria-haspopup="menu"][aria-expanded="true"]')) return;
    wk.menu = null;
  }
</script>

<svelte:window onkeydown={onKey} onmousedown={onDocClick} />

{#if wk.menu}
  {@const m = wk.menu}
  <div class="wk-menu" role="menu" style:left="{m.x}px" style:top="{m.y}px">
    {#if m.kind === "vault"}
      <!-- 長い絶対パスは先頭側を省略して末尾（ワークスペース名）を残す（rtl の型。両端の &lrm; で頭の / や末尾の ) . - _ が反対側へ回らないようにする） -->
      <div class="wk-menu-head wk-menu-head-path" title={m.path}>&lrm;{m.path}&lrm;</div>
      <button role="menuitem" onclick={() => ((wk.menu = null), void openWikiPage(null))}>{t("ホーム（参照フォルダ・ページ情報）")}</button>
      <button role="menuitem" onclick={() => ((wk.menu = null), void pickRoot())}>{t("OpenNote のフォルダを選び直す…")}</button>
      <button role="menuitem" onclick={() => ((wk.menu = null), void addRefDir())}>{t("参照フォルダ（読み取り専用）を追加…")}</button>
    {:else if m.kind === "file"}
      <button role="menuitem" onclick={() => askRename(m.path)}>{t("名前を変更…")}</button>
      <button role="menuitem" class="wk-menu-danger" onclick={() => askTrash(m.path)}>{t("ゴミ箱に移す…")}</button>
    {:else}
      {#if m.kind === "dir"}<div class="wk-menu-head" title={m.path}>{baseName(m.path)}/</div>{/if}
      <button role="menuitem" onclick={() => askNewPage(m.path)}>{t("新規ページ…")}</button>
      <button role="menuitem" onclick={() => askNewFolder(m.path)}>{t("新規フォルダ…")}</button>
      {#if m.kind === "dir"}
        <button role="menuitem" class="wk-menu-danger" onclick={() => askTrash(m.path, true)}>{t("フォルダをゴミ箱に移す…")}</button>
      {/if}
    {/if}
  </div>
{/if}

{#if wk.dialog}
  {@const d = wk.dialog}
  <div class="overlay" onclick={(e) => e.target === e.currentTarget && close()} role="presentation">
    <div class="modal" role="dialog" aria-labelledby="wk-dlg-h">
      {#if d.kind === "new"}
        <div class="modal-head"><h3 id="wk-dlg-h">{t("新規ページ")}</h3></div>
        <div class="modal-body">
          <p class="wk-dlg-note">{t("作る場所")}: {d.dir || t("ワークスペースの直下")}</p>
          <input
            use:focusMe
            bind:this={nameInput}
            bind:value={name}
            placeholder="Untitled"
            spellcheck="false"
            aria-label={t("ページの名前")}
            onkeydown={(e) => onEnter(e, () => run(() => createPage(d.dir, name, ext)))} />
          {#if nameProblem(name)}<p class="wk-dlg-bad">{nameProblem(name)}</p>{/if}
          <div class="wk-dlg-ext" role="radiogroup" aria-label={t("ページの形式")}>
            <label class:wk-dlg-ext-on={pageExt(name, ext) === "md"}>
              <input type="radio" name="wk-new-ext" value="md" bind:group={ext} onchange={() => nameInput?.focus()} /> Markdown (.md)
            </label>
            <label class:wk-dlg-ext-on={pageExt(name, ext) === "html"}>
              <input type="radio" name="wk-new-ext" value="html" bind:group={ext} onchange={() => nameInput?.focus()} /> HTML (.html)
            </label>
          </div>
          <p class="wk-dlg-note">{t("空のままなら Untitled。名前に拡張子（.md / .html）を書けばそれが優先")}</p>
        </div>
        <div class="modal-foot">
          <button onclick={close}>{t("キャンセル")}</button>
          <button class="primary" disabled={busy || !!nameProblem(name)} onclick={() => run(() => createPage(d.dir, name, ext))}>{t("作成")}</button>
        </div>
      {:else if d.kind === "newdir"}
        <div class="modal-head"><h3 id="wk-dlg-h">{t("新規フォルダ")}</h3></div>
        <div class="modal-body">
          <p class="wk-dlg-note">{t("作る場所")}: {d.dir || t("ワークスペースの直下")}</p>
          <input
            use:focusMe
            bind:value={name}
            spellcheck="false"
            aria-label={t("フォルダの名前")}
            onkeydown={(e) => onEnter(e, () => run(() => createFolder(d.dir, name)))} />
          {#if folderNameProblem(name)}<p class="wk-dlg-bad">{folderNameProblem(name)}</p>{/if}
          <p class="wk-dlg-note">{t("空のフォルダは次の起動まで木に残ります（ページを置くと以後も出ます）")}</p>
        </div>
        <div class="modal-foot">
          <button onclick={close}>{t("キャンセル")}</button>
          <button class="primary" disabled={busy || !name.trim() || !!folderNameProblem(name)} onclick={() => run(() => createFolder(d.dir, name))}>{t("作成")}</button>
        </div>
      {:else if d.kind === "rename" && !d.preview}
        <div class="modal-head"><h3 id="wk-dlg-h">{t("名前を変更")}</h3></div>
        <div class="modal-body">
          <p class="wk-dlg-note">{d.path}</p>
          <input
            use:focusMe
            bind:value={name}
            spellcheck="false"
            aria-label={t("新しい名前")}
            onkeydown={(e) => onEnter(e, () => run(() => previewRename(d.path, name)))} />
          {#if nameProblem(name)}<p class="wk-dlg-bad">{nameProblem(name)}</p>{/if}
          <p class="wk-dlg-note">{t("「/」を含めるとワークスペースの中の別のフォルダへ移します")}</p>
        </div>
        <div class="modal-foot">
          <button onclick={close}>{t("キャンセル")}</button>
          <button class="primary" disabled={busy || !name.trim() || !!nameProblem(name)} onclick={() => run(() => previewRename(d.path, name))}>{t("次へ")}</button>
        </div>
      {:else if d.kind === "rename" && d.preview}
        {@const pv = d.preview}
        <div class="modal-head"><h3 id="wk-dlg-h">{t("リンクも書き換えますか")}</h3></div>
        <div class="modal-body">
          <p class="wk-dlg-lead">
            [[{stem(d.path)}]] → [[{stem(d.to)}]] · {t("書き換えるページ")}: {pv.files.length} · {t("書き換えるリンク")}: {pv.links}
          </p>
          <ul class="wk-dlg-list">
            {#each pv.files.slice(0, 12) as f (f)}<li>{f}</li>{/each}
            {#if pv.files.length > 12}<li class="wk-dlg-more">… {pv.files.length - 12}</li>{/if}
          </ul>
          {#if pv.other?.length}
            <p class="wk-dlg-note">{t("書き換えない参照（html のリンク・file:// の絶対リンク・引用符を含む frontmatter の行・チャットのログ）")} ({pv.other.length})</p>
          {/if}
          <p class="wk-dlg-note">{t("書き換える各ページは上書き前の内容を退避します")}</p>
        </div>
        <div class="modal-foot">
          <button onclick={close}>{t("キャンセル")}</button>
          <button disabled={busy} onclick={() => run(() => doRename(d.path, d.to, false))}>{t("書き換えずに変更")}</button>
          <button class="primary" disabled={busy} use:focusBtn onclick={() => run(() => doRename(d.path, d.to, true))}>{t("書き換えて変更")}</button>
        </div>
      {:else if d.kind === "discard"}
        <div class="modal-head"><h3 id="wk-dlg-h">{t("保存できていない変更があります")}</h3></div>
        <div class="modal-body">
          <p class="wk-dlg-lead">{d.path}</p>
          {#if d.error}<p class="wk-dlg-note">{d.error}</p>{/if}
          <p class="wk-dlg-note">{t("保存できるまで自動で再試行しています。移るには変更を破棄してください（先にコピーできます）")}</p>
        </div>
        <div class="modal-foot">
          <button use:focusBtn onclick={close}>{t("キャンセル")}</button>
          <button onclick={() => void copyText()}>{copied ? t("コピーしました") : t("クリップボードにコピー")}</button>
          <button class="danger" onclick={() => answerDiscard(true)}>{t("変更を破棄して移る")}</button>
        </div>
      {:else if d.kind === "trash"}
        <div class="modal-head"><h3 id="wk-dlg-h">{d.dir ? t("フォルダをゴミ箱に移す") : t("ゴミ箱に移す")}</h3></div>
        <div class="modal-body">
          <p class="wk-dlg-lead">{d.path}{d.dir ? "/" : ""}</p>
          {#if d.dir}
            <p class="wk-dlg-note">{t("中のページ")}: {pagesUnder(d.path)}{t("件（画像などのファイルも一緒に移ります）")}</p>
            <p class="wk-dlg-note">{t("フォルダを中身ごと macOS のゴミ箱（ワークスペースの外）へ移します（Finder の「戻す」で戻せます）。中のページへのリンクはそのまま残ります")}</p>
          {:else}
            <p class="wk-dlg-note">{t("macOS のゴミ箱へ移します（Finder の「戻す」で戻せます）。このページへのリンクはそのまま残ります")}</p>
          {/if}
          {#if editingUnder(d.path, d.dir)}<p class="wk-dlg-note wk-dlg-warn">{t("編集中の未保存の変更は失われます（ゴミ箱に入るのは最後に保存した内容）")}</p>{/if}
        </div>
        <div class="modal-foot">
          <button onclick={close}>{t("キャンセル")}</button>
          <button class="danger" disabled={busy} onclick={() => run(() => doTrash(d.path, d.dir))}>{t("ゴミ箱に移す")}</button>
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .wk-menu {
    position: fixed; z-index: 45; min-width: 170px; padding: 4px; border: 1px solid var(--border); border-radius: 8px;
    background: var(--panel); box-shadow: 0 8px 24px rgba(0, 0, 0, 0.18); display: flex; flex-direction: column;
  }
  .wk-menu button { text-align: left; padding: 6px 10px; border-radius: 5px; font-size: 12.5px; }
  .wk-menu button:hover { background: var(--accent-soft); }
  .wk-menu-danger { color: var(--danger); }
  .wk-menu-head { padding: 4px 10px 6px; font-size: 11.5px; color: var(--dim); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 260px; }
  .wk-menu-head-path { direction: rtl; text-align: left; }
  .wk-dlg-ext { display: flex; gap: 14px; font-size: 12.5px; }
  .wk-dlg-ext label { display: inline-flex; align-items: center; gap: 5px; cursor: pointer; color: var(--dim); }
  .wk-dlg-ext label.wk-dlg-ext-on { color: var(--text); }
  .wk-dlg-ext input { margin: 0; }
  .wk-dlg-note { font-size: 11.5px; color: var(--dim); line-height: 1.5; overflow-wrap: anywhere; }
  .wk-dlg-warn { color: var(--danger); }
  .wk-dlg-bad { font-size: 12px; color: var(--danger); }
  .wk-dlg-lead { font-size: 13px; line-height: 1.6; overflow-wrap: anywhere; }
  .wk-dlg-list { margin: 0; padding-left: 18px; font-size: 12px; max-height: 180px; overflow-y: auto; }
  .wk-dlg-list li { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wk-dlg-more { list-style: none; margin-left: -18px; color: var(--dim); }
</style>
