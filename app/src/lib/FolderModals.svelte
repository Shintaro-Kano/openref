<script lang="ts">
  import { t, tn } from "./i18n.svelte";
  /* フォルダのモーダル（Paperpile 準拠）。
     New folder（"Folder name" → Create folder。親ありなら New subfolder）/ Rename folder（現名 → Save）/
     Delete folder（サブフォルダ数と「論文は消えない」を明示・即時）/ Manage folders（検索・総数・[New folder][Find and replace][Delete]
     + 表（☐ / 📁 Name ↕ / Number of references ↕ / 🗑・サブフォルダは字下げ）。名前クリックで改名・🗑 は行内で確認）。
     表の状態と操作は Manage（manage.svelte.ts）・表と Find and replace の欄は ManageTable / FindReplacePanel（TagModals と共用）。
     ここに残るのはフォルダ固有（木・親のパス・サブフォルダ数・New / Rename / Delete の独立ダイアログ）とモーダルの開閉・Esc の順。
     class は nf-（New / Rename / Delete）/ mf-（Manage。表と操作欄は TagModals の ml- を共用）。
     Esc は内側（行内の改名 → 行の確認 → 操作欄）から順に閉じる。IME 変換中の Enter / Esc は拾わない
     （WebKit は確定の Enter を compositionend の後に isComposing=false・keyCode=229 で送る = word/cite-dialog.js と同じ判定）。
     開いたら必ずモーダルの中へフォーカスを移す（WKWebView はボタンのクリックでフォーカスを移さないので、背後のメモ欄・行に残る） */
  import { untrack } from "svelte";
  import { api, type CollectionRow } from "./api";
  import FindReplacePanel from "./FindReplacePanel.svelte";
  import { focusMe, onEnter } from "./input";
  import { Manage } from "./manage.svelte";
  import ManageTable from "./ManageTable.svelte";
  import Modal from "./Modal.svelte";
  import {
    app,
    createFolder,
    deleteFolders,
    descendants,
    findReplaceFolders,
    flatFolders,
    folderDeleteRoots,
    folderNameError,
    folderPath,
    renameFolder,
  } from "./state.svelte";

  // ---------- Manage folders ----------
  const m = new Manage<CollectionRow>({
    text: {
      empty: () => t("フォルダはまだありません。「＋ 新規フォルダ」で作成できます。"),
      noMatch: (q) => `${t("「::no-folder")}${q}${t("」に一致するフォルダなし")}`,
      remove: () => t("このフォルダを削除（文献は消えません）"),
      noFind: () => t("一致するフォルダなし"),
    },
    all: () => app.collections,
    // ソートは兄弟の中で効く（木の形は保つ）。Name ↓ = A→Z、Number ↓ = 多い順
    rows: (k, asc) =>
      flatFolders({ sort: k === "name" ? "alpha" : "count", reverse: k === "name" ? !asc : asc }).map((r) => ({ item: r.c, depth: r.depth })),
    // folderNameError は空に ""（理由なし）を返す → 空は赤枠も文も出さない（確定は commitRename が先に弾く）
    nameError: (n, c) => {
      const err = folderNameError(n, c.parentId, c.id);
      return err ? (n.includes("/") ? t("「/」不可") : t("同名あり")) : null;
    },
    rename: renameFolder,
    remove: deleteFolders,
    dryRun: (f, r) => api.findReplaceCollections(f, r, true),
    replace: findReplaceFolders,
  });
  let qEl = $state<HTMLInputElement | null>(null);
  const searching = $derived(m.q.trim().length > 0);
  /** Delete で実際に消す根（祖先も選ばれている id を除く）と、子孫を含めて消えるフォルダの数 */
  const delRoots = $derived(folderDeleteRoots([...m.sel]));
  const delCount = $derived(delRoots.reduce((n, id) => n + 1 + descendants(id).length, 0));

  // ---------- New folder / Rename folder / Delete folder ----------
  let name = $state("");
  let nameEl = $state<HTMLInputElement | null>(null);
  let deleteEl = $state<HTMLDivElement | null>(null);
  /** Rename / Delete の対象（別の経路で消えていれば null） */
  const target = $derived(m.find(app.folderTarget.id));
  /** New folder の親（root なら null） */
  const parent = $derived(m.find(app.folderTarget.parentId));
  const parentGone = $derived(app.folderTarget.parentId !== null && !parent);
  const newErr = $derived(folderNameError(name, app.folderTarget.parentId));
  const renameErr = $derived(target ? folderNameError(name, target.parentId, target.id) : "");
  const subCount = $derived(target ? descendants(target.id).length : 0);

  let prevModal: typeof app.modal = null;
  $effect(() => {
    const mo = app.modal;
    untrack(() => {
      if (mo === prevModal) return;
      m.busy = false;
      if (mo === "newFolder" || mo === "renameFolder") {
        name = mo === "renameFolder" ? (target?.name ?? "") : "";
        // autofocus は他要素がフォーカス中だと効かない（WKWebView）→ 明示 focus。改名は現名を全選択
        queueMicrotask(() => {
          nameEl?.focus();
          nameEl?.select();
        });
      }
      if (mo === "deleteFolder") queueMicrotask(() => deleteEl?.focus());
      if (mo === "manageFolders") {
        m.reset();
        queueMicrotask(() => qEl?.focus());
      }
    });
    prevModal = mo;
  });

  function close() {
    app.modal = null;
  }

  async function submitNew() {
    if (m.busy || newErr !== null || parentGone) return;
    const id = await m.run(() => createFolder(name, app.folderTarget.parentId));
    if (id !== null) close();
  }

  async function submitRename() {
    const t = target;
    if (m.busy || !t || renameErr !== null) return;
    if (name.trim() === t.name) return close();
    if (await m.run(() => renameFolder(t.id, name))) close();
  }

  /** 先に閉じてから消す（消した後の再読込の間に「見つかりません」を一瞬出さない）。結果は toast */
  function submitDelete() {
    const t = target;
    if (!t) return;
    close();
    void deleteFolders([t.id]);
  }

  /** Manage folders を閉じる（×・背景のクリック）。行内改名の途中なら先に確定する */
  function closeManage() {
    m.flushRename();
    close();
  }

  /** ＋ New folder 欄の名前の誤り（"" = 空・null = 作れる） */
  const inlineErr = $derived(folderNameError(m.newName, null));

  async function createInline() {
    m.flushRename();
    if (m.busy || inlineErr !== null) return;
    const id = await m.run(() => createFolder(m.newName, null));
    if (id !== null) {
      m.newName = "";
      m.panel = null;
    }
  }

  /** Manage folders の Esc（App の Esc 連鎖は app.modal 中は何もしない = App.svelte。PdfViewer は capture 段で控えた coveredAtKey で見送る）。
      IME の変換中の Esc は骨格（Modal）が見送る。New / Rename / Delete の Esc は close */
  function escManage() {
    if (m.renaming !== null) m.renaming = null;
    else if (m.confirmRow !== null) m.confirmRow = null;
    else if (m.panel) m.panel = null;
    else close();
  }

  const why = $derived({
    conflict: t("同じ階層に同名のフォルダがあるため変えません"),
    empty: t("空の名前になるため変えません"),
    invalid: t("「/」を含むため変えません"),
  });
