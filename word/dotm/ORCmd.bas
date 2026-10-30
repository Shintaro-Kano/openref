Attribute VB_Name = "ORCmd"
Option Explicit

' リボンの 7 コマンドの流れ。各コマンドは引数なしの Public Sub = マクロとしても呼べる
' （ツール > ユーザー設定 > キーボード でショートカットを割り当てられる）。
' ダイアログは connectd に頼んで OpenRef Dialog.app に出し、結果を long-poll で待つ（待つ間も Word は操作できる = DoEvents）。

Public Const OR_VERSION As String = "2.0.0"

Private gBusy As Boolean
' 変更履歴の記録を止めている文書（OpenRef の書き換えの間だけ。位置で書式を当てるので、削除の記録が残ると位置がずれる）
Private gTrackDoc As Document
Private gTrackPrev As Boolean
' E2E テスト用（word/dotm/e2e）: ダイアログを出さずに返す R 行の列と、出したはずのダイアログの記録
Private gTestMode As Boolean
Private gTestQueue As Collection
Private gTestLog As String

Public Sub OpenRefAddEdit()
    RunCmd "addEdit"
End Sub

Public Sub OpenRefUpdate()
    RunCmd "update"
End Sub

Public Sub OpenRefCitationStyle()
    RunCmd "style"
End Sub

Public Sub OpenRefDocumentSettings()
    RunCmd "docSettings"
End Sub

Public Sub OpenRefMoveBibliography()
    RunCmd "moveBib"
End Sub

Public Sub OpenRefRemoveCitationCodes()
    RunCmd "removeCodes"
End Sub

Public Sub OpenRefShowDiagnostics()
    RunCmd "diagnostics"
End Sub

' ---------- コマンドの枠 ----------
' 実行中（ダイアログ待ち）に別のボタンが押されたら、ダイアログを前面に出すだけ
Private Sub RunCmd(ByVal name As String)
    Dim doc As Document, num As Long, msg As String, det As String, r As String
    If gBusy Then
        DialogFocus
        Exit Sub
    End If
    If Documents.Count = 0 Then
        Alert "文書を開いてから実行してください。"
        Exit Sub
    End If
    gBusy = True
    ORIpc.gErrDetail = ""
    On Error GoTo EH
    Set doc = ActiveDocument
    ORDoc.SkippedReset
    ' 保護された文書（編集の制限 = 変更履歴のみ・読み取り・フォーム）は書き換えられない → ダイアログを出す前に止める。診断は読むだけ
    If name <> "diagnostics" Then
        If doc.ProtectionType <> wdNoProtection Then ORIpc.Fail "この文書は保護されているため実行できません。", "文書の保護（校閲タブ）を解除してから実行してください。"
    End If
    Select Case name
        Case "addEdit": CmdAddEdit doc
        Case "update": CmdUpdate doc
        Case "style": CmdSettings doc, "style"
        Case "docSettings": CmdSettings doc, "doc"
        Case "moveBib": CmdMoveBib doc
        Case "removeCodes": CmdRemoveCodes doc
        Case "diagnostics": CmdDiagnostics doc
    End Select
    ' 変更履歴の記録が残る引用・文献リストを書き換えずに残したら知らせる（止めはしない = 共著者の記録を消さずに作業を続けられる）
    msg = ORDoc.SkippedNote()
    If Len(msg) > 0 Then r = Notice("info", "変更履歴が残っている箇所", msg, "校閲 で変更を承認または却下してから Update すると、番号と文献リストが揃います。")
    gBusy = False
    Exit Sub
EH:
    num = Err.Number
    msg = Err.Description
    det = ORIpc.gErrDetail
    ORIpc.gErrDetail = ""
    On Error Resume Next
    TrackResume
    UndoEnd
    Application.ScreenUpdating = True
    ' エラーの通知を待つ間（DoEvents）に押されたボタンも前面化だけにする
    ShowError name, num, msg, det
    gBusy = False
End Sub

' OpenRef の書き換えの間だけ変更履歴の記録を止める（Zotero の Word プラグインと同じ）。ダイアログを待つ間（利用者が打つ文字）は止めない
Private Sub TrackPause(ByVal doc As Document)
    Dim still As Boolean
    If Not gTrackDoc Is Nothing Then Exit Sub
    gTrackPrev = False
    On Error Resume Next
    gTrackPrev = doc.TrackRevisions
    If gTrackPrev Then doc.TrackRevisions = False
    still = doc.TrackRevisions
    Err.Clear
    On Error GoTo 0
    ' 編集の制限が「変更履歴のみ」の文書では止められない（黙って続けると書式の位置がずれる）→ 止める
    If still Then ORIpc.Fail "変更履歴の記録を止められないため実行できません（この文書は編集が制限されています）。", "文書の保護（校閲タブ）を解除してから実行してください。"
    Set gTrackDoc = doc
