<script lang="ts">
  import { t } from "./i18n.svelte";
  import type { CollectionRow } from "./api";
  import LabelBadge from "./LabelBadge.svelte";
  import {
    app,
    beginLabelDrag,
    consumeDragClick,
    flatFolders,
    openFolderModal,
    openTab,
    setFolderSort,
    setTagSort,
    setView,
    toggleFolderCollapsed,
    type View,
  } from "./state.svelte";
  import { composing } from "./input";

  function isActive(kind: string, id?: number): boolean {
    const v = app.view as any;
    if (v.kind !== kind) return false;
    return (kind !== "collection" && kind !== "tag") || v.id === id;
  }

  /** 行のクリック: 素 = 前面のタブのビューを変える / ⌘クリック = 新しいタブで開く。
      主ボタン以外の click（中クリックで click も出す環境）は捨てる（中クリックは goAux が受ける = 二重に動かさない） */
  function go(e: MouseEvent | KeyboardEvent, v: View) {
    if (e instanceof MouseEvent && e.button !== 0) return;
    if (e.metaKey) void openTab(v);
    else void setView(v);
  }
  /** 行の中クリック = 新しいタブで開く */
  function goAux(e: MouseEvent, v: View) {
    if (e.button !== 1) return;
    e.preventDefault();
    void openTab(v);
  }

  // ---- MY LIBRARY のフォルダ（Paperpile: 折畳 chevron + 📁 + 名前 + 件数 + hover の「…」/ 右クリックでメニュー）。
  // 行は <button> にしない（中に chevron と「…」のボタンを置くため。<button> の入れ子は Svelte 5 でコンパイルエラー）
  const folderRows = $derived(flatFolders({ respectCollapse: true }));
  const hasKids = $derived(new Set(app.collections.map((c) => c.parentId)));

  const folderView = (c: CollectionRow): View => ({ kind: "collection", id: c.id, name: c.name });
  function onFolderKey(e: KeyboardEvent, c: CollectionRow) {
    if (e.target !== e.currentTarget) return; // 中の chevron / 「…」での Enter はそのボタンが受ける
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      go(e, folderView(c));
    }
  }
  /** フォルダの「…」メニューの「新しいタブで開く」 */
  function openFolderInTab(id: number) {
    app.folderMenu = null;
    const c = app.collections.find((c) => c.id === id);
    if (c) void openTab(folderView(c));
  }

  /** フォルダ行のメニュー。「…」はボタンの直下、右クリックはポインタの位置に fixed で出す（画面の端では内側へ寄せる） */
  function openFolderMenu(e: MouseEvent, id: number) {
    e.preventDefault();
    e.stopPropagation();
    if (e.type === "click" && app.folderMenu?.id === id) {
      app.folderMenu = null;
      return;
    }
    app.listMenu = null; // ツールバーのメニューと重ねない
    app.labelMenu = false;
    let top = e.clientY;
    let left = e.clientX;
    if (e.type === "click") {
      const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
      top = r.bottom + 4;
      left = r.left - 12;
    }
    app.folderMenu = {
      id,
      top: Math.max(8, Math.min(top, window.innerHeight - 180)),
      left: Math.max(8, Math.min(left, window.innerWidth - 240)),
    };
  }

  // ---- LABELS（Paperpile: 🔍 検索欄の表示切替 / ＋ New label / … Sort labels・Manage labels） ----
  let searchOpen = $state(false);
  let labelQ = $state("");
  let searchEl = $state<HTMLInputElement | null>(null);
  const shownTags = $derived.by(() => {
    const q = labelQ.trim().toLowerCase();
    return q ? app.tags.filter((t) => t.name.toLowerCase().includes(q)) : app.tags;
  });

  function toggleSearch() {
    searchOpen = !searchOpen;
    if (searchOpen) queueMicrotask(() => searchEl?.focus());
    else labelQ = "";
  }

  /** 見出しの「…」メニュー（MY LIBRARY / LABELS 共用）は fixed で「…」ボタンの直下に出す
      （サイドバーは overflow 指定があり absolute だと切れる・ずれる） */
  let menuPos = $state({ top: 0, left: 0 });
  let subOpen = $state(false);
  function openMenu(e: MouseEvent, which: "labels" | "folders") {
    e.stopPropagation();
    if (app.labelMenu === which) {
      app.labelMenu = false;
      return;
    }
    app.listMenu = null; // ツールバーのメニューと重ねない
    app.folderMenu = null;
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    menuPos = { top: r.bottom + 4, left: Math.max(8, r.left - 12) };
    subOpen = false;
    app.labelMenu = which;
  }
  const forFolders = $derived(app.labelMenu === "folders");
  const curSort = $derived(forFolders ? app.folderSort : app.tagSort);
  function sortBy(s: "alpha" | "count") {
    const folders = forFolders;
    app.labelMenu = false;
    if (folders) setFolderSort(s);
    else void setTagSort(s);
  }
  function manage() {
    if (forFolders) openFolderModal("manageFolders");
    else {
      app.labelMenu = false;
      app.modal = "manageLabels";
    }
  }
