<script lang="ts">
  import { t } from "./i18n.svelte";
  /* タブ列（ブラウザ型・ヘッダーの上に 1 段）。[アイコン 題名 ×]…[＋]。
     タブ本体は div role="tab"（× を中に置くと <button> の入れ子 = Svelte 5 のコンパイルエラー・tab の子は支援技術から見えない）、
     × は兄弟の button を重ねる。tablist が持つのはタブだけ（× は aria-hidden のマウス用・キーは ⌘W、＋ は tablist の外）。
     クリック = 切替 / ← → Home End = 隣・端へ / 中クリック = 閉じる / 空きのダブルクリック・＋ = 新しいタブ（すべて）。
     最後の 1 枚は × を出さない（PDF タブなら出す = ビューアだけ閉じる）。題名は id から引く（viewTitle）。検索語・チップは題名に
     足さず tooltip に出す。PDF タブは文書のアイコン + 論文の題名（tooltip は題名の全文）。
     並べ替え（D&D）は作らない。class は tabs- 接頭辞（tb- は viewer のツールバーが使用中） */
  import { tick } from "svelte";
  import LabelBadge from "./LabelBadge.svelte";
  import { app, activateTab, closeTab, openTab, tabView, viewTitle, type Tab, type View } from "./state.svelte";

  let listEl = $state<HTMLDivElement | null>(null);

  function tip(tab: Tab, v: View): string {
    const front = tab.id === app.tab;
    const search = front ? app.search : tab.search;
    const f = front ? app.filters : tab.filters;
    const parts = [viewTitle(v)];
    if (f.author) parts.push(`${t("著者")} ${f.author.label}`);
    if (f.journal) parts.push(`${t("誌名")} ${f.journal}`);
    if (search) parts.push(`${t("「")}${search}${t("」")}`);
    return parts.join(" · ");
  }

  function onAux(e: MouseEvent, id: number) {
    if (e.button !== 1) return;
    e.preventDefault();
    void closeTab(id);
  }

  /** タブの上のキー: Enter / Space = 前面に・← → = 隣（端からは反対の端）・Home / End = 端。移った先のタブにフォーカス
      （切替を待つ間に user がフォーカスを列の外 = 検索欄などへ移していたら奪わない）。修飾キー付きは扱わない */
  async function onTabKey(e: KeyboardEvent, i: number) {
    if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
    const n = app.tabs.length;
    let j: number;
    if (e.key === "Enter" || e.key === " ") j = i;
    else if (e.key === "ArrowRight") j = (i + 1) % n;
    else if (e.key === "ArrowLeft") j = (i - 1 + n) % n;
    else if (e.key === "Home") j = 0;
    else if (e.key === "End") j = n - 1;
    else return;
    e.preventDefault();
    await activateTab(app.tabs[j].id);
    await tick();
    // 前面になった PDF ビューアはページにフォーカスを取る（PdfViewer）が、タブの上で ← → を続けられるよう列へ戻す
    const el = document.activeElement;
    if (listEl?.contains(el) || el?.closest(".viewer-overlay")) listEl?.querySelector<HTMLElement>(".tabs-item.front .tabs-tab")?.focus();
  }

  /** タブ列の空き（タブ・＋ の外）のダブルクリック = 新しいタブ。マウスだけの近道なので、列はフォーカスを取らない
      （tabindex を付けると空きのクリックで検索欄からフォーカスが外れる）= リスナーは action で付ける */
  function dblclickNew(node: HTMLElement) {
    const f = (e: MouseEvent) => {
      if (e.target === node || e.target === listEl) void openTab();
    };
    node.addEventListener("dblclick", f);
    return { destroy: () => node.removeEventListener("dblclick", f) };
  }

  // 前面のタブが列の外（タブが溢れて横に送られている）なら見える位置へ送る（⌘9 で末尾へ移ったとき等）
  $effect(() => {
    app.tab;
    app.tabs.length;
    void tick().then(() =>
      listEl?.querySelector<HTMLElement>(".tabs-item.front")?.scrollIntoView({ block: "nearest", inline: "nearest" })
    );
  });