End Sub

Private Sub TrackResume()
    If gTrackDoc Is Nothing Then Exit Sub
    On Error Resume Next
    If gTrackPrev Then gTrackDoc.TrackRevisions = True
    Set gTrackDoc = Nothing
    Err.Clear
End Sub

Private Sub ShowError(ByVal cmd As String, ByVal num As Long, ByVal msg As String, ByVal det As String)
    Dim r As String
    If num <> ORIpc.OR_ERR And num <> ORIpc.OR_ERR_CONN Then
        ' VBA / Word の実行時エラー（利用者向けの文言ではない）= 番号と場所を補足に
        det = "VBA " & (num And &HFFFF&) & " · " & cmd & IIf(Len(det) > 0, " · " & det, "")
        ORIpc.LogLine "error", cmd & ": " & num & " " & msg
    End If
    If num = ORIpc.OR_ERR_CONN Then
        Alert msg & vbLf & vbLf & "OpenRef をインストールした Mac で、取込サービス（connectd）が動いているか確認してください。" & vbLf & vbLf & det
        Exit Sub
    End If
    On Error Resume Next
    r = Notice("error", "", msg, det)
    If Err.Number <> 0 Then
        Err.Clear
        Alert msg & IIf(Len(det) > 0, vbLf & vbLf & det, "")
    End If
End Sub

' ダイアログを出せないとき（connectd に繋がらない等）の Word 標準の警告。テスト中は記録だけ
Private Sub Alert(ByVal text As String)
    If gTestMode Then
        gTestLog = gTestLog & "ALERT " & Replace(text, vbLf, " ") & vbLf
    Else
        MsgBox text, vbExclamation, "OpenRef"
    End If
End Sub

Private Sub UndoStart(ByVal undoLabel As String)
    On Error Resume Next
    Application.UndoRecord.StartCustomRecord undoLabel
    Err.Clear
End Sub

Private Sub UndoEnd()
    On Error Resume Next
    If Application.UndoRecord.IsRecordingCustomRecord Then Application.UndoRecord.EndCustomRecord
    Err.Clear
End Sub

Private Function NoCitesMsg() As String
    NoCitesMsg = "この文書に OpenRef の引用が見つかりません。"
End Function

Private Function NoCitesDetail() As String
    NoCitesDetail = "Add/Edit Citation で引用を挿入してください。"
End Function

' ---------- ダイアログ ----------
' body = connectd /word/v2/dialog の要求。戻り値 = R 行の欄（0 = "R", 1 = type, 2… = kind ごと）
Private Function RunDialog(ByVal body As String) As String()
    Dim r() As String, sid As String, i As Long, rl As String, app As String
    If gTestMode Then
        gTestLog = gTestLog & body & vbLf
        If Not gTestQueue Is Nothing Then
            If gTestQueue.Count > 0 Then
                RunDialog = Split(gTestQueue(1), vbTab)
                gTestQueue.Remove 1
                Exit Function
            End If
        End If
        RunDialog = Split("R" & vbTab & "cancel", vbTab)
        Exit Function
    End If
    r = ORIpc.Post("/word/v2/dialog", body)
    For i = 1 To UBound(r)
        If Left$(r(i), 2) = "D" & vbTab Then
            sid = ORIpc.Field(r(i), 1)
            app = ORIpc.Unesc(ORIpc.Field(r(i), 2))
        End If
    Next
    If Len(sid) = 0 Or sid Like "*[!a-z0-9]*" Then ORIpc.Fail "ダイアログを開けませんでした。", "connectd /word/v2/dialog"
    ' connectd（常駐）が起動したアプレットは macOS のフォーカス横取り防止で前面に来ないことがある → 前面にいる Word から起こし直す
    ORIpc.OpenApp app
    Do
        ' 1 回の待ちは短く（待つ間 Word の画面は止まる。間の DoEvents で Word を操作できる）
        r = ORIpc.GetLines("/word/v2/dialog/" & sid & "?wait=200")
        rl = ""
        For i = 1 To UBound(r)
            If Len(r(i)) > 0 Then
                rl = r(i)
                Exit For
            End If
        Next
        If Left$(rl, 2) = "R" & vbTab Then
            RunDialog = Split(rl, vbTab)
            Exit Function
        End If
        DoEvents
    Loop
End Function

Private Sub DialogFocus()
    Dim r() As String
    On Error Resume Next
    If Not gTestMode Then r = ORIpc.Post("/word/v2/dialog/focus", "{}", 5)
    Err.Clear
