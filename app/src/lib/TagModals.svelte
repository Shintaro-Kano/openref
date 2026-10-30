<script lang="ts">
  import { t, tn } from "./i18n.svelte";
  /* ラベルのモーダル（Paperpile 準拠）。
     New label = 色ボタン（5×5 の見本）+ 名前。Manage labels = 検索・件数・[New label][Color ▾][Merge][Find and replace][Delete]
     + 表（☐ / 色 / 名前 ↓ / 件数 / 🗑）。名前クリックで改名・色クリックで個別の色変更。
     表の状態と操作は Manage（manage.svelte.ts）・表と Find and replace の欄は ManageTable / FindReplacePanel（FolderModals と共用）。
     ここに残るのはラベル固有（色・統合・New label）とモーダルの開閉・Esc の順。
     class は nl- / ml- 接頭辞（app.css は単一グローバル）。Esc は内側（色見本・入力中の欄）から順に閉じる。
     IME 変換中の Enter / Esc は拾わない・開いたらモーダルの中へフォーカスを移す（FolderModals と同じ。理由はそちらの冒頭） */
  import { untrack } from "svelte";
  import { api, type TagRow } from "./api";
  import FindReplacePanel from "./FindReplacePanel.svelte";
  import { composing, focusMe, onEnter } from "./input";
  import LabelBadge from "./LabelBadge.svelte";
  import LabelPalette from "./LabelPalette.svelte";
  import { DEFAULT_LABEL_COLOR, isSolid, labelVars } from "./labels";
  import { Manage, type Panel } from "./manage.svelte";
  import ManageTable from "./ManageTable.svelte";
  import { sameName } from "./folders";
  import { byName } from "./sort";
  import {
    app,
    createLabel,
    deleteLabels,
    findReplaceLabels,
    labelNameTaken,
    mergeLabels,
    recolorLabels,
    renameLabel,
  } from "./state.svelte";

  // ---------- New label ----------
  let name = $state("");
  let color = $state<string>(DEFAULT_LABEL_COLOR);
  let palOpen = $state(false);
  let nameEl = $state<HTMLInputElement | null>(null);
  const taken = $derived(labelNameTaken(name));

  // ---------- Manage labels ----------
  const m = new Manage<TagRow>({
    text: {
      empty: () => t("ラベルはまだありません。「＋ New label」で作成できます。"),
      noMatch: (q) => `${t("「::no-label")}${q}${t("」に一致するラベルなし")}`,
      remove: () => t("このラベルを削除（論文は消えません）"),
      noFind: () => t("一致するラベルなし"),
    },
    all: () => app.tags,
    rows: (k, asc) => {
      const dir = asc ? 1 : -1;
      return app.tags
        .slice()
        .sort((a, b) => (k === "count" ? (a.nItems - b.nItems) * dir || byName(a, b) : byName(a, b) * dir))
        .map((t) => ({ item: t, depth: 0 }));
    },
    nameError: (n, row) => (labelNameTaken(n, row.id) ? t("同名あり") : null),
    rename: renameLabel,
    remove: deleteLabels,
    dryRun: (f, r) => api.findReplaceTags(f, r, true),
    replace: findReplaceLabels,
  });
  let qEl = $state<HTMLInputElement | null>(null);
  let newColor = $state<string>(DEFAULT_LABEL_COLOR);
  let newPal = $state(false);
  let mergeTarget = $state<number | null>(null);
  let rowPal = $state<number | null>(null);

  let prevModal: typeof app.modal = null;
  $effect(() => {
    const mo = app.modal;
    untrack(() => {
      if (mo === "newLabel" && prevModal !== "newLabel") {
        name = "";
        color = DEFAULT_LABEL_COLOR;
        palOpen = false;
        queueMicrotask(() => nameEl?.focus());
      }
      if (mo === "manageLabels" && prevModal !== "manageLabels") {
        m.reset();
        rowPal = null;
        queueMicrotask(() => qEl?.focus());
      }
    });
    prevModal = mo;
  });
  // 色見本の行が一覧から消えたら（検索で隠れた・削除・統合）閉じる（見えないもので Esc が空振りしない）
  $effect(() => {
    if (rowPal !== null && !m.rows.some((r) => r.item.id === rowPal)) rowPal = null;
  });

  function close() {
    app.modal = null;
  }

  async function create() {
    if (!name.trim() || taken) return;
    const id = await createLabel(name, color);
    if (id !== null) close();
  }

  /** 統合先 = 選んだラジオ。それが選択から外れたら選択中の先頭（表示と実行が常に一致し、外したラベルへは統合しない） */
  const mergeTo = $derived(m.selected.some((t) => t.id === mergeTarget) ? mergeTarget : (m.selected[0]?.id ?? null));
  const mergeOk = $derived(m.selected.length >= 2 && mergeTo !== null);

  /** Manage labels を閉じる（×・背景のクリック）。行内改名の途中なら先に確定する */
  function closeManage() {
    m.flushRename();
    close();
  }

  function openPanel(p: Panel) {
    rowPal = null;
    m.openPanel(p);
    if (m.panel === "new") {
      newColor = DEFAULT_LABEL_COLOR;
      newPal = false;
    } else if (m.panel === "merge") mergeTarget = m.selected[0]?.id ?? null;
  }

  /** 確定中の改名の名前と同じか（まだ一覧に無い名前。同名で作ると core の create_tag は既存として色だけ書き換えて返す） */
  const pendingName = (n: string) => [...m.sending.values()].some((s) => sameName(s, n));
  /** ＋ New label 欄の名前が既存のラベル・確定中の改名と同じか */
  const inlineTaken = $derived(labelNameTaken(m.newName) || pendingName(m.newName));
  async function createInline() {
    m.flushRename();
    if (!m.newName.trim() || inlineTaken) return;
    const id = await createLabel(m.newName, newColor);
    if (id !== null) {
      m.newName = "";
      newPal = false;
      m.panel = null;
    }
  }

  async function doMerge() {
    m.flushRename();
    const target = mergeTo;
    if (!mergeOk || target === null) return;
    await m.run(() =>
      mergeLabels(
        m.selected.map((t) => t.id),
        target
      )
    );
    m.sel = new Set([target]);
    m.pruneSel();
    m.panel = null;
  }

  function startRename(t: TagRow) {
    rowPal = null;
    m.startRename(t);
  }

  function toggleRowPal(t: TagRow) {
    m.flushRename();
    rowPal = rowPal === t.id ? null : t.id;
  }

  /** Esc（App の Esc 連鎖は app.modal 中は何もしない = App.svelte。PdfViewer は capture 段で控えた coveredAtKey で見送る） */
  function onKey(e: KeyboardEvent) {
    if (e.key !== "Escape" || composing(e)) return;
    if (app.modal === "newLabel") {
      if (palOpen) palOpen = false;
      else close();
    } else if (app.modal === "manageLabels") {
      if (rowPal !== null) rowPal = null;
      else if (m.renaming !== null) m.renaming = null;
      else if (m.confirmRow !== null) m.confirmRow = null;
      else if (newPal) newPal = false;
      else if (m.panel) m.panel = null;
      else close();
    }
  }

  const why = $derived({
    conflict: t("同名のラベルがあるため変えません（まとめるなら Merge）"),
    empty: t("空の名前になるため変えません"),
  });
