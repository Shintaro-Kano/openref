<script lang="ts">
  import { t } from "../i18n.svelte";
  /* 目次タブ（Acrobat しおり参照）。
     doc.getOutline() の木を flattenOutline で平坦化して描画。折りたたみ可。
     dest（string / 配列）→ getDestination/getPageIndex でページ解決 → onJump。
     ※ getOutline/getDestination/getPageIndex は素の Promise（for await なし）なので JSC 問題は無い。 */
  import { flattenOutline, type RawOutlineNode, type FlatOutlineRow } from "./util";

  let { doc, onJump }: { doc: any; onJump: (page: number) => void } = $props();

  let loaded = $state(false);
  let raw = $state<RawOutlineNode[] | null>(null);
  let collapsed = $state<Set<string>>(new Set());
  let err = $state("");

  const rows = $derived<FlatOutlineRow[]>(raw ? flattenOutline(raw, collapsed) : []);

  // doc が来たら一度だけ getOutline
  $effect(() => {
    if (!doc || loaded) return;
    loaded = true;
    doc
      .getOutline()
      .then((o: RawOutlineNode[] | null) => {
        raw = o && o.length ? o : [];
      })
      .catch((e: unknown) => {
        err = String(e);
        raw = [];
      });
  });

  function toggle(id: string) {
    const next = new Set(collapsed);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    collapsed = next;
  }

  /** dest（string なら getDestination で解決）→ 先頭 RefProxy → getPageIndex → 1-indexed page */
  async function resolveDest(dest: any): Promise<number | null> {
    try {
      let explicit = dest;
      if (typeof dest === "string") explicit = await doc.getDestination(dest);
      if (!Array.isArray(explicit) || !explicit.length) return null;
      const ref = explicit[0];
      if (ref && typeof ref === "object" && "num" in ref) {
        const idx = await doc.getPageIndex(ref);
        return idx + 1;
      }
      if (typeof ref === "number") return ref + 1;
      return null;
    } catch {
      return null;
    }
  }

  async function go(row: FlatOutlineRow) {
    const page = await resolveDest(row.dest);
    if (page != null) onJump(page);
  }
</script>

<div class="side-scroll">
  {#if !loaded || raw === null}
    <p class="dim hint">{t("目次を読み込み中…")}</p>
  {:else if rows.length === 0}
    <p class="dim hint">{t("この PDF に目次はありません。")}{err ? `${t("（")}${err}${t("）")}` : ""}</p>
  {:else}
    <ul class="ol-tree">
      {#each rows as row (row.id)}
        <li class="ol-row" style="--depth:{row.depth}">
          {#if row.hasChildren}
            <button
              class="ol-toggle"
              class:collapsed={collapsed.has(row.id)}
              title={collapsed.has(row.id) ? t("展開") : t("折りたたむ")}
              onclick={() => toggle(row.id)}>▾</button
            >
          {:else}
            <span class="ol-toggle ph"></span>
          {/if}
          <button class="ol-title" title={row.title} onclick={() => go(row)}
            >{row.title || t("（無題）")}</button
          >
        </li>
      {/each}
    </ul>
  {/if}
</div>
