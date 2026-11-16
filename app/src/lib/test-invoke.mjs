/* 試験の invoke の偽物の土台 = command 名 → 本物と同じ形の最小の応答の表 1 つ。
   App を丸ごと mount する試験（app / sidebarw / wiki/focus / shelf/shelf）は、これを土台に自分の場面に要る command だけを上書きする:
     const inv = fakeInvoke(calls, { list_items: () => items, ... });  globalThis.__invoke = inv.invoke;  …  inv.done();
   - 表にも上書きにも無い command は、黙って undefined を返さずに投げる（名前を言う）。App が握りつぶして toast にしても、
     試験の最後の inv.done() がそこで落とす = command を足したときに、ここへ足し忘れたことに気づく
   - 値の形は本物（app/src-tauri/src の command の戻り値・app/src/lib/api.ts・wiki/types.ts・shelf/ipc.ts の型）に合わせる。
     中身は「何も無い」状態（空の一覧・OFF・null）。場面の値（蔵書・ページ・棚）は試験の側で上書きする
   - 応答の関数は (args, payload) を受ける。args = payload.args ?? payload ?? {}（wiki_* は { args: {...} } で包む契約・ほかは素の引数）
   node の依存なし（vite からも読める） */

/** command → 応答（本物と同じ形の最小の値） */
export const INVOKE_DEFAULTS = {
  // 設定（ui_*）: 何も保存されていない
  get_ui_pref: () => null,
  set_ui_pref: () => null,
  data_version: () => 1,
  check_update: () => null, // string | null = 新しい版が無い
  // 蔵書: 空
  list_items: () => [],
  collections_tree: () => [],
  list_tags: () => [],
  view_counts: () => ({ all: 0, starred: 0, trash: 0 }),
  item_types: () => [],
  subtypes: () => [],
  set_star: () => null, // ★ の付け外し（main.rs set_star = 戻り値なし）
  membership: () => ({ collections: [], tags: [] }), // 選んだ item の所属の集計（core search::Membership = 0 件の所属先は含まない）
  list_annotations: () => [[], []], // [注釈（AnnotationRow）, メモ（NoteRow）] = core annotate.rs
  attachment_data: () => new ArrayBuffer(0),
  // 引用のスタイル
  cite_prefs: () => ({ style: "vancouver", locale: "", doi_always: false }),
  cite_catalog: () => [],
  cite_installed: () => [],
  cite_style_xml: () => "",
  // ライブラリ移行: job なし・履歴なし
  migration_status: () => null,
  migration_history: () => [],
  // OpenNote・チャット: OFF
  wiki_settings: () => ({ enabled: false, root: null }),
  wiki_status: () => ({ enabled: false, root: null }),
  // 無効のときの形（wiki_chat_gui.rs の wiki_chat_status_now の早い return = auth・model は無い・reason は OpenNote が無効の文）
  wiki_chat_status: () => ({ enabled: false, cli: null, version: null, logged_in: null, mcp: null, reason: "OpenNote が無効です", state: "off", session_id: null, root: null }),
  wiki_front_root: (a) => ({ root: a.root, restart: false }),
  wiki_list: () => ({ total: 0, count: 0, pages: [], excluded: [] }),
  // 空の md（core wiki/commands.rs の read: 本文のある種類の欄・sha256 = 空のバイト列の物）
  wiki_read_gui: (a) => ({
    path: a.path,
    abs_path: `/v/${a.path}`,
    kind: "md",
    size: 0,
    mtime: 0,
    title: a.path,
    tags: [],
    items: [],
    frontmatter: null,
    links: [],
    sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    chars: 0,
    truncated: false,
    content: "",
  }),
  wiki_backlinks: (a) => ({ target: a.path ?? a.key ?? null, total: 0, count: 0, pages: [] }), // target = 渡した path か key
  wiki_tags: () => ({ total: 0, count: 0, tags: [] }),
  wiki_inbox: (a) => ({ dir: `${a.dir ?? "ingest"}/`, total: 0, count: 0, files: [] }),
  wiki_ref_dirs_get: () => ({ dirs: [] }),
  // 本棚: 棚なし
  shelf_status: () => ({ shelves: [] }),
  shelf_front_root: (a) => ({ root: a.root }),
  shelf_tree: (a) => ({
    root: a.root,
    dir: null,
    tree: { id: `f-${a.root}`, name: a.root.slice(a.root.lastIndexOf("/") + 1), path: a.root, books: [], children: [], totalBooks: 0 },
    unreadable: 0,
    used_find: false,
    thumbs_dir: null,
  }),
  shelf_thumb: (a) => ({ path: `/thumbs/${a.id}.png` }),
  shelf_reveal: () => null,
  shelf_open_external: () => null,
};

/**
 * invoke の偽物を作る。calls = 記録の配列（{ name, args } を積む）・overrides = この試験の場面の応答（表の上書き・追加）。
 * 戻り値: invoke（globalThis.__invoke に入れる）/ unknown（表に無くて呼ばれた command の名前）/ done()（試験の最後に呼ぶ = unknown があれば投げる）
 * @param {{ name: string, args: any }[]} calls
 * @param {Record<string, (args: any, payload: any) => any>} [overrides]
 */
export function fakeInvoke(calls, overrides = {}) {
  const table = { ...INVOKE_DEFAULTS, ...overrides };
  const unknown = new Set();
  const invoke = async (name, payload) => {
    const a = payload?.args ?? payload ?? {};
    calls.push({ name, args: a });
    const h = table[name];
    if (!h) {
      const msg = `偽の invoke に無い command: ${name}（app/src/lib/test-invoke.mjs の INVOKE_DEFAULTS に足すか、試験で上書きする）`;
      // 初回はその場で出す = App が例外を握りつぶして別の表明で先に落ちても、出力に名前が残る
      if (!unknown.has(name)) console.error(msg);
      unknown.add(name);
      throw new Error(msg);
    }
    return h(a, payload);
  };
  const done = () => {
    if (unknown.size) throw new Error(`偽の invoke に無い command が呼ばれた: ${[...unknown].sort().join(", ")}`);
  };
  return { invoke, unknown, done };
}
