<script lang="ts" generics="T extends { id: number; name: string; nItems: number }">
  import { t } from "./i18n.svelte";
  /* Manage labels / Manage folders の表（ヘッダ + 行 = ☐ / [色] / 名前 / 件数 / 🗑）。状態と操作は Manage（manage.svelte.ts）。
     名前の表示は呼ぶ側の snippet（ラベル = バッジ・フォルダ = 親のパス + 行内の確認）、色の列はラベルだけ（snippet があれば出す）。
     行内改名の欄・🗑 の確認・押している間の並びの凍結（window の pointer）はここ。class は ml-（nl-err は nf-err と同じ見た目） */
  import type { Snippet } from "svelte";
  import { focusMe, onEnter } from "./input";
  import type { Manage } from "./manage.svelte";

  let {
    m,
    nameCell,
    icon,
    colorCell,
    nameClass = "",
  }: {
    m: Manage<T>;
    /** 名前の列（改名中でないとき）。引数 = 行の item と木の深さ */
    nameCell: Snippet<[T, number]>;
    /** 名前の列の先頭（改名中も出す。フォルダのアイコン） */
    icon?: Snippet<[T]>;
    /** 色の列（ラベルだけ。あればヘッダに "Color" も出す） */
    colorCell?: Snippet<[T]>;
    /** 名前の列に足す class（フォルダの字下げ = mf-name） */
    nameClass?: string;
  } = $props();
</script>

<svelte:window
  onpointerdowncapture={(e) => m.onPointerDown(e)}
  onpointerupcapture={() => m.onPointerUp()}
  onpointercancelcapture={() => m.onPointerUp()} />

<div class="ml-table">
  <div class="ml-tr ml-th">
    <span class="ml-c-cb">
      <button class="cb" class:on={m.allOn} class:some={m.someOn && !m.allOn} aria-label={t("すべて選択")} onclick={() => m.toggleAll()}></button>
    </span>
    {#if colorCell}<span class="ml-c-color">Color</span>{/if}
    <button class="ml-c-name ml-sort" onclick={() => m.sortBy("name")}>
      Name {#if m.sortKey === "name"}<span class="ml-arrow">{m.asc ? "↓" : "↑"}</span>{/if}
    </button>
    <button class="ml-c-n ml-sort" onclick={() => m.sortBy("count")}>
      Number of references {#if m.sortKey === "count"}<span class="ml-arrow">{m.asc ? "↑" : "↓"}</span>{/if}
    </button>
    <span class="ml-c-del"></span>
  </div>
  <div class="ml-body">
    {#if m.opts.all().length === 0}
      <div class="ml-empty">{m.opts.text.empty()}</div>
    {:else if m.rows.length === 0}
      <div class="ml-empty">{m.opts.text.noMatch(m.q.trim())}</div>
    {/if}
    {#each m.rows as { item, depth } (item.id)}
      <div class="ml-tr" class:on={m.sel.has(item.id)}>
        <span class="ml-c-cb">
          <button class="cb" class:on={m.sel.has(item.id)} aria-label={t("選択::checkbox")} onclick={() => m.toggleSel(item.id)}></button>
        </span>
        {#if colorCell}<span class="ml-c-color">{@render colorCell(item)}</span>{/if}
        <span class="ml-c-name {nameClass}" style="--depth:{depth}">
          {@render icon?.(item)}
          {#if m.renaming === item.id}
            {@const err = m.opts.nameError(m.renameText, item)}
            <input
              class="ml-rename"
              class:bad={!!err}
              use:focusMe
              readonly={m.sending.has(item.id)}
              bind:value={m.renameText}
              oninput={() => (m.typed = true)}
              onblur={() => m.commitRename(item)}
              onkeydown={(e) => onEnter(e, () => m.commitRename(item))} />
            {#if err}<span class="nl-err">{err}</span>{/if}
          {:else}
            {@render nameCell(item, depth)}
          {/if}
        </span>
        <span class="ml-c-n">{item.nItems}</span>
        <span class="ml-c-del">
          {#if m.confirmRow === item.id}
            <button class="ml-btn danger-solid small" disabled={m.busy} onclick={() => m.doDelete([item.id])}>{t("削除")}</button>
            <button class="ml-btn small" onclick={() => (m.confirmRow = null)}>×</button>
          {:else}
            <button class="ml-trash" title={m.opts.text.remove()} aria-label={t("削除")} onclick={() => m.askDeleteRow(item)}>
              <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"
                ><path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8.3c.05.68.6 1.2 1.28 1.2h3.24c.68 0 1.23-.52 1.28-1.2l.6-8.3"
                  fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" /></svg>
            </button>
          {/if}
        </span>
      </div>
    {/each}
  </div>
</div>
