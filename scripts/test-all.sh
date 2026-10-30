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
run "i18n" node app/src/lib/i18n.test.mjs
# フォルダ・ラベルの名前順 / 件数順の比較関数(core の COLLATE NOCASE と同じ並び)
run "sort" node app/src/lib/sort.test.mjs
# フォルダ・ラベルの名前の規則(core と同じ = trim・フォルダは `/` 不可で同じ親の下・ラベルは全体で COLLATE NOCASE 一意)
run "folders" node app/src/lib/folders.test.mjs
# 参照エディタの CSL ⇄ フォーム変換(cslForm.ts)
run "editor csl" node app/src/lib/cslForm.test.mjs
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
# 配布物のシェルの構文・依存のライセンス
run "shell syntax" bash -n site/install.sh site/uninstall.sh scripts/bundle-word.sh scripts/build-dialog-host.sh word/dotm/e2e/run.sh scripts/test-all.sh
run "licenses" python3 scripts/check-licenses.py

echo
if [ ${#FAILED[@]} -gt 0 ]; then
  echo "FAILED (${#FAILED[@]}): ${FAILED[*]}" >&2
  exit 1
fi
echo "ALL PASS"
