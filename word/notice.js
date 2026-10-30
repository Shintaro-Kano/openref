/* OpenRef — 通知ダイアログ（Paperpile のエラー表示の配置: 左にロゴ・見出し・本文・右下にボタン）。
   d = {kind:"error"|"confirm"|"info", title?, message, detail?, ok?（confirm の OK ラベル）, danger?, copy?}。
   親へ: error / info → {type:"close"}、confirm → {type:"ok"} / {type:"cancel"} */
(function () {
  "use strict";
  var RD = window.OpenRefDialog;
  var d = RD.params().d || {};
  var kind = d.kind === "confirm" || d.kind === "info" ? d.kind : "error";
  var sent = false;

  function $(id) {
    return document.getElementById(id);
  }
  function finish(type) {
    if (sent) return;
    sent = true;
    $("rd-ok").disabled = true;
    $("rd-cancel").disabled = true;
    if (!RD.send({ type: type })) {
      setTimeout(function () {
        sent = false;
        $("rd-ok").disabled = false;
        $("rd-cancel").disabled = false;
      }, 1500);
    }
  }

  RD.ready(function () {
    document.body.classList.add("rd-" + kind);
    var ok = $("rd-ok");
    var cancel = $("rd-cancel");
    var copy = $("rd-copy");
    $("rd-warn").hidden = kind !== "error";
    $("rd-title").textContent = d.title || (kind === "error" ? "エラー" : kind === "confirm" ? "確認" : "情報");
    $("rd-msg").textContent = d.message || "";
    if (d.detail) {
      $("rd-detail").textContent = d.detail;
      $("rd-detail").hidden = false;
    }

    if (kind === "confirm") {
      cancel.hidden = false;
      ok.textContent = d.ok || "OK";
      if (d.danger) {
        ok.classList.remove("rd-primary");
        ok.classList.add("rd-danger");
      }
      ok.addEventListener("click", function () {
        finish("ok");
      });
      cancel.addEventListener("click", function () {
        finish("cancel");
      });
    } else {
      ok.textContent = "閉じる";
      ok.addEventListener("click", function () {
        finish("close");
      });
    }
    if (kind === "info" || d.copy) {
      copy.hidden = false;
      copy.addEventListener("click", function () {
        var text = [d.title, d.message, d.detail].filter(Boolean).join("\n");
        RD.copyText(text).then(
          function () {
            $("rd-copied").textContent = "コピーしました";
          },
          function () {
            // クリップボードに書けない環境: 本文を選択状態にして ⌘C に任せる
            try {
              window.getSelection().selectAllChildren($("rd-msg"));
            } catch (e) {
              /* 選択もできない */
            }
            $("rd-copied").textContent = "自動でコピーできません — 選択したので ⌘C でコピーしてください";
          }
        );
      });
    }
    document.addEventListener("keydown", function (e) {
      if (e.key === "Escape") {
        e.preventDefault();
        finish(kind === "confirm" ? "cancel" : "close");
      }
    });
    // 破壊的な確認は既定をキャンセルに（Enter で消さない）
    RD.focus(kind === "confirm" && d.danger ? cancel : ok);
  });
})();
