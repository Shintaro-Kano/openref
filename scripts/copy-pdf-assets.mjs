// pdf.js の補助データを app/public/pdfjs/ へ写す(app/package.json の predev / prebuild から)。写した先は git に入れない(.gitignore)。
// PdfViewer の getDocument が URL で読む。wasm/ が無いと JBIG2・CCITT・JPEG 2000 の画像が、cmaps/ が無いと埋め込みなしの CID フォントの字が黙って消える。
// standard_fonts/ = 埋め込みなしの標準フォント。iccs/ と wasm/qcms_bg.wasm は useWorkerFetch: false の間は読まれない(ICC の色は使っていない)。
// pdfjs-dist と版が揃わないと壊れるので、毎回消して写し直す(約 3 MB)。
import { cpSync, existsSync, rmSync } from "node:fs";
import { basename } from "node:path";
import { fileURLToPath } from "node:url";

const app = fileURLToPath(new URL("../app/", import.meta.url));
const from = `${app}node_modules/pdfjs-dist/`;
const to = `${app}public/pdfjs`; // 末尾の / を付けない = symlink のときリンク先の中身を消さない
const dirs = ["cmaps", "standard_fonts", "wasm", "iccs"];

for (const d of dirs) {
  if (!existsSync(from + d)) {
    console.error(`copy-pdf-assets: ${from + d} が無い(app で npm ci 済み?)`);
    process.exit(1);
  }
}
rmSync(to, { recursive: true, force: true });
for (const d of dirs) {
  // quickjs-eval = pdf.js の JavaScript sandbox 用。使っていない(pdf.mjs / pdf.worker.mjs から読まれない)ので写さない
  cpSync(from + d, `${to}/${d}`, { recursive: true, filter: (p) => !basename(p).startsWith("quickjs-eval") });
}
