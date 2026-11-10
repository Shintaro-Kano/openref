// 読み手の PDF の読み方 = asset:// + pdf.js の PDFDataRangeTransport。下敷きは実測。
// asset_reply の制約: 1 区間の 206 は 1,024,000 byte で切られる / Range の無い GET と HEAD は全量を読む（= 素の GET を出さない）。
// pdf.js の制約: 連続する chunk を 1 つの requestDataRange にまとめる・onDataRange は 1 回で reader が閉じる = 分けて fetch して繋いでから 1 回で渡す
import * as pdfjs from "pdfjs-dist";
import { ASSET_MAX, rangeParts } from "./tree";

/** pdf.js の rangeChunkSize。pdf.js は最上位の /Pages の Kids を全部読む = 開くだけで「ページ数 × chunk」を読む（1,000 ページで 64 KiB → 66 MB・512 KiB → 524 MB） */
export const CHUNK = 64 * 1024;
/** 文書単位の同時 fetch の上限（1,000 ページの平らな木では開くだけで 1,000 本の range が一度に来る） */
const MAX_IN_FLIGHT = 16;
/** 1 つの requestDataRange の中で同時に出す fetch の数 */
const PARTS_AT_ONCE = 4;

/** 長さ = `bytes=0-0` の content-range の分母 */
export async function probeLength(url: string, signal: AbortSignal): Promise<number> {
  const r = await fetch(url, { headers: { Range: "bytes=0-0" }, signal, cache: "no-store" });
  if (r.status !== 206) throw new Error(`status ${r.status}`);
  const m = /\/(\d+)$/.exec(r.headers.get("content-range") ?? "");
  if (!m) throw new Error(`content-range: ${r.headers.get("content-range")}`);
  await r.arrayBuffer();
  return Number(m[1]);
}

class Gate {
  #free = MAX_IN_FLIGHT;
  #wait: (() => void)[] = [];
  async take() {
    if (this.#free > 0) {
      this.#free--;
      return;
    }
    await new Promise<void>((r) => this.#wait.push(r));
  }
  give() {
    const w = this.#wait.shift();
    if (w) w();
    else this.#free++;
  }
}

export class AssetRangeTransport extends pdfjs.PDFDataRangeTransport {
  #url: string;
  #ac = new AbortController();
  #onError: (e: unknown) => void;
  #gate = new Gate();

  /** onError = range の読込の失敗。pdf.js の transport には失敗を伝える口が無い（待ちが残り続ける）= 呼ぶ側で読み手を閉じる */
  constructor(url: string, length: number, onError: (e: unknown) => void) {
    // progressiveDone = true: 全量の順読みはしない（worker の readData がすぐ終わる）
    super(length, null as unknown as Uint8Array, true);
    this.#url = url;
    this.#onError = onError;
  }

  async #part(b: number, e: number): Promise<Uint8Array> {
    const signal = this.#ac.signal;
    await this.#gate.take();
    try {
      signal.throwIfAborted();
      // no-store = WebKit の memory cache に response を残さない（1.5 GB の本を飛ぶと全部残る）
      const r = await fetch(this.#url, { headers: { Range: `bytes=${b}-${e - 1}` }, signal, cache: "no-store" });
      if (r.status !== 206) throw new Error(`range ${b}-${e}: status ${r.status}`);
      const buf = new Uint8Array(await r.arrayBuffer());
      if (buf.byteLength !== e - b) throw new Error(`range ${b}-${e}: ${buf.byteLength} byte`);
      return buf;
    } finally {
      this.#gate.give();
    }
  }

  override requestDataRange(begin: number, end: number) {
    void this.#load(begin, end);
  }

  async #load(begin: number, end: number) {
    const signal = this.#ac.signal;
    try {
      const parts = rangeParts(begin, end, ASSET_MAX);
      let out: Uint8Array;
      if (parts.length === 1) {
        out = await this.#part(begin, end);
      } else {
        const dst = new Uint8Array(end - begin);
        let next = 0;
        const pump = async () => {
          while (next < parts.length) {
            const [b, e] = parts[next++];
            dst.set(await this.#part(b, e), b - begin);
          }
        };
        await Promise.all(Array.from({ length: Math.min(PARTS_AT_ONCE, parts.length) }, pump));
        out = dst;
      }
      if (!signal.aborted) this.onDataRange(begin, out);
    } catch (e) {
      if (!signal.aborted) this.#onError(e); // 閉じた（abort の AbortError・閉じた後に届いた分）は失敗ではない
    }
  }

  override abort() {
    this.#ac.abort();
  }
}
