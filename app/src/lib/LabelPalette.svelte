<script lang="ts">
  import { t } from "./i18n.svelte";
  /* ラベル色の選択（Paperpile の New label → 色ボタンで開く 5×5 の「A」見本）。class は lp- 接頭辞 */
  import { PALETTE, labelVars } from "./labels";

  let { value = null, onpick }: { value?: string | null; onpick: (id: string) => void } = $props();
</script>

<div class="lp-grid" role="listbox" aria-label={t("ラベルの色")}>
  {#each PALETTE as p (p.id)}
    <button
      class="lp-sw lbadge"
      class:solid={p.solid}
      class:on={value === p.id}
      style={labelVars(p.id)}
      role="option"
      aria-selected={value === p.id}
      title={p.id}
      onclick={(e) => {
        e.stopPropagation();
        onpick(p.id);
      }}>A</button>
  {/each}
</div>
