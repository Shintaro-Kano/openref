<script lang="ts">
  /* OpenNote: html ページの表示。iframe ではなく shadow DOM（同一 origin = リンクのクリックを
     横取りでき md ⇄ html の遷移が成立）。script は常に無効・`<link rel=stylesheet>` は vault 内ならインライン化（render/html.ts） */
  import type { WikiViewProps } from "./types";
  import ShadowPage from "./render/ShadowPage.svelte";

  let props: WikiViewProps = $props();
</script>

<ShadowPage {...props} mode="html" />
