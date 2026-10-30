/* 呼び手が catch しない reject（onclick から呼ぶ IPC・onMount の読込など）を握りつぶさず toast へ。
   main.ts（と一覧ハーネス）が起動時に 1 回だけ呼ぶ。console への既定の出力（preventDefault しない）は残す */
import { toast } from "./state.svelte";
import { t } from "./i18n.svelte";

/** 意図的な中断の reject（toast に出さない）: pdf.js の描画取消（RenderingCancelledException = PdfViewer の
    invalidateAll / 解放 / unmount）・ストリーム中断（AbortException。worker 終了の "Worker was terminated." もこの型）・
    unmount の loadingTask.destroy による文書の破棄（pdfjs-dist 6.1.200 の文言: 読込中の getDocument が
    "Worker was destroyed" / "Loading aborted"・破棄の後に届いた getPage の応答が "Transport destroyed"）・
    AbortController（AbortError） */
export function intentionalAbort(r: unknown): boolean {
  const name = (r as { name?: unknown } | null)?.name;
  if (name === "RenderingCancelledException" || name === "AbortException" || name === "AbortError") return true;
  return r instanceof Error && /^(Worker was (destroyed|terminated)|Loading aborted|Transport destroyed)/.test(r.message);
}

export function toastUnhandledRejections() {
  window.addEventListener("unhandledrejection", (e) => {
    if (intentionalAbort(e.reason)) return;
    toast(`✗ ${t("処理に失敗しました")}: ${String(e.reason).slice(0, 200)}`);
  });
}
