<script lang="ts">
  import { t } from "./i18n.svelte";
  /* PDF 一括取込の進捗と結果一覧（Paperpile の Upload files に相当）。
     行をクリックすると登録された item へジャンプ。実行中は閉じられず「中止」のみ。 */
  import Modal from "./Modal.svelte";
  import { holdFocus } from "./focusReturn";
  import { app, cancelImport, closeImport, retryImportFailed, reveal, type ImportFile } from "./state.svelte";

  const job = $derived(app.importJob);
  const total = $derived(job?.files.length ?? 0);
  const done = $derived(job?.files.filter((f) => f.status !== "pending" && f.status !== "running").length ?? 0);
  const counts = $derived.by(() => {
    const c = { added: 0, attached: 0, needs_review: 0, duplicate: 0, error: 0, canceled: 0 };
    for (const f of job?.files ?? []) {
      const k = f.status === "attached_additional" ? "attached" : f.status === "no_id" ? "needs_review" : f.status;
      if (k in c) c[k as keyof typeof c]++;
    }
    return c;
  });
  const canRetry = $derived(!!job && !job.running && counts.error + counts.canceled > 0);

  const LABEL: Record<string, string> = $derived({
    pending: t("待機"),
    running: t("処理中…"),
    added: t("✓ 登録"),
    attached: t("✓ 既存に添付"),
    attached_additional: t("✓ 2 本目を添付"),
    needs_review: t("？ 要確認"),
    no_id: t("識別子なし"),
    duplicate: t("重複"),
    error: t("✗ 失敗"),
    canceled: t("中止"),
  });

  /** 開いたときの焦点 = 枠（tabindex=-1）。PDF をドロップしたとき・追加のときに出る窓 = 裏のエディタ・一覧の欄から焦点を外す
      （打った字・⌘F が裏に入らない）。ボタン（実行中は [中止]・終われば [閉じる]・行 = 登録した文献へ移る）ではなく枠 = Enter / Space で
      何も起きない。枠が出たとき（閉 → 開）に 1 回だけ（進み具合の更新では移し直さない）。明示の focus()（WKWebView では、焦点が他の欄に残っていると autofocus が無視される）。
      閉じたら（Esc・ボタン・覆い・処理の側のどれでも = 枠が外れて frame が null になる）開く前の焦点へ戻す（focusReturn.ts の条件） */
  let frame: HTMLDivElement | null = $state(null);
  $effect(() => {
    const f = frame;
    if (!f) return;
    const back = holdFocus(f); // 枠へ移す前に、開く前の焦点を覚える
    queueMicrotask(() => f.focus());
    return back;
  });

  function jump(f: ImportFile) {
    if (!f.key) return;
    // モーダルを閉じてから出す（PDF タブの上の取込では reveal が新しいタブを開く = 取込のモーダルの間はタブを開けない）。
    // 実行中は閉じない（closeImport が無視する）= 一覧のタブなら従来どおりモーダルの下で出す
    closeImport();
    void reveal(f.key);
  }
</script>

<!-- 背景（overlay 自身）のクリックだけで閉じる。実行中は closeImport が無視する。Esc は App の Esc の連鎖（closeImport）= 骨格では扱わない -->
<Modal open={!!job} cls="im-modal" label={t("PDF 取込")} tabindex={-1} onbackdrop={closeImport} bind:frame>
  {#if job}
    <div class="modal-head">
      <h3>{t("PDF を取り込む")}</h3>
      {#if !job.running}<button class="x" onclick={closeImport}>×</button>{/if}
    </div>
    <div class="im-progress"><div class="im-bar" style="width: {total ? (done / total) * 100 : 0}%"></div></div>
    <div class="im-summary">
      {done} / {total} {job.running ? t("処理中…") : t("完了")}
      {t("· 登録")} {counts.added} {t("· 既存に添付")} {counts.attached} {t("· 要確認")} {counts.needs_review} {t("· 重複")} {counts.duplicate} {t("· 失敗")} {counts.error}{#if counts.canceled} {t("· 中止")} {counts.canceled}{/if}
    </div>
    <div class="modal-body im-list">
      {#each job.files as f (f.path)}
        <button class="im-row {f.status}" disabled={!f.key} title={f.key ? t("クリックで開く") : f.path} onclick={() => jump(f)}>
          <span class="im-status">{LABEL[f.status] ?? f.status}</span>
          <span class="im-name">{f.name}</span>
          {#if f.message}<span class="im-msg">{f.message}</span>{/if}
        </button>
      {/each}
    </div>
    <div class="modal-foot">
      {#if job.running}
        <button onclick={cancelImport} disabled={job.cancel}>{job.cancel ? t("中止します…") : t("中止")}</button>
      {:else}
        {#if canRetry}<button onclick={retryImportFailed}>{t("失敗分を再試行")}</button>{/if}
        <button class="primary" onclick={closeImport}>{t("閉じる")}</button>
      {/if}
    </div>
  {/if}
</Modal>