End Sub

' 通知（error / confirm / info）。戻り値 = R 行の type（ok / close / cancel）
Private Function Notice(ByVal kind As String, ByVal title As String, ByVal message As String, Optional ByVal detail As String = "", _
        Optional ByVal okLabel As String = "", Optional ByVal danger As Boolean = False, Optional ByVal copy As Boolean = False) As String
    Dim body As String, r() As String
    body = "{""kind"":""notice"",""notice"":" & ORIpc.JStr(kind) & ",""message"":" & ORIpc.JStr(message)
    If Len(title) > 0 Then body = body & ",""title"":" & ORIpc.JStr(title)
    If Len(detail) > 0 Then body = body & ",""detail"":" & ORIpc.JStr(detail)
    If Len(okLabel) > 0 Then body = body & ",""ok"":" & ORIpc.JStr(okLabel)
    If danger Then body = body & ",""danger"":true"
    If copy Then body = body & ",""copy"":true"
    body = body & "}"
    r = RunDialog(body)
    If UBound(r) >= 1 Then Notice = r(1)
End Function

' ---------- Add/Edit Citation ----------
Private Sub CmdAddEdit(ByVal doc As Document)
    Dim kind As String, at As ContentControl, target As ContentControl, tag As String, mode As String
    Dim existing As String, style As String, doi As Boolean, pinned As Boolean, body As String, r() As String
    Dim num As Long, desc As String
    ORDoc.ImportLegacy doc
    ORDoc.Context doc, kind, at
    If kind = "bib" Then ORIpc.Fail "文献リストの中には引用を挿入できません。本文にカーソルを置いてください。"
    If kind = "cite" Then
        ' この引用の中に変更履歴の記録が残っていれば、編集・削除で記録が消える → ダイアログの前に止める（Move bibliography と同じ）
        If ORDoc.HasRevisions(at.Range) Then ORIpc.Fail "この引用には変更履歴が残っているため編集できません。", "校閲 で変更を承認または却下してから、もう一度実行してください。"
        mode = "edit"
        existing = ORDoc.PayloadGet(doc, at.Tag)
        Set target = at
    Else
        mode = "add"
        ' 前回の残骸の中: 中身が […] だけなら再利用、文字が打たれていれば文字を残して外し、その直後に新しい仮置き
        TrackPause doc
        If kind = "pending-reusable" Then
            Set target = at
        ElseIf kind = "pending" Then
            Set target = ORDoc.InsertPlaceholder(doc, at)
        Else
            Set target = ORDoc.InsertPlaceholder(doc, Nothing)
        End If
        TrackResume
    End If
    tag = target.Tag
    On Error GoTo Cleanup
    ORDoc.EffectivePrefs doc, style, doi, pinned
    body = "{""kind"":""cite"",""mode"":" & ORIpc.JStr(mode) & ",""style"":" & ORIpc.JStr(style) & ",""missing"":" & ORIpc.JBool(mode = "edit" And Len(existing) = 0) & ",""existing"":"
    If Len(existing) > 0 Then body = body & existing & "}" Else body = body & "null}"
    r = RunDialog(body)
    ' 文書を書き換える分岐だけ記録を止める（編集のキャンセルは文書に触れない = 「変更あり」にしない）
    If r(1) = "ok" Then
        TrackPause doc
        UndoStart "OpenRef: Add/Edit Citation"
        ORDoc.CommitCitation doc, target, tag, ORIpc.Unesc(ORIpc.Field(Join(r, vbTab), 2))
        UndoEnd
        TrackResume
    ElseIf r(1) = "delete" And mode = "edit" Then
        TrackPause doc
        UndoStart "OpenRef: Delete Citation"
        ORDoc.DeleteCitation doc, target, tag
        UndoEnd
        TrackResume
    ElseIf mode = "add" Then
        ' cancel / × で閉じた（追加の delete も同じ）: 仮置きを消す。編集は何もしない
        TrackPause doc
        ORDoc.RemovePlaceholder doc, tag
        TrackResume
    End If
    Exit Sub
Cleanup:
    ' 確定前に失敗したら仮置きを残さない（確定後 = refract-cite- に付け替え済みなら見つからず何もしない）
    num = Err.Number
    desc = Err.Description
    On Error Resume Next
    UndoEnd
    If mode = "add" Then
        TrackPause doc
        ORDoc.RemovePlaceholder doc, tag
        TrackResume
    End If
    On Error GoTo 0
    Err.Raise num, "OpenRef", desc
End Sub

