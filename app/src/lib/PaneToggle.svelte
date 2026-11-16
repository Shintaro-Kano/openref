<script lang="ts">
  /* 右ペインの開閉（▣ = 一覧・OpenNote・本棚・本の読み手で同じ部品 = 同じ絵・大きさ・title・aria-pressed・⌘⌥I）。
     置き場は「タブ列の下の最初の段の右端」= 一覧・本棚・OpenNote はアプリのヘッダーの右端（App / WikiHeader）・本の読み手は読み手の段の右端。
     PDF のビューアは自分のツールバーの .tb-pane（同じ絵・同じ大きさ = app.css で右端を揃える）。見た目は app.css の .pn-toggle */
  import { t } from "./i18n.svelte";

  let { open, ontoggle }: { open: boolean; ontoggle: () => void } = $props();
  const label = $derived(open ? t("右のペインを隠す（⌘⌥I）") : t("右のペインを表示（⌘⌥I）"));
</script>

<button class="pn-toggle" class:pn-off={!open} title={label} aria-label={label} aria-pressed={open} onclick={ontoggle}>
  <svg width="16" height="14" viewBox="0 0 16 14" aria-hidden="true">
    <rect x="0.75" y="0.75" width="14.5" height="12.5" rx="2" fill="none" stroke="currentColor" stroke-width="1.5" />
    <rect class="pn-fill" x="10" y="1.5" width="4.5" height="11" fill="currentColor" />
    <line x1="10" y1="1" x2="10" y2="13" stroke="currentColor" stroke-width="1.5" />
  </svg>
</button>