</script>

<div class="tabs-strip" use:dblclickNew>
  <div class="tabs-list" role="tablist" aria-label={t("タブ")} style="--n:{app.tabs.length}" bind:this={listEl}>
    {#each app.tabs as tab, i (tab.id)}
      {@const v = tabView(tab)}
      {@const front = tab.id === app.tab}
      <div class="tabs-item" class:front role="presentation">
        <div
          class="tabs-tab"
          role="tab"
          tabindex={front ? 0 : -1}
          aria-selected={front}
          title={tab.viewer ? tab.viewer.title : tip(tab, v)}
          onclick={(e) => {
            if (e.button === 0) void activateTab(tab.id);
          }}
          onauxclick={(e) => onAux(e, tab.id)}
          onkeydown={(e) => onTabKey(e, i)}>
          {#if tab.viewer}
            <span class="tabs-ico" aria-hidden="true">
              <svg viewBox="0 0 16 16" width="13" height="13"
                ><path
                  d="M4 1.8h5.2l3.3 3.3v8.4c0 .39-.31.7-.7.7H4c-.39 0-.7-.31-.7-.7v-11c0-.39.31-.7.7-.7ZM9 1.9v3.4h3.4M5.6 8.2h4.8M5.6 10.6h4.8"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.2"
                  stroke-linecap="round"
                  stroke-linejoin="round" /></svg>
            </span>
            <span class="tabs-title">{tab.viewer.title || "PDF"}</span>
          {:else if v.kind === "tag"}
            {@const tv = v}
            <LabelBadge name={viewTitle(tv)} color={app.tags.find((x) => x.id === tv.id)?.color ?? null} />
          {:else}
            <!-- ★ Starred は題名の ★ がアイコンを兼ねる -->
            {#if v.kind !== "starred"}<span class="tabs-ico" aria-hidden="true">
                {#if v.kind === "collection"}
                  <svg viewBox="0 0 16 16" width="13" height="13"
                    ><path
                      d="M1.8 4.2c0-.66.54-1.2 1.2-1.2h3.1l1.5 1.6h5.4c.66 0 1.2.54 1.2 1.2v6.2c0 .66-.54 1.2-1.2 1.2H3c-.66 0-1.2-.54-1.2-1.2V4.2Z"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1.3"
                      stroke-linejoin="round" /></svg>
                {:else if v.kind === "trash"}
                  <svg viewBox="0 0 16 16" width="13" height="13"
                    ><path
                      d="M2.8 4.3h10.4M6.2 4.3V2.9c0-.39.31-.7.7-.7h2.2c.39 0 .7.31.7.7v1.4M4.1 4.3l.6 8.6c.04.55.5.98 1.05.98h4.5c.55 0 1.01-.43 1.05-.98l.6-8.6"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1.3"
                      stroke-linecap="round"
                      stroke-linejoin="round" /></svg>
                {:else if v.kind === "all"}
                  <svg viewBox="0 0 16 16" width="13" height="13"
                    ><path
                      d="M2.5 3h2.6v10H2.5zM6.4 3H9v10H6.4zM10.3 3.6l2.5-.7 2.4 9.6-2.5.7z"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1.2"
                      stroke-linejoin="round" /></svg>
                {/if}
              </span>{/if}
            <span class="tabs-title">{viewTitle(v)}</span>
          {/if}
        </div>
        {#if app.tabs.length > 1 || tab.viewer}
          <button
            class="tabs-x"
            tabindex="-1"
            aria-hidden="true"
            title={front ? t("タブを閉じる（⌘W）") : t("タブを閉じる")}
            onclick={(e) => {
              if (e.button === 0) void closeTab(tab.id);
            }}
            onauxclick={(e) => onAux(e, tab.id)}>×</button>
        {/if}
      </div>
    {/each}
  </div>
  <button class="tabs-new" title={t("新しいタブ（⌘T）")} aria-label={t("新しいタブ")} onclick={() => void openTab()}>{t("＋")}</button>
</div>
