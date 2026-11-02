<script lang="ts">
  /* OpenNote の左ペイン = セグメント + vault の木（wiki_list の全件から組む）。
     フォルダは閉じた状態から（開いた物だけ行を作る = 7,000 ページでも行は少ない）。前面のページの祖先は自動で開く。
     クリック = 前面の Wiki タブで開く・⌘クリック / 中クリック = 新しいタブ。木の読み直しは wiki.svelte.ts（wiki-changed）。
     検索欄はヘッダー（WikiSearch）→ 中央に結果一覧（WikiResults）。
     木の下に Ingest 箱（ingest/ の未処理の素材。OpenRef は処理しない = 開く・改名・ゴミ箱だけ）と Chat log 箱（chat-log/ = チャットの記録）と Tags 箱（件数の多い順・クリックで AND の絞込） */
  import { t } from "../i18n.svelte";
  import { frontWiki } from "../state.svelte";
  import type { WikiListEntry } from "./types";
  import { baseName } from "./render/asset";
  import WikiSegment from "./WikiSegment.svelte";
  import { openWikiPage, vaultName, wk } from "./wiki.svelte";
  import { toggleTag } from "./search.svelte";

  /** Tags 箱を畳んだときに出す数 */
  const TAGS_FOLDED = 24;
  let tagsAll = $state(false);
  let ingestOpen = $state(true);
  let chatLogOpen = $state(false);
  let tagsOpen = $state(true);

  const shownTags = $derived(tagsAll ? wk.tags : wk.tags.slice(0, TAGS_FOLDED));
  const picked = (tag: string) => wk.qTags.some((x) => x.toLowerCase() === tag.toLowerCase());

  function fmtSize(n: number): string {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
    if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
    return `${(n / 1024 / 1024 / 1024).toFixed(1)} GB`;
  }

  interface Dir {
    name: string;
    path: string;
    dirs: Map<string, Dir>;
    files: WikiListEntry[];
  }
  type Row = { dir: Dir; depth: number } | { file: WikiListEntry; depth: number };

  const tree = $derived.by(() => {
    const root: Dir = { name: "", path: "", dirs: new Map(), files: [] };
    const dirAt = (parts: string[]): Dir => {
      let d = root;
      for (let i = 0; i < parts.length; i++) {
        let next = d.dirs.get(parts[i]);
        if (!next) {
          next = { name: parts[i], path: parts.slice(0, i + 1).join("/"), dirs: new Map(), files: [] };
          d.dirs.set(parts[i], next);
        }
        d = next;
      }
      return d;
    };
    for (const p of wk.pages) {
      const parts = p.path.split("/");
      dirAt(parts.slice(0, -1)).files.push(p);
    }
    // この起動で作った空のフォルダ（wiki_list には出ない）
    for (const d of wk.newDirs) if (d) dirAt(d.split("/"));
    return root;
  });

  const byName = (a: string, b: string) => a.localeCompare(b, undefined, { numeric: true, sensitivity: "base" });

  const rows = $derived.by(() => {
    const out: Row[] = [];
    const walk = (d: Dir, depth: number) => {
      for (const sub of [...d.dirs.values()].sort((a, b) => byName(a.name, b.name))) {
        out.push({ dir: sub, depth });
        if (wk.open.has(sub.path)) walk(sub, depth + 1);
      }
      for (const f of [...d.files].sort((a, b) => byName(baseName(a.path), baseName(b.path)))) out.push({ file: f, depth });
    };
    walk(tree, 0);
    return out;
  });

  const current = $derived(frontWiki()?.path ?? null);

  // 前面のページの祖先を開く（リンクで遷移したページが木の中に見える）
  $effect(() => {
    const p = current;
    if (!p) return;
    const parts = p.split("/");
    for (let i = 1; i < parts.length; i++) wk.open.add(parts.slice(0, i).join("/"));
  });

  function toggle(path: string) {
    if (wk.open.has(path)) wk.open.delete(path);
    else wk.open.add(path);
  }

  function label(f: WikiListEntry): string {
    const b = baseName(f.path);
    return f.kind === "md" ? b.replace(/\.md$/i, "") : b;
  }

  /** ワークスペース名の行のクリック / 右クリック: ホーム / OpenNote のフォルダを選び直す… / 参照フォルダ（読み取り専用）を追加…（WikiDialogs が出す）。位置はボタンの直下 */
  function vaultMenu(e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    wk.menu = { kind: "vault", path: wk.status?.root ?? "", x: Math.min(r.left, window.innerWidth - 280), y: Math.min(r.bottom + 2, window.innerHeight - 130) };
  }

  /** 右クリック: ページ = 名前の変更・ゴミ箱 / フォルダ = その中に新規ページ・新規フォルダ・フォルダをゴミ箱（WikiDialogs が出す）。
      y の上限 = メニューの高さの見積もり（フォルダは見出し + 3 項目） */
  function menu(e: MouseEvent, kind: "file" | "dir", path: string) {
    e.preventDefault();
    const h = kind === "dir" ? 150 : 110;
    wk.menu = { kind, path, x: Math.min(e.clientX, window.innerWidth - 190), y: Math.min(e.clientY, window.innerHeight - h) };
  }

  function pick(e: MouseEvent, path: string) {
    if (e.button === 1 || e.metaKey) {
      e.preventDefault();
      void openWikiPage(path, true);
    } else if (e.button === 0) void openWikiPage(path);
  }
