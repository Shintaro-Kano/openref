<script lang="ts">
  /* OpenNote: md ページの表示。中身は render/ShadowPage（html ページと共通）。
     props は契約 WikiViewProps（types.ts） */
  import type { WikiViewProps } from "./types";
  import ShadowPage from "./render/ShadowPage.svelte";

  let props: WikiViewProps = $props();
</script>

<ShadowPage {...props} mode="md" />
