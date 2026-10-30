<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "./i18n.svelte";
  import { api, type TypeDef } from "./api";
  import { cslToForm, editorType, filledExtras, formToCsl, type Orig } from "./cslForm";
  import { formatBibliography } from "./cite";
  import { app, closeEditor, refreshAll, reloadDetail, reveal, toast } from "./state.svelte";
  import { onEnter } from "./input";

  let types = $state<TypeDef[]>([]);
  let subs = $state<string[]>([]);
  let typeId = $state("article-journal");
  let subtype = $state("");
  let values = $state<Record<string, any>>({});
  let extraFields = $state<string[]>([]);
  let addPick = $state("");
  let preview = $state("");
  // 既定スタイルで開く。選択肢は同梱 + 導入済み
  let previewStyle = $state(app.cite.style);
  let busy = $state(false);
  let err = $state("");
  let lookupInput = $state("");

  const def = $derived(types.find((t) => t.id === typeId));
  const shown = $derived(
    (def?.fields ?? []).filter((f) => f.core || extraFields.includes(f.key))
  );
  const addable = $derived(
    (def?.fields ?? []).filter((f) => !f.core && !extraFields.includes(f.key))
  );

  /** core types.rs の型名・説明・欄名（日本語のラベルが鍵。"::type" で UI の同じ語と訳を分ける。網羅は i18n.test.mjs が types.rs から検査） */
  const tt = (label: string) => t(`${label}::type`);

  /* ---- CSL ⇄ フォーム値（変換は cslForm.ts） ---- */
  // 読んだときの欄の文字列と元の値（欄がそのままなら保存は元の値を出す）。描画には使わない = $state にしない
  let orig: Orig = {};

  function loadCsl(csl: any) {
    typeId = editorType(csl);
    subtype = csl?.custom?.subtype ?? "";
    const r = cslToForm(csl, types);
    values = r.values;
    orig = r.orig;
    extraFields = filledExtras(types, typeId, r.values);
  }

  function buildCsl(): any {
    return formToCsl(typeId, subtype, values, orig, types);
  }

  async function updatePreview() {
    try {
      const csl = buildCsl();
      preview = csl.title ? await formatBibliography(csl, previewStyle, app.cite.locale, app.cite.doiAlways) : "";
    } catch {
      preview = "";
    }
  }

  async function doLookup() {
    const q = lookupInput.trim();
    if (!q) return;
    busy = true;
    err = "";
    try {
      loadCsl(await api.lookup(q));
      await updatePreview();
    } catch (e) {
      err = String(e);
    } finally {
      busy = false;
    }
  }

  async function save() {
    busy = true;
    err = "";
    try {
      const csl = buildCsl();
      if (app.editorKey) {
        await api.updateItem(app.editorKey, csl);
        toast(t("✓ 更新しました"));
        await refreshAll();
        // 編集は詳細の item（= editorKey）から開く。読み直すだけにして、ほかのチェックは保つ（select はその行だけをチェックする）
        await reloadDetail();
      } else {
        const key = await api.createManual(csl);
        toast(t("✓ 登録しました"));
        await refreshAll();
        // エディタを閉じてから出す（PDF タブの上で作ったときは reveal が新しいタブを開く = エディタの間はタブを開けない）
        closeEditor();
        await reveal(key);
        return;
      }
      closeEditor();
    } catch (e) {
      err = String(e);
    } finally {
      busy = false;
    }
  }

  function addField() {
    if (addPick) {
      extraFields = [...extraFields, addPick];
      addPick = "";
    }
  }

  onMount(async () => {
    types = await api.itemTypes();
    subs = await api.subtypes();
    if (app.editorCsl) loadCsl(app.editorCsl);
    await updatePreview();
  });
</script>

<!-- 背景のクリックでは閉じない（× / キャンセルのみ）: 入力中の書誌を 1 クリックで捨てない・入力欄のドラッグ選択を外で離しても閉じない -->
<div class="overlay" role="presentation">
  <div class="modal editor" role="dialog" aria-label="reference editor">
    <div class="modal-head">
      <h3>{app.editorKey ? t("参照を編集") : t("参照を新規作成")}</h3>
      <div class="head-tools">
        <input
          class="lookup"
          placeholder={t("DOI / PMID / URL から自動入力")}
          bind:value={lookupInput}
          onkeydown={(e) => onEnter(e, doLookup)}
        />
        <button onclick={doLookup} disabled={busy || !lookupInput.trim()}>{t("取得")}</button>
        <button class="x" onclick={closeEditor}>×</button>
      </div>
    </div>

    <div class="modal-body">
      <div class="grid">
        <span class="lbl">{t("種類")}</span>
        <select bind:value={typeId} onchange={() => { extraFields = filledExtras(types, typeId, values); updatePreview(); }}>
          {#each types as tp (tp.id)}
            <option value={tp.id}>{tt(tp.label)} — {tt(tp.desc)}</option>
          {/each}
        </select>

        <span class="lbl">{t("サブタイプ")}</span>
        <select bind:value={subtype}>
          <option value="">{t("（なし）")}</option>
          {#each subs as s (s)}<option value={s}>{s}</option>{/each}
        </select>

        {#each shown as f (f.key)}
          <span class="lbl">{tt(f.label)}</span>
          {#if f.kind === "multiline"}
            <textarea rows="3" bind:value={values[f.key]} oninput={updatePreview}></textarea>
          {:else if f.kind === "names"}
            <textarea
              rows="3"
              placeholder={t("1 行 1 名（Gaballah, Ayman H ／ 山田太郎 ／ 日本医学放射線学会）")}
              bind:value={values[f.key]}
              oninput={updatePreview}
            ></textarea>
          {:else if f.kind === "date"}
            <input
              placeholder="2026 / 2026-07 / 2026-07-25"
              bind:value={values[f.key]}
              oninput={updatePreview}
            />
          {:else}
            <input bind:value={values[f.key]} oninput={updatePreview} />
          {/if}
        {/each}

        {#if addable.length}
          <span class="lbl">{t("＋")}</span>
          <select bind:value={addPick} onchange={addField}>
            <option value="">{t("フィールドを追加…")}</option>
            {#each addable as f (f.key)}<option value={f.key}>{tt(f.label)}</option>{/each}
          </select>
        {/if}
      </div>

      <div class="preview">
        <div class="pv-head">
          <span>{t("引用プレビュー")}</span>
          <select bind:value={previewStyle} onchange={updatePreview}>
            {#each app.cite.installed as s (s.id)}<option value={s.id}>{s.title}</option>{/each}
          </select>
        </div>
        <div class="pv-body">{preview || t("（タイトルを入力すると表示されます）")}</div>
      </div>

      {#if err}<div class="err">{err}</div>{/if}
    </div>

    <div class="modal-foot">
      <button onclick={closeEditor}>{t("キャンセル")}</button>
      <button class="primary" onclick={save} disabled={busy || !String(values.title ?? "").trim()}>
        {app.editorKey ? t("保存") : t("作成")}
      </button>
    </div>
  </div>
</div>
