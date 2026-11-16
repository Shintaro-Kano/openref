<script lang="ts">
  /* OpenNote の小さなダイアログ: 新規ページの名前と形式（md / html）/ 新規フォルダの名前 / 名前の変更（[[旧名]] の書換えの確認）/ ゴミ箱へ。
     木の右クリック・ツールバーの「…」・ヘッダーの [＋ 新規]・ワークスペース名（最近のワークスペースの切替・× で外す・フォルダの設定…）のメニューもここ（wk.menu）。
     「フォルダの設定…」の窓（参照フォルダ・除外フォルダ）の中身は WikiFolders。状態は wiki.svelte.ts・動詞は files.svelte.ts */
  import { t } from "../i18n.svelte";
  import { composing, focusMe, onEnter } from "../input";
  import Modal from "../Modal.svelte";
  import { answerDiscard, forgetWorkspace, openFolderSettings, openPath, openWikiPage, pickRoot, reindex, revealInFinder, rootInUse, setEditing, setExcluded, switchWorkspace, toggleHeadColors, wk, workspaceList, workspaceName } from "./wiki.svelte";
  import { askNewFolder, askNewPage, askRename, askTrash, createFolder, createPage, doRename, doTrash, duplicateFile, editingUnder, folderNameProblem, nameProblem, pageExt, pagesUnder, previewRename, type PageExt } from "./files.svelte";
  import WikiFolders from "./WikiFolders.svelte";
  import { editText } from "./edit.svelte";
  import { baseName } from "./render/asset";
  import { covered, frontWiki, toast } from "../state.svelte";
  import { errText } from "../err";
  import { holdFocus } from "../focusReturn";

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

  /** 前面のページを編集しているか・分割で編集しているか（[⋯] の page のメニューのチェック = 今の状態） */
  const pageEditing = () => !!frontWiki()?.editing;
  const splitOn = () => pageEditing() && wk.split;
  /** 分割: 表示中なら分割で編集へ（前の [分割] のボタンと同じ）・編集中なら エディタだけ ⇄ 分割 */
  function toggleSplit() {
    void setEditing(true, pageEditing() ? !wk.split : true);
  }

  async function copyText() {
    try {
      await navigator.clipboard.writeText(editText());
      copied = true;
    } catch (e) {
      copied = false;
      toast(`✗ ${t("コピーできませんでした")}: ${errText(e, 160)}`);
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
      if (wk.menu) closeMenu(true);
      else close();
    }
  }

  // ---- 浮きメニューのキー操作（どの種類のメニューでも = 焦点がメニューの中にあるとき）----
  // ↑↓ = 項目を移る（区切りと押せない項目を飛ばす・端で回る）・Home / End・Enter / Space = 実行（既定の押下を止めて 1 回だけ click）・
  // Esc = 閉じる・Tab = 閉じる（既定の Tab は奪わない）。キーで開いた（menu.keyboard）ときは最初の項目へ焦点を移し、
  // Esc / Tab / 実行の後に開いたボタン（menu.opener）へ戻す。マウスで開いたときは焦点を動かさない（前のまま）
  let menuEl: HTMLDivElement | undefined = $state();
  const menuItems = () => (menuEl ? [...menuEl.querySelectorAll<HTMLButtonElement>('[role^="menuitem"]:not(:disabled)')] : []);
  $effect(() => {
    const m = wk.menu;
    const el = menuEl;
    if (m?.keyboard && el) queueMicrotask(() => menuItems()[0]?.focus());
  });
  /** 名前のメニューの × = 閉じて名前のボタン（opener）へ焦点を戻してから外す（確認なし）。2 回目の押下（ダブルクリック・Enter の連打）は
      閉じたメニューには当たらない・forgetWorkspace も同じ root の実行中は弾く */
  function forgetFromMenu(root: string) {
    const op = wk.menu?.opener;
    wk.menu = null;
    if (op?.isConnected) op.focus();
    void forgetWorkspace(root);
  }
  /** 閉じる。back = 焦点がメニューの中にあれば opener へ戻す（Esc・Tab） */
  function closeMenu(back: boolean) {
    const op = wk.menu?.opener;
    const inside = !!menuEl?.contains(document.activeElement);
    wk.menu = null;
    if (back && inside && op?.isConnected) op.focus();
  }
  function menuKey(e: KeyboardEvent) {
    if (composing(e) || e.metaKey || e.ctrlKey || e.altKey) return;
    const list = menuItems();
    const i = list.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (!list.length) return;
      const d = e.key === "ArrowDown" ? 1 : -1;
      list[i < 0 ? (d > 0 ? 0 : list.length - 1) : (i + d + list.length) % list.length].focus();
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      (e.key === "Home" ? list[0] : list.at(-1))?.focus();
    } else if ((e.key === "Enter" || e.key === " ") && i >= 0) {
      e.preventDefault(); // 既定の押下（Enter の click・Space の keyup の click）を止めて 1 回だけ
      list[i].click();
    } else if (e.key === "Tab") {
      closeMenu(true); // 焦点を opener に置く = 既定の Tab がそこから次（Shift なら前）へ進む
    }
  }
  /** 項目の実行（キーで開いたメニュー）: 実行の**前**に焦点を opener へ置く（click の capture の段 = 項目の onclick より先）。
      項目が窓を開くなら、その窓が後から自分で焦点を取る（名前の欄・ゴミ箱の [キャンセル] ほか）= 後から opener へ戻し直さない
      （戻し直すと、窓が取った焦点を奪う・覆いの下の [⋯] に焦点が残る）。窓の焦点は開く前の焦点（= opener）を覚えて閉じたら戻す */
  function menuClick() {
    const m = wk.menu;
    if (!m?.keyboard || !m.opener?.isConnected) return;
    m.opener.focus();
  }

  /** 開いたダイアログの既定のボタンへフォーカス（WKWebView はクリックでフォーカスを移さない）。出た瞬間に処理中（disabled={busy}）なら、
      押せるようになってから移す（名前の変更の 2 段目 = 前の段の処理の終わりに出る = 出た時点では disabled で focus() が効かず body に落ちていた）。
      待つのは 1 秒まで（それでも押せなければ移さない） */
  function focusBtn(node: HTMLButtonElement) {
    let tries = 0;
    const go = () => {
      if (!node.isConnected) return;
      if (node.disabled) {
        if (++tries < 60) setTimeout(go, 16);
        return;
      }
      node.focus();
    };
    queueMicrotask(go);
  }

  /** ダイアログ（新規ページ・新規フォルダ・名前の変更・破棄の確認・ゴミ箱）の枠。開いたら（枠が出たとき）開く前の焦点を覚え、閉じたら戻す
      （救出・取込の窓と同じ型 = focusReturn.ts の holdFocus。戻さない場面の決まりも同じ）。窓の中の焦点は各段の欄 / 既定のボタンが取る。
      名前の変更の 1 段目 → 2 段目のように中身が替わっても枠は同じ = 開いたときの 1 回だけ覚える */
  let dlgFrame: HTMLDivElement | null = $state(null);
  $effect(() => {
    const f = dlgFrame;
    if (!f) return;
    return holdFocus(f);
  });

  // 覆い（窓・参照エディタ・取込）が出たら浮きメニューを閉じる（メニュー z 45 は窓の覆い z 40 より上 = 残すと窓の上に重なり、その項目で窓が 2 枚になる）
  $effect(() => {
    if (wk.menu && covered()) wk.menu = null;
  });

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
  <div class="wk-menu" class:wk-menu-vault={m.kind === "vault"} role="menu" tabindex="-1" style:left="{m.x}px" style:top="{m.y}px" bind:this={menuEl} onkeydown={menuKey} onclickcapture={menuClick}>
    {#if m.kind === "vault"}
      <!-- 名前のメニュー（本棚の名前のメニューと同じ型）: ホーム / 最近のワークスペース（各行の × = 履歴から外す）/
           別のフォルダを選ぶ… / フォルダの設定…（参照フォルダ・除外フォルダの窓）。全項目にチェックの欄 = 字の頭が揃う（ページの [⋯] と同じ）。
           長い絶対パスは先頭側を省略して末尾（ワークスペース名）を残す（rtl の型。両端の &lrm; で頭の / や末尾の ) . - _ が反対側へ回らないようにする） -->
      <div class="wk-menu-head wk-menu-head-path wk-menu-head-ind" title={m.path}>&lrm;{m.path}&lrm;</div>
      <button role="menuitem" class="wk-menu-chk" onclick={() => ((wk.menu = null), void openWikiPage(null))}><span class="wk-menu-check" aria-hidden="true"></span><span class="wk-menu-name">{t("ホーム")}</span></button>
      <!-- 最近のワークスペース = 1 クリックで切替。現在は ✓ で押せない・無いフォルダは灰色で押せない。
           × = 履歴から外す（確認なし・前の右ペインの [外す] と同じ command）。外すとそのワークスペースの参照フォルダ・
           除外フォルダの設定・索引・最近開いたページの記録も消える（戻せない・フォルダの中のファイルは消えない）= title と読み上げで言う。
           押したらメニューを閉じて名前のボタンへ焦点（ほかの項目と同じ = 行が詰まって 2 回目の押下が下の行に当たる道を無くす）。
           どれかのタブ・チャットが使っている root（現在を含む）には × を出さない（前の [外す] と同じ = rootInUse）。無いフォルダの行にも出す -->
      <div class="wk-menu-head wk-menu-head-ind">{t("最近のワークスペース")}</div>
      {#each workspaceList() as w (w.root)}
        <div class="wk-menu-wsrow">
          <button
            role="menuitem"
            class="wk-menu-ws wk-menu-chk"
            class:wk-menu-gone={!w.exists && !w.current}
            disabled={w.current || !w.exists}
            title={w.exists || w.current ? w.root : `${w.root}\n${t("見つかりません")}`}
            onclick={() => ((wk.menu = null), void switchWorkspace(w.root))}>
            <span class="wk-menu-check" aria-hidden="true">{w.current ? "✓" : ""}</span><span class="wk-menu-name">{workspaceName(w.root) || w.root}</span>
          </button>
          {#if !rootInUse(w.root)}
            <button
              role="menuitem"
              class="wk-menu-x"
              aria-label={`${t("このワークスペースを履歴から外す（参照フォルダ・除外フォルダの設定・索引・最近開いたページの記録も消えます）")}: ${workspaceName(w.root) || w.root}`}
              title={`${t("このワークスペースを履歴から外す（参照フォルダ・除外フォルダの設定・索引・最近開いたページの記録も消えます）")}\n${w.root}`}
              onclick={() => forgetFromMenu(w.root)}>×</button>
          {/if}
        </div>
      {/each}
      <div class="wk-menu-sep" role="separator"></div>
      <button role="menuitem" class="wk-menu-chk" onclick={() => ((wk.menu = null), void pickRoot())}><span class="wk-menu-check" aria-hidden="true"></span><span class="wk-menu-name">{t("別のフォルダを選ぶ…")}</span></button>
      <button role="menuitem" class="wk-menu-chk" title={t("参照フォルダ・除外フォルダ")} onclick={() => openFolderSettings()}><span class="wk-menu-check" aria-hidden="true"></span><span class="wk-menu-name">{t("フォルダの設定…")}</span></button>
      <!-- 索引を作り直す。command wiki_reindex は引数を取らず設定の root
           （= 前面の OpenNote タブの root に揃えた物）を作り直す = 揃っていればメニューの頭のパスと同じ。揃えられない間（「⚠ ワークスペースを
           切り替えられません」= status.frontError）は前のワークスペースを作り直してしまう → 押せない（title で理由）。
           command を待つ間・作り直しの間（ヘッダーの「索引を作成中… N%」）も押せない -->
      <button
        role="menuitem"
        class="wk-menu-chk"
        disabled={wk.reindexing || !!wk.status?.rebuilding || !!wk.status?.frontError}
        title={wk.status?.frontError ? t("ワークスペースを切り替えられない間は作り直せません（作り直すと前のワークスペースの索引になります）") : t("ワークスペースを全部読み直して索引を作り直す（監視も張り直す）")}
        onclick={() => ((wk.menu = null), void reindex())}><span class="wk-menu-check" aria-hidden="true"></span><span class="wk-menu-name">{t("索引を作り直す")}</span></button>
    {:else if m.kind === "excluded"}
      <!-- 索引から除いたフォルダ（木の薄い行） -->
      <div class="wk-menu-head" title={m.path}>{baseName(m.path)}/</div>
      <button role="menuitem" onclick={() => ((wk.menu = null), void setExcluded(m.path, false))}>{t("索引に入れる")}</button>
      <button role="menuitem" onclick={() => ((wk.menu = null), void revealInFinder(m.path))}>{t("Finder で表示")}</button>
    {:else if m.kind === "file" || m.kind === "page"}
      {#if m.kind === "page"}
        <!-- ページのツールバーの [⋯]: 表示の切替（チェック = 今の状態）→ 外部で開く → ファイルの操作（木の右クリックと同じ 3 つ） -->
        {#if m.canEdit}
          <button role="menuitemcheckbox" class="wk-menu-chk" aria-checked={splitOn()} title={t("左に編集・右にプレビュー")} onclick={() => ((wk.menu = null), toggleSplit())}>
            <span class="wk-menu-check" aria-hidden="true">{splitOn() ? "✓" : ""}</span><span class="wk-menu-name">{t("分割")}</span>
          </button>
        {/if}
        {#if m.md && !pageEditing()}
          <button role="menuitemcheckbox" class="wk-menu-chk" aria-checked={wk.headColors} title={t("見出しをレベル（h1〜h6）ごとに色分けする")} onclick={() => ((wk.menu = null), toggleHeadColors())}>
            <span class="wk-menu-check" aria-hidden="true">{wk.headColors ? "✓" : ""}</span><span class="wk-menu-name">{t("見出しの色")}</span>
          </button>
        {/if}
        {#if m.canEdit || (m.md && !pageEditing())}<div class="wk-menu-sep" role="separator"></div>{/if}
        <button role="menuitem" class="wk-menu-chk" title={t("既定のアプリで開く")} onclick={() => {
            const abs = m.abs ?? ""; // 閉じる前に読む（m は wk.menu の写し = 閉じると null）
            wk.menu = null;
            void openPath(abs);
          }}><span class="wk-menu-check" aria-hidden="true"></span><span class="wk-menu-name">{t("外部で開く")}</span></button>
      {/if}
      <!-- ページの [⋯] では全項目にチェックの欄を取る = 字の頭が揃う（木の右クリックの file のメニューは前のまま） -->
      <button role="menuitem" class:wk-menu-chk={m.kind === "page"} onclick={() => askRename(m.path)}>{#if m.kind === "page"}<span class="wk-menu-check" aria-hidden="true"></span>{/if}{t("名前を変更…")}</button>
      <button role="menuitem" class:wk-menu-chk={m.kind === "page"} onclick={() => void duplicateFile(m.path)}>{#if m.kind === "page"}<span class="wk-menu-check" aria-hidden="true"></span>{/if}{t("複製する")}</button>
      <button role="menuitem" class="wk-menu-danger" class:wk-menu-chk={m.kind === "page"} onclick={() => askTrash(m.path)}>{#if m.kind === "page"}<span class="wk-menu-check" aria-hidden="true"></span>{/if}{t("ゴミ箱に移す…")}</button>
    {:else}
      {#if m.kind === "dir"}<div class="wk-menu-head" title={m.path}>{baseName(m.path)}/</div>{/if}
      <button role="menuitem" onclick={() => askNewPage(m.path)}>{t("新規ページ…")}</button>
      <button role="menuitem" onclick={() => askNewFolder(m.path)}>{t("新規フォルダ…")}</button>
      {#if m.kind === "dir"}
        <button role="menuitem" onclick={() => ((wk.menu = null), void setExcluded(m.path, true))}>{t("索引から除く")}</button>
        <button role="menuitem" class="wk-menu-danger" onclick={() => askTrash(m.path, true)}>{t("フォルダをゴミ箱に移す…")}</button>
      {/if}
    {/if}
  </div>
{/if}

<!-- 覆いと枠は骨格（Modal = .overlay > .modal）。覆いで押して覆いで離したときだけ閉じる（名前の欄で字を選んで外へ出ても閉じない）。
     Esc（IME の変換中は除く）は上の onKey = メニューと共用の受け手のまま（骨格では扱わない）。開いているかの数え方は wk.dialog（wiki.svelte の guard） -->
<Modal open={!!wk.dialog} onbackdrop={close} labelledby="wk-dlg-h" bind:frame={dlgFrame}>
  {#if wk.dialog}
    {@const d = wk.dialog}
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
        <!-- 既定の焦点 = [キャンセル]（安全な側。前は焦点を取らず、押下で焦点を移さない WKWebView ではエディタに打った字が窓の下の本文に入った） -->
        <button use:focusBtn onclick={close}>{t("キャンセル")}</button>
        <button class="danger" disabled={busy} onclick={() => run(() => doTrash(d.path, d.dir))}>{t("ゴミ箱に移す")}</button>
      </div>
    {:else if d.kind === "folders"}
      <WikiFolders onclose={close} />
    {/if}
  {/if}
</Modal>

<style>
  .wk-menu {
    position: fixed; z-index: 45; min-width: 170px; padding: 4px; border: 1px solid var(--border); border-radius: 8px;
    background: var(--panel); box-shadow: 0 8px 24px rgba(0, 0, 0, 0.18); display: flex; flex-direction: column;
  }
  .wk-menu button { text-align: left; padding: 6px 10px; border-radius: 5px; font-size: 12.5px; }
  .wk-menu button:hover { background: var(--accent-soft); }
  /* キーで項目を移ったときだけ輪を出す（ほかの行の作法 = 2px のアクセントの輪・内側） */
  .wk-menu button:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
  .wk-menu-danger { color: var(--danger); }
  .wk-menu-head { padding: 4px 10px 6px; font-size: 11.5px; color: var(--dim); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 260px; }
  .wk-menu-head-path { direction: rtl; text-align: left; }
  /* 押せない項目（作り直し中の「索引を作り直す」）は淡く・ホバーの背景を出さない。現在のワークスペースの行は下の規則で字の色を戻す */
  .wk-menu button:disabled { color: var(--dim); cursor: default; }
  .wk-menu button:disabled:hover { background: transparent; }
  /* 押せない行（現在・無いフォルダ）は .wk-menu button:hover の背景を出さない（同じ形の selector で後に書く = 勝つ） */
  .wk-menu-ws { display: flex; align-items: center; max-width: 260px; }
  .wk-menu button.wk-menu-ws:disabled { color: var(--text); cursor: default; }
  .wk-menu button.wk-menu-ws:disabled:hover { background: transparent; }
  .wk-menu button.wk-menu-ws.wk-menu-gone { color: var(--dim); opacity: 0.6; }
  .wk-menu-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wk-menu-check { flex: none; width: 16px; color: var(--accent); }
  /* チェックつきの項目（ページの [⋯] の分割・見出しの色）= 左にチェックの欄 */
  .wk-menu-chk { display: flex; align-items: center; }
  .wk-menu-sep { height: 1px; margin: 4px 6px; background: var(--border); }
  /* 名前のメニュー: 見出しの字の頭も項目の字の頭（チェックの欄 16px の右）に揃える・ワークスペースの行の右端に × */
  .wk-menu-head-ind { padding-left: 26px; }
  /* 名前のメニューの幅は中身で縮めない（長い項目を外した後、170px まで縮んで「別のフォルダを選ぶ…」が右端に付いた）。本棚の名前のメニュー（330px）より
     狭い = 左バー（最小 180px）から出すので、出す位置の上限（WikiSidebar の innerWidth − 280）に収める */
  .wk-menu.wk-menu-vault { width: 260px; }
  .wk-menu-wsrow { display: flex; align-items: center; gap: 2px; }
  .wk-menu-wsrow .wk-menu-ws { flex: 1; min-width: 0; }
  .wk-menu button.wk-menu-x { flex: none; width: 24px; padding: 4px 0; text-align: center; color: var(--dim); font-size: 13px; line-height: 1; }
  .wk-menu button.wk-menu-x:hover { color: var(--text); background: var(--bg); }
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