</script>

<svelte:window onkeydown={onKey} />

{#snippet tagIcon()}
  <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"
    ><path
      d="M2 2.8C2 2.36 2.36 2 2.8 2h4.6c.2 0 .4.08.55.23l5.8 5.8c.33.33.33.85 0 1.18l-4.17 4.17a.83.83 0 0 1-1.18 0l-5.8-5.8A.78.78 0 0 1 2 7V2.8Z"
      fill="none"
      stroke="currentColor"
      stroke-width="1.4"
      stroke-linejoin="round" /><circle cx="5.2" cy="5.2" r="1.1" fill="currentColor" /></svg>
{/snippet}

<!-- 色ボタン + パレット（New label・Manage labels の ＋ New label 欄）。value = 今の色・open = パレットの開閉 -->
{#snippet colorPicker(value: string, open: boolean, toggle: () => void, pick: (c: string) => void)}
  <span class="nl-colorwrap">
    <button
      class="nl-color lbadge"
      class:solid={isSolid(value)}
      style={labelVars(value)}
      title={t("色を選ぶ")}
      aria-label={t("色を選ぶ")}
      onclick={(e) => {
        e.stopPropagation();
        toggle();
      }}>{@render tagIcon()}</button>
    {#if open}
      <div class="nl-pal">
        <LabelPalette {value} onpick={pick} />
      </div>
    {/if}
  </span>
{/snippet}

{#if app.modal === "newLabel"}
  <div class="overlay" onclick={(e) => e.target === e.currentTarget && close()} role="presentation">
    <div class="modal nl-modal" onclick={() => (palOpen = false)} role="dialog" aria-modal="true" aria-label="New label">
      <div class="modal-head">
        <h3>New label</h3>
        <button class="x" onclick={close}>×</button>
      </div>
      <div class="modal-body nl-body">
        <div class="nl-row">
          {@render colorPicker(color, palOpen, () => (palOpen = !palOpen), (c) => {
            color = c;
            palOpen = false;
            nameEl?.focus();
          })}
          <input
            bind:this={nameEl}
            class="nl-name"
            placeholder="Label name"
            bind:value={name}
            onkeydown={(e) => onEnter(e, create)} />
        </div>
        <div class="nl-hint">
          {#if taken}
            <span class="nl-err">{t("同名のラベル「")}{name.trim()}{t("」が既にあります")}</span>
          {:else if name.trim()}
            <span class="dim">{t("表示:")}</span>
            <LabelBadge name={name.trim()} {color} />
          {/if}
        </div>
      </div>
      <div class="modal-foot">
        <button onclick={close}>Cancel</button>
        <button class="primary" disabled={!name.trim() || taken} onclick={create}>Create label</button>
      </div>
    </div>
  </div>
{/if}

{#if app.modal === "manageLabels"}
  <div class="overlay" onclick={(e) => e.target === e.currentTarget && closeManage()} role="presentation">
    <div
      class="modal ml-modal"
      onclick={() => {
        rowPal = null;
        newPal = false;
      }}
      role="dialog"
      aria-modal="true"
      aria-label="Manage labels">
      <div class="modal-head">
        <h3>Manage labels</h3>
        <button class="x" onclick={closeManage}>×</button>
      </div>
      <div class="ml-top">
        <input class="ml-q" placeholder="Type to search" bind:this={qEl} bind:value={m.q} />
        <span class="ml-total">{app.tags.length} label{app.tags.length === 1 ? "" : "s"} total{#if m.sel.size}<b> · {m.sel.size} {t("選択")}</b>{/if}</span>
      </div>
      <div class="ml-tools">
        <button class="ml-btn primary" onclick={() => openPanel("new")}>{t("＋ New label")}</button>
        <button class="ml-btn" class:on={m.panel === "color"} disabled={m.sel.size === 0} onclick={() => openPanel("color")}>
          Color <span class="caret">▾</span>
        </button>
        <button class="ml-btn" class:on={m.panel === "merge"} disabled={m.sel.size < 2} title={t("選択した 2 個以上を 1 つにまとめる")} onclick={() => openPanel("merge")}>Merge</button>
        <button class="ml-btn" class:on={m.panel === "find"} title={t("ラベル名の検索置換")} onclick={() => openPanel("find")}>Find and replace</button>
        <button class="ml-btn danger" class:on={m.panel === "delete"} disabled={m.sel.size === 0} onclick={() => openPanel("delete")}>Delete</button>
      </div>

      {#if m.panel}
        <div class="ml-panel">
          {#if m.panel === "new"}
            <div class="nl-row">
              {@render colorPicker(newColor, newPal, () => (newPal = !newPal), (c) => { newColor = c; newPal = false; })}
              <input
                class="nl-name"
                placeholder="Label name"
                use:focusMe
                bind:value={m.newName}
                onkeydown={(e) => onEnter(e, createInline)} />
              <button class="ml-btn primary" disabled={!m.newName.trim() || inlineTaken} onclick={createInline}>Create label</button>
            </div>
            {#if inlineTaken}<div class="nl-err">{t("同名のラベルが既にあります")}</div>{/if}
          {:else if m.panel === "color"}
            <div class="ml-ptitle">{t("選択した")} {tn(m.sel.size, "個の色を変える")}</div>
            <LabelPalette
              onpick={async (c) => {
                m.flushRename();
                await recolorLabels([...m.sel], c);
                m.panel = null;
              }} />
          {:else if m.panel === "merge"}
            <div class="ml-ptitle">
              {#if m.selected.length < 2}
                {t("まとめるラベルを 2 個以上チェックしてください")}
              {:else}
                {t("選択した")} {tn(m.selected.length, "個を 1 つにまとめます（付いていた論文はすべて残すラベルへ）")}
              {/if}
            </div>
            <div class="ml-merge">
              <span class="dim">{t("残すラベル")}</span>
              {#each m.selected as t (t.id)}
                <label class="ml-radio">
                  <input type="radio" name="ml-merge" checked={mergeTo === t.id} onchange={() => (mergeTarget = t.id)} />
                  <LabelBadge name={t.name} color={t.color} /><span class="dim">{t.nItems}</span>
                </label>
              {/each}
            </div>
            <div class="ml-pfoot">
              <button class="ml-btn" onclick={() => (m.panel = null)}>Cancel</button>
              <button class="ml-btn primary" disabled={m.busy || !mergeOk} onclick={doMerge}>Merge</button>
            </div>
          {:else if m.panel === "find"}
            <FindReplacePanel {m} {why} />
          {:else if m.panel === "delete"}
            <div class="ml-ptitle">
              {t("選択した")} {tn(m.sel.size, "個のラベルを削除します。")}<b>{t("論文は消えません")}</b>{t("（ラベルが外れるだけ）。")}
            </div>
            {#if m.hiddenSel}
              <div class="nl-err">{t("検索で隠れている選択")} {tn(m.hiddenSel, "個を含みます")}</div>
            {/if}
            <div class="ml-pfoot">
              <button class="ml-btn" onclick={() => (m.panel = null)}>Cancel</button>
              <button class="ml-btn danger-solid" disabled={m.busy} onclick={() => m.doDelete([...m.sel])}>Delete</button>
            </div>
          {/if}
        </div>
      {/if}

      <ManageTable {m}>
        {#snippet colorCell(tag)}
          <button
            class="nl-color lbadge small"
            class:solid={isSolid(tag.color)}
            style={labelVars(tag.color)}
            title={t("色を変える")}
            aria-label={t("色を変える")}
            onclick={(e) => {
              e.stopPropagation();
              toggleRowPal(tag);
            }}>{@render tagIcon()}</button>
          {#if rowPal === tag.id}
            <div class="nl-pal ml-rowpal">
              <LabelPalette
                value={tag.color}
                onpick={async (c) => {
                  rowPal = null;
                  await recolorLabels([tag.id], c);
                }} />
            </div>
          {/if}
        {/snippet}
        {#snippet nameCell(tag)}
          <button class="ml-namebtn" title={t("クリックで名前を変更")} onclick={() => startRename(tag)}>
            <LabelBadge name={m.sending.get(tag.id) ?? tag.name} color={tag.color} />
          </button>
        {/snippet}
      </ManageTable>
    </div>
  </div>
{/if}