</script>

<nav class="wk-sidebar">
  <WikiSegment />
  <!-- ワークスペース名 = メニュー（ホーム / OpenNote のフォルダを選び直す… / 参照フォルダ（読み取り専用）を追加…）。Settings のメニュー項目「フォルダを選ぶ…」をここへ移した -->
  <button class="wk-vault" class:wk-cur={current === null} title={wk.status?.root ?? ""} aria-haspopup="menu" aria-expanded={wk.menu?.kind === "vault"} onclick={vaultMenu} oncontextmenu={vaultMenu}>
    {vaultName() || "OpenNote"}
    {#if wk.pages.length}<span class="wk-n">{wk.pages.length}</span>{/if}
  </button>
  {#if wk.status?.error}
    <p class="wk-msg wk-bad">{wk.status.error}</p>
  {:else if wk.status?.rebuilding}
    <p class="wk-msg">{t("索引を作成中…")} {wk.status.rebuilding.percent}%</p>
  {:else if wk.treeError}
    <p class="wk-msg wk-bad">{wk.treeError}</p>
  {:else if wk.treeLoading && !wk.pages.length}
    <p class="wk-msg">{t("読込中…")}</p>
  {:else if !wk.pages.length}
    <p class="wk-msg">{t("ページがありません")}</p>
  {/if}
  <div class="wk-tree" role="tree" aria-label={t("ページの一覧")}>
    {#each rows as r ("dir" in r ? "d:" + r.dir.path : "f:" + r.file.path)}
      {#if "dir" in r}
        <button
          class="wk-row wk-dir"
          role="treeitem"
          aria-selected="false"
          aria-expanded={wk.open.has(r.dir.path)}
          style:padding-left="{6 + r.depth * 12}px"
          onclick={() => toggle(r.dir.path)}
          oncontextmenu={(e) => menu(e, "dir", r.dir.path)}>
          <span class="wk-chev" class:wk-chev-open={wk.open.has(r.dir.path)} aria-hidden="true">▸</span>
          <span class="wk-name">{r.dir.name}</span>
        </button>
      {:else}
        <button
          class="wk-row wk-file wk-k-{r.file.kind}"
          class:wk-cur={current === r.file.path}
          role="treeitem"
          aria-selected={current === r.file.path}
          title={r.file.title && r.file.title !== label(r.file) ? `${r.file.title}\n${r.file.path}` : r.file.path}
          style:padding-left="{18 + r.depth * 12}px"
          onclick={(e) => pick(e, r.file.path)}
          onauxclick={(e) => pick(e, r.file.path)}
          oncontextmenu={(e) => menu(e, "file", r.file.path)}>
          <span class="wk-name">{label(r.file)}</span>
        </button>
      {/if}
    {/each}
  </div>
  {#if wk.pages.length}
    <section class="wk-box" aria-label="Ingest">
      <button class="wk-box-h" aria-expanded={ingestOpen} onclick={() => (ingestOpen = !ingestOpen)}>
        <span class="wk-chev" class:wk-chev-open={ingestOpen} aria-hidden="true">▸</span>
        <span class="wk-box-t">Ingest</span>
        <span class="wk-n">{wk.inboxTotal}</span>
      </button>
      {#if ingestOpen}
        {#if !wk.inbox.length}
          <p class="wk-msg">{t("ingest/ に素材はありません")}</p>
        {:else}
          {#each wk.inbox as f (f.path)}
            <button
              class="wk-row wk-file wk-k-{f.kind}"
              class:wk-cur={current === f.path}
              title={f.path}
              style:padding-left="18px"
              onclick={(e) => pick(e, f.path)}
              onauxclick={(e) => pick(e, f.path)}
              oncontextmenu={(e) => menu(e, "file", f.path)}>
              <span class="wk-name">{f.path.replace(/^ingest\//, "")}</span>
              <span class="wk-n wk-size">{fmtSize(f.size)}</span>
            </button>
          {/each}
          {#if wk.inboxTotal > wk.inbox.length}<p class="wk-msg">… {wk.inboxTotal - wk.inbox.length}</p>{/if}
        {/if}
      {/if}
    </section>
    <!-- Chat log 箱 = chat-log/（チャットの記録。ingest/ = 未処理の素材 とは別の箱） -->
    <section class="wk-box" aria-label="Chat log">
      <button class="wk-box-h" aria-expanded={chatLogOpen} onclick={() => (chatLogOpen = !chatLogOpen)}>
        <span class="wk-chev" class:wk-chev-open={chatLogOpen} aria-hidden="true">▸</span>
        <span class="wk-box-t">Chat log</span>
        <span class="wk-n">{wk.chatLogTotal}</span>
      </button>
      {#if chatLogOpen}
        {#if !wk.chatLog.length}
          <p class="wk-msg">{t("chat-log/ に記録はありません")}</p>
        {:else}
          {#each wk.chatLog as f (f.path)}
            <button
              class="wk-row wk-file wk-k-{f.kind}"
              class:wk-cur={current === f.path}
              title={f.path}
              style:padding-left="18px"
              onclick={(e) => pick(e, f.path)}
              onauxclick={(e) => pick(e, f.path)}
              oncontextmenu={(e) => menu(e, "file", f.path)}>
              <span class="wk-name">{f.path.replace(/^chat-log\//, "")}</span>
              <span class="wk-n wk-size">{fmtSize(f.size)}</span>
            </button>
          {/each}
          {#if wk.chatLogTotal > wk.chatLog.length}<p class="wk-msg">… {wk.chatLogTotal - wk.chatLog.length}</p>{/if}
        {/if}
      {/if}
    </section>
    <section class="wk-box" aria-label="Tags">
      <button class="wk-box-h" aria-expanded={tagsOpen} onclick={() => (tagsOpen = !tagsOpen)}>
        <span class="wk-chev" class:wk-chev-open={tagsOpen} aria-hidden="true">▸</span>
        <span class="wk-box-t">Tags</span>
        <span class="wk-n">{wk.tagsTotal}</span>
      </button>
      {#if tagsOpen}
        {#if !wk.tags.length}
          <p class="wk-msg">{t("tag はありません")}</p>
        {:else}
          <div class="wk-tags">
            {#each shownTags as tg (tg.tag)}
              <button class="wk-tag" class:wk-tag-on={picked(tg.tag)} aria-pressed={picked(tg.tag)} title={`#${tg.tag}`} onclick={() => toggleTag(tg.tag)}>
                <span class="wk-tag-name">{tg.tag}</span><span class="wk-tag-n">{tg.count}</span>
              </button>
            {/each}
          </div>
          {#if wk.tags.length > TAGS_FOLDED}
            <button class="wk-more" onclick={() => (tagsAll = !tagsAll)}>
              {tagsAll ? t("畳む") : `${t("すべて表示")} (${wk.tags.length})`}
            </button>
          {/if}
          {#if tagsAll && wk.tagsTotal > wk.tags.length}
            <p class="wk-msg">{t("件数の多い順に先頭だけを表示しています")} ({wk.tags.length} / {wk.tagsTotal})</p>
          {/if}
        {/if}
      {/if}
    </section>
  {/if}
</nav>

<style>
  .wk-sidebar {
    border-right: 1px solid var(--border); background: var(--panel); overflow-y: auto; padding: 8px 6px;
    display: flex; flex-direction: column; gap: 1px; min-width: 0;
  }
  .wk-vault {
    display: flex; justify-content: space-between; align-items: center; gap: 6px; padding: 5px 8px; margin-bottom: 4px;
    border: 0; border-radius: 6px; background: transparent; font-weight: 700; font-size: 12px; text-align: left; cursor: pointer;
    color: var(--text);
  }
  .wk-vault:hover { background: var(--bg); }
  .wk-n { color: var(--dim); font-size: 11px; font-weight: 400; }
  .wk-msg { margin: 4px 8px; font-size: 12px; color: var(--dim); overflow-wrap: anywhere; }
  .wk-bad { color: var(--danger); }
  .wk-tree { display: flex; flex-direction: column; gap: 1px; }
  .wk-row {
    display: flex; align-items: center; gap: 4px; width: 100%; padding: 3px 8px; border: 0; border-radius: 5px;
    background: transparent; font-size: 12.5px; text-align: left; cursor: pointer; color: var(--text); min-width: 0;
  }
  .wk-row:hover { background: var(--bg); }
  .wk-row.wk-cur { background: var(--accent-soft); color: var(--accent); font-weight: 600; }
  .wk-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wk-chev { display: inline-block; width: 10px; font-size: 9px; color: var(--dim); transition: transform 0.1s; }
  .wk-chev-open { transform: rotate(90deg); }
  .wk-dir { font-weight: 500; }
  .wk-k-media, .wk-k-other { color: var(--dim); }
  .wk-box { margin-top: 10px; border-top: 1px solid var(--border); padding-top: 6px; display: flex; flex-direction: column; gap: 1px; }
  .wk-box-h {
    display: flex; align-items: center; gap: 4px; padding: 3px 6px; border: 0; border-radius: 5px; background: transparent;
    cursor: pointer; color: var(--dim); font-size: 11px; font-weight: 700; letter-spacing: 0.04em; text-transform: uppercase; text-align: left;
  }
  .wk-box-h:hover { background: var(--bg); }
  .wk-box-t { flex: 1; }
  .wk-size { margin-left: auto; flex-shrink: 0; padding-left: 6px; }
  .wk-tags { display: flex; flex-wrap: wrap; gap: 4px; padding: 4px 6px; }
  .wk-tag {
    display: inline-flex; align-items: baseline; gap: 4px; max-width: 100%; padding: 1px 7px; border: 1px solid var(--border);
    border-radius: 10px; background: var(--bg); color: var(--text); font-size: 11.5px; cursor: pointer; min-width: 0;
  }
  .wk-tag:hover { border-color: var(--accent); }
  .wk-tag.wk-tag-on { background: var(--accent-soft); border-color: var(--accent); color: var(--accent); font-weight: 600; }
  .wk-tag-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wk-tag-n { color: var(--dim); font-size: 10.5px; font-weight: 400; }
  .wk-more {
    align-self: flex-start; margin: 2px 6px; border: 0; background: transparent; color: var(--accent); font-size: 11.5px; cursor: pointer; padding: 2px 0;
  }
</style>
