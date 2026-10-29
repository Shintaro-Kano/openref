<script lang="ts">
  import { t } from "./i18n.svelte";
  import { api } from "./api";
  import LabelBadge from "./LabelBadge.svelte";
  import {
    app,
    askPurgeSelected,
    attachViaDialog,
    fetchPdfSelected,
    filterByLabel,
    openEditorFor,
    openNoteEditor,
    openTab,
    openViewer,
    restoreSelected,
    setView,
  } from "./state.svelte";

  const d = $derived(app.detail);
  const c = $derived(d?.csl ?? {});
  const authors = $derived(
    ((c.author ?? []) as any[]).map((a) =>
      a.literal ? a.literal : `${a.family ?? ""} ${a.given ?? ""}`.trim()
    )
  );
  const journalLine = $derived.by(() => {
    let s = c["container-title"] ?? "";
    const year = c.issued?.["date-parts"]?.[0]?.[0];
    if (year) s += `  (${year})`;
    if (c.volume) s += `  ${c.volume}`;
    if (c.issue) s += `(${c.issue})`;
    if (c.page) s += `:${c.page}`;
    return s;
  });
  const mesh = $derived((c.custom?.mesh ?? []) as string[]);
  const primaryPdf = $derived(d?.attachments.find((a) => a.exists) ?? null);

  /** フォルダのチップ → そのフォルダのビュー（ラベルの filterByLabel と同じく、表示中のフォルダなら何もしない）。
      ⌘クリック・中クリック = 新しいタブで開く */
  function openCollection(e: MouseEvent, id: number, name: string) {
    if (e.metaKey || e.button === 1) {
      e.preventDefault();
      void openTab({ kind: "collection", id, name });
      return;
    }
    if (app.view.kind === "collection" && app.view.id === id) return;
    void setView({ kind: "collection", id, name });
  }
</script>

<aside class="detail">
  {#if !d}
    <div class="empty">{t("アイテムを選択してください")}</div>
  {:else}
    {#if d.retraction}
      <div class="banner">{t("⚠ 撤回論文の可能性:")} {d.retraction}</div>
    {/if}
    {#if d.deleted}
      <div class="banner trash">
        {t("ゴミ箱内のアイテムです")}
        <button onclick={restoreSelected}>{t("↩ 復元")}</button>
        <button class="danger" title={t("ライブラリから完全に削除（取り消せません）")} onclick={askPurgeSelected}>{t("✕ 完全に削除")}</button>
      </div>
    {/if}

    <h2>{c.title ?? "(no title)"}</h2>
    {#if authors.length}<p class="authors">{authors.join("; ")}</p>{/if}
    {#if journalLine}<p class="journal">{journalLine}</p>{/if}

    <div class="actions">
      {#if primaryPdf}
        <button class="primary" title={t("新しいタブで開く")} onclick={() => void openViewer(d.key, primaryPdf.id, c.title ?? "")}>
          {t("PDF を読む")}
        </button>
        <button onclick={() => api.openAttachment(primaryPdf.id)}>{t("外部で開く")}</button>
      {:else}
        <button class="primary" onclick={fetchPdfSelected}>{t("PDF 自動取得")}</button>
        <button onclick={attachViaDialog}>{t("添付…")}</button>
      {/if}
      <button onclick={() => openEditorFor(d.key, c)}>{t("編集")}</button>
    </div>

    <dl class="ids">
      {#if c.DOI}
        <dt>DOI</dt>
        <dd><button class="link" onclick={() => api.openExternal(`https://doi.org/${c.DOI}`)}>{c.DOI}</button></dd>
      {/if}
      {#if c.PMID}
        <dt>PMID</dt>
        <dd><button class="link" onclick={() => api.openExternal(`https://pubmed.ncbi.nlm.nih.gov/${c.PMID}/`)}>{c.PMID}</button></dd>
      {/if}
      {#if c.PMCID}
        <dt>PMCID</dt>
        <dd><button class="link" onclick={() => api.openExternal(`https://pmc.ncbi.nlm.nih.gov/articles/${c.PMCID}/`)}>{c.PMCID}</button></dd>
      {/if}
    </dl>

    {#if d.collections.length}
      <div class="section">
        <h3>{t("フォルダ")}</h3>
        <div class="chips">
          {#each d.collections as col (col.id)}
            <button
              class="chip"
              title={t("このフォルダを開く（⌘クリックで新しいタブ）")}
              onclick={(e) => {
                if (e.button === 0) openCollection(e, col.id, col.name);
              }}
              onauxclick={(e) => {
                if (e.button === 1) openCollection(e, col.id, col.name);
              }}>{col.name}</button>
          {/each}
        </div>
      </div>
    {/if}

    {#if d.tags.length}
      <div class="section">
        <h3>{t("ラベル")}</h3>
        <div class="chips">
          {#each d.tags as tag (tag.id)}
            <button class="dl-tagbtn" title={t("このラベルで絞り込む")} onclick={() => filterByLabel(tag.id)}>
              <LabelBadge name={tag.name} color={tag.color} />
            </button>
          {/each}
        </div>
      </div>
    {/if}

    <div class="section">
      <h3>{t("メモ")}</h3>
      {#each d.notes as nt (nt.key)}
        <div class="note-card" class:other={nt.author !== "user"}>
          {#if nt.author !== "user"}<span class="note-author">{nt.author}</span>{/if}
          <p class="note-body">{nt.contentMd}</p>
          {#if nt.author === "user"}<button class="link" onclick={() => openNoteEditor(d.key, nt.key)}>{t("メモを編集…")}</button>{/if}
        </div>
      {/each}
      <button class="link" onclick={() => openNoteEditor(d.key, null)}>
        {t("＋ メモを書く…")}
      </button>
    </div>

    {#if c.abstract}
      <div class="section">
        <h3>Abstract</h3>
        <p class="abstract">{c.abstract}</p>
      </div>
    {/if}

    {#if c.keyword || mesh.length}
      <div class="section">
        <h3>Keywords / MeSH</h3>
        <p class="kw">{[c.keyword, mesh.join("; ")].filter(Boolean).join(" · ")}</p>
      </div>
    {/if}

    <div class="section attachments">
      <h3>{t("添付")}</h3>
      {#if d.attachments.length === 0}
        <p class="dim">{t("なし")}</p>
      {/if}
      {#each d.attachments as a (a.id)}
        <p class="att" class:missing={!a.exists}>
          <span class="mode">{a.mode}</span>
          <span class="path" title={a.path}>{a.path.split("/").pop()}</span>
          {#if a.exists}<button class="link" onclick={() => void openViewer(d.key, a.id, `${c.title ?? ""} — ${a.path.split("/").pop()}`)}>{t("この添付を読む")}</button>{/if}
          {#if !a.exists}<span class="warn">{t("（実体なし）")}</span>{/if}
          {#if a.doiCheck === "unknown"}<span class="warn" title={t("自動取得した PDF の冒頭 2 頁に、この文献の DOI が見つからなかった（別論文の可能性。中身を確認してください）")}>{t("（DOI 未確認）")}</span>{/if}
        </p>
      {/each}
    </div>

    <p class="footer dim">
      {d.key} {t("· 追加")} {d.dateAdded} · source: {c.custom?.source ?? "?"}
    </p>
  {/if}
</aside>