' ---------- Update Citations and Bibliography ----------
Private Sub CmdUpdate(ByVal doc As Document)
    Dim n As Long, withData As Long, noData As Long, nItems As Long, nBib As Long, nHead As Long, nPending As Long, r As String
    ORDoc.ImportLegacy doc
    UndoStart "OpenRef: Update Citations"
    TrackPause doc
    n = ORDoc.RenderAll(doc)
    TrackResume
    UndoEnd
    ' 成功時はダイアログを出さない（Paperpile と同じ）。引用の CC はあるのに payload が 1 つも無い = 別文書から貼り付けた引用だけ
    If n = 0 Then
        ORDoc.Summary doc, withData, noData, nItems, nBib, nHead, nPending
        If noData > 0 Then
            r = Notice("error", "", "引用はありますが OpenRef の引用データがありません（別の文書から貼り付けた引用は Add/Edit で付け直してください）")
        Else
            r = Notice("error", "", NoCitesMsg(), NoCitesDetail())
        End If
    End If
End Sub

' ---------- Settings and Tools ----------
Private Sub CmdSettings(ByVal doc As Document, ByVal tabName As String)
    Dim style As String, doi As Boolean, pinned As Boolean, fname As String, fsize As Double, body As String, r() As String
    Dim withData As Long, noData As Long, nItems As Long, nBib As Long, nHead As Long, nPending As Long, rl As String
    ORDoc.ImportLegacy doc
    ORDoc.EffectivePrefs doc, style, doi, pinned
    ORDoc.FontConf doc, fname, fsize
    ORDoc.Summary doc, withData, noData, nItems, nBib, nHead, nPending
    body = "{""kind"":""settings"",""tab"":" & ORIpc.JStr(tabName) & ",""style"":" & ORIpc.JStr(style) & ",""doiAlways"":" & ORIpc.JBool(doi) & _
        ",""fontName"":" & ORIpc.JStr(fname) & ",""fontSize"":" & ORDoc.NumStr(fsize) & ",""cites"":" & withData & ",""pinned"":" & ORIpc.JBool(pinned) & "}"
    r = RunDialog(body)
    If r(1) <> "apply" Then Exit Sub
    rl = Join(r, vbTab)
    UndoStart "OpenRef: Settings"
    TrackPause doc
    ORDoc.ApplySettings doc, ORIpc.Unesc(ORIpc.Field(rl, 2)), ORIpc.Field(rl, 3) = "1", ORIpc.Unesc(ORIpc.Field(rl, 4)), Val(ORIpc.Field(rl, 5))
    ORDoc.RenderAll doc
    TrackResume
    UndoEnd
End Sub

Private Sub CmdMoveBib(ByVal doc As Document)
    Dim withData As Long, noData As Long, nItems As Long, nBib As Long, nHead As Long, nPending As Long, kind As String, at As ContentControl, r As String
    ORDoc.ImportLegacy doc
    ORDoc.Summary doc, withData, noData, nItems, nBib, nHead, nPending
    If withData = 0 Then
        r = Notice("error", "", NoCitesMsg(), NoCitesDetail())
        Exit Sub
    End If
    ORDoc.Context doc, kind, at
    If kind <> "body" Then ORIpc.Fail "本文の段落にカーソルを置いてください。", "引用・文献リスト・References 見出し・仮置きの中には移動できません。"
    UndoStart "OpenRef: Move Bibliography"
    TrackPause doc
    ORDoc.MoveBibliographyHere doc
    TrackResume
    UndoEnd
End Sub

Private Function RemoveMsg() As String
    RemoveMsg = "引用と文献リストを通常のテキストに変換し、文書内の引用データ（書誌のスナップショット）も削除します。" & _
        "以後 OpenRef で更新・編集できません。投稿用の最終版で使い、編集用の原本は別名で保存しておくことをおすすめします。"
End Function

Private Sub CmdRemoveCodes(ByVal doc As Document)
    Dim withData As Long, noData As Long, nItems As Long, nBib As Long, nHead As Long, nPending As Long, r As String
    Dim nCites As Long, nB As Long, nP As Long, nVars As Long, hasVars As Boolean, v As Variable
    ORDoc.ImportLegacy doc
    ORDoc.Summary doc, withData, noData, nItems, nBib, nHead, nPending
    For Each v In doc.Variables
        If Left$(v.Name, Len(ORDoc.OR_PREFIX)) = ORDoc.OR_PREFIX Then
            hasVars = True
            Exit For
        End If
    Next
    If withData = 0 And noData = 0 And nBib = 0 And nHead = 0 And nPending = 0 And Not hasVars Then
        r = Notice("error", "", NoCitesMsg(), NoCitesDetail())
        Exit Sub
    End If
    r = Notice("confirm", "引用コードの削除", RemoveMsg(), "", "削除", True)
    If r <> "ok" Then Exit Sub
    UndoStart "OpenRef: Remove Citation Codes"
    TrackPause doc
    ORDoc.RemoveCodes doc, nCites, nB, nP, nVars
    TrackResume
    UndoEnd
    ' 旧版（Office JS アドイン）の document settings は VBA から消せない（webextension パート）
    If Len(ORDoc.LegacyXml(doc)) > 0 Then
        r = Notice("info", "旧版のデータが残っています", "この文書には旧版（Office アドイン版）の引用データの控えが残っています。", _
            "完全に消すには、全文を選択して新しい文書にコピー＆ペーストしてください。")
    End If
