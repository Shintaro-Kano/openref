// 本棚の Tauri command の包み（契約 = app/src-tauri/src/shelf_gui.rs・引数は OpenNote と同じ { args: {...} }）。
// state.svelte.ts の復元も使う = ここは state を import しない
import { invoke } from "@tauri-apps/api/core";
import type { ShelfFolder, ShelfRoot } from "./tree";

export interface ShelfTreeReply {
  root: string;
  dir: null;
  tree: ShelfFolder;
  /** 読めなかったサブフォルダの数 */
  unreadable: number;
  used_find: boolean;
  /** 表紙の置き場（canonical）。無ければ表紙を描かない（in-memory の DB） */
  thumbs_dir: string | null;
}

const call = <T>(name: string, args: Record<string, unknown>) => invoke<T>(name, { args });

export const shelfIpc = {
  status: () => invoke<{ shelves: ShelfRoot[] } | null>("shelf_status"),
  setOpts: (root: string, opts: { collapse_single?: boolean }) => call<unknown>("shelf_set_opts", { root, ...opts }),
  /** native のフォルダの選択（取り消し = root: null）。失敗の toast は Rust */
  openRoot: () => call<{ root: string | null }>("shelf_open_root", {}),
  /** 履歴の本棚へ（走査し直す = 読み直しもこれ）。失敗の toast は Rust */
  switchRoot: (root: string) => call<{ root: string }>("shelf_switch_root", { root }),
  /** 履歴から外す（表と表紙も消す）。結果の toast は Rust */
  forgetRoot: (root: string) => call<{ thumbs: number; unknown: boolean }>("shelf_forget_root", { root }),
  /** 走査しない・asset scope だけ = 再起動で戻したタブ */
  frontRoot: (root: string) => call<{ root: string }>("shelf_front_root", { root }),
  tree: (root: string) => call<ShelfTreeReply>("shelf_tree", { root }),
  thumb: (root: string, id: string) => call<{ path: string }>("shelf_thumb", { root, id }),
  /** id なし = 本棚のフォルダ自身 */
  reveal: (root: string, id?: string) => call<null>("shelf_reveal", id ? { root, id } : { root }),
  openExternal: (root: string, id: string) => call<null>("shelf_open_external", { root, id }),
};
