// pdf.js の文書の開き方 = 文献の読み手（PdfViewer）と本棚の読み手（shelf/ShelfViewer）が共に通る 1 か所。
// 片方だけ直す事故を防ぐ（補助データが無いと、例外なしで字や図が黙って消える）。
//   - worker: vite の ?url で同梱の pdf.worker.mjs（開く前に毎回設定 = 前は各読み手の部品の初期化で設定していた）
//   - 補助データ: scripts/copy-pdf-assets.mjs が predev / prebuild で public/pdfjs/ へ写す 4 つ（cmaps・standard_fonts・wasm・iccs）。
//     無いと埋め込みなしの CID フォントの字・JBIG2 / CCITT / JPEG 2000 の画像が黙って消える
//   - useWorkerFetch: false = 本番の既定（tauri:// は pdf.js が worker から読めない URL と見なす）を dev の http:// でも同じにする。
//     false の間 ICC の色は使わない（iccUrl は読まれない）
//   - enableXfa: false
// 読み手ごとの違いは src に残す（揃えない）: 文献 = { data }（attachment_data の全量）/ 本棚 = { range, rangeChunkSize,
// disableAutoFetch, disableStream }（巨大な PDF を asset:// の Range で分けて読む = shelf/rangeTransport.ts）。
// 描画の倍率の上限・ページの解放（page.cleanup）・エラーの見せ方・破棄の順は各読み手の側のまま。
import * as pdfjs from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.mjs?url";

/** 補助データのフォルダの URL（今の文書の origin の /pdfjs/<dir>/） */
export const pdfAsset = (dir: string) => new URL(`/pdfjs/${dir}/`, location.href).href;

/** 読み手に依らない getDocument の引数（補助データの 4 つの URL・useWorkerFetch・enableXfa） */
export function pdfCommonParams() {
  return {
    enableXfa: false,
    cMapUrl: pdfAsset("cmaps"),
    standardFontDataUrl: pdfAsset("standard_fonts"),
    wasmUrl: pdfAsset("wasm"),
    iccUrl: pdfAsset("iccs"),
    useWorkerFetch: false,
  };
}

/** 文書を開く（pdf.js の読込タスクを返す。後始末は呼び手が destroy する）。src = 読み手ごとの読み方 */
export function openPdf(
  src: { data: Uint8Array } | { range: pdfjs.PDFDataRangeTransport; rangeChunkSize: number; disableAutoFetch: boolean; disableStream: boolean },
) {
  pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;
  return pdfjs.getDocument({ ...src, ...pdfCommonParams() });
}