End Sub

' ---------- Show Diagnostics ----------
Private Sub CmdDiagnostics(ByVal doc As Document)
    Dim t As String, st As String, conn As String, style As String, doi As Boolean, pinned As Boolean, fname As String, fsize As Double
    Dim withData As Long, noData As Long, nItems As Long, nBib As Long, nHead As Long, nPending As Long, legacy As String, r As String
    t = "OpenRef for Word（Startup テンプレート）" & OR_VERSION & vbLf
    t = t & "Word: " & Application.Version & " (" & Application.Build & ") / " & System.OperatingSystem & vbLf
    On Error Resume Next
    st = ORIpc.GetRaw("/api/status", 5)
    If Err.Number <> 0 Then
        conn = "connectd: 接続できません（" & Err.Description & IIf(Len(ORIpc.gErrDetail) > 0, " · " & ORIpc.gErrDetail, "") & "）"
        Err.Clear
    End If
    On Error GoTo 0
    If Len(conn) = 0 Then conn = "connectd: " & Trim$(Replace(st, vbLf, " "))
    t = t & conn & vbLf
    ' connectd に繋がらなくても文書の情報は出す（取り込み・app の既定は諦める）
    On Error Resume Next
    ORDoc.ImportLegacy doc
    Err.Clear
    ORDoc.EffectivePrefs doc, style, doi, pinned
    If Err.Number <> 0 Then
        Err.Clear
        style = ORDoc.CurrentStyle(doc)
        doi = ORDoc.CurrentDoi(doc)
        pinned = ORDoc.IsPinned(doc)
    End If
    On Error GoTo 0
    ORDoc.FontConf doc, fname, fsize
    ORDoc.Summary doc, withData, noData, nItems, nBib, nHead, nPending
    If Len(ORDoc.LegacyXml(doc)) = 0 Then
        legacy = "なし"
    ElseIf noData = 0 Then
        legacy = "あり（引用データは取り込み済み）"
    Else
        legacy = "あり（取り込めていない引用 " & noData & "）"
    End If
    t = t & "スタイル: " & style & "（" & IIf(pinned, "文書に保存済み", "未保存 = 次の挿入で固定") & "）" & vbLf
    t = t & "DOI / URL を常に含める: " & IIf(doi, "オン", "オフ") & " · フォント: " & fname & " " & ORDoc.NumStr(fsize) & "pt" & vbLf
    t = t & "引用: " & withData & " 箇所" & IIf(noData > 0, "（データなし " & noData & "）", "") & " · 文献: " & nItems & " 本" & vbLf
    t = t & "文献リスト: " & IIf(nBib > 0, "あり", "なし") & " · 見出し: " & IIf(nHead > 0, "あり", "なし") & " · 仮置き: " & nPending & " · 旧版データ: " & legacy
    On Error Resume Next
    r = Notice("info", "OpenRef 診断情報", t, "", "", False, True)
    If Err.Number <> 0 Then
        Err.Clear
        Alert t
    End If
End Sub

' ---------- E2E テスト用（word/dotm/e2e から Application.Run で呼ぶ） ----------
' 引数ありの Sub = マクロ一覧（vbaData・ツール > マクロ）に出ない（利用者が誤って実行しない）
Public Sub OR_TestBegin(Optional ByVal unused As Long = 0)
    gTestMode = True
    Set gTestQueue = New Collection
    gTestLog = ""
    gBusy = False
End Sub

Public Sub OR_TestQueue(ByVal rline As String)
    If gTestQueue Is Nothing Then Set gTestQueue = New Collection
    gTestQueue.Add rline
End Sub

Public Function OR_TestLog() As String
    OR_TestLog = gTestLog
    gTestLog = ""
End Function

Public Sub OR_TestEnd(Optional ByVal unused As Long = 0)
    gTestMode = False
    Set gTestQueue = Nothing
End Sub
