<script lang="ts" generics="T extends { id: number; name: string }">
  import { t, tn } from "./i18n.svelte";
  /* Manage の Find and replace 欄: Find / Replace with → dry run のプレビュー（取得は Manage の effect）→ Replace。
     変えない理由の文（status → 文）と各行の前に出すもの（フォルダ = 親のパス）は呼ぶ側から */
  import type { Snippet } from "svelte";
  import type { RenameOutcome } from "./api";
  import { focusMe } from "./input";
  import type { Manage } from "./manage.svelte";

  let {
    m,
    why,
    prefix,
  }: {
    m: Manage<T>;
    /** 変えない理由（RenameOutcome.status → 文。無い status は出さない） */
    why: Partial<Record<RenameOutcome["status"], string>>;
    prefix?: Snippet<[RenameOutcome]>;
  } = $props();
</script>

<div class="ml-find">
  <input placeholder={t("Find（大小無視）")} use:focusMe bind:value={m.findText} />
  <span class="dim">→</span>
  <input placeholder="Replace with" bind:value={m.replaceText} />
</div>
{#if m.findText}
  <div class="ml-preview">
    {#if m.previewErr}
      <span class="nl-err">{t("プレビューできません:")} {m.previewErr}</span>
    {:else if m.preview.length === 0}
      <span class="dim">{m.opts.text.noFind()}</span>
    {/if}
    {#each m.preview as p (p.id)}
      <div class="ml-prow" class:skip={p.status !== "renamed"}>
        {@render prefix?.(p)}
        <span>{p.old}</span><span class="dim">→</span><b>{p.new || t("（空）")}</b>
        {#if why[p.status]}<span class="ml-why">{why[p.status]}</span>{/if}
      </div>
    {/each}
  </div>
{/if}
<div class="ml-pfoot">
  <button class="ml-btn" onclick={() => (m.panel = null)}>Cancel</button>
  <button
    class="ml-btn primary"
    disabled={m.busy || m.willRename === 0 || m.sending.size > 0 || m.previewing}
    onpointerdown={() => m.armReplace()}
    onkeydown={() => m.armReplace()}
    onclick={() => m.doFindReplace()}>
    Replace{m.willRename ? `${t("（")}${tn(m.willRename, "個）")}` : ""}
  </button>
</div>