</script>

{#snippet folderIcon()}
  <svg class="mf-ico" viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"
    ><path
      d="M1.8 4.2c0-.66.54-1.2 1.2-1.2h3.1l1.5 1.6h5.4c.66 0 1.2.54 1.2 1.2v6.2c0 .66-.54 1.2-1.2 1.2H3c-.66 0-1.2-.54-1.2-1.2V4.2Z"
      fill="none"
      stroke="currentColor"
      stroke-width="1.3"
      stroke-linejoin="round" /></svg>
{/snippet}

<Modal open={app.modal === "newFolder"} cls="nf-modal" label={parent ? t("新規サブフォルダ") : t("新規フォルダ")} ariaModal tabindex={-1} onbackdrop={close} onescape={close}>
  <div class="modal-head">
    <h3>{app.folderTarget.parentId === null ? t("新規フォルダ") : t("新規サブフォルダ")}</h3>
    <button class="x" onclick={close}>×</button>
  </div>
  <div class="modal-body nf-body">
    {#if parent}
      <div class="nf-sub">{@render folderIcon()}<span>{t("「::create-in")}{parent.name}{t("」の中に作成")}</span></div>
    {:else if parentGone}
      <div class="nf-err">{t("親フォルダが見つかりません（別の経路で削除された可能性があります）")}</div>
    {/if}
    <input
      bind:this={nameEl}
      class="nf-name"
      placeholder={t("フォルダの名前")}
      bind:value={name}
      onkeydown={(e) => onEnter(e, submitNew)} />
    <div class="nf-hint">{#if newErr}<span class="nf-err">{newErr}</span>{/if}</div>
  </div>
  <div class="modal-foot">
    <button onclick={close}>{t("キャンセル")}</button>
    <button class="primary" disabled={m.busy || newErr !== null || parentGone} onclick={submitNew}>{t("フォルダを作成")}</button>
  </div>
</Modal>

<Modal open={app.modal === "renameFolder"} cls="nf-modal" label={t("フォルダの名前を変更")} ariaModal tabindex={-1} onbackdrop={close} onescape={close}>
  <div class="modal-head">
    <h3>{t("フォルダの名前を変更")}</h3>
    <button class="x" onclick={close}>×</button>
  </div>
  <div class="modal-body nf-body">
    {#if !target}
      <div class="nf-err">{t("フォルダが見つかりません（別の経路で削除された可能性があります）")}</div>
    {:else if target.parentId !== null}
      <div class="nf-sub">{@render folderIcon()}<span>{folderPath(target.id)}</span></div>
    {/if}
    <input
      bind:this={nameEl}
      class="nf-name"
      placeholder={t("フォルダの名前")}
      disabled={!target}
      bind:value={name}
      onkeydown={(e) => onEnter(e, submitRename)} />
    <div class="nf-hint">{#if renameErr}<span class="nf-err">{renameErr}</span>{/if}</div>
  </div>
  <div class="modal-foot">
    <button onclick={close}>{t("キャンセル")}</button>
    <button class="primary" disabled={m.busy || !target || renameErr !== null} onclick={submitRename}>{t("保存")}</button>
  </div>
</Modal>

<Modal open={app.modal === "deleteFolder"} cls="nf-modal" label={t("フォルダを削除")} ariaModal tabindex={-1} onbackdrop={close} onescape={close} bind:frame={deleteEl}>
  <div class="modal-head">
    <h3>{t("フォルダを削除")}</h3>
    <button class="x" onclick={close}>×</button>
  </div>
  <div class="modal-body nf-body">
    {#if !target}
      <div class="nf-err">{t("フォルダが見つかりません（別の経路で削除された可能性があります）")}</div>
    {:else}
      <p class="nf-lead">
        {t("「")}<b>{target.name}</b>{t("」を削除します。")}{#if subCount}{t("サブフォルダ")} <b>{tn(subCount, "個")}</b>{t("も一緒に削除されます。")}{/if}
      </p>
      <p class="nf-lead"><b>{t("文献は消えません")}</b>{t("（フォルダから外れるだけ）。この操作は取り消せません。")}</p>
    {/if}
  </div>
  <div class="modal-foot">
    <button onclick={close}>{t("キャンセル")}</button>
    <button class="danger" disabled={m.busy || !target} onclick={submitDelete}>{t("削除")}</button>
  </div>
</Modal>

<Modal open={app.modal === "manageFolders"} cls="ml-modal mf-modal" label={t("フォルダを管理")} ariaModal tabindex={-1} onbackdrop={closeManage} onescape={escManage}>
  <div class="modal-head">
    <h3>{t("フォルダを管理")}</h3>
    <button class="x" onclick={closeManage}>×</button>
  </div>
  <div class="ml-top">
    <input class="ml-q" placeholder={t("入力して検索")} bind:this={qEl} bind:value={m.q} />
    <span class="ml-total">{tn(app.collections.length, "個のフォルダ")}{#if m.sel.size}<b>{" · "}{m.sel.size} {t("選択")}</b>{/if}</span>
  </div>
  <div class="ml-tools">
    <button class="ml-btn primary" class:on={m.panel === "new"} onclick={() => m.openPanel("new")}>{t("＋ 新規フォルダ")}</button>
    <button class="ml-btn" class:on={m.panel === "find"} title={t("フォルダ名の検索置換")} onclick={() => m.openPanel("find")}>{t("検索と置換")}</button>
    <button class="ml-btn danger" class:on={m.panel === "delete"} disabled={m.sel.size === 0} onclick={() => m.openPanel("delete")}>{t("削除")}</button>
  </div>

  {#if m.panel}
    <div class="ml-panel">
      {#if m.panel === "new"}
        <div class="mf-newrow">
          <input
            class="nf-name"
            placeholder={t("フォルダの名前（LIBRARY の直下に作成）")}
            use:focusMe
            bind:value={m.newName}
            onkeydown={(e) => onEnter(e, createInline)} />
          <button class="ml-btn primary" disabled={m.busy || inlineErr !== null} onclick={createInline}>{t("フォルダを作成")}</button>
        </div>
        {#if inlineErr}<div class="nf-err">{inlineErr}</div>{/if}
      {:else if m.panel === "find"}
        <FindReplacePanel {m} {why}>
          {#snippet prefix(p)}
            {@const path = folderPath(p.id)}
            {#if path}<span class="dim">{path} ›</span>{/if}
          {/snippet}
        </FindReplacePanel>
      {:else if m.panel === "delete"}
        <div class="ml-ptitle">
          {t("次の")} {tn(delRoots.length, "個のフォルダを削除します")}{#if delCount > delRoots.length}{t("（サブフォルダを含めて計")} <b>{tn(delCount, "個")}</b>{t("）")}{/if}{t("。")}<b>{t("文献は消えません")}</b>{t("（フォルダから外れるだけ）。")}
        </div>
        <ul class="mf-dellist">
          {#each delRoots as id (id)}
            {@const c = m.find(id)}
            {@const path = folderPath(id)}
            {@const subs = descendants(id).length}
            <li>{#if path}<span class="dim">{path} › </span>{/if}<b>{c?.name}</b>{#if subs}<span class="dim">{t("（サブフォルダ")} {tn(subs, "個も）")}</span>{/if}</li>
          {/each}
        </ul>
        {#if m.hiddenSel}
          <div class="nf-err">{t("検索で隠れている選択")} {tn(m.hiddenSel, "個を含みます")}</div>
        {/if}
        <div class="ml-pfoot">
          <button class="ml-btn" onclick={() => (m.panel = null)}>{t("キャンセル")}</button>
          <button class="ml-btn danger-solid" disabled={m.busy} onclick={() => m.doDelete([...m.sel])}>{t("削除")}</button>
        </div>
      {/if}
    </div>
  {/if}

  <ManageTable {m} nameClass="mf-name">
    {#snippet icon()}{@render folderIcon()}{/snippet}
    {#snippet nameCell(c)}
      {#if searching && c.parentId !== null}<span class="mf-path">{folderPath(c.id)} ›</span>{/if}
      <button class="mf-namebtn" title={t("クリックで名前を変更")} onclick={() => m.startRename(c)}>{m.sending.get(c.id) ?? c.name}</button>
      {#if m.confirmRow === c.id}
        {@const subs = descendants(c.id).length}
        <span class="mf-confirm">{subs ? `${t("サブフォルダ")} ${tn(subs, "個も削除・")}` : ""}{t("文献は残る")}</span>
      {/if}
    {/snippet}
  </ManageTable>
</Modal>
