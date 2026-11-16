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
    setView,
    type View,
  } from "./state.svelte";
  import { setFolderSort, setTagSort, toggleFolderCollapsed } from "./prefs.svelte";
  import { composing } from "./input";
  import ModeSegment from "./ModeSegment.svelte";

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

  // ---- LIBRARY のフォルダ（Paperpile: 折畳 chevron + 📁 + 名前 + 件数 + hover の「…」/ 右クリックでメニュー）。
  // 行 = 枠の div（role なし・ドロップの data-drop・右クリック・中クリック・選ばれた / hover / 焦点の輪の見た目）+ 行全体を覆う
  // 透明の <button>（名前 + 件数 = Enter / Space・クリック）+ 兄弟の <button>（chevron・「…」= 覆いの上に重ねる）。
  // div role="button" の中にボタンを入れる作りは、ARIA の button が子を飾り扱いにする = 読み上げで中のボタンに届かない恐れがあった
  const folderRows = $derived(flatFolders({ respectCollapse: true }));
  const hasKids = $derived(new Set(app.collections.map((c) => c.parentId)));

  const folderView = (c: CollectionRow): View => ({ kind: "collection", id: c.id, name: c.name });
  /** 行を覆うボタンの Enter / Space = 前面のタブのビュー / ⌘ つき = 新しいタブ。素のボタンの押下（Enter の keypress・Space の keyup の
      click）は既定を止めて出さない（⌘ をそのまま渡す・二重に動かさない）。中の ＋ / … / chevron は兄弟 = この keydown は来ない */
  function onRowKey(e: KeyboardEvent, v: View) {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      go(e, v);
    }
  }
  /** フォルダの行の中クリック（新しいタブ）と右クリック（メニュー）。行の中の 3 つのボタン（覆い・chevron・「…」）に付ける =
      行のどこで押しても効く（前の div role="button" の行と同じ。枠の div は全面を覆いと中のボタンが占める = 枠に付けると
      role の無い要素に操作を付けることになる） */
  const folderMouse = (c: CollectionRow) => ({
    onauxclick: (e: MouseEvent) => goAux(e, folderView(c)),
    oncontextmenu: (e: MouseEvent) => openFolderMenu(e, c.id),
  });
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

  /** 「…」メニュー（「すべて」の行の … = フォルダ / LABELS の見出しの … = ラベル の共用）は fixed で「…」ボタンの直下に出す
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
    else setTagSort(s);
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

<nav class="ls-nav sidepane">
  <ModeSegment />
  <!-- フォルダ節の見出しは置かない（最上段で LIBRARY が選ばれている直下に同じ字の見出しが並ぶ）。
       その見出しにあった ＋（新規フォルダ）と …（フォルダメニュー = 並び順・フォルダを管理）は「すべて」の行の右（件数の左）に常に出す
       （hover で出す形にしない = 隠れる操作を作らない）。行の作り = フォルダの行 fd-row と同じ（枠の div + 行を覆うボタン ls-all-hit +
       兄弟の ＋ / …）。各行: クリック・Enter / Space = 前面のタブのビュー / ⌘クリック・中クリック = 新しいタブで開く。
       覆いのボタンの名前 =「すべて 8」（見えている名前と件数は aria-hidden = 二重に読ませない）。覆いのボタン（ここ・フォルダの行の
       .fd-hit）は tabindex="0" を明示する = macOS の WebKit（WKWebView）は「フルキーボードアクセス」が切（既定）だと素の <button> に
       Tab が止まらず、押しても焦点を取らない（div tabindex="0" の行は届く）。Chrome の Tab の順は変わらない -->
  <div class="ls-all" class:active={isActive("all")}>
    <button
      class="ls-all-hit"
      tabindex="0"
      aria-label="{t('すべて')} {app.counts.all}"
      onclick={(e) => go(e, { kind: "all" })}
      onauxclick={(e) => goAux(e, { kind: "all" })}
      onkeydown={(e) => onRowKey(e, { kind: "all" })}></button>
    <span class="name" aria-hidden="true">{t("すべて")}</span>
    <button class="ls-all-act" title={t("新規フォルダ")} aria-label={t("新規フォルダ")} onclick={(e) => { e.stopPropagation(); openFolderModal("newFolder"); }}>{t("＋")}</button>
    <button class="ls-all-act" class:on={app.labelMenu === "folders"} title={t("フォルダメニュー")} aria-label={t("フォルダメニュー")} onclick={(e) => openMenu(e, "folders")}>…</button>
    <span class="n" aria-hidden="true">{app.counts.all}</span>
  </div>
  <button
    class:active={isActive("starred")}
    onclick={(e) => go(e, { kind: "starred" })}
    onauxclick={(e) => goAux(e, { kind: "starred" })}>
    <span>{t("★ スター付き")}</span><span class="n">{app.counts.starred}</span>
  </button>

  {#each folderRows as { c, depth } (c.id)}
    <!-- クリック = そのフォルダの一覧 / 行をここへドラッグ = 追加 / 「…」・右クリック = メニュー。
         中クリック・右クリックは行の中のボタンの全部が受ける（前と同じく chevron / 「…」の上でも効く）。覆いのボタンの名前 =「<フォルダ名> 2」 -->
    <div
      class="fd-row"
      style="--depth:{depth}"
      data-drop="col:{c.id}"
      class:active={isActive("collection", c.id)}
      class:drophover={app.drag?.over === `col:${c.id}`}
      class:menu-open={app.folderMenu?.id === c.id}>
      <button
        class="fd-hit"
        tabindex="0"
        aria-label="{c.name} {c.nItems}"
        {...folderMouse(c)}
        onclick={(e) => go(e, folderView(c))}
        onkeydown={(e) => onRowKey(e, folderView(c))}></button>
      {#if hasKids.has(c.id)}
        {@const open = !app.foldersCollapsed.has(c.key)}
        <button
          class="fd-chev"
          class:open
          title={open ? t("折り畳む") : t("展開する")}
          aria-label={open ? t("折り畳む") : t("展開する")}
          aria-expanded={open}
          {...folderMouse(c)}
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
      <span class="name" aria-hidden="true">{c.name}</span>
      <span class="n" aria-hidden="true">{c.nItems}</span>
      <button class="fd-more" title={t("フォルダメニュー")} aria-label={t("フォルダメニュー")} {...folderMouse(c)} onclick={(e) => openFolderMenu(e, c.id)}>…</button>
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
        placeholder={t("ラベルを検索")}
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
      title={t("クリックで絞り込み・文献へドラッグで付与")}
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
      <span><span class="lmenu-ico">⇅</span>{forFolders ? t("フォルダの並び") : t("ラベルの並び")}</span><span class="lmenu-caret">›</span>
      {#if subOpen}
        <div class="lmenu-sub" role="menu">
          <button onclick={() => sortBy("alpha")}><span class="lmenu-check">{curSort === "alpha" ? "✓" : ""}</span>{t("名前順")}</button>
          <button onclick={() => sortBy("count")}><span class="lmenu-check">{curSort === "count" ? "✓" : ""}</span>{t("件数順")}</button>
        </div>
      {/if}
    </div>
    <button class="lmenu-item" onmouseenter={() => (subOpen = false)} onclick={manage}>
      <span><span class="lmenu-ico">⚙</span>{forFolders ? t("フォルダを管理…") : t("ラベルを管理…")}</span>
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
      <span><span class="lmenu-ico">✎</span>{t("名前を変更…")}</span>
    </button>
    <button class="lmenu-item" onclick={() => openFolderModal("newFolder", { parentId: fm.id })}>
      <span><span class="lmenu-ico">{t("＋")}</span>{t("サブフォルダを作成…")}</span>
    </button>
    <div class="lmenu-sep"></div>
    <button class="lmenu-item lmenu-danger" onclick={() => openFolderModal("deleteFolder", { id: fm.id })}>
      <span><span class="lmenu-ico">🗑</span>{t("削除…")}</span>
    </button>
  </div>
{/if}
