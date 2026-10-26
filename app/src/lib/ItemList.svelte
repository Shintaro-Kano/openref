<script lang="ts">
  import { t } from "./i18n.svelte";
  import LabelBadge from "./LabelBadge.svelte";
  import ListToolbar from "./ListToolbar.svelte";
  import {
    app,
    autoSaveAndCloseNote,
    beginRowDrag,
    checkOnly,
    closeNoteEditor,
    filterByAuthor,
    filterByJournal,
    filterByLabel,
    consumeDragClick,
    noteDirty,
    openNoteEditor,
    saveNote,
    select,
    toggleCheck,
  } from "./state.svelte";

  /** メモ欄のキー: ⌘Enter = 保存して閉じる / Esc = 未編集なら閉じる（編集中は何もしない）。一覧の Esc 連鎖には渡さない */
  function onNoteKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      closeNoteEditor();
    } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      void saveNote(true);
    }
  }

  /** 行クリック: 素 = その行だけをチェック（他は解除）+ 詳細表示。詳細に出ている行で、チェックもその行だけなら（2 回目のクリック）
      外して詳細を閉じる / shift = 範囲チェック / ⌘ = チェック反転（蓄積）のみ、詳細は変えない（詳細の行を外したときだけ閉じる）。
      ☐ までカーソルを運ばせない方針で、蓄積型は煩わしいので単一選択にする。
      開いているメモが別の行のものなら、先に自動保存して閉じる（保存を待つ間に別のタブへ切り替わったら、そのクリックは捨てる） */
  async function onRowClick(e: MouseEvent, key: string) {
    if (consumeDragClick()) return;
    if (app.noteEditor && app.noteEditor.key !== key) {
      const tab = app.tab;
      await autoSaveAndCloseNote();
      if (app.tab !== tab) return;
    }
    if (e.shiftKey) toggleCheck(key, true);
    else if (e.metaKey || e.ctrlKey) toggleCheck(key);
    else checkOnly(key);
  }

  /** 行のキー: Enter = その行だけをチェック + 詳細表示（2 回目でも外さない）/ Space = チェック反転（shift で範囲）。
      行の中のボタン（☐・著者 / 誌名 / ラベルのチップ）から上がってきたキーは扱わない（ボタン自身の click に任せる） */
  function onRowKey(e: KeyboardEvent, key: string) {
    if (e.target !== e.currentTarget) return;
    if (e.key === "Enter") void select(key);
    else if (e.key === " ") {
      e.preventDefault();
      toggleCheck(key, e.shiftKey);
    }
  }

  /** 本文抜粋をマーカー（U+E000 … U+E001）で分割して <mark> 用の区間に分ける */
  function snippetParts(s: string): { text: string; hit: boolean }[] {
    const out: { text: string; hit: boolean }[] = [];
    const re = /\uE000([^\uE001]*)\uE001/g;
    let last = 0;
    for (const m of s.matchAll(re)) {
      if (m.index! > last) out.push({ text: s.slice(last, m.index), hit: false });
      out.push({ text: m[1], hit: true });
      last = m.index! + m[0].length;
    }
    if (last < s.length) out.push({ text: s.slice(last), hit: false });
    return out;
  }
</script>

