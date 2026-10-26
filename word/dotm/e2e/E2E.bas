Attribute VB_Name = "E2E"
Option Explicit

' OpenRef.dotm の E2E（実機の Word で 7 コマンドを流す）。build-word-dotm.py --e2e で同じプロジェクトに入り（OPENREF_E2E = 1）、
' ORRibbon.AutoExec から呼ばれる。word/dotm/e2e/run.sh が Startup に置いて Word を起動する。
' ダイアログはテストモード（ORCmd.OR_TestBegin）で R 行を先に積んでおき、出したはずのダイアログは OR_TestLog で読む。
' 結果は $TMPDIR/openref-e2e.txt（PASS / FAIL 行と最後に DONE）。設定 = ~/Documents/openref-e2e.conf（1 行目 = 接続先。空 = 本番 connectd）

Private gOut As String
Private gPass As Long
Private gFail As Long
Private gReal As Boolean

Private Const K1 As String = "PXFE4QBW"
Private Const K2 As String = "J4SS6G6R"
Private Const K3 As String = "4G45U6RX"

Private Function TmpDir() As String
    Dim p As String
    p = Environ("TMPDIR")
    If Right$(p, 1) <> "/" Then p = p & "/"
    TmpDir = p
End Function

Private Sub Note(ByVal s As String)
    gOut = gOut & s & vbLf
End Sub

Private Sub Flush()
    Dim fh As Integer, f As String
    f = TmpDir() & "openref-e2e.txt"
    fh = FreeFile
    Open f For Output As #fh
    Print #fh, gOut;
    Close #fh
End Sub

Private Sub Check(ByVal name As String, ByVal ok As Boolean, Optional ByVal detail As String = "")
    If ok Then
        gPass = gPass + 1
        Note "PASS " & name
    Else
        gFail = gFail + 1
        Note "FAIL " & name & " :: " & detail
    End If
    Flush
End Sub

Private Function Esc(ByVal s As String) As String
    Esc = ORIpc.JStr(s)
End Function

Private Function Payload(ParamArray keys() As Variant) As String
    Dim i As Long, s As String
    For i = LBound(keys) To UBound(keys)
        If Len(s) > 0 Then s = s & ","
        s = s & "{""key"":""" & keys(i) & """,""csl"":{""type"":""article-journal"",""title"":""Snapshot " & keys(i) & """}}"
    Next
    Payload = "{""items"":[" & s & "]}"
End Function

' 行形式の欄のエスケープ（payload は ASCII の JSON = % とバックスラッシュだけ）
Private Function LineEsc(ByVal s As String) As String
    LineEsc = Replace(Replace(s, "%", "%%"), ChrW(92), "%u005C")
End Function

Private Sub QueueOk(ByVal p As String)
    ORCmd.OR_TestQueue "R" & vbTab & "ok" & vbTab & LineEsc(p)
End Sub

Private Sub Queue(ByVal rline As String)
    ORCmd.OR_TestQueue rline
End Sub

Private Function TestLog() As String
    TestLog = ORCmd.OR_TestLog()
End Function

' ダイアログの要求（JSON）の中の日本語を探すための断片（JStr は非 ASCII を uXXXX に逃がす）
Private Function JFrag(ByVal s As String) As String
    Dim j As String
    j = ORIpc.JStr(s)
    JFrag = Mid$(j, 2, Len(j) - 2)
End Function

Private Function Cites(ByVal doc As Document) As Collection
    Dim cc As ContentControl, c As New Collection
    For Each cc In doc.ContentControls
        If Left$(cc.Tag, 13) = "refract-cite-" Then c.Add cc
    Next
    Set Cites = c
End Function

Private Function CountTag(ByVal doc As Document, ByVal tag As String) As Long
    Dim cc As ContentControl
    For Each cc In doc.ContentControls
        If cc.Tag = tag Then CountTag = CountTag + 1
    Next
End Function

Private Function FirstTag(ByVal doc As Document, ByVal tag As String) As ContentControl
    Dim cc As ContentControl
    For Each cc In doc.ContentControls
        If cc.Tag = tag Then
            Set FirstTag = cc
            Exit Function
        End If
    Next
End Function

Private Function VarOf(ByVal doc As Document, ByVal name As String) As String
    VarOf = ORDoc.VarGet(doc, name)
End Function

Private Function CiteTexts(ByVal doc As Document) As String
    Dim cc As ContentControl, s As String
    For Each cc In Cites(doc)
        s = s & cc.Range.Text & "|"
    Next
    CiteTexts = s
End Function

Private Sub CursorEnd(ByVal doc As Document)
    doc.Activate
    Selection.EndKey Unit:=wdStory
