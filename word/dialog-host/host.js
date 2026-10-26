// OpenRef Dialog.app — Word の Startup テンプレート版（OpenRef.dotm）のダイアログの器。
// scripts/build-dialog-host.sh が osacompile -l JavaScript でアプレットにする（Info.plist に LSUIElement・ad-hoc 再署名）。
// connectd が `open -a "<app>" --args <base-url>` で起動 → GET /word/v2/dialog/next で待ち行列のセッションを取る →
// 浮動窓（NSWindow・NSFloatingWindowLevel・hidesOnDeactivate = false）+ WKWebView でページを開く →
// ページが document.title = "openref:done" にしたら閉じる。× で閉じたら POST …/closed（= キャンセル）。
// 続きのセッションがあれば同じ起動で処理してから終了する。
// 前提: 素の osascript の窓は macOS 14+ で前面化されない（LaunchServices で起動したアプレットなら
// キー窓になり入力欄にフォーカスまで入る）・NSPanel は hidesOnDeactivate で消える・setActivationPolicy は呼ばない・
// JS の null を NSURL の代わりに渡すと NSNull に化けて落ちる・stringWithContentsOfURL ではヘッダを足せない（NSMutableURLRequest）。
// テスト用: `--close-after <ms>` で × と同じ performClose: を指定 ms 後に起こす。
ObjC.import("Cocoa");
ObjC.import("WebKit");

var DEFAULT_BASE = "https://localhost:23120";
var DONE = "openref:done";
/** 1 回の起動で処理するセッションの上限（connectd が壊れて同じものを返し続けても止まる） */
var MAX_SESSIONS = 50;

function args() {
  var a = $.NSProcessInfo.processInfo.arguments;
  var out = [];
  for (var i = 0; i < a.count; i++) out.push(ObjC.unwrap(a.objectAtIndex(i)));
  return out;
}

/** 同期 HTTP（X-OpenRef ヘッダ付き）。戻り値 {ok（応答が来た）, text（本文。204 は ""）}。
    returningResponse の Ref は読まない（JXA で中身を参照すると osascript ごと落ちる・アプレットでは statusCode が
    undefined になる）→ 判定は本文だけで行う */
function http(method, url, body) {
  var req = $.NSMutableURLRequest.requestWithURL($.NSURL.URLWithString(url));
  req.HTTPMethod = method;
  req.timeoutInterval = 15;
  req.setValueForHTTPHeaderField("word", "X-OpenRef");
  if (body !== undefined) {
    req.setValueForHTTPHeaderField("application/json", "Content-Type");
    req.HTTPBody = $(body).dataUsingEncoding($.NSUTF8StringEncoding);
  }
  var data = $.NSURLConnection.sendSynchronousRequestReturningResponseError(req, Ref(), Ref());
  if (!data || data.isNil()) return { ok: false, text: "" };
  var s = $.NSString.alloc.initWithDataEncoding(data, $.NSUTF8StringEncoding);
  return { ok: true, text: s && !s.isNil() ? s.js : "" };
}

/** connectd のログへ（アプレットの stderr はどこにも残らない） */
function report(base, level, message) {
  try {
    http("POST", base + "/word/v2/log", JSON.stringify({ level: level, message: "OpenRef Dialog.app: " + message }));
  } catch (e) {
    /* connectd に届かない */
  }
}

/** 編集メニュー（無いと WKWebView で ⌘C / ⌘V / ⌘A / ⌘Z が効かない）+ ⌘W。LSUIElement なのでメニューバーには出ない */
function installMenu(app) {
  var main = $.NSMenu.alloc.init;
  var appItem = $.NSMenuItem.alloc.init;
  main.addItem(appItem);
  var appMenu = $.NSMenu.alloc.initWithTitle("OpenRef");
  appMenu.addItem($.NSMenuItem.alloc.initWithTitleActionKeyEquivalent("Close", "performClose:", "w"));
  appItem.submenu = appMenu;
  var editItem = $.NSMenuItem.alloc.init;
  main.addItem(editItem);
  var edit = $.NSMenu.alloc.initWithTitle("Edit");
  [
    ["Undo", "undo:", "z"],
    ["Redo", "redo:", "Z"],
    ["Cut", "cut:", "x"],
    ["Copy", "copy:", "c"],
    ["Paste", "paste:", "v"],
    ["Select All", "selectAll:", "a"],
  ].forEach(function (m) {
    edit.addItem($.NSMenuItem.alloc.initWithTitleActionKeyEquivalent(m[0], m[1], m[2]));
  });
  editItem.submenu = edit;
  app.mainMenu = main;
}

