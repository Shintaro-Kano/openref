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
  import AddMenu from "./lib/AddMenu.svelte";
  import TagModals from "./lib/TagModals.svelte";
  import FolderModals from "./lib/FolderModals.svelte";
  import ImportModal from "./lib/ImportModal.svelte";
  import MigrationModal from "./lib/MigrationModal.svelte";
  import PurgeModal from "./lib/PurgeModal.svelte";
  import StyleModal from "./lib/StyleModal.svelte";
  import LabelBadge from "./lib/LabelBadge.svelte";
  import ReferenceEditor from "./lib/ReferenceEditor.svelte";
  import PdfViewer from "./lib/PdfViewer.svelte";
  import WikiHeader from "./lib/wiki/WikiHeader.svelte";
  import WikiSidebar from "./lib/wiki/WikiSidebar.svelte";
  import WikiPage from "./lib/wiki/WikiPage.svelte";
  import WikiInfo from "./lib/wiki/WikiInfo.svelte";
  import WikiChatConsent from "./lib/chat/ChatConsent.svelte";
  import ShelfGrid from "./lib/shelf/ShelfGrid.svelte";
  import ShelfHeader from "./lib/shelf/ShelfHeader.svelte";
  import PaneToggle from "./lib/PaneToggle.svelte";
  import ShelfRight from "./lib/shelf/ShelfRight.svelte";
  import ShelfSidebar from "./lib/shelf/ShelfSidebar.svelte";
  import ShelfViewer from "./lib/shelf/ShelfViewer.svelte";
  import { loadShelf, sh } from "./lib/shelf/shelf.svelte";
  import { chat, chatShown, initChat, toggleDock } from "./lib/chat/chat.svelte";
  import { initViewing } from "./lib/viewing.svelte";
  import ChatDock from "./lib/chat/ChatDock.svelte";
  import WikiRescue from "./lib/wiki/WikiRescue.svelte";
  import WikiLinkMenu from "./lib/wiki/WikiLinkMenu.svelte";
  import WikiPreview from "./lib/wiki/WikiPreview.svelte";
  import WikiLightbox from "./lib/wiki/WikiLightbox.svelte";
  import { lightbox } from "./lib/wiki/lightbox.svelte";
  import { initRecent } from "./lib/wiki/recent.svelte";
  import SidebarGrip from "./lib/SidebarGrip.svelte";
  import { loadSidebarW, sidebarW } from "./lib/sidebarw.svelte";
  import { ed } from "./lib/wiki/edit.svelte";
  import { focusMode, toggleFocus } from "./lib/wiki/focus.svelte";
  import { infoKeyAllowed, initWiki, loadWikiEnabled, loadWikiStatus, togglePane, wikiDrop, wk } from "./lib/wiki/wiki.svelte";
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
    covered,
    frontKind,
    frontShelf,
    dropBlocked,
    handleDrop,
    loadCitePrefs,
    modalOpen,
    openEditorNew,
    openMigration,
    openTabInMode,
    pollDataVersion,
    refreshList,
    refreshSidebar,
    reloadDetail,
    restoreTabs,
    saveTabsPref,
    setView,
    tabsPref,
    toast,
    toggleDetail,
  } from "./lib/state.svelte";
  import { loadFolderPrefs } from "./lib/prefs.svelte";
  import { composing } from "./lib/input";
  import { errText } from "./lib/err";

  let searchInput = $state("");
  let searchTimer: ReturnType<typeof setTimeout> | undefined;

  // 新版のお知らせの帯（起動時の確認 = checkUpdate）。入れ替えはしない: 更新は install.sh の再実行 = LP の手順と同じ 1 行をコピーさせる
  const INSTALL_CMD = "curl -fsSL https://openref.unitedradiology.jp/install.sh | bash";
  async function copyInstallCmd() {
    try {
      await navigator.clipboard.writeText(INSTALL_CMD);
      toast(t("✓ インストールコマンドをコピー"));
    } catch (e) {
      toast("✗ " + errText(e, 200));
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
    if (cmd && !e.shiftKey && k.toLowerCase() === "t") act = () => void openTabInMode(); // 今のモード（Wiki ならホーム）
    else if (cmd && !e.shiftKey && k.toLowerCase() === "w") act = () => void closeTab(app.tab);
    else if (cmd && e.shiftKey && (k === "]" || k === "}")) act = () => cycleTab(1);
    else if (cmd && e.shiftKey && (k === "[" || k === "{")) act = () => cycleTab(-1);
    else if (e.ctrlKey && !e.metaKey && !e.altKey && k === "Tab") act = () => cycleTab(e.shiftKey ? -1 : 1);
    else if (cmd && !e.shiftKey && /^[1-9]$/.test(k)) act = () => activateTabAt(Number(k));
    else return false;
    e.preventDefault();
    if (!e.repeat && !composing(e)) act();
    return true;
  }

  /** 前面のタブの種類（state の frontKind = 一覧 / PDF / OpenNote / 本棚 / 本）。キー・ドロップ・ヘッダー・main の出し分けはこの 1 語で見る */
  const kind = $derived(frontKind());
  /** OpenNote の集中モード（前面が OpenNote のときだけ効く = 木・右ペイン・ヘッダーを隠す。focus.svelte.ts） */
  const wkFocus = $derived(kind === "wiki" && focusMode.on);
  /** 左の列の幅を変えられるモード（LIBRARY / NOTE / SHELF）。本の読み手は左の列が無い（null）。PDF のタブの下は一覧 */
  const SB_MODE = { list: "lib", viewer: "lib", wiki: "wiki", shelf: "shelf", book: null } as const;
  const sbMode = $derived(SB_MODE[kind]);

  function onKeydown(e: KeyboardEvent) {
    if (onTabKey(e)) return;
    // ⌘⌥S = OpenNote の集中モードの切替（前面が OpenNote・覆いが無いとき）。⌥S は配列により別の字（ß 等）= e.code で見る
    if (kind === "wiki" && !covered() && e.metaKey && e.altKey && !e.ctrlKey && !e.shiftKey && e.code === "KeyS") {
      e.preventDefault();
      if (!e.repeat && !composing(e)) toggleFocus();
      return;
    }
    // ⌘⌥I = 前面のモードの右ペインの開閉（OpenNote = ページ情報・PDF のタブ = 右バー・一覧 / 本棚 / 本 = 詳細）。⌥I は配列により dead key（e.key = "Dead" / "ˆ"）なので
    // e.code で見る。覆い（窓・Wiki のダイアログ・参照エディタ・取込）の下は効かせない（infoKeyAllowed）
    if (infoKeyAllowed() && !wkFocus && e.metaKey && e.altKey && !e.ctrlKey && !e.shiftKey && e.code === "KeyI") {
      e.preventDefault();
      if (!e.repeat && !composing(e)) togglePane();
      return;
    }
    // ⌘⌥C = チャットの柱の開閉（全タブ種で有効・入力欄の中でも効く）。⌥C は配列により文字が変わる（ç 等）ので e.code で見る。
    // チャットが出せない（OpenNote OFF / チャット OFF）・モーダル（本体・Wiki のダイアログ・参照エディタ・PDF 取込 = tabOpsAllowed と同じ
    // 「覆われている」条件）の間は何もしない = preventDefault もしない（素通り。覆いの裏で柱を開いて入力欄の focus を奪わない）
    if (chatShown() && !covered() && e.metaKey && e.altKey && !e.ctrlKey && !e.shiftKey && e.code === "KeyC") {
      e.preventDefault();
      if (!e.repeat && !composing(e)) toggleDock();
      return;
    }
    if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "n") {
      // ⌘⇧N = 手動作成。窓（モーダル・OpenNote / チャットの窓）・取込の下では開かない（窓の上に参照エディタが重なり、Esc が下の窓を
      // 閉じていた）。参照エディタだけが開いているときは今のまま（openEditorNew が中身を新規に戻す）。キーは常に奪う（メニューへ素通しさせない）
      e.preventDefault();
      if (!modalOpen() && !app.importJob) openEditorNew();
    } else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "n") {
      e.preventDefault();
      // ⌘N = 「＋ 追加 ▾」を開いて識別子の欄へ焦点（開いた枠が焦点を入れる = AddMenu。開いていれば欄へ焦点を戻すだけ）。
      // PDF ビューアの下（見えない一覧のヘッダー）・Wiki / 本棚（ヘッダーに「＋ 追加」が無い）では開かない。覆い（窓・参照エディタ・
      // 取込 = covered()）の下でも開かない（裏の登録欄へ focus が移り、打った字が裏に入っていた）。キーは常に奪う（メニュー / 既定へ流さない）
      if (kind === "list" && !covered()) {
        if (app.newMenu) document.getElementById("quickadd")?.focus();
        else app.newMenu = true;
      }
    } else if (
      (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "a" &&
      !isEditable(e.target) && kind === "list" && !covered()
    ) {
      // ⌘A = 一覧をすべてチェック（Paperpile 準拠）。覆い（窓・参照エディタ・取込 = covered()）の下では素通し（取込の窓の下で裏の一覧を
      // 全部チェックしていた）。入力欄の中（窓の中の欄も）は欄の文字の全選択 = isEditable で奪わない
      e.preventDefault();
      checkAll();
    } else if (e.key === "Escape" && !composing(e)) {
      // IME の変換取消の Esc は入力欄のもの（メニュー・チェックを畳まない）。Esc の優先順位: モーダル → PDF ビューア → ＋ 追加 メニュー → サイドバーのメニュー → ツールバーのメニュー → 取込モーダル（実行中は無視）→ チェック解除。
      // モーダルは各コンポーネントが自分の window keydown で内側から閉じる。window の keydown は App が先に受けるので
      // （モーダル側の stopPropagation はここに届かない）、モーダルが開いている間はここで背後のもの（メモ欄など）を閉じない。
      // モーダル = modalOpen()（本体の app.modal と、どのタブの上にも出る OpenNote / チャットの窓 = 同意の窓・救出の窓など）。
      // 前面の PDF ビューアは自分の window keydown で扱う（ハイライトの編集・選択の取消 → 入力欄から抜ける → タブを閉じる）。
      // ここで先に閉じると、ビューアの内側の Esc が効かない（App の keydown が先に走る）。ビューアの上のエディタ・取込は従来どおり
      if (modalOpen()) {
        /* 何もしない */
      } else if (kind !== "list" && !covered()) {
        /* PdfViewer・Wiki・本棚に任せる（見えない一覧のチェック・メモを畳まない） */
      } else if (app.newMenu) app.newMenu = false;
      else if (app.labelMenu) app.labelMenu = false;
      else if (app.folderMenu) app.folderMenu = null;
      else if (app.listMenu) app.listMenu = null;
      else if (app.noteEditor) closeNoteEditor(); // 未保存なら閉じない（textarea 内の Esc は自前処理）
      else if (app.importJob) closeImport();
      // チャットの入力欄の Esc でチェックを外さない（選んでいる文献が閉じて文脈のチップが消える）
      else if (!covered() && app.checked.size && !(e.target as Element | null)?.closest?.(".wk-ch")) clearChecks();
    } else if (e.key === "/" && !isEditable(e.target) && kind === "list" && !covered()) {
      // 一覧の検索欄へ。覆い（窓・参照エディタ・取込）の下では素通し（裏の検索欄へ focus が移っていた。窓の中の欄では字として入る）
      e.preventDefault();
      document.getElementById("searchbox")?.focus();
    } else if (e.key === "/" && !isEditable(e.target) && kind === "shelf" && !covered() && !composing(e)) {
      // 本棚が前面: 本棚の検索欄（ヘッダー）へ
      e.preventDefault();
      document.getElementById("sh-search")?.focus();
    } else if (e.key === "/" && !isEditable(e.target) && kind === "wiki" && !wkFocus && !covered() && !composing(e)) {
      // Wiki が前面: Wiki の検索欄（ヘッダー）へ。LIBRARY の検索欄とは別
      e.preventDefault();
      document.getElementById("wk-search")?.focus();
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
    let sidebarFailed = false;
    // OpenNote の有効 / 無効だけ（設定を読むだけ・索引に触らない。Wiki タブを戻すかを restoreTabs が見る。
    // 読めなければ無効 = 従来どおり）。状態と木は一覧の後で流す（索引の走査で LIBRARY を待たせない）
    await loadWikiEnabled();
    await loadShelf(); // 履歴と ui_shelf_*（タブを戻す前 = 本棚のタブの並び）。読むのは設定だけ（走査しない）
    try {
      await refreshSidebar();
      await restoreTabs();
    } catch (e) {
      sidebarFailed = true;
      toast(t("✗ フォルダ・ラベルの読込に失敗（タブは戻さず・保存もしません）: ") + errText(e, 160));
    }
    // 一覧の失敗も握りつぶさない。ただし上の具体的な知らせ（たいてい同じ原因）を汎用の文言で上書きしない
    try {
      await refreshList();
      await reloadDetail();
    } catch (e) {
      if (!sidebarFailed) toast(`✗ ${t("一覧の読込に失敗")}: ${errText(e, 160)}`);
    }
    void loadWikiStatus();
  }

  // PDF ビューア・本の読み手を置いたタブ。そのタブが初めて前面になったときに作り（起動時に戻した PDF / 本のタブを全部は読まない）、
  // 以後はタブを閉じるまで生かしておく（切替は即時・ページ位置・ズーム・検索語・注釈の編集中を失わない・PDF を読み直さない）。
  // タブの中身は常に 1 つ = 1 つの集合で両方を数える（描くときに t.viewer / t.shelfBook で分ける）
  const seen = new SvelteSet<number>();
  $effect(() => {
    const front = kind === "viewer" || kind === "book" ? app.tab : null;
    const live = app.tabs.filter((t) => t.viewer || t.shelfBook).map((t) => t.id);
    untrack(() => {
      if (front !== null) seen.add(front);
      for (const id of seen) if (!live.includes(id)) seen.delete(id); // 閉じた・ビューア / 読み手を外したタブ
    });
  });

  onMount(() => {
    void startup();
    void checkUpdate();
    loadCitePrefs();
    loadFolderPrefs();
    void loadSidebarW(); // 左の列の幅（ui_sidebar_w）
    // Settings › Language（Rust が DB に書けてから emit する）= 再起動なしで表記を切り替える。起動時の値は main.ts が mount の前に読む
    let unlistenLang: (() => void) | undefined;
    listen<string>("ui-lang", (e) => setLang(e.payload)).then((fn) => (unlistenLang = fn));
    // Settings › 起動時に新版を確認 を外した → 出ている帯をその場で消す（Rust が DB に書けてから emit する）
    let unlistenUpdateOff: (() => void) | undefined;
    listen("update-check-off", () => (app.updateLatest = null)).then((fn) => (unlistenUpdateOff = fn));
    let unlistenMenuToast: (() => void) | undefined;
    listen<string>("menu-toast", (e) => {
      toast(e.payload);
      void reloadDetail();
    }).then((fn) => (unlistenMenuToast = fn));
    // Settings › 他の文献管理ソフトから移行…（Rust の "migration-open"）。バックグラウンドで続行中なら進行中の窓に戻る（state.svelte.ts の openMigration）
    let unlistenMigration: (() => void) | undefined;
    // OpenNote の木 / 本棚の見出しの浮きメニューは窓の覆いより上（z-index）= 開いたら閉じる（窓の上から別のダイアログを開かせない）
    listen("migration-open", () => {
      if (openMigration()) {
        wk.menu = null;
        sh.menu = null;
      }
    }).then((fn) => (unlistenMigration = fn));
    let unlisten: (() => void) | undefined;
    const stopWiki = initWiki();
    const stopRecent = initRecent(); // OpenNote の最近開いたページの記録
    const stopViewing = initViewing();
    const stopChat = initChat();
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const p = event.payload as any;
        // 窓（本体のモーダル・OpenNote / チャットの窓 = modalOpen）・参照エディタ・OpenNote の画像の拡大表示が開いている間は
        // 受けない = 窓の下で取り込む（一覧 = 取込の窓が下に開いて見えない窓が焦点を持つ / OpenNote = 窓の下のページの caret に
        // `![[…]]` を書く）のを防ぐ。ドロップの枠も出さない（受けるように見えて受けない形にしない）・落としたら理由を toast で（1 回だけ）
        const shut = dropBlocked() || !!lightbox.cur;
        // 前面が Wiki のときは蔵書の取込に流さない（drive モードでは Drive にも書く = 事故）。編集中なら各ファイルを vault へ
        // コピーして caret に `![[…]]`（マウスの位置ではない = dragDropEnabled では HTML5 の drop がエディタに届かない）
        if (kind === "wiki") {
          app.dropActive = false;
          if (p.type === "drop") {
            if (shut) toast(t("開いている窓を閉じてからドロップしてください"));
            else void wikiDrop(p.paths ?? []);
          }
          return;
        }
        // 本棚 / 読み手が前面なら捨てる（見えない一覧の取込に流さない = drive モードで Drive に書く事故）
        if (kind === "shelf" || kind === "book") {
          app.dropActive = false;
          return;
        }
        if (shut) {
          app.dropActive = false;
          if (p.type === "drop") toast(t("開いている窓を閉じてからドロップしてください"));
          return;
        }
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
      stopWiki();
      stopRecent();
      stopChat();
      stopViewing();
      unlisten?.();
      unlistenLang?.();
      unlistenUpdateOff?.();
      unlistenMenuToast?.();
      unlistenMigration?.();
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
  <!-- タブの中身（.tabs-page）とチャットの柱（ChatDock）を横に並べる。縮むのは .tabs-page の側（app.css の .dock-row） -->
  <div class="dock-row">
  <!-- タブの中身（ヘッダー・一覧・PDF ビューア）。PDF タブのビューアはこの全面を覆う（柱は覆わない = 柱は .tabs-page の外） -->
  <div class="tabs-page">
    <!-- OpenNote の集中モードではヘッダーを隠す（表示だけ = 検索欄の状態を保つ・ヘッダーの検索欄へ入る `/` も効かせない） -->
    <header style:display={wkFocus ? "none" : null}>
      <!-- OpenNote が前面ならアプリ名も OpenNote -->
      <div class="brand">{kind === "wiki" ? "OpenNote" : "OpenRef"}</div>
      {#if kind === "wiki"}
        <WikiHeader />
      {:else if kind === "shelf"}
        <ShelfHeader />
      {:else}
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
          title={t("英・和 / 題名・著者・抄録・誌名・PDF 本文（/ でフォーカス）")}
          bind:value={searchInput}
          oninput={onSearchInput}
        />
      </div>
      <!-- ＋ 追加 ▾（識別子の欄・PDF を取り込む…・手動で作成… を 1 つに。⌘N はここを開いて識別子の欄へ） -->
      <AddMenu />
      {/if}
      {#if app.busy}<div class="spinner" title={t("処理中")}></div>{/if}
      <!-- 右ペインの開閉（▣ = タブ列の下の最初の段の右端 = 3 モードで同じ場所）。OpenNote は WikiHeader の中の同じ部品・
           PDF / 本の読み手はそれぞれのツールバーの右端（ヘッダーの上を覆う） -->
      {#if kind !== "wiki"}<PaneToggle open={app.detailOpen} ontoggle={toggleDetail} />{/if}
    </header>
    {#if app.updateLatest}
      <!-- 新版のお知らせ（起動時の確認 = state.checkUpdate・ub- 接頭辞）。× はこの起動の間だけ隠す。止めるのは Settings › 起動時に新版を確認 -->
      <div class="ub-bar" role="status">
        <span>{t("新版 ")}<b>{app.updateLatest}</b>{t(" があります。ターミナルでインストールコマンドをもう一度実行すると更新されます")}</span>
        <code class="ub-cmd">{INSTALL_CMD}</code>
        <button class="ub-copy" onclick={copyInstallCmd}>{t("コピー")}</button>
        <button class="ub-x" title={t("この起動の間は隠す（止めるのは 設定 › 起動時に新版を確認）")} aria-label={t("この起動の間は隠す（止めるのは 設定 › 起動時に新版を確認）")} onclick={() => (app.updateLatest = null)}>×</button>
      </div>
    {/if}
    <!-- 右ペインを隠した = 2 列（OpenNote は wk-main-noinfo・一覧は lib-nodetail）。PDF のタブの下の一覧も一覧の規則のまま -->
    <main
      class={[kind === "wiki" ? (wkFocus ? "wk-main-focus" : wk.infoOpen && wk.prefsReady ? undefined : "wk-main-noinfo") : app.detailOpen ? undefined : "lib-nodetail", sbMode && !wkFocus ? "sbw" : undefined]}
      style:--sb-w={sbMode ? `${sidebarW[sbMode]}px` : undefined}>
      {#if kind === "wiki"}
        <!-- 集中モード = 木と右ペインを表示だけ隠す（main.wk-main-focus の CSS = WikiPage。作り直さない = スクロール位置と中身を保つ・
             右ペインの開閉の設定 ui_wiki_info は変えない） -->
        <WikiSidebar />
        <WikiPage />
        {#if wk.infoOpen && wk.prefsReady}<WikiInfo />{/if}
      {:else if kind === "shelf"}
        <ShelfSidebar />
        <!-- タブごとに作り直す = スクロール位置はそのタブの物（一覧の入替方式の外） -->
        {#key app.tab}<ShelfGrid />{/key}
        {#if app.detailOpen}<ShelfRight book={sh.info?.root === frontShelf()?.root ? sh.info : null} />{/if}
      {:else if kind !== "book"}
        <!-- 読み手の下には何も置かない（読み手が全面を覆う） -->
        <Sidebar />
        <ItemList />
        {#if app.detailOpen}<ItemDetail />{/if}
      {/if}
      <!-- 左の列と中央の境界のつまみ（LIBRARY / NOTE / SHELF・集中モードでは置かない）。main の最後の子 = 格子の外（absolute） -->
      {#if sbMode && !wkFocus}<SidebarGrip mode={sbMode} right={sbMode === "wiki" ? wk.infoOpen && wk.prefsReady : app.detailOpen} />{/if}
    </main>
    <!-- id 順で回す（タブの並びには依らない）: 並べ替え（moveTab）で keyed each がビューアの DOM を動かすと、動いた要素の
         scrollTop が 0 に戻りフォーカスも外れる。id は増える一方なので、開く = 末尾に足す・閉じる = 抜くだけで DOM は動かない -->
    {#each [...app.tabs].sort((a, b) => a.id - b.id) as t (t.id)}
      {#if t.viewer && (t.id === app.tab || seen.has(t.id))}
        {#key t.viewer?.attachmentId}
          <PdfViewer viewer={t.viewer} front={t.id === app.tab} onClose={() => void closeTab(t.id)} />
        {/key}
      {/if}
      {#if t.shelfBook && (t.id === app.tab || seen.has(t.id))}
        {#key t.shelfBook?.path}
          <ShelfViewer book={t.shelfBook} front={t.id === app.tab} onClose={() => void closeTab(t.id)} />
        {/key}
      {/if}
    {/each}
  </div>
  <!-- チャットの柱（どのタブ種でも同じ 1 つ = タブを切り替えても作り直さない。門 = OpenNote ON + チャット ON） -->
  {#if chatShown() && chat.dock.open}<ChatDock />{/if}
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
  <MigrationModal />
  <PurgeModal />
  <StyleModal />
  {#if app.editorOpen}<ReferenceEditor />{/if}
  {#if ed.stranded.length}<WikiRescue />{/if}
  {#if app.wikiOn}<WikiChatConsent />{/if}
  <!-- OpenNote のリンクの右クリックのメニュー（ページ・分割のプレビュー・チャットの応答で共通 = 1 つ） -->
  {#if app.wikiOn}<WikiLinkMenu />{/if}
  <!-- OpenNote のリンクのプレビューの浮き窓（本文・バックリンク・検索の結果・チャットの応答で共通 = 1 つ・ShadowPage の contain の外） -->
  {#if app.wikiOn}<WikiPreview />{/if}
  <!-- OpenNote の画像を大きく見る覆い（本文の埋め込みの画像を押す = 全画面） -->
  {#if app.wikiOn}<WikiLightbox />{/if}
</div>