End Sub

Private Sub CursorInside(ByVal cc As ContentControl)
    cc.Range.Select
    Selection.Collapse wdCollapseStart
    Selection.MoveRight wdCharacter, 1
End Sub

Private Function NewDoc(ByVal text As String) As Document
    Dim d As Document
    Set d = Documents.Add
    d.Range.Text = text
    Set NewDoc = d
End Function

Private Sub CloseDoc(ByVal d As Document)
    On Error Resume Next
    d.Close SaveChanges:=wdDoNotSaveChanges
    Err.Clear
End Sub

' ---------- primitives ----------
Private Sub TPrimitives()
    Dim s As String, d As Document, big As String, cc As ContentControl, kind As String, at As ContentControl, v As Variable, n As Long
    On Error GoTo EH
    s = ORIpc.JStr("a""\" & ChrW(&H3042) & vbTab)
    Check "JStr", s = """a\""\\\u3042\u0009""", s
    s = ORIpc.Unesc("x%u3042%t%n%%%u005C%u00e9")
    Check "Unesc", s = "x" & ChrW(&H3042) & vbTab & vbLf & "%" & ChrW(92) & ChrW(&HE9), s
    Check "Placeholder", ORDoc.Placeholder() = "[" & ChrW(&H2026) & "]"
    Set d = NewDoc("Hello world.")
    big = String$(130000, "x") & "END"
    ORDoc.PayloadSet d, "refract-cite-chunk001", big
    Check "payload chunked roundtrip", ORDoc.PayloadGet(d, "refract-cite-chunk001") = big
    For Each v In d.Variables
        If Left$(v.Name, 21) = "refract-cite-chunk001" Then n = n + 1
    Next
    Check "payload uses 3 variables", n = 3, CStr(n)
    ORDoc.PayloadDel d, "refract-cite-chunk001"
    Check "payload deleted", d.Variables.Count = 0, CStr(d.Variables.Count)
    CursorEnd d
    Set cc = ORDoc.InsertPlaceholder(d, Nothing)
    Check "placeholder tag", Left$(cc.Tag, 16) = "refract-pending-", cc.Tag
    Check "placeholder text", cc.Range.Text = "[" & ChrW(&H2026) & "]", cc.Range.Text
    Check "cursor after placeholder", Selection.Start = cc.Range.End + 1, Selection.Start & " vs " & cc.Range.End
    kind = ORDoc.ContextKind(d)
    Check "context after placeholder = body", kind = "body", kind
    CursorInside cc
    kind = ORDoc.ContextKind(d)
    Check "context inside placeholder = pending-reusable", kind = "pending-reusable", kind
    ORDoc.RemovePlaceholder d, cc.Tag
    Check "placeholder removed", d.ContentControls.Count = 0 And d.Range.Text = "Hello world." & vbCr, d.Range.Text
    CloseDoc d
    Exit Sub
EH:
    Check "primitives (error)", False, Err.Number & " " & Err.Description
    CloseDoc d
End Sub

' ---------- 追加・更新・編集・削除・キャンセル ----------
Private Sub TAddEdit()
    Dim d As Document, c As Collection, bib As ContentControl, head As ContentControl, t As String, tag1 As String, lg As String, cc As ContentControl
    On Error GoTo EH
    Set d = NewDoc("Intro text. ")
    CursorEnd d
    QueueOk Payload(K1)
    ORCmd.OpenRefAddEdit
    lg = TestLog()
    Set c = Cites(d)
    Check "add: one citation", c.Count = 1, CStr(c.Count) & " log=" & lg
    If c.Count = 0 Then GoTo Done
    tag1 = c(1).Tag
    Check "add: payload stored", Len(VarOf(d, tag1)) > 0
    Check "add: style pinned", Len(VarOf(d, "refract-style")) > 0 And Len(VarOf(d, "refract-doi-always")) > 0, VarOf(d, "refract-style") & "/" & VarOf(d, "refract-doi-always")
    Check "add: dialog asked for cite", InStr(lg, """kind"":""cite""") > 0 And InStr(lg, """mode"":""add""") > 0, lg
    Set bib = FirstTag(d, "refract-bib")
    Set head = FirstTag(d, "refract-bib-heading")
    Check "add: bibliography created", Not bib Is Nothing
    Check "add: heading created", Not head Is Nothing
    If Not head Is Nothing Then Check "add: heading text/bold", Left$(head.Range.Text, 10) = "References" And head.Range.Font.Bold = True, head.Range.Text
    If Not bib Is Nothing Then Check "add: bibliography has 1 entry", bib.Range.Paragraphs.Count = 1, CStr(bib.Range.Paragraphs.Count) & " text=" & bib.Range.Text
    Check "add: font Century 10.5", c(1).Range.Font.Name = "Century" And c(1).Range.Font.Size = 10.5, c(1).Range.Font.Name & " " & c(1).Range.Font.Size
    Check "add: cursor after citation", Selection.Start = c(1).Range.End + 1, Selection.Start & " vs " & c(1).Range.End
    Check "add: document ends with an empty paragraph", d.Paragraphs(d.Paragraphs.Count).Range.Text = vbCr, d.Paragraphs(d.Paragraphs.Count).Range.Text
    ' 2 つ目（本文の末尾 = 文献リストより前に入るよう、最初の段落の末尾）
    c(1).Range.Select
    Selection.Collapse wdCollapseEnd
    Selection.MoveRight wdCharacter, 1
    Selection.TypeText " more. "
    QueueOk Payload(K2)
    ORCmd.OpenRefAddEdit
    lg = TestLog()
    Set c = Cites(d)
    Check "add2: two citations", c.Count = 2, CStr(c.Count) & " log=" & lg
    Set bib = FirstTag(d, "refract-bib")
    If Not bib Is Nothing Then Check "add2: bibliography has 2 entries", bib.Range.Paragraphs.Count = 2, CStr(bib.Range.Paragraphs.Count)
    Note "  texts after add2: " & CiteTexts(d)
    ' 先頭に 3 つ目（K3）→ 初出順で番号が振り直される
    d.Activate
    Selection.HomeKey Unit:=wdStory
    QueueOk Payload(K3)
    ORCmd.OpenRefAddEdit
    lg = TestLog()
    Set c = Cites(d)
    Check "add3: three citations", c.Count = 3, CStr(c.Count) & " log=" & lg
    Note "  texts after add3: " & CiteTexts(d)
    If Not gReal And c.Count = 3 Then Check "add3: renumbered by first appearance", CiteTexts(d) = "[1]|[2]|[3]|", CiteTexts(d)
    ' 編集: 2 番目（元の K1）を K1 + K2 に
    If c.Count = 3 Then
        t = c(2).Tag
        CursorInside c(2)
        ORCmd.OR_TestQueue "R" & vbTab & "ok" & vbTab & LineEsc(Payload(K1, K2))
        ORCmd.OpenRefAddEdit
        lg = TestLog()
        Set c = Cites(d)
        Check "edit: dialog mode edit with existing", InStr(lg, """mode"":""edit""") > 0 And InStr(lg, K1) > 0, lg
        Check "edit: same tag kept (no duplicates)", c(2).Tag = t, c(2).Tag & " vs " & t
        Check "edit: payload has both keys", InStr(VarOf(d, t), K2) > 0, VarOf(d, t)
        Note "  texts after edit: " & CiteTexts(d)
        If Not gReal Then Check "edit: cluster text", c(2).Range.Text = "[2,3]", c(2).Range.Text
    End If
    ' 複製（コピー＆ペースト）を編集 → 編集した方だけ新しい tag
    Set c = Cites(d)
    If c.Count = 3 Then
        t = c(3).Tag
        Set cc = c(3)
        d.Range(cc.Range.End + 1, cc.Range.End + 1).InsertAfter " "
        d.Range(cc.Range.End + 2, cc.Range.End + 2).FormattedText = d.Range(cc.Range.Start - 1, cc.Range.End + 1).FormattedText
        Check "dup: tag duplicated by copy", CountTag(d, t) = 2, CStr(CountTag(d, t))
        If CountTag(d, t) = 2 Then
            CursorInside c(3)
            QueueOk Payload(K3)
            ORCmd.OpenRefAddEdit
            lg = TestLog()
            Check "dup: edited copy forked to a new tag", CountTag(d, t) = 1, CStr(CountTag(d, t)) & " log=" & lg
            Check "dup: old payload kept for the other copy", Len(VarOf(d, t)) > 0
        End If
    End If
    ' 削除（編集で 0 件 = delete）
    Set c = Cites(d)
    t = c(1).Tag
    CursorInside c(1)
    Queue "R" & vbTab & "delete"
    ORCmd.OpenRefAddEdit
    lg = TestLog()
    Check "delete: citation removed", CountTag(d, t) = 0, CStr(CountTag(d, t)) & " log=" & lg
    Check "delete: payload removed", Len(VarOf(d, t)) = 0
    ' キャンセル: 仮置きが残らない
    t = d.Range.Text
    CursorEnd d
    Queue "R" & vbTab & "cancel"
    ORCmd.OpenRefAddEdit
    Check "cancel: no placeholder left", CountPending(d) = 0, CStr(CountPending(d))
    Check "cancel: text unchanged", d.Range.Text = t, d.Range.Text
Done:
    CloseDoc d
    Exit Sub
EH:
    Check "add/edit (error)", False, Err.Number & " " & Err.Description & " log=" & TestLog()
    CloseDoc d
End Sub

Private Function CountPending(ByVal doc As Document) As Long
    Dim cc As ContentControl
    For Each cc In doc.ContentControls
        If Left$(cc.Tag, 16) = "refract-pending-" Then CountPending = CountPending + 1
    Next
End Function

' ---------- 設定・移動・Update・入れ子・削除 ----------
Private Sub TSettingsMoveRemove()
    Dim d As Document, c As Collection, lg As String, bib As ContentControl, head As ContentControl, v As Variable, n As Long, inner As ContentControl
    On Error GoTo EH
    Set d = NewDoc("First paragraph. ")
    d.Range.InsertParagraphAfter
    d.Paragraphs(2).Range.InsertBefore "Second paragraph. "
    d.Activate
    d.Paragraphs(1).Range.Select
    Selection.Collapse wdCollapseEnd
    Selection.MoveLeft wdCharacter, 1
    QueueOk Payload(K1)
    ORCmd.OpenRefAddEdit
    d.Paragraphs(2).Range.Select
    Selection.Collapse wdCollapseEnd
    Selection.MoveLeft wdCharacter, 1
    QueueOk Payload(K2)
    ORCmd.OpenRefAddEdit
    lg = TestLog()
    Set c = Cites(d)
    Check "setup: two citations", c.Count = 2, CStr(c.Count) & " log=" & lg
    ' 設定の適用
    Queue "R" & vbTab & "apply" & vbTab & "apa" & vbTab & "1" & vbTab & "Times New Roman" & vbTab & "12"
    ORCmd.OpenRefDocumentSettings
    lg = TestLog()
    Check "settings: dialog asked", InStr(lg, """kind"":""settings""") > 0 And InStr(lg, """tab"":""doc""") > 0, lg
    Check "settings: style saved", VarOf(d, "refract-style") = "apa", VarOf(d, "refract-style")
    Check "settings: doi saved", VarOf(d, "refract-doi-always") = "true", VarOf(d, "refract-doi-always")
    Check "settings: font saved", VarOf(d, "refract-font-name") = "Times New Roman" And VarOf(d, "refract-font-size") = "12", VarOf(d, "refract-font-name") & "/" & VarOf(d, "refract-font-size")
    Set c = Cites(d)
    Check "settings: font applied", c(1).Range.Font.Name = "Times New Roman" And c(1).Range.Font.Size = 12, c(1).Range.Font.Name & " " & c(1).Range.Font.Size
    Note "  texts after apa: " & CiteTexts(d)
    ' Update（引用ありでダイアログを出さない）
    ORCmd.OpenRefUpdate
    lg = TestLog()
    Check "update: no dialog on success", Len(lg) = 0, lg
    ' 文献リストを 1 段落目の後ろへ
    d.Paragraphs(1).Range.Select
    Selection.Collapse wdCollapseStart
    ORCmd.OpenRefMoveBibliography
    lg = TestLog()
    Check "move: one bibliography and one heading", CountTag(d, "refract-bib") = 1 And CountTag(d, "refract-bib-heading") = 1, CountTag(d, "refract-bib") & "/" & CountTag(d, "refract-bib-heading") & " log=" & lg
    Set head = FirstTag(d, "refract-bib-heading")
    If Not head Is Nothing Then Check "move: heading right after paragraph 1", head.Range.Start >= d.Paragraphs(1).Range.End And head.Range.Start <= d.Paragraphs(2).Range.End, head.Range.Start & " p1end=" & d.Paragraphs(1).Range.End & " p2end=" & d.Paragraphs(2).Range.End
    ' 入れ子: 引用の中に引用を作って Update → エラー（文書は無傷）
    Set c = Cites(d)
    Set inner = d.ContentControls.Add(wdContentControlRichText, d.Range(c(1).Range.Start + 1, c(1).Range.Start + 1))
    inner.Tag = "refract-cite-nested01"
    inner.Range.Text = "[x]"
    ORCmd.OpenRefUpdate
    lg = TestLog()
    Check "nested: error notice", InStr(lg, """notice"":""error""") > 0, lg
    inner.Delete True
    ' Remove Citation Codes（確認 = ok）
    Queue "R" & vbTab & "ok"
    ORCmd.OpenRefRemoveCitationCodes
    lg = TestLog()
    Check "remove: confirm asked", InStr(lg, """notice"":""confirm""") > 0, lg
    Check "remove: no OpenRef content controls", CountOpenRef(d) = 0, CStr(CountOpenRef(d))
    For Each v In d.Variables
        If Left$(v.Name, 8) = "refract-" Then n = n + 1
    Next
    Check "remove: no refract variables", n = 0, CStr(n)
    Check "remove: text kept", InStr(d.Range.Text, "First paragraph.") > 0 And InStr(d.Range.Text, "References") > 0, d.Range.Text
    CloseDoc d
    Exit Sub
EH:
    Check "settings/move/remove (error)", False, Err.Number & " " & Err.Description & " log=" & TestLog()
    CloseDoc d
End Sub

Private Function CountOpenRef(ByVal doc As Document) As Long
    Dim cc As ContentControl
    For Each cc In doc.ContentControls
        If Left$(cc.Tag, 8) = "refract-" Then CountOpenRef = CountOpenRef + 1
    Next
End Function

' ---------- 旧文書の文献リスト（見出し CC が無い・素の "References" 段落がある） ----------
Private Sub TOldBibliography()
    Dim d As Document, bib As ContentControl, head As ContentControl, lg As String
    On Error GoTo EH
    ' 見出しの無い文献リスト CC だけがある → 文献リストの直前に見出しを作る（文献リストの中に入れない）
    Set d = NewDoc("Body text. ")
    d.Range.InsertParagraphAfter
    d.Range.InsertParagraphAfter
    d.Paragraphs(2).Range.InsertBefore "old entry"
    Set bib = d.ContentControls.Add(wdContentControlRichText, d.Paragraphs(2).Range)
    bib.Tag = "refract-bib"
    d.Activate
    d.Paragraphs(1).Range.Select
    Selection.Collapse wdCollapseEnd
    Selection.MoveLeft wdCharacter, 1
    QueueOk Payload(K1)
    ORCmd.OpenRefAddEdit
    lg = TestLog()
    Set head = FirstTag(d, "refract-bib-heading")
    Check "old bib: heading created", Not head Is Nothing, lg
    If Not head Is Nothing Then
        Check "old bib: heading is not inside another CC", head.Range.ParentContentControl Is Nothing Or head.ParentContentControl Is Nothing
        Check "old bib: heading before the bibliography", head.Range.Start < FirstTag(d, "refract-bib").Range.Start, head.Range.Start & " vs " & FirstTag(d, "refract-bib").Range.Start
        Check "old bib: bibliography not nested in heading", FirstTag(d, "refract-bib").ParentContentControl Is Nothing
    End If
    Check "old bib: still one bibliography", CountTag(d, "refract-bib") = 1, CStr(CountTag(d, "refract-bib"))
    CloseDoc d
    ' 素の "References" 段落 + 文献リスト CC → その段落を見出しとして取り込む（二重にしない）
    Set d = NewDoc("Body text. ")
    d.Range.InsertParagraphAfter
    d.Paragraphs(2).Range.InsertBefore "References"
    d.Range.InsertParagraphAfter
    d.Range.InsertParagraphAfter
    d.Paragraphs(3).Range.InsertBefore "old entry"
    Set bib = d.ContentControls.Add(wdContentControlRichText, d.Paragraphs(3).Range)
    bib.Tag = "refract-bib"
    d.Activate
    d.Paragraphs(1).Range.Select
    Selection.Collapse wdCollapseEnd
    Selection.MoveLeft wdCharacter, 1
    QueueOk Payload(K2)
    ORCmd.OpenRefAddEdit
    lg = TestLog()
    Check "legacy References paragraph adopted as heading", CountTag(d, "refract-bib-heading") = 1 And CountReferences(d) = 1, CountTag(d, "refract-bib-heading") & "/" & CountReferences(d) & " log=" & lg
    CloseDoc d
    Exit Sub
EH:
    Check "old bibliography (error)", False, Err.Number & " " & Err.Description & " log=" & TestLog()
    CloseDoc d
End Sub

Private Function CountReferences(ByVal doc As Document) As Long
    Dim p As Paragraph
    For Each p In doc.Paragraphs
        If Left$(p.Range.Text, 10) = "References" Then CountReferences = CountReferences + 1
    Next
End Function

' ---------- 本物のダイアログ経路（conf の 2 行目が realdialog のときだけ。結果はシェルが connectd へ POST する） ----------
Private Sub TRealDialog()
    Dim d As Document, c As Collection
    On Error GoTo EH
    ORCmd.OR_TestEnd
    Set d = NewDoc("Real dialog test. ")
    CursorEnd d
    Note "  realdialog: waiting for OpenRef Dialog.app"
    Flush
    ORCmd.OpenRefAddEdit
    Set c = Cites(d)
    Check "real dialog: citation inserted through OpenRef Dialog.app", c.Count = 1, CStr(c.Count)
    If c.Count = 1 Then Note "  real dialog text: " & c(1).Range.Text
    ORCmd.OR_TestBegin
    CloseDoc d
    Exit Sub
EH:
    Check "real dialog (error)", False, Err.Number & " " & Err.Description
    ORCmd.OR_TestBegin
    CloseDoc d
End Sub

' ---------- 変更履歴の記録中（OpenRef の書き換えは記録しない・書式の位置がずれない・設定は元に戻す） ----------
Private Sub TTrackChanges()
    Dim d As Document, c As Collection, lg As String, bib As ContentControl
    On Error GoTo EH
    Set d = NewDoc("Tracked text. ")
    d.TrackRevisions = True
    CursorEnd d
    QueueOk Payload(K1)
    ORCmd.OpenRefAddEdit
    lg = TestLog()
    Set c = Cites(d)
    Check "track: citation added", c.Count = 1, CStr(c.Count) & " log=" & lg
    Check "track: TrackRevisions restored", d.TrackRevisions = True
    Check "track: OpenRef edits are not tracked", d.Revisions.Count = 0, CStr(d.Revisions.Count)
    Queue "R" & vbTab & "apply" & vbTab & "vancouver" & vbTab & "0" & vbTab & "Century" & vbTab & "10.5"
    ORCmd.OpenRefDocumentSettings
    Set c = Cites(d)
    If c.Count = 1 And Not gReal Then Check "track: superscript run on the number", d.Range(c(1).Range.Start + 1, c(1).Range.Start + 2).Font.Superscript = True, c(1).Range.Text
    Check "track: still no revisions after re-render", d.Revisions.Count = 0, CStr(d.Revisions.Count)
    Check "track: TrackRevisions restored after settings", d.TrackRevisions = True
    ' 編集のキャンセルは文書に触れない（記録の切替もしない）
    Set c = Cites(d)
    If c.Count = 1 Then
        d.Saved = True
        CursorInside c(1)
        Queue "R" & vbTab & "cancel"
        ORCmd.OpenRefAddEdit
        lg = TestLog()
        Check "track: cancelled edit leaves the document unchanged", d.Saved = True And d.Revisions.Count = 0 And InStr(lg, """mode"":""edit""") > 0, "saved=" & d.Saved & " rev=" & d.Revisions.Count & " log=" & lg
    End If
    ' 引用の直後に打った文字（記録付き）は引用の中の記録ではない → Update は普通に書き換える（見送りの通知も出ない）
    Set c = Cites(d)
    If c.Count = 1 Then
        ORDoc.SelectAfter d, c(1)
        Selection.TypeText "x"
        Check "track: typing after the citation is tracked", d.Revisions.Count = 1, CStr(d.Revisions.Count)
        d.TrackRevisions = False
        c(1).Range.Text = "zz"
        d.TrackRevisions = True
        ORCmd.OpenRefUpdate
        lg = TestLog()
        Check "track: update ignores a revision that only touches the citation", InStr(lg, "notice") = 0 And InStr(lg, "ALERT") = 0 And d.Revisions.Count = 1 And c(1).Range.Text <> "zz", "rev=" & d.Revisions.Count & " text=" & c(1).Range.Text & " log=" & lg
        d.Revisions.AcceptAll
    End If
    ' 共著者の記録付きの削除が引用の中に残っている → Update はその引用を書き換えず（記録は残る）、info で知らせる
    Set c = Cites(d)
    If c.Count = 1 Then
        d.Range(c(1).Range.Start, c(1).Range.Start + 1).Delete
        Check "track: tracked deletion recorded inside the citation", d.Revisions.Count = 1, CStr(d.Revisions.Count)
        ORCmd.OpenRefUpdate
        lg = TestLog()
        Check "track: update keeps the revision and notifies", InStr(lg, """notice"":""info""") > 0 And InStr(lg, """notice"":""error""") = 0 And d.Revisions.Count = 1, "rev=" & d.Revisions.Count & " log=" & lg
        Check "track: TrackRevisions restored after the skip", d.TrackRevisions = True
        ' 記録のある引用そのものの編集はダイアログの前に止まる（記録は残る）
        CursorInside c(1)
        ORCmd.OpenRefAddEdit
        lg = TestLog()
        Check "track: editing the citation that holds a revision is refused before the dialog", InStr(lg, JFrag("編集できません")) > 0 And InStr(lg, """mode"":""edit""") = 0 And d.Revisions.Count = 1, "rev=" & d.Revisions.Count & " log=" & lg
        ' 記録が残っていても Add/Edit はできる（新しい引用は整形され、記録のある引用はそのまま・通知は出る）
        d.Paragraphs(1).Range.Select
        Selection.Collapse wdCollapseEnd
        Selection.MoveLeft wdCharacter, 1
        QueueOk Payload(K2)
        ORCmd.OpenRefAddEdit
        lg = TestLog()
        Set c = Cites(d)
        Check "track: add works while another citation holds a revision", c.Count = 2 And d.Revisions.Count = 1 And InStr(lg, """notice"":""info""") > 0, CStr(c.Count) & " rev=" & d.Revisions.Count & " log=" & lg
        If c.Count = 2 Then Check "track: the new citation is rendered", InStr(c(2).Range.Text, "2") > 0, c(2).Range.Text
        d.Revisions.RejectAll
        ORCmd.OpenRefUpdate
        lg = TestLog()
        Check "track: update works again once the revision is rejected", InStr(lg, "notice") = 0 And InStr(lg, "ALERT") = 0 And d.Revisions.Count = 0, "rev=" & d.Revisions.Count & " log=" & lg
    End If
    ' 改訂で最も多い場面: 記録付きで打った文の中に新しい引用を入れる → 引用は整形され、仮置きのまま残らない
    Set c = Cites(d)
    If c.Count = 2 Then
        d.Paragraphs(1).Range.Select
        Selection.Collapse wdCollapseEnd
        Selection.MoveLeft wdCharacter, 1
        Selection.TypeText " New tracked sentence here."
        Check "track: new sentence is tracked", d.Revisions.Count >= 1, CStr(d.Revisions.Count)
        Selection.MoveLeft wdCharacter, 6
        QueueOk Payload(K3)
        ORCmd.OpenRefAddEdit
        lg = TestLog()
        Set c = Cites(d)
        Check "track: citation inside a tracked insertion is rendered", c.Count = 3 And InStr(lg, "notice") = 0 And InStr(lg, "ALERT") = 0, CStr(c.Count) & " log=" & lg
        If c.Count = 3 Then Check "track: citation inside a tracked insertion is not left as a placeholder", InStr(c(3).Range.Text, "3") > 0 And c(3).Range.Text <> ORDoc.Placeholder(), c(3).Range.Text
        d.Revisions.AcceptAll
    End If
    ' 文献リストの中の記録: Move bibliography here は文書に触れずに止まり、Update は文献リストを書き換えずに知らせる
    Set bib = FirstTag(d, "refract-bib")
    If Not bib Is Nothing Then
        d.Range(bib.Range.Start, bib.Range.Start + 1).Delete
        Check "track: tracked deletion recorded inside the bibliography", d.Revisions.Count = 1, CStr(d.Revisions.Count)
        d.Paragraphs(1).Range.Select
        Selection.Collapse wdCollapseStart
        ORCmd.OpenRefMoveBibliography
        lg = TestLog()
        Check "track: move bibliography refuses while the bibliography holds a revision", InStr(lg, JFrag("移動できません")) > 0 And CountTag(d, "refract-bib") = 1 And d.Revisions.Count = 1, "bib=" & CountTag(d, "refract-bib") & " rev=" & d.Revisions.Count & " log=" & lg
        ORCmd.OpenRefUpdate
        lg = TestLog()
        Check "track: update keeps the bibliography revision and notifies", InStr(lg, """notice"":""info""") > 0 And d.Revisions.Count = 1, "rev=" & d.Revisions.Count & " log=" & lg
        d.Revisions.RejectAll
    End If
    ' 保護された文書（編集の制限 = 変更履歴のみ）= ダイアログを出す前に止まる・仮置きも作らない
    d.Protect wdAllowOnlyRevisions
    ORCmd.OpenRefUpdate
    lg = TestLog()
    Check "track: protected document refuses update", InStr(lg, JFrag("保護されている")) > 0, lg
    ORCmd.OpenRefAddEdit
    lg = TestLog()
    Check "track: protected document refuses add before the dialog", InStr(lg, JFrag("保護されている")) > 0 And InStr(lg, """kind"":""cite""") = 0 And CountPending(d) = 0, "pending=" & CountPending(d) & " log=" & lg
    Set c = Cites(d)
    If c.Count > 0 Then
        CursorInside c(1)
        ORCmd.OpenRefAddEdit
        lg = TestLog()
        Check "track: protected document refuses edit before the dialog", InStr(lg, JFrag("保護されている")) > 0 And InStr(lg, """mode"":""edit""") = 0, lg
    End If
    ORCmd.OpenRefDocumentSettings
    lg = TestLog()
    Check "track: protected document refuses settings before the dialog", InStr(lg, JFrag("保護されている")) > 0 And InStr(lg, """kind"":""settings""") = 0, lg
    Check "track: TrackRevisions still on under protection", d.TrackRevisions = True
    d.Unprotect
    CloseDoc d
    Exit Sub
EH:
    Check "track changes (error)", False, Err.Number & " " & Err.Description & " log=" & TestLog()
    On Error Resume Next
    d.Unprotect
    CloseDoc d
End Sub

' ---------- 引用の無い文書・診断・旧版文書 ----------
Private Sub TMisc()
    Dim d As Document, lg As String, c As Collection
    On Error GoTo EH
    Set d = NewDoc("No citations here.")
    ORCmd.OpenRefUpdate
    lg = TestLog()
    Check "no cites: error notice", InStr(lg, """notice"":""error""") > 0, lg
    ORCmd.OpenRefShowDiagnostics
    lg = TestLog()
    Check "diagnostics: info notice with copy", InStr(lg, """notice"":""info""") > 0 And InStr(lg, """copy"":true") > 0, lg
    Note "  diagnostics: " & Left$(lg, 600)
    CloseDoc d
    ' 旧版（Office JS アドイン）の文書: payload は webextension の document settings にだけある
    Set d = Documents.Open(FileName:=Environ("HOME") & "/Documents/openref-e2e-legacy.docx", AddToRecentFiles:=False)
    ORCmd.OpenRefUpdate
    lg = TestLog()
    Check "legacy: payload imported", Len(VarOf(d, "refract-cite-abcd1234")) > 0, "log=" & lg
    Check "legacy: style imported", VarOf(d, "refract-style") = "vancouver", VarOf(d, "refract-style")
    Set c = Cites(d)
    Check "legacy: rendered (bibliography created)", CountTag(d, "refract-bib") = 1, CStr(CountTag(d, "refract-bib")) & " log=" & lg
    Note "  legacy texts: " & CiteTexts(d)
    CloseDoc d
    Exit Sub
EH:
    Check "misc (error)", False, Err.Number & " " & Err.Description & " log=" & TestLog()
    CloseDoc d
End Sub

Public Sub RunAll()
    Dim conf As String, fh As Integer, baseUrl As String, mode As String
    On Error GoTo EH
    gOut = "openref e2e " & Format$(Now, "yyyy-mm-dd hh:nn:ss") & vbLf
    Flush
    conf = Environ("HOME") & "/Documents/openref-e2e.conf"
    If Len(Dir$(conf)) > 0 Then
        fh = FreeFile
        Open conf For Input As #fh
        If Not EOF(fh) Then Line Input #fh, baseUrl
        If Not EOF(fh) Then Line Input #fh, mode
        Close #fh
    End If
    baseUrl = Trim$(baseUrl)
    ' 模擬サーバ（http）のときだけ表示文字列まで照合する。本物の connectd（https・既定は本番）は整形結果がスタイル次第
    gReal = (Left$(baseUrl, 7) <> "http://")
    Note "base=" & IIf(Len(baseUrl) = 0, "(production connectd)", baseUrl) & IIf(gReal, " real", " mock")
    ' 各モジュールを 1 つずつ呼ぶ（コンパイルエラーで止まった場所が結果ファイルで分かるように）
    Note "smoke ipc: " & ORIpc.JBool(True)
    Flush
    Note "smoke strings: " & Len(ORStr(1))
    Flush
    Note "smoke doc: " & ORDoc.Placeholder()
    Flush
    Note "smoke cmd: " & ORCmd.OR_VERSION
    Flush
    ORIpc.OR_TestSetBase baseUrl
    ORCmd.OR_TestBegin
    If Trim$(mode) = "realdialog-only" Then
        TRealDialog
        ORCmd.OR_TestEnd
        Note "DONE pass=" & gPass & " fail=" & gFail
        Flush
        Exit Sub
    End If
    TPrimitives
    TAddEdit
    TSettingsMoveRemove
    TOldBibliography
    TTrackChanges
    TMisc
    If Trim$(mode) = "realdialog" Then TRealDialog
    ORCmd.OR_TestEnd
    Note "DONE pass=" & gPass & " fail=" & gFail
    Flush
    Exit Sub
EH:
    Note "ABORT " & Err.Number & " " & Err.Description
    Note "DONE pass=" & gPass & " fail=" & (gFail + 1)
    Flush
End Sub