/** 1 セッションの窓を開き、閉じるまで回す。戻り値 "done"（ページが結果を送った）/ "closed"（× など）。
    開いて 0.8 秒後に前面化できたか（active / key）を connectd のログへ 1 行（macOS は前面のアプリ以外からの起動を
    前面化しないことがある = そのときは窓をクリックすれば入力できる） */
function show(app, s, closeAfter, base) {
  var w = s.width || 600;
  var h = s.height || 460;
  try {
    var vf = $.NSScreen.mainScreen.visibleFrame;
    w = Math.max(320, Math.min(w, vf.size.width - 40));
    h = Math.max(200, Math.min(h, vf.size.height - 60));
  } catch (e) {
    /* 画面の大きさが取れなければ要求どおり */
  }
  var rect = $.NSMakeRect(0, 0, w, h);
  var style = $.NSWindowStyleMaskTitled | $.NSWindowStyleMaskClosable | $.NSWindowStyleMaskResizable;
  var win = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer(rect, style, $.NSBackingStoreBuffered, false);
  win.title = s.title || "OpenRef";
  win.level = $.NSFloatingWindowLevel;
  win.releasedWhenClosed = false;
  win.hidesOnDeactivate = false;
  var wv = $.WKWebView.alloc.initWithFrameConfiguration(rect, $.WKWebViewConfiguration.alloc.init);
  win.contentView = wv;
  wv.loadRequest($.NSURLRequest.requestWithURL($.NSURL.URLWithString(s.url)));
  win.center;
  win.makeKeyAndOrderFront(null);
  win.makeFirstResponder(wv);
  app.activateIgnoringOtherApps(true);

  var t0 = Date.now();
  var how = "closed";
  // 前面・キー窓になったかを connectd のログへ（0.8 秒・3 秒・6 秒。VBA が前面の Word から open -a し直すと後から前面になる）
  var notes = [800, 3000, 6000];
  for (;;) {
    var ev = app.nextEventMatchingMaskUntilDateInModeDequeue(
      $.NSEventMaskAny,
      $.NSDate.dateWithTimeIntervalSinceNow(0.05),
      $.NSDefaultRunLoopMode,
      true
    );
    if (ev && !ev.isNil()) app.sendEvent(ev);
    if (!win.isVisible) break;
    var t = wv.title;
    if (t && !t.isNil() && t.js.indexOf(DONE) === 0) {
      how = "done";
      break;
    }
    if (notes.length && Date.now() - t0 > notes[0]) {
      report(base, "info", "shown " + s.sid + " +" + notes.shift() + "ms active=" + app.isActive + " key=" + win.isKeyWindow);
    }
    if (closeAfter && Date.now() - t0 > closeAfter) {
      closeAfter = 0;
      win.performClose(null);
    }
  }
  win.orderOut(null);
  return how;
}

function run() {
  var a = args();
  var base = DEFAULT_BASE;
  var closeAfter = 0;
  for (var i = 1; i < a.length; i++) {
    if (/^https?:\/\//.test(a[i])) base = a[i].replace(/\/+$/, "");
    else if (a[i] === "--close-after" && i + 1 < a.length) closeAfter = parseInt(a[++i], 10) || 0;
  }
  var app = $.NSApplication.sharedApplication;
  installMenu(app);
  for (var n = 0; n < MAX_SESSIONS; n++) {
    // 待ち行列のセッション 1 件 = {sid,url,width,height,title}。無ければ 204（本文なし）→ 終了
    var r = http("GET", base + "/word/v2/dialog/next");
    if (!r.ok || !r.text) break;
    var s = null;
    try {
      s = JSON.parse(r.text);
    } catch (e) {
      s = null;
    }
    if (!s || typeof s.sid !== "string" || typeof s.url !== "string") {
      report(base, "error", "next の応答を読めません: " + r.text.slice(0, 200));
      break;
    }
    try {
      show(app, s, closeAfter, base);
    } catch (e) {
      report(base, "error", "窓を開けません（" + s.sid + "）: " + e.message);
    }
    // 窓が閉じた。結果が無いまま（× など）なら connectd が cancel にする（結果があれば何もしない）
    var c = http("POST", base + "/word/v2/dialog/" + encodeURIComponent(s.sid) + "/closed", "{}");
    if (!c.ok) report(base, "warn", "closed を送れません（" + s.sid + "）");
  }
}
