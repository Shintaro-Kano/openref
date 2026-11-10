#!/bin/bash
# テスト一式(CI・release・手元で同じものを走らせる唯一の一覧)。テストを足すときはここに 1 行足す。
# 前提: Rust(cargo)・Node.js 22.18+・python3・app/ で npm ci 済み(svelte-check が app/node_modules にある)
# 使い方: bash scripts/test-all.sh   … 全部走らせて最後に失敗の一覧を出す(1 つでも失敗すれば exit 1)
set -uo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH" # rustup を --no-modify-path で入れた環境向け

FAILED=()
run() {
  local name="$1"
  shift
  echo "==> $name"
  if "$@"; then
    echo "    ok: $name"
  else
    echo "    FAILED: $name" >&2
    FAILED+=("$name")
  fi
}

# Rust(実 API / 実 PDF / 実 DB のものは #[ignore])
run "cargo test" cargo test --workspace --locked
# フロント: Svelte / TS の型検査(errors で失敗・warnings は数だけ出る)
run "svelte-check" npm --prefix app run --silent check
# PdfViewer の純関数・PDF 埋め込み注釈の無害化(XSS の防壁)・英語辞書の網羅・参照エディタの CSL ⇄ フォーム値
run "viewer util" node app/src/lib/viewer/util.test.mjs
run "viewer embedded" node app/src/lib/viewer/embedded.test.mjs
# PDF ビューアの右バーの開閉(実部品 PdfViewer + jsdom・pdf.js は偽物): ボタン / 既定は開 / 閉じた状態の ⌘F・⌕ で開いて検索欄へ / 隠れたビューアは開かない
run "viewer side" node app/src/lib/viewer/side.test.mjs
run "i18n" node app/src/lib/i18n.test.mjs
# フォルダ・ラベルの名前順 / 件数順の比較関数(core の COLLATE NOCASE と同じ並び)
run "sort" node app/src/lib/sort.test.mjs
# フォルダ・ラベルの名前の規則(core と同じ = trim・フォルダは `/` 不可で同じ親の下・ラベルは全体で COLLATE NOCASE 一意)
run "folders" node app/src/lib/folders.test.mjs
# 参照エディタの CSL ⇄ フォーム変換(cslForm.ts)
run "editor csl" node app/src/lib/cslForm.test.mjs
# OpenNote の描画: DOMPurify の許可リストと URL 規則・<style> の掃除・asset URL・表の分割(jsdom)・作者の data-wk-* が遷移の印にならない・contain の文字列 / リンク抽出の fixture が core と一致
run "wiki render" node app/src/lib/wiki/render.test.mjs
run "wiki links" node app/src/lib/wiki/links.test.mjs
# 実部品(Svelte 5 + jsdom)の 5 本は土台 app/src/lib/wiki/test-harness.mjs(loader・jsdom の global 化・Tauri の偽物)を共有。MediaView: page の差し替えで確かめていない src の PDF iframe を作らない
run "wiki mediaview" node app/src/lib/wiki/mediaview.test.mjs
# OpenNote の編集(実部品 + 実 CodeMirror + jsdom・invoke は差し替え): 自動保存の debounce・conflict の分岐・外部の書換え・caret への挿入・保存に失敗したまま閉じない・PICK_EXT = core の media_kind
run "wiki edit" node app/src/lib/wiki/edit.test.mjs
# OpenNote のチャット(実部品 + jsdom・invoke と event "wiki-chat" は差し替え): stream-json の reducer・送信と文脈・変更の一覧と元に戻す・診断と異常終了・右端の柱(ChatDock)= OFF でボタンなし・幅とつまみ
run "wiki chat" node app/src/lib/chat/chat.test.mjs
# App の組立て(実部品 App.svelte + jsdom): 一覧の右ペインの開閉(2 列・ListToolbar のボタン・⌘⌥I の振り分け)・行の選択で開かない・reveal は行を見せる・チャットの入力欄の Esc・PDF のタブ
run "app shell" node app/src/lib/app.test.mjs
# 本棚: 純関数(検索 NFKC + AND・並び・dir の索引・上限つきの設定・asset の区間)と組立て(実部品 App.svelte + jsdom): 見出しのメニュー・タブの永続と復元・⌘N / ⌘A / Esc / `/` / ドロップの遮断・表紙と温め。
# range = 読み手の読み方(偽の fetch): 1 区間の上限を Rust の MAX_LEN と結ぶ・素の GET を出さない・no-store・同時 16・閉じた後は呼ばない
run "shelf tree" node app/src/lib/shelf/tree.test.mjs
run "shelf" node app/src/lib/shelf/shelf.test.mjs
run "shelf range" node app/src/lib/shelf/range.test.mjs
run "shelf viewer" node app/src/lib/shelf/viewer.test.mjs
# IME ガード(実部品 + jsdom): かなの確定 Enter・取消 Esc(isComposing / keyCode 229)をページ内検索・検索欄・window の Esc・モーダルが拾わない
run "wiki ime" node app/src/lib/wiki/ime.test.mjs
# エラーの契約(実部品 wiki.svelte.ts + jsdom): wiki_run の {"code","message"} を ipc.ts が解き、画面は code(rebuilding / bad_query)で分岐・欄と toast は message
run "wiki ipc" node app/src/lib/wiki/ipc.test.mjs
# ワークスペースの切替(実部品 wiki.svelte.ts + jsdom・invoke は差し替え): 一覧(新しい順・現在は ✓)・切替は先に保存してからタブをホームへ・履歴から外す・仮の状態からの最初の読みは変化と見ない
run "wiki workspaces" node app/src/lib/wiki/workspaces.test.mjs
# Word: citeproc 実整形(同梱スタイル・型・誌名スタイル)・OpenRef.dotm の整形とダイアログ・dotm の生成器
run "word cite" node word/test-cite.cjs
run "word types" node word/test-types.cjs
run "word styles" node word/test-styles.cjs
run "word dotm-render" node word/test-dotm-render.cjs
run "word dialog-dotm" node word/test-dialog-dotm.cjs
run "word dialog-pages" node word/test-dialog-pages.cjs
run "dotm build" python3 word/dotm/test-build.py
# Chrome 拡張(OpenRef Capture)
run "capture extension" node --test extension/tests/capture.test.cjs
# 配布物のシェルの構文(bash -n は最初のファイルしか見ないので 1 本ずつ)・インストーラの Claude Desktop 登録(偽 HOME・偽 ps)・依存のライセンス
for f in site/install.sh site/uninstall.sh scripts/bundle-word.sh scripts/build-dialog-host.sh word/dotm/e2e/run.sh scripts/test-all.sh scripts/test-install-desktop-mcp.sh; do
  run "shell syntax $f" bash -n "$f"
done
run "install desktop-mcp" /bin/bash scripts/test-install-desktop-mcp.sh   # /bin/bash 3.2 = curl | bash の利用者が使う bash
run "licenses" python3 scripts/check-licenses.py
# pdf.js の補助データ(cmaps / standard_fonts / wasm / iccs)を写せるか = pdfjs-dist の配置が変わったらここで止まる
run "pdf assets" node scripts/copy-pdf-assets.mjs
# 任意の追加検査(ファイルが無ければ飛ばす)
if [ -f scripts/dev-checks.sh ]; then run "dev checks" bash scripts/dev-checks.sh; fi

echo
if [ ${#FAILED[@]} -gt 0 ]; then
  echo "FAILED (${#FAILED[@]}): ${FAILED[*]}" >&2
  exit 1
fi
echo "ALL PASS"
