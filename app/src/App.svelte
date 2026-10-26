<script lang="ts">
  import { setLang, t } from "./lib/i18n.svelte";
  import { onMount, untrack } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { listen } from "@tauri-apps/api/event";
  import TabStrip from "./lib/TabStrip.svelte";
  import Sidebar from "./lib/Sidebar.svelte";
  import ItemList from "./lib/ItemList.svelte";
  import ItemDetail from "./lib/ItemDetail.svelte";
  import QuickAdd from "./lib/QuickAdd.svelte";
  import TagModals from "./lib/TagModals.svelte";
  import FolderModals from "./lib/FolderModals.svelte";
  import ImportModal from "./lib/ImportModal.svelte";
  import PurgeModal from "./lib/PurgeModal.svelte";
  import StyleModal from "./lib/StyleModal.svelte";
  import LabelBadge from "./lib/LabelBadge.svelte";
  import ReferenceEditor from "./lib/ReferenceEditor.svelte";
  import PdfViewer from "./lib/PdfViewer.svelte";
  import {
    activateTabAt,
    app,
    checkAll,
    checkUpdate,
    clearFilter,
    closeNoteEditor,
    clearChecks,
    closeImport,
    closeTab,
    cycleTab,
    doSearch,
    frontViewer,
    handleDrop,
    loadCitePrefs,
    loadFolderPrefs,
    openEditorNew,
    openImportDialog,
    openTab,
    pollDataVersion,
    refreshList,
    refreshSidebar,
    reloadDetail,
    restoreTabs,
    saveTabsPref,
    setView,
    tabsPref,
    toast,
  } from "./lib/state.svelte";

  let searchInput = $state("");
  let searchTimer: ReturnType<typeof setTimeout> | undefined;

  // 新版のお知らせの帯（起動時の確認 = checkUpdate）。入れ替えはしない: 更新は install.sh の再実行 = LP の手順と同じ 1 行をコピーさせる
  const INSTALL_CMD = "curl -fsSL https://openref.unitedradiology.jp/install.sh | bash";
  async function copyInstallCmd() {
    try {
      await navigator.clipboard.writeText(INSTALL_CMD);
      toast(t("✓ インストールコマンドをコピー"));
    } catch (e) {
      toast("✗ " + String(e).slice(0, 200));
    }
  }

  function onSearchInput() {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => doSearch(searchInput), 250);
  }

  // reveal が検索語を外したら（新しく作った item を一覧に出すため）・タブを切り替えた / 復元したら、入力欄の文字を
  // app.search に揃え、待ち中の検索も捨てる（打鍵中でも = 意図したリセット）。合図は app.searchReset で、app.search は
  // 見張らない（見張ると、打鍵の 250ms 後に doSearch が app.search を書くたびに入力欄を書き戻してしまう）
  $effect(() => {
    app.searchReset;
    clearTimeout(searchTimer);
    searchInput = untrack(() => app.search);
  });

  // タブの永続（ui_tabs）: ビュー・検索語・チップ・前面の番号が変わったら 300ms 後に保存する。復元が済むまでは保存しない
  // （この effect はマウント直後に既定の 1 枚で走る。復元より先に保存すると保存済みのタブを上書きする）
  let tabsTimer: ReturnType<typeof setTimeout> | undefined;
  /** 最後に保存できた ui_tabs（失敗したら進めない = 次の変化で保存し直す） */
  let tabsSaved = "";
  let tabsSaving = false;
  /** 今の状態を保存する。保存を待つ間に変わった（元に戻した・⌘T の直後に ⌘W 等）なら、その後の状態も続けて保存する
      （effect は「最後に保存できたもの」と同じ JSON には動かないので、待つ間に戻した変化を取りこぼさない） */
  async function flushTabs() {
    if (tabsSaving) return; // 保存中の flush が終わりに見直す
    tabsSaving = true;
    try {
      for (let json = tabsPref(); json !== tabsSaved; json = tabsPref()) {
        if (!(await saveTabsPref(json))) break;
        tabsSaved = json;
      }
    } finally {
      tabsSaving = false;
    }
  }
  $effect(() => {
    if (!app.tabsRestored) return;
    const json = tabsPref();
    clearTimeout(tabsTimer);
    if (json !== tabsSaved) tabsTimer = setTimeout(flushTabs, 300);
  });

  /** タブのキー: ⌘T 新規 / ⌘W 閉じる / ⌘⇧] ⌘⇧[ ⌃Tab ⌃⇧Tab 巡回 / ⌘1〜8 n 番目・⌘9 末尾。扱ったキーなら true。
      ⌘W は閉じる対象が無くても常に奪う（Tauri の既定メニューの Close Window = ⌘W に素通しすると窓が閉じる = アプリが終わる。
      wry は単独の webview ではキーを先にページへ渡し、ページが処理しなかったものだけメニューへ回す）。
      ⌘⇧[ ] は e.key で [ ] と { } の両方を受ける（JIS 配列・Shift で文字が変わる）。入力欄の中でも効かせる（ブラウザのタブと同じ）。
      キーリピート・IME の変換中は何もしない。処理中・エディタ・モーダル・取込中は state 側（tabOpsAllowed）が受け付けない（PDF ビューアの
      中では効く。⌘W は PDF タブを閉じる・最後の 1 枚ならビューアだけ閉じる = closeTab） */
  function onTabKey(e: KeyboardEvent): boolean {
    const cmd = e.metaKey && !e.ctrlKey && !e.altKey;
    const k = e.key;
    let act: () => void;
    if (cmd && !e.shiftKey && k.toLowerCase() === "t") act = () => void openTab();
    else if (cmd && !e.shiftKey && k.toLowerCase() === "w") act = () => void closeTab(app.tab);
    else if (cmd && e.shiftKey && (k === "]" || k === "}")) act = () => cycleTab(1);
    else if (cmd && e.shiftKey && (k === "[" || k === "{")) act = () => cycleTab(-1);
    else if (e.ctrlKey && !e.metaKey && !e.altKey && k === "Tab") act = () => cycleTab(e.shiftKey ? -1 : 1);
    else if (cmd && !e.shiftKey && /^[1-9]$/.test(k)) act = () => activateTabAt(Number(k));
    else return false;
    e.preventDefault();
    if (!e.repeat && !e.isComposing && e.keyCode !== 229) act();
    return true;
  }

  function onKeydown(e: KeyboardEvent) {
    if (onTabKey(e)) return;
    if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "n") {
      e.preventDefault();
      openEditorNew();
    } else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "n") {
      e.preventDefault();
      // PDF ビューアの下（見えない一覧のヘッダー）にはフォーカスを送らない
      if (!frontViewer()) document.getElementById("quickadd")?.focus();
    } else if (
      (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "a" &&
      !isEditable(e.target) && !frontViewer() && !app.editorOpen && !app.modal
    ) {
      // ⌘A = 一覧をすべてチェック（Paperpile 準拠）
      e.preventDefault();
      checkAll();
    } else if (e.key === "Escape") {
      // Esc の優先順位: モーダル → PDF ビューア → ＋新規メニュー → サイドバーのメニュー → ツールバーのメニュー → 取込モーダル（実行中は無視）→ チェック解除。
      // モーダルは各コンポーネントが自分の window keydown で内側から閉じる。window の keydown は App が先に受けるので
      // （モーダル側の stopPropagation はここに届かない）、モーダルが開いている間はここで背後のもの（メモ欄など）を閉じない。
      // 前面の PDF ビューアは自分の window keydown で扱う（ハイライトの編集・選択の取消 → 入力欄から抜ける → タブを閉じる）。
      // ここで先に閉じると、ビューアの内側の Esc が効かない（App の keydown が先に走る）。ビューアの上のエディタ・取込は従来どおり
      if (app.modal) {
        /* 何もしない */
      } else if (frontViewer() && !app.editorOpen && !app.importJob) {
        /* PdfViewer に任せる */
      } else if (app.newMenu) app.newMenu = false;
      else if (app.labelMenu) app.labelMenu = false;
      else if (app.folderMenu) app.folderMenu = null;
      else if (app.listMenu) app.listMenu = null;
      else if (app.noteEditor) closeNoteEditor(); // 未保存なら閉じない（textarea 内の Esc は自前処理）
      else if (app.importJob) closeImport();
      else if (!app.editorOpen && !app.modal && app.checked.size) clearChecks();
    } else if (e.key === "/" && !isEditable(e.target) && !frontViewer()) {
      e.preventDefault();
      document.getElementById("searchbox")?.focus();
    }
  }

  function isEditable(t: EventTarget | null) {
    const el = t as HTMLElement | null;
    return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable);
  }

  /** 起動: フォルダ / ラベルを読んでから（タブの実在確認に要る）タブを戻し、前面の一覧を取る（戻した PDF タブはその item を
      選んでいる = 一覧にあれば詳細も読む）。フォルダ / ラベルを読めなければタブは戻さず、このセッションでは保存もしない
      （保存済みのタブを既定の 1 枚で上書きしない） */
  async function startup() {
    try {
      await refreshSidebar();
      await restoreTabs();
    } catch (e) {
      toast(t("✗ フォルダ・ラベルの読込に失敗（タブは戻さず・保存もしません）: ") + String(e).slice(0, 160));
    }
    await refreshList();
    await reloadDetail();
  }

  // PDF ビューアを置いたタブ。ビューアはそのタブが初めて前面になったときに作り（起動時に戻した PDF タブを全部は読まない）、
  // 以後はタブを閉じるまで生かしておく（切替は即時・ページ位置・ズーム・検索語・注釈の編集中を失わない・PDF を読み直さない）
  const viewerSeen = new SvelteSet<number>();
  $effect(() => {
    const front = frontViewer() ? app.tab : null;
    const live = app.tabs.filter((t) => t.viewer).map((t) => t.id);
    untrack(() => {
      if (front !== null) viewerSeen.add(front);
      for (const id of viewerSeen) if (!live.includes(id)) viewerSeen.delete(id); // 閉じた・ビューアを外したタブ
    });
  });

  onMount(() => {
    void startup();
    void checkUpdate();
    loadCitePrefs();
    loadFolderPrefs();
    // Settings › Language（Rust が DB に書けてから emit する）= 再起動なしで表記を切り替える。起動時の値は main.ts が mount の前に読む
    let unlistenLang: (() => void) | undefined;
    listen<string>("ui-lang", (e) => setLang(e.payload)).then((fn) => (unlistenLang = fn));
    // Settings › 起動時に新版を確認 を外した → 出ている帯をその場で消す（Rust が DB に書けてから emit する）
    let unlistenUpdateOff: (() => void) | undefined;
    listen("update-check-off", () => (app.updateLatest = null)).then((fn) => (unlistenUpdateOff = fn));
    let unlisten: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const p = event.payload as any;
        if (p.type === "enter" || p.type === "over") app.dropActive = true;
        else if (p.type === "drop") {
          app.dropActive = false;
          handleDrop(p.paths ?? []);
        } else {
          app.dropActive = false;
        }
      })
      .then((fn) => (unlisten = fn));
    // 外（MCP・CLI・connectd）の書込を 2 秒以内に画面へ
    const dataVersionTimer = setInterval(() => void pollDataVersion(), 2000);
    return () => {
      clearInterval(dataVersionTimer);
      unlisten?.();
      unlistenLang?.();
      unlistenUpdateOff?.();
    };
  });