</script>

<svelte:window
  onclick={() => {
    app.labelMenu = false;
    app.folderMenu = null;
  }}
  oncontextmenu={() => (app.folderMenu = null)} />

<nav class="sidebar">
  <div class="sep sec-head">
    <span>MY LIBRARY</span>
    <span class="head-actions">
      <button title={t("新規フォルダ")} aria-label={t("新規フォルダ")} onclick={() => openFolderModal("newFolder")}>{t("＋")}</button>
      <button title={t("フォルダメニュー")} aria-label={t("フォルダメニュー")} onclick={(e) => openMenu(e, "folders")}>…</button>
    </span>
  </div>
  <!-- 各行: クリック = 前面のタブのビュー / ⌘クリック・中クリック = 新しいタブで開く -->
  <button
    class:active={isActive("all")}
    onclick={(e) => go(e, { kind: "all" })}
    onauxclick={(e) => goAux(e, { kind: "all" })}>
    <span>{t("すべて")}</span><span class="n">{app.counts.all}</span>
  </button>
  <button
    class:active={isActive("starred")}
    onclick={(e) => go(e, { kind: "starred" })}
    onauxclick={(e) => goAux(e, { kind: "starred" })}>
    <span>★ Starred</span><span class="n">{app.counts.starred}</span>
  </button>

  {#each folderRows as { c, depth } (c.id)}
    <!-- クリック = そのフォルダの一覧 / 行をここへドラッグ = 追加 / 「…」・右クリック = メニュー -->
    <div
      class="fd-row"
      role="button"
      tabindex="0"
      style="--depth:{depth}"
      data-drop="col:{c.id}"
      class:active={isActive("collection", c.id)}
      class:drophover={app.drag?.over === `col:${c.id}`}
      class:menu-open={app.folderMenu?.id === c.id}
      onclick={(e) => go(e, folderView(c))}
      onauxclick={(e) => goAux(e, folderView(c))}
      onkeydown={(e) => onFolderKey(e, c)}
      oncontextmenu={(e) => openFolderMenu(e, c.id)}>
      {#if hasKids.has(c.id)}
        {@const open = !app.foldersCollapsed.has(c.key)}
        <button
          class="fd-chev"
          class:open
          title={open ? t("折り畳む") : t("展開する")}
          aria-label={open ? t("折り畳む") : t("展開する")}
          aria-expanded={open}
          onclick={(e) => {
            e.stopPropagation();
            toggleFolderCollapsed(c.key);
          }}>
          <svg viewBox="0 0 16 16" width="10" height="10" aria-hidden="true"
            ><path d="M6 3.5 10.5 8 6 12.5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" /></svg>
        </button>
      {:else}
        <span class="fd-chev"></span>
      {/if}
      <svg class="fd-ico" viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"
        ><path
          d="M1.8 4.2c0-.66.54-1.2 1.2-1.2h3.1l1.5 1.6h5.4c.66 0 1.2.54 1.2 1.2v6.2c0 .66-.54 1.2-1.2 1.2H3c-.66 0-1.2-.54-1.2-1.2V4.2Z"
          fill="none"
          stroke="currentColor"
          stroke-width="1.3"
          stroke-linejoin="round" /></svg>
      <span class="name">{c.name}</span>
      <span class="n">{c.nItems}</span>
      <button class="fd-more" title={t("フォルダメニュー")} aria-label={t("フォルダメニュー")} onclick={(e) => openFolderMenu(e, c.id)}>…</button>
    </div>
  {/each}

  <div class="sep sec-head">
    <span>LABELS</span>
    <span class="head-actions">
      <button
        class:on={searchOpen}
        title={searchOpen ? t("検索欄を隠す") : t("ラベルを検索")}
        aria-label={searchOpen ? t("検索欄を隠す") : t("ラベルを検索")}
        onclick={toggleSearch}>
        <svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true"
          ><circle cx="7" cy="7" r="4.6" fill="none" stroke="currentColor" stroke-width="1.6" /><path
            d="M10.4 10.4 14 14" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
      </button>
      <button title={t("新規ラベル")} aria-label={t("新規ラベル")} onclick={() => (app.modal = "newLabel")}>{t("＋")}</button>
      <button title={t("ラベルメニュー")} aria-label={t("ラベルメニュー")} onclick={(e) => openMenu(e, "labels")}>…</button>
    </span>
  </div>
  {#if searchOpen}
    <div class="ls-search">
      <input
        bind:this={searchEl}
        bind:value={labelQ}
        placeholder="Search labels"
        onkeydown={(e) => {
          if (e.key === "Escape" && !composing(e)) {
            e.stopPropagation();
            if (labelQ) labelQ = "";
            else toggleSearch();
          }
        }} />
      {#if labelQ}<button class="ls-x" title={t("消去")} onclick={() => (labelQ = "")}>×</button>{/if}
    </div>
  {/if}
  {#if app.tags.length === 0}
    <div class="nolabels">{t("ラベルはまだありません（＋ で作成）")}</div>
  {:else if shownTags.length === 0}
    <div class="nolabels">{t("「::no-label")}{labelQ.trim()}{t("」に一致するラベルなし")}</div>
  {/if}
  {#each shownTags as tag (tag.id)}
    <!-- クリック = そのラベルの一覧 / 行をここへドラッグ = 付与 / ここから行へドラッグ = 付与（Paperpile と同じ両方向） -->
    <button
      class="tagrow"
      data-drop="tag:{tag.id}"
      class:active={isActive("tag", tag.id)}
      class:drophover={app.drag?.kind === "rows" && app.drag?.over === `tag:${tag.id}`}
      title={t("クリックで絞り込み・論文へドラッグで付与")}
      onpointerdown={(e) => beginLabelDrag(e, tag.id, tag.name)}
      onclick={(e) => {
        if (consumeDragClick()) return;
        go(e, { kind: "tag", id: tag.id, name: tag.name });
      }}
      onauxclick={(e) => goAux(e, { kind: "tag", id: tag.id, name: tag.name })}
    >
      <LabelBadge name={tag.name} color={tag.color} /><span class="n">{tag.nItems}</span>
    </button>
  {/each}

  <div class="sep"></div>
  <button
    class:active={isActive("trash")}
    onclick={(e) => go(e, { kind: "trash" })}
    onauxclick={(e) => goAux(e, { kind: "trash" })}>
    <span>{t("ゴミ箱")}</span><span class="n">{app.counts.trash}</span>
  </button>
</nav>

{#if app.labelMenu}
  <div class="lmenu" style="top:{menuPos.top}px;left:{menuPos.left}px" role="menu" onclick={(e) => e.stopPropagation()}>
    <div
      class="lmenu-item lmenu-parent"
      class:open={subOpen}
      role="menuitem"
      tabindex="-1"
      onmouseenter={() => (subOpen = true)}
      onclick={() => (subOpen = !subOpen)}
      onkeydown={() => {}}>
      <span><span class="lmenu-ico">⇅</span>{forFolders ? "Sort folders" : "Sort labels"}</span><span class="lmenu-caret">›</span>
      {#if subOpen}
        <div class="lmenu-sub" role="menu">
          <button onclick={() => sortBy("alpha")}><span class="lmenu-check">{curSort === "alpha" ? "✓" : ""}</span>Alphabetically</button>
          <button onclick={() => sortBy("count")}><span class="lmenu-check">{curSort === "count" ? "✓" : ""}</span>Number of references</button>
        </div>
      {/if}
    </div>
    <button class="lmenu-item" onmouseenter={() => (subOpen = false)} onclick={manage}>
      <span><span class="lmenu-ico">⚙</span>{forFolders ? "Manage folders…" : "Manage labels…"}</span>
    </button>
  </div>
{/if}

{#if app.folderMenu}
  {@const fm = app.folderMenu}
  <!-- フォルダ行のメニュー（新しいタブで開く / Paperpile の順: Rename / Create subfolder / … / Delete） -->
  <div class="lmenu fd-menu" style="top:{fm.top}px;left:{fm.left}px" role="menu" tabindex="-1">
    <button class="lmenu-item" onclick={() => openFolderInTab(fm.id)}>
      <span><span class="lmenu-ico">⧉</span>{t("新しいタブで開く")}</span>
    </button>
    <button class="lmenu-item" onclick={() => openFolderModal("renameFolder", { id: fm.id })}>
      <span><span class="lmenu-ico">✎</span>Rename…</span>
    </button>
    <button class="lmenu-item" onclick={() => openFolderModal("newFolder", { parentId: fm.id })}>
      <span><span class="lmenu-ico">{t("＋")}</span>Create subfolder…</span>
    </button>
    <div class="lmenu-sep"></div>
    <button class="lmenu-item lmenu-danger" onclick={() => openFolderModal("deleteFolder", { id: fm.id })}>
      <span><span class="lmenu-ico">🗑</span>Delete…</span>
    </button>
  </div>
{/if}
