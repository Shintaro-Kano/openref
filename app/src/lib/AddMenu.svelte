<script lang="ts">
  /* ヘッダーの「＋ 追加 ▾」（識別子の欄 QuickAdd と「＋ 新規 ▾」を 1 つにした物）。開くと最上段が識別子の欄（PMID / DOI /
     PubMed の URL・id="quickadd" = ⌘N はここを開いて焦点を入れる = App）、その下に「PDF を取り込む…」「手動で作成… ⌘⇧N」。
     - Enter（IME の確定の Enter は除く）= 登録（quickAdd = 処理中はヘッダーのスピナー・結果と失敗は toast = 前と同じ）。押したら閉じる
       （欄は空に戻す = 前と同じ。続けて足すときは ⌘N をもう一度）
     - 登録中（app.busy）も欄は無効にしない（無効の欄には焦点が入らず、⌘N の後に貼った識別子が検索欄に落ちていた）。
       登録中の Enter は何もしない（貼った物を消さない・欄の横にスピナー）= 前の登録が済んだら、そのまま Enter で登録できる。
       登録が済んでも開いている欄は閉じない・貼りかけを消さない（閉じるのは Enter・Esc・外側のクリック・覆い・ほかのメニューだけ）
     - 閉じる: Esc（App の Esc の連鎖）・外側のクリック（App の window の click）・もう一度ボタン・項目を押す。閉じたら開く前の焦点へ
       戻す（focusReturn の holdFocus = 窓と同じ規則）
     - 欄の文字は閉じても持ったまま（前の欄と同じ = 貼りかけを失わない）。開いている間は覆いではない（covered() に入れない = 前の欄と同じ）
     - 窓（覆い）が出たら閉じる（⌘⇧N・ドロップの取込・メニューバーの項目 = 窓の下に開いたまま残り、焦点が裏の欄に残っていた）。
     - ほかのメニュー（左バーの … / フォルダの行 / 一覧のツールバー = app.labelMenu / folderMenu / listMenu）とは同時に開かない:
       開くときにそれらを閉じ、それらが開いたら閉じる（それらどうしの閉じ合いと同じ型） */
  import { untrack } from "svelte";
  import { holdFocus } from "./focusReturn";
  import { t } from "./i18n.svelte";
  import { composing } from "./input";
  import { app, covered, openEditorNew, openImportDialog, quickAdd } from "./state.svelte";

  let value = $state("");

  // 覆いが出たら閉じる（焦点は窓が取る = holdFocus は窓が増えたときは戻さない）
  $effect(() => {
    if (app.newMenu && covered()) untrack(() => (app.newMenu = false));
  });
  // 開いたら、ほかのメニューを閉じる / ほかのメニューが開いたら閉じる
  $effect(() => {
    if (!app.newMenu) return;
    untrack(() => {
      app.labelMenu = false;
      app.folderMenu = null;
      app.listMenu = null;
    });
  });
  $effect(() => {
    if (app.labelMenu || app.folderMenu || app.listMenu) untrack(() => (app.newMenu = false));
  });

  function onKey(e: KeyboardEvent) {
    if (e.key !== "Enter" || composing(e)) return;
    e.preventDefault();
    const v = value.trim();
    if (!v || app.busy) return; // 登録中は何もしない（貼った物は持ったまま = 済んだらもう一度 Enter）
    value = "";
    app.newMenu = false;
    void quickAdd(v);
  }
  /** 開いた枠: 開く前の焦点を覚え（閉じたら戻す）、識別子の欄へ焦点を入れる */
  function frame(node: HTMLElement) {
    const back = holdFocus(node);
    const input = node.querySelector<HTMLInputElement>("#quickadd");
    queueMicrotask(() => input?.focus());
    return { destroy: back };
  }
</script>

<div class="nm-wrap">
  <button class="newref" aria-haspopup="menu" aria-expanded={app.newMenu} onclick={() => (app.newMenu = !app.newMenu)} title={t("識別子で登録・PDF を取り込む・手動で作成（⌘N）")}>
    {t("＋ 追加")} <span class="nm-caret">▾</span>
  </button>
  {#if app.newMenu}
    <div class="nm-menu" role="menu" use:frame>
      <input
        id="quickadd"
        class="nm-qa"
        placeholder={t("PMID / DOI / PubMed の URL を貼って Enter")}
        aria-label={t("識別子で登録（PMID / DOI / PubMed の URL）")}
        autocomplete="off"
        spellcheck="false"
        bind:value
        onkeydown={onKey} />
      {#if app.busy}<div class="spinner nm-qa-busy" title={t("処理中")} aria-label={t("処理中")} role="status"></div>{/if}
      <div class="nm-sep"></div>
      <button role="menuitem" onclick={() => { app.newMenu = false; void openImportDialog(); }}>
        <span>{t("PDF を取り込む…")}</span>
      </button>
      <button role="menuitem" onclick={() => { app.newMenu = false; openEditorNew(); }}>
        <span>{t("手動で作成…")}</span><kbd>⌘⇧N</kbd>
      </button>
    </div>
  {/if}
</div>
