<script lang="ts">
  import { t } from "./i18n.svelte";
  import { app, quickAdd } from "./state.svelte";

  let value = $state("");

  async function submit(e: Event) {
    e.preventDefault();
    const v = value;
    value = "";
    await quickAdd(v);
  }
</script>

<form class="quickadd" onsubmit={submit}>
  <input
    id="quickadd"
    placeholder={t("⌘N — PMID / DOI / PubMed URL を貼って一発登録")}
    bind:value
    disabled={app.busy}
  />
</form>