<section class="list">
  <ListToolbar />
  <div class="rows">
    {#if app.items.length === 0}
      {#if app.view.kind === "tag" && !app.search && !app.filters.author && !app.filters.journal}
        <!-- Paperpile: "No references with this label / To assign a label, drag it onto one or more references" -->
        <div class="empty lv-empty">
          <svg viewBox="0 0 16 16" width="22" height="22" aria-hidden="true"
            ><path
              d="M2 2.8C2 2.36 2.36 2 2.8 2h4.6c.2 0 .4.08.55.23l5.8 5.8c.33.33.33.85 0 1.18l-4.17 4.17a.83.83 0 0 1-1.18 0l-5.8-5.8A.78.78 0 0 1 2 7V2.8Z"
              fill="currentColor" opacity="0.55" /></svg>
          <b>{t("このラベルの論文はありません")}</b>
          <span>{t("ラベルを付けるには、サイドバーのラベルを論文へドラッグするか、論文をチェックしてツールバーの「ラベル ▾」から")}</span>
        </div>
      {:else}
        <div class="empty">
          {app.search ? `${t("「::no-match")}${app.search}${t("」に一致なし")}` : t("（アイテムなし）")}
        </div>
      {/if}
    {/if}
    {#each app.items as it (it.key)}
      <div
        class="row"
        data-drop="row:{it.key}"
        class:selected={app.selected === it.key}
        class:checked={app.checked.has(it.key)}
        class:drophover={app.drag?.kind === "label" && app.drag?.over === `row:${it.key}`}
        role="button"
        tabindex="0"
        onpointerdown={(e) => beginRowDrag(e, it.key, it.title)}
        onclick={(e) => onRowClick(e, it.key)}
        onkeydown={(e) => onRowKey(e, it.key)}>
        <button
          class="cb"
          class:on={app.checked.has(it.key)}
          aria-label={t("選択::checkbox")}
          aria-pressed={app.checked.has(it.key)}
          onpointerdown={(e) => e.stopPropagation()}
          onclick={(e) => {
            e.stopPropagation();
            toggleCheck(it.key, e.shiftKey);
          }}></button>
        <span class="star">{it.starred ? "★" : ""}</span>
        <span class="meta">
          {#if it.firstAuthor && it.firstAuthor !== "?"}
            <!-- 筆頭著者クリック = 著者チップで絞り込み（行の選択・ドラッグには伝播させない） -->
            <button
              class="fchip"
              title={t("この著者で絞り込む")}
              onpointerdown={(e) => e.stopPropagation()}
              onclick={(e) => { e.stopPropagation(); void filterByAuthor(it.firstAuthor, it.firstAuthorGiven); }}>{it.firstAuthor}</button>
          {:else}{it.firstAuthor}{/if}
          <br /><span class="year">{it.year ?? "—"}</span>
        </span>
        <span class="body">
          <span class="title">{#if it.retracted}<span class="retracted">{t("⚠撤回")}</span>{/if}{it.title}{#each it.tags as tag (tag.id)}<button
                class="row-lbl"
                title={`${t("「::filter")}${tag.name}${t("」で絞り込む")}`}
                onpointerdown={(e) => e.stopPropagation()}
                onclick={(e) => { e.stopPropagation(); void filterByLabel(tag.id); }}><LabelBadge name={tag.name} color={tag.color} /></button>{/each}</span>
          <span class="journal">
            {#if it.container}
              <button
                class="fchip"
                title={t("この誌名で絞り込む")}
                onpointerdown={(e) => e.stopPropagation()}
                onclick={(e) => { e.stopPropagation(); void filterByJournal(it.container); }}>{it.container}</button>
            {/if}
          </span>
          {#if it.fulltextSnippet}
            <!-- 検索語が PDF 本文に一致: 抜粋（Paperpile の full-text search）。一致箇所はマーカー U+E000/U+E001 → <mark> -->
            <span class="ft-snip" title={t("PDF 本文の一致箇所")}>
              <span class="ft-label">{t("本文:")}</span>
              {#each snippetParts(it.fulltextSnippet) as part, i (i)}
                {#if part.hit}<mark class="ft-hl">{part.text}</mark>{:else}{part.text}{/if}
              {/each}
            </span>
          {/if}
        </span>
        <span class="badges">
          {#if it.hasNote}
            <button
              class="note"
              title={t("メモを開く")}
              onpointerdown={(e) => e.stopPropagation()}
              onclick={(e) => {
                e.stopPropagation();
                if (app.noteEditor?.key === it.key) closeNoteEditor();
                else void openNoteEditor(it.key);
              }}>{t("メモ")}</button>
          {/if}
          <span class="pdf">{it.hasPdf ? "PDF" : ""}</span>
        </span>
      </div>
      {#if app.noteEditor && app.noteEditor.key === it.key}
        <!-- 行下のインラインメモ（Paperpile の note エディタ） -->
        <div class="ne-box" role="group" aria-label={t("メモ")}>
          <textarea
            class="ne-text"
            placeholder={t("この文献についてのメモ（Markdown 可）")}
            bind:value={app.noteEditor.text}
            onkeydown={onNoteKey}
            autofocus></textarea>
          <div class="ne-foot">
            {#if app.noteEditor.noteKey}
              <button
                class="ne-del"
                disabled={app.busy}
                title={t("メモを削除")}
                onclick={() => { if (app.noteEditor) { app.noteEditor.text = ""; void saveNote(true); } }}>{t("削除")}</button>
            {/if}
            <span class="ne-spacer"></span>
            {#if noteDirty()}<span class="ne-dirty">{t("未保存")}</span>{/if}
            <button onclick={() => closeNoteEditor(true)}>{noteDirty() ? t("破棄して閉じる") : t("閉じる")}</button>
            <button class="primary" disabled={app.busy || !noteDirty()} onclick={() => saveNote(false)}>{t("保存")}</button>
            <button class="primary" disabled={app.busy} onclick={() => saveNote(true)}>{t("保存して閉じる")}</button>
          </div>
        </div>
      {/if}
    {/each}
  </div>
</section>
