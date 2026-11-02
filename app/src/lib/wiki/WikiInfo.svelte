<script lang="ts">
  /* OpenNote の右ペイン: ページ情報 / リンク先の文献（items の key を get_item で引いた item カード）/
     バックリンク（wiki_backlinks）/ 目次（描画部品の onHeadings）。ホームは vault の概要。
     書誌は捏造しない: 蔵書に無い key は「見つかりません」と key のまま出す */
  import { untrack } from "svelte";
  import { t } from "../i18n.svelte";
  import { api, type ItemDetail } from "../api";
  import { openViewer, reveal } from "../state.svelte";
  import { backlinks, jumpToHeading, openWikiPage, touchedAny, wk } from "./wiki.svelte";
  import { addRefDir, removeRefDir } from "./refdirs.svelte";
  import type { WikiBacklink } from "./types";

  /** embedded = 右ペインのタブ（WikiRight）の中 = 外枠の線と大きさはタブの側が持つ */
  let { embedded = false }: { embedded?: boolean } = $props();

  const page = $derived(wk.page);
  // 読み直しの鍵: wk.page は保存のたびに新しい物に置き換わる（本文・hash だけが変わる）ので、
  // 文献カードとバックリンクの effect は page の同一性ではなく path と items の中身だけに依存させる
  const pagePath = $derived(page?.path ?? null);
  const pageExternal = $derived(!!page?.external);
  const itemKeys = $derived((page?.items ?? []).join("\0"));

  // ---- リンク先の文献 ----
  let items = $state<{ key: string; d: ItemDetail | null; err: string | null }[]>([]);
  let itemSeq = 0;
  let itemsFor: string | null = null;
  $effect(() => {
    const keys = itemKeys ? itemKeys.split("\0") : [];
    const p = pagePath;
    const my = ++itemSeq;
    // 別のページに移ったら、読む間は前のページのカードを出さない（同じページの読み直しでは残す）
    if (p !== itemsFor) {
      itemsFor = p;
      items = [];
    }
    if (!keys.length) {
      items = [];
      return;
    }
    void Promise.all(
      keys.map(async (key) => {
        try {
          return { key, d: await api.getItem(key), err: null };
        } catch (e) {
          return { key, d: null, err: String(e) };
        }
      })
    ).then((r) => {
      if (my === itemSeq) items = r;
    });
  });

  // ---- バックリンク ----
  let back = $state<WikiBacklink[]>([]);
  let backTotal = $state(0);
  let backErr = $state<string | null>(null);
  let backSeq = 0;
  async function loadBack(p: string) {
    const my = ++backSeq;
    try {
      const r = await backlinks({ path: p });
      if (my !== backSeq) return;
      back = r.pages;
      backTotal = r.total;
      backErr = null;
    } catch (e) {
      if (my !== backSeq) return;
      back = [];
      backTotal = 0;
      backErr = String(e);
    }
  }
  // 対象のページが変わった = 前の結果を消してから読む（同じページの保存では消さない・読み直さない）
  $effect(() => {
    const p = pagePath;
    const ext = pageExternal;
    back = [];
    backTotal = 0;
    backErr = null;
    if (p && !ext) void loadBack(p); // 参照フォルダのファイルは索引に無い = バックリンクも無い
  });
  // 索引が変わった = このページか、いま出しているバックリンク元が変更に入るとき（全走査なら必ず）だけ読み直す
  let seenStamp = untrack(() => wk.stamp);
  $effect(() => {
    const s = wk.stamp;
    if (s === seenStamp) return;
    seenStamp = s;
    untrack(() => {
      const p = pagePath;
      if (p && touchedAny([p, ...back.map((b) => b.path)])) void loadBack(p);
    });
  });

  function itemTitle(d: ItemDetail): string {
    return typeof d.csl?.title === "string" && d.csl.title ? d.csl.title : "(no title)";
  }
  function itemByline(d: ItemDetail): string {
    const a = (d.csl?.author ?? [])[0];
    const who = a ? (a.literal ?? a.family ?? "") : "";
    const year = d.csl?.issued?.["date-parts"]?.[0]?.[0];
    return [who && (d.csl.author.length > 1 ? `${who} et al.` : who), year].filter(Boolean).join(", ");
  }

  const fmShown = $derived((page?.frontmatter ?? []).filter((f) => !["title", "tags", "items"].includes(f.key)));
  const fmText = (v: unknown, raw?: string) => raw ?? (typeof v === "string" ? v : JSON.stringify(v));
  const date = (s: number) => (s ? new Date(s * 1000).toLocaleString() : "");
  const size = (n: number) => (n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`);

  function openBack(e: MouseEvent, p: string) {
    if (e.button === 1 || e.metaKey) {
      e.preventDefault();
      void openWikiPage(p, true);
    } else if (e.button === 0) void openWikiPage(p);
  }
</script>

<aside class="wk-info" class:wk-info-in={embedded}>
  {#if !page}
    <div class="wk-sec">
      <h3>OpenNote</h3>
      {#if wk.status}
        <p class="wk-dim">{wk.status.root ?? ""}</p>
        {#if wk.status.pages != null}<p>{t("ページ数")}: {wk.status.pages}</p>{/if}
        {#if wk.status.inbox}<p>{t("ingest/ の素材")}: {wk.status.inbox}</p>{/if}
        {#if wk.status.last_scan}<p class="wk-dim">{t("最終走査")}: {date(wk.status.last_scan)}</p>{/if}
      {/if}
    </div>
    <!-- 参照フォルダ: ワークスペースの外の読み取り専用のルート。その下の pdf / html / 画像へのリンクを内蔵の表示で開く -->
    <div class="wk-sec">
      <h3>{t("参照フォルダ")}{#if wk.refDirs.length}<span class="wk-n">{wk.refDirs.length}</span>{/if}</h3>
      <p class="wk-dim">{t("ワークスペースの外のフォルダ。その中の PDF・html・画像へのリンクを内蔵の表示で開きます（読み取り専用・索引には入りません）")}</p>
      {#each wk.refDirs as d (d)}
        <div class="wk-ref">
          <span class="wk-ref-path" title={d}>&lrm;{d}&lrm;</span>
          <button class="wk-btn" title={d} onclick={() => void removeRefDir(d)}>{t("外す")}</button>
        </div>
      {/each}
      <div class="wk-card-acts">
        <button class="wk-btn" onclick={() => void addRefDir()}>{t("追加…")}</button>
      </div>
    </div>
  {:else}
    <div class="wk-sec">
      <h3>{t("ページ情報")}</h3>
      <p class="wk-title">{page.title}</p>
      <p class="wk-dim">{page.media ?? page.kind} · {size(page.size)} · {date(page.mtime)}</p>
      {#if page.external}
        <p class="wk-dim" title={page.abs_path}>{t("参照フォルダ（読み取り専用）")}: {page.external}</p>
      {/if}
      {#if page.tags?.length}
        <div class="wk-tags">{#each page.tags as tag (tag)}<span class="wk-tag">#{tag}</span>{/each}</div>
      {/if}
      {#if fmShown.length}
        <dl class="wk-fm">
          {#each fmShown as f, i (f.key + i)}
            <dt>{f.key}</dt>
            <dd>{fmText(f.value, f.raw)}</dd>
          {/each}
        </dl>
      {/if}
    </div>

    {#if page.items?.length}
      <div class="wk-sec">
        <h3>{t("リンク先の文献")}</h3>
        {#each items as it (it.key)}
          <div class="wk-card">
            {#if it.d}
              {@const d = it.d}
              {@const pdf = d.attachments.find((a) => a.exists)}
              <p class="wk-card-title">{itemTitle(d)}</p>
              <p class="wk-dim">{itemByline(d)}{d.deleted ? ` · ${t("ゴミ箱")}` : ""}</p>
              <div class="wk-card-acts">
                {#if pdf}<button class="wk-btn" onclick={() => void openViewer(d.key, pdf.id, itemTitle(d))}>{t("PDF を読む")}</button>{/if}
                <button class="wk-btn" onclick={() => void reveal(d.key)}>{t("MY LIBRARY で表示")}</button>
              </div>
            {:else if it.err}
              <p class="wk-bad">{it.key}: {it.err.slice(0, 160)}</p>
            {:else}
              <p class="wk-bad">{t("蔵書に見つかりません")}: {it.key}</p>
            {/if}
          </div>
        {/each}
      </div>
    {/if}

    {#if !page.external}
    <div class="wk-sec">
      <h3>{t("バックリンク")}{#if backTotal}<span class="wk-n">{backTotal}</span>{/if}</h3>
      {#if backErr}
        <p class="wk-bad">{backErr.slice(0, 200)}</p>
      {:else if !back.length}
        <p class="wk-dim">{t("このページを参照するページはありません")}</p>
      {/if}
      {#each back as b (b.path)}
        <button class="wk-link" title={b.path} onclick={(e) => openBack(e, b.path)} onauxclick={(e) => openBack(e, b.path)}>
          <span class="wk-link-title">{b.title}</span>
          {#if b.excerpt}<span class="wk-link-ex">{b.excerpt}</span>{/if}
        </button>
      {/each}
    </div>
    {/if}

    {#if wk.headings.length}
      <div class="wk-sec">
        <h3>{t("目次")}</h3>
        {#each wk.headings.filter((h) => h.level <= 3) as h, i (h.id + i)}
          <button class="wk-toc" style:padding-left="{(h.level - 1) * 12 + 4}px" onclick={() => jumpToHeading(h.id)}>{h.text}</button>
        {/each}
      </div>
    {/if}
  {/if}
</aside>

<style>
  .wk-info { border-left: 1px solid var(--border); background: var(--panel); overflow-y: auto; padding: 12px 14px; min-width: 0; }
  .wk-info.wk-info-in { border-left: 0; flex: 1; min-height: 0; }
  .wk-sec { margin-bottom: 16px; }
  .wk-sec h3 {
    margin: 0 0 6px; font-size: 11px; font-weight: 700; letter-spacing: 0.4px; text-transform: uppercase; color: var(--dim);
    display: flex; gap: 6px; align-items: baseline;
  }
  .wk-n { font-weight: 400; }
  .wk-title { margin: 0 0 2px; font-weight: 600; overflow-wrap: anywhere; }
  .wk-dim { margin: 2px 0; color: var(--dim); font-size: 12px; overflow-wrap: anywhere; }
  .wk-bad { margin: 2px 0; color: var(--danger); font-size: 12px; overflow-wrap: anywhere; }
  .wk-tags { display: flex; flex-wrap: wrap; gap: 4px; margin: 6px 0; }
  .wk-tag { font-size: 11px; padding: 1px 6px; border-radius: 4px; background: var(--accent-soft); color: var(--accent); }
  .wk-fm { display: grid; grid-template-columns: auto 1fr; gap: 2px 8px; margin: 6px 0 0; font-size: 12px; }
  .wk-fm dt { color: var(--dim); }
  .wk-fm dd { margin: 0; overflow-wrap: anywhere; }
  .wk-card { border: 1px solid var(--border); border-radius: 6px; padding: 8px; margin-bottom: 6px; }
  .wk-card-title { margin: 0 0 2px; font-size: 12.5px; font-weight: 600; overflow-wrap: anywhere; }
  .wk-card-acts { display: flex; gap: 6px; margin-top: 6px; flex-wrap: wrap; }
  .wk-ref { display: flex; align-items: center; gap: 6px; padding: 3px 0; font-size: 12px; }
  .wk-ref-path { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; direction: rtl; text-align: left; }
  .wk-btn {
    border: 1px solid var(--border); border-radius: 6px; padding: 2px 8px; background: var(--bg); font-size: 11.5px; cursor: pointer;
    color: var(--text);
  }
  .wk-btn:hover { border-color: var(--accent); }
  .wk-link, .wk-toc {
    display: flex; flex-direction: column; align-items: flex-start; width: 100%; border: 0; background: transparent; text-align: left;
    padding: 4px; border-radius: 5px; cursor: pointer; color: var(--text); font-size: 12.5px;
  }
  .wk-link:hover, .wk-toc:hover { background: var(--bg); }
  .wk-link-title { font-weight: 500; overflow-wrap: anywhere; }
  .wk-link-ex { color: var(--dim); font-size: 11.5px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 100%; }
</style>