</script>

<svelte:window
  onkeydown={onKeydown}
  onclick={(e) => {
    if (app.newMenu && !(e.target as Element | null)?.closest?.(".nm-wrap")) app.newMenu = false;
  }} />

<div class="root">
  <TabStrip />
  <!-- タブの中身（ヘッダー・一覧・PDF ビューア）。PDF タブのビューアはこの全面を覆う -->
  <div class="tabs-page">
    <header>
      <div class="brand">OpenRef</div>
      <!-- 検索ボックス: チップ（著者 / 誌名。行のクリックで入る・× で外す）+ 自由語。Paperpile の search box 準拠 -->
      <div class="sb-wrap" role="search" onclick={() => document.getElementById("searchbox")?.focus()}>
        {#if app.view.kind === "tag"}
          <!-- ラベルのビュー = チップ（Paperpile の "liver ×"）。× ですべてに戻る -->
          {@const lv = app.view}
          {@const lt = app.tags.find((t) => t.id === lv.id)}
          <span class="sb-chip sb-lbl" title={t("ラベルで絞り込み中")}>
            <LabelBadge name={lt?.name ?? lv.name} color={lt?.color ?? null} />
            <button class="sb-x" title={t("ラベルの絞り込みを外す")} onclick={(e) => { e.stopPropagation(); void setView({ kind: "all" }); }}>×</button>
          </span>
        {/if}
        {#if app.filters.author}
          <span class="sb-chip" title={t("著者で絞り込み中（姓 + 頭文字・著者の位置は問わない）")}>
            <span class="sb-ico">👤</span>{app.filters.author.label}
            <button class="sb-x" title={t("著者の絞り込みを外す")} onclick={(e) => { e.stopPropagation(); void clearFilter("author"); }}>×</button>
          </span>
        {/if}
        {#if app.filters.journal}
          <span class="sb-chip" title={t("誌名で絞り込み中")}>
            <span class="sb-ico">📖</span>{app.filters.journal}
            <button class="sb-x" title={t("誌名の絞り込みを外す")} onclick={(e) => { e.stopPropagation(); void clearFilter("journal"); }}>×</button>
          </span>
        {/if}
        <input
          id="searchbox"
          class="search"
          placeholder={app.filters.author || app.filters.journal || app.view.kind === "tag" ? t("さらに絞り込む…") : t("検索")}
          title={t("英・和 / title・著者・abstract・誌名・PDF 本文（/ でフォーカス）")}
          bind:value={searchInput}
          oninput={onSearchInput}
        />
      </div>
      <QuickAdd />
      <div class="nm-wrap">
        <button class="newref" onclick={() => (app.newMenu = !app.newMenu)} title={t("手動作成 / PDF 取込")}>
          {t("＋ 新規")} <span class="nm-caret">▾</span>
        </button>
        {#if app.newMenu}
          <div class="nm-menu" role="menu">
            <button onclick={() => { app.newMenu = false; openEditorNew(); }}>
              <span>{t("手動で作成…")}</span><kbd>⌘⇧N</kbd>
            </button>
            <button onclick={() => { app.newMenu = false; void openImportDialog(); }}>
              <span>{t("PDF を取り込む…")}</span>
            </button>
          </div>
        {/if}
      </div>
      {#if app.busy}<div class="spinner" title={t("処理中")}></div>{/if}
    </header>
    {#if app.updateLatest}
      <!-- 新版のお知らせ（起動時の確認 = state.checkUpdate・ub- 接頭辞）。× はこの起動の間だけ隠す。止めるのは Settings › 起動時に新版を確認 -->
      <div class="ub-bar" role="status">
        <span>{t("新版 ")}<b>{app.updateLatest}</b>{t(" があります。ターミナルでインストールコマンドをもう一度実行すると更新されます")}</span>
        <code class="ub-cmd">{INSTALL_CMD}</code>
        <button class="ub-copy" onclick={copyInstallCmd}>{t("コピー")}</button>
        <button class="ub-x" title={t("この起動の間は隠す（止めるのは Settings › 起動時に新版を確認）")} aria-label={t("この起動の間は隠す（止めるのは Settings › 起動時に新版を確認）")} onclick={() => (app.updateLatest = null)}>×</button>
      </div>
    {/if}
    <main>
      <Sidebar />
      <ItemList />
      <ItemDetail />
    </main>
    {#each app.tabs as t (t.id)}
      {#if t.viewer && (t.id === app.tab || viewerSeen.has(t.id))}
        {#key t.viewer?.attachmentId}
          <PdfViewer viewer={t.viewer} front={t.id === app.tab} onClose={() => void closeTab(t.id)} />
        {/key}
      {/if}
    {/each}
  </div>
  {#if app.toast}
    <div class="toast">{app.toast}</div>
  {/if}
  {#if app.dropActive}
    <div class="dropzone"><div>{t("PDF をドロップして登録（DOI/PMID 自動抽出）")}</div></div>
  {/if}
  {#if app.drag}
    <div class="dragghost" style="left: {app.drag.x + 14}px; top: {app.drag.y + 10}px">
      {app.drag.label.slice(0, 40)}
    </div>
  {/if}
  <TagModals />
  <FolderModals />
  <ImportModal />
  <PurgeModal />
  <StyleModal />
  {#if app.editorOpen}<ReferenceEditor />{/if}
</div>
