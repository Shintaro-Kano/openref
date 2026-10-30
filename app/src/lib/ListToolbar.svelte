<script lang="ts">
  import { t, tn } from "./i18n.svelte";
  /* 一覧上部の一括操作ツールバー [☑] [フォルダ ▾][ラベル ▾][ゴミ箱] [★][引用 ▾]。
     Paperpile の [☑▾][📁▾][🏷▾][🗑] | Cite ▾ | More ▾ から ☑ の ▾ と More ▾ を引いたもの
     （1 操作 1 場所: 一括できる操作はここにだけ置き、1 件専用の操作（メモ・PDF 取得）はツールバーに置かない）。
     フォルダ/ラベルのピッカーは選択中 item 群の所属を三状態（全部 ✓ / 一部 − / なし）で示す。
     フォルダはクリックで即反映（全部なら外す・それ以外は全件に付ける）、ラベルは Paperpile と同じく
     チェックを切り替えて「Apply changes」でまとめて反映（閉じれば破棄）。
     ゴミ箱ビューは Paperpile の Trash と同じく [☑] [✕ 完全に削除] [↩ 復元] の 3 操作だけに絞る
     （フォルダ・ラベル・★・引用はゴミ箱の中では出さない）。 */
  import { untrack } from "svelte";
  import { api } from "./api";
  import LabelBadge from "./LabelBadge.svelte";
  import { composing } from "./input";
  import { nocase } from "./sort";
  import {
    app,
    askPurgeChecked,
    bulkAddToCollection,
    bulkCopy,
    bulkRemoveFromCollection,
    bulkRestore,
    bulkStar,
    bulkTrash,
    checkAll,
    checkedKeys,
    clearChecks,
    createCollectionAndAdd,
    applyLabelChanges,
    createLabel as createLabelNamed,
    flatFolders,
    folderNameError,
    folderPath,
    labelNameTaken,
    openStyleModal,
    styleTitle,
  } from "./state.svelte";

  type Menu = typeof app.listMenu;

  let q = $state("");
  /** 開いているピッカー向けの所属件数（id → 件数） */
  let member = $state<Record<number, number>>({});
  /** ラベル ▾ の保留中の変更（Paperpile: チェックを切り替えて「Apply changes」でまとめて反映。閉じれば破棄） */
  let pending = $state<Record<number, "all" | "none">>({});
  const changed = $derived(Object.keys(pending).length > 0);
  let inputEl = $state<HTMLInputElement | null>(null);

  const n = $derived(app.checked.size);
  const total = $derived(app.items.length);
  const any = $derived(n > 0);
  const all = $derived(total > 0 && n >= total);
  const inTrash = $derived(app.view.kind === "trash");
  /** チェック中の全件が ★ 済みなら「解除」、1 件でも未 ★ なら「登録」（Paperpile の star トグル） */
  const allStarred = $derived(any && app.items.every((i) => !app.checked.has(i.key) || i.starred));

  const needle = $derived(q.trim().toLowerCase());
  /** フォルダの候補 = 木の順（名前順・折畳なし）+ 段の字下げ */
  const folderCands = $derived(flatFolders({ sort: "alpha" }).filter((r) => r.c.name.toLowerCase().includes(needle)));
  /** 別の親の下に同名（大小無視 = nocase）があるフォルダ名（nocase で畳んだもの）。候補に親の名前を添えて見分ける */
  const dupNames = $derived.by(() => {
    const n = new Map<string, number>();
    for (const c of app.collections) n.set(nocase(c.name), (n.get(nocase(c.name)) ?? 0) + 1);
    return new Set([...n].filter(([, k]) => k > 1).map(([name]) => name));
  });
  const labelCands = $derived(app.tags.filter((t) => t.name.toLowerCase().includes(needle)));
  /** 「作成して追加」は root に作る = New folder と同じ規則（`/` 不可・root の兄弟の中で大小無視一意）。
      作れない理由（`/`）は候補が無いときに出す */
  const folderErr = $derived(folderNameError(q, null));
  const canCreateFolder = $derived(folderErr === null);
  const canCreateLabel = $derived(needle.length > 0 && !labelNameTaken(q));

  async function loadMember(kind: "folder" | "label") {
    const ms = await api.membership(checkedKeys());
    const rec: Record<number, number> = {};
    for (const [id, n] of kind === "folder" ? ms.collections : ms.tags) rec[id] = n;
    member = rec;
  }

  async function toggleMenu(m: Menu) {
    if (app.listMenu === m) {
      app.listMenu = null;
      return;
    }
    app.listMenu = m;
    q = "";
    member = {};
    pending = {};
    if (m === "folder" || m === "label") {
      await loadMember(m);
      setTimeout(() => inputEl?.focus(), 0);
    }
  }

  function close() {
    app.listMenu = null;
  }

  // モーダル（New label / Manage labels / 引用スタイル等）が開いたらツールバーのメニューは閉じる（保留中のラベル変更は破棄）。
  // サイドバーのボタンはクリックを止めるので外側クリック判定では閉じられない
  $effect(() => {
    if (app.modal) untrack(() => (app.listMenu = null));
  });

  function stateOf(id: number): "all" | "some" | "none" {
    const c = member[id] ?? 0;
    if (c === 0) return "none";
    return c >= n ? "all" : "some";
  }

  async function toggleFolder(id: number) {
    if (stateOf(id) === "all") await bulkRemoveFromCollection(id);
    else await bulkAddToCollection(id);
    await loadMember("folder");
  }

  /** 表示上の状態 = 保留中の変更 → 無ければ現在の所属 */
  function eff(id: number): "all" | "some" | "none" {
    return pending[id] ?? stateOf(id);
  }

  /** クリックで 全部 ⇄ なし（一部だけに付いていたものは 一部 → 全部 → なし → 一部 の順に戻れる） */
  function toggleLabel(id: number) {
    const orig = stateOf(id);
    const cur = eff(id);
    const next: "all" | "some" | "none" =
      cur === "all" ? "none" : cur === "none" ? (orig === "some" ? "some" : "all") : "all";
    const p = { ...pending };
    if (next === orig) delete p[id];
    else p[id] = next as "all" | "none";
    pending = p;
  }

  async function applyLabels() {
    const add = Object.entries(pending).filter(([, v]) => v === "all").map(([k]) => Number(k));
    const remove = Object.entries(pending).filter(([, v]) => v === "none").map(([k]) => Number(k));
    close();
    await applyLabelChanges(add, remove);
  }

  async function createFolder() {
    await createCollectionAndAdd(q);
    q = "";
    await loadMember("folder");
  }

  /** 検索欄の語で新しいラベルを作り、保留中の「付ける」に入れる（反映は Apply changes） */
  async function createLabel() {
    const id = await createLabelNamed(q);
    if (id === null) return;
    q = "";
    pending = { ...pending, [id]: "all" };
    inputEl?.focus();
  }

  /** ピッカー検索欄の Enter: 候補が 1 件ならそれをトグル、無ければ新規作成 */
  function onPickerKey(e: KeyboardEvent, kind: "folder" | "label") {
    // IME の変換確定の Enter では作成・切替しない
    if (e.key !== "Enter" || composing(e)) return;
    e.preventDefault();
    if (kind === "folder") {
      if (folderCands.length === 1) void toggleFolder(folderCands[0].c.id);
      else if (canCreateFolder) void createFolder();
    } else {
      if (labelCands.length === 1) toggleLabel(labelCands[0].id);
      else if (canCreateLabel) void createLabel();
      else if (!needle && changed) void applyLabels();
    }
  }
