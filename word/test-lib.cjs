// Word の node 試験の共通の準備（test-cite / test-styles / test-dotm-render が同じ物を読む）。
// citeproc（vendor/citeproc.js）と cite-core.js（と dotm-render.js）を、この process の global に vm で読み込む = 本番と同じ
// 「script として読む」形（connectd の JavaScriptCore・Dialog.app の WebView と同じく、module ではなく global に CSL / OpenRefCite /
// OpenRefDotm を定義する）。呼んだ後は試験の側で CSL・OpenRefCite・OpenRefDotm を global の名前で使う。
"use strict";
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const dir = __dirname;
const read = (rel) => fs.readFileSync(path.join(dir, rel), "utf8");

let loaded = null;
/** citeproc + cite-core（dotm: true なら dotm-render も）を global に読む。2 度目以降は読み直さない。
 *  戻り値 = { dir, locale（vendor の en-US の locale XML）, style(id)（同梱の styles/<id>.csl）} */
function loadCite(opts = {}) {
  if (!loaded) {
    vm.runInThisContext(read("vendor/citeproc.js"));
    vm.runInThisContext(read("cite-core.js"));
    loaded = { dir, locale: read("vendor/locales-en-US.xml"), style: (id) => read(path.join("styles", id + ".csl")), dotm: false };
  }
  if (opts.dotm && !loaded.dotm) {
    vm.runInThisContext(read("dotm-render.js"));
    loaded.dotm = true;
  }
  return loaded;
}

module.exports = { loadCite };