</script>

<!-- ツールバーの外をクリックしたらメニューを閉じる（内側は closest で除外） -->
<svelte:window
  onclick={(e) => {
    if (app.listMenu && !(e.target as Element | null)?.closest?.(".listbar")) close();
  }} />

<div class="listbar" aria-label={t("一括操作")}>
  <!-- 選択（クリックで すべて選択 ⇄ 選択解除。⌘A / Esc と同じ） -->
  <div class="lb-group">
    <button
      class="lb-btn lb-checkbtn"
      title={all ? t("選択解除（Esc）") : t("すべて選択（⌘A）")}
      onclick={() => (all ? clearChecks() : checkAll())}>
      <span class="cb" class:on={all} class:some={any && !all}></span>
    </button>
  </div>

  {#if inTrash}
    <!-- ゴミ箱: 完全に削除 / 復元（Paperpile の Delete permanently / Restore）。削除は確認モーダルを挟む -->
    <div class="lb-group" class:disabled={!any}>
      <button
        class="lb-btn danger"
        disabled={!any}
        title={t("選択した item をライブラリから完全に削除（取り消せません）")}
        onclick={askPurgeChecked}>{t("✕ 完全に削除")}</button>
      <button class="lb-btn" disabled={!any} title={t("選択した item を復元")} onclick={bulkRestore}>{t("↩ 復元")}</button>
    </div>
  {:else}
    <!-- フォルダ / ラベル / ゴミ箱 -->
    <div class="lb-group" class:disabled={!any}>
      <div class="lb-wrap">
        <button class="lb-btn" disabled={!any} title={t("フォルダへ追加 / 外す")} onclick={() => toggleMenu("folder")}>
          {t("フォルダ")} <span class="caret">▾</span>
        </button>
        {#if app.listMenu === "folder"}
          <div class="lb-pop lb-picker">
            <input
              bind:this={inputEl}
              placeholder={t("フォルダを検索または新規作成")}
              bind:value={q}
              onkeydown={(e) => onPickerKey(e, "folder")} />
            <div class="pop-list">
              {#each folderCands as { c, depth } (c.id)}
                <button class="pop-row" onclick={() => toggleFolder(c.id)}>
                  <span class="cb" class:on={stateOf(c.id) === "all"} class:some={stateOf(c.id) === "some"}></span>
                  <span class="name" style="padding-left:{depth * 14}px"
                    >{c.name}{#if c.parentId !== null && dupNames.has(nocase(c.name))}<span class="lb-path"> — {folderPath(c.id)}</span>{/if}</span>
                  <span class="n">{c.nItems}</span>
                </button>
              {/each}
              {#if canCreateFolder}
                <button class="pop-row create" onclick={createFolder}>{t("＋ 「")}{q.trim()}{t("」を作成して追加")}</button>
              {/if}
              {#if folderCands.length === 0 && !canCreateFolder}
                <div class="lb-empty">{folderErr || t("（フォルダなし）")}</div>
              {/if}
            </div>
          </div>
        {/if}
      </div>

      <div class="lb-wrap">
        <button class="lb-btn" disabled={!any} title={t("ラベルを付与 / 外す")} onclick={() => toggleMenu("label")}>
          {t("ラベル")} <span class="caret">▾</span>
        </button>
        {#if app.listMenu === "label"}
          <!-- Paperpile のラベル ▾: チェックを切り替え → 「Apply changes」でまとめて反映（閉じれば破棄） -->
          <div class="lb-pop lb-picker">
            <input
              bind:this={inputEl}
              placeholder="Search or create new label"
              bind:value={q}
              onkeydown={(e) => onPickerKey(e, "label")} />
            <div class="pop-list">
              {#each labelCands as t (t.id)}
                <button class="pop-row" class:changed={pending[t.id] !== undefined} onclick={() => toggleLabel(t.id)}>
                  <span class="cb" class:on={eff(t.id) === "all"} class:some={eff(t.id) === "some"}></span>
                  <span class="name"><LabelBadge name={t.name} color={t.color} /></span>
                </button>
              {/each}
              {#if canCreateLabel}
                <button class="pop-row create" onclick={createLabel}>{t("＋ Create label「")}{q.trim()}{t("」")}</button>
              {/if}
              {#if labelCands.length === 0 && !canCreateLabel}
                <div class="lb-empty">{t("（ラベルなし）")}</div>
              {/if}
            </div>
            {#if changed}
              <button class="lb-apply" onclick={applyLabels}>Apply changes</button>
            {/if}
          </div>
        {/if}
      </div>

      <button class="lb-btn danger" disabled={!any} title={t("選択した item をゴミ箱へ（復元可）")} onclick={bulkTrash}>{t("ゴミ箱")}</button>
    </div>

    <!-- Star / 引用 -->
    <div class="lb-group" class:disabled={!any}>
      <button
        class="lb-btn lb-star"
        class:on={allStarred}
        disabled={!any}
        title={allStarred ? t("選択した item の ★ を解除") : t("選択した item に ★")}
        onclick={() => bulkStar(!allStarred)}>★</button>
      <div class="lb-wrap">
        <button class="lb-btn" disabled={!any} title={t("選択した item の引用をまとめてコピー")} onclick={() => toggleMenu("cite")}>
          {t("引用::button")} <span class="caret">▾</span>
        </button>
        {#if app.listMenu === "cite"}
          <!-- Paperpile の Cite ▾: Copy to clipboard（Citation / BibTeX / Citation key / LaTeX citation / DOI）+ Change citation style… -->
          <div class="lb-pop lb-menu">
            <div class="lb-menu-title">{t("クリップボードへコピー（")}{tn(n, "件）")}</div>
            <button onclick={() => { void bulkCopy("citation"); close(); }}>
              <span>{t("引用")}</span><kbd class="lb-style-name">{styleTitle(app.cite.style)}</kbd>
            </button>
            <button onclick={() => { void bulkCopy("bibtex"); close(); }}><span>BibTeX</span></button>
            <button onclick={() => { void bulkCopy("citekey"); close(); }}><span>{t("引用キー")}</span></button>
            <button onclick={() => { void bulkCopy("latex"); close(); }}><span>{t("LaTeX 引用")}</span><kbd>\cite{"{…}"}</kbd></button>
            <button onclick={() => { void bulkCopy("doi"); close(); }}><span>DOI</span></button>
            <div class="lb-sep"></div>
            <button onclick={openStyleModal}><span>{t("引用スタイルを変更…")}</span></button>
          </div>
        {/if}
      </div>
    </div>

  {/if}

  <div class="lb-spacer"></div>
  <div class="lb-count">{tn(total, "件")}{#if any}<b> · {n} {t("選択")}</b>{/if}</div>
</div>
