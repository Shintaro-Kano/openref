Attribute VB_Name = "ORDoc"
Option Explicit

' 文書の操作。
' 引用 = inline の content control（tag refract-cite-<8桁>）+ 同名の Document Variable に payload（connectd が作る JSON。不透明に扱う）。
' 文献リスト = CC tag refract-bib / 見出し = CC tag refract-bib-heading（どちらも段落ごと包む block CC）。
' Add/Edit の仮置き = CC tag refract-pending-<8桁>。識別子は旧版（Office JS アドイン）の文書と同じ（旧文書の引用をそのまま認識する）。
' cc.Delete(DeleteContents) の引数は True = 中身ごと消す・False = 中身を残して CC だけ外す。

Public Const OR_PREFIX As String = "refract-"
Public Const OR_CITE As String = "refract-cite-"
Public Const OR_PENDING As String = "refract-pending-"
Public Const OR_BIB As String = "refract-bib"
Public Const OR_HEAD As String = "refract-bib-heading"
Public Const OR_S_STYLE As String = "refract-style"
Public Const OR_S_DOI As String = "refract-doi-always"
Public Const OR_S_FONT_NAME As String = "refract-font-name"
Public Const OR_S_FONT_SIZE As String = "refract-font-size"
Public Const OR_DEFAULT_FONT As String = "Century"
Public Const OR_DEFAULT_SIZE As Double = 10.5
' Document Variable は 1 個 65,000 文字まで（66,000 で 5854）→ 超える payload は <tag>#2, #3 … に続ける
Private Const CHUNK As Long = 60000

' 旧版文書の取り込みを済ませた文書（このセッション中）
Private gLegacyDone As String
Private gSeeded As Boolean
' 直前のコマンドで書き換えを見送った引用の数と文献リスト（変更履歴の記録が中に残っていたもの）。ORCmd が最後に知らせる
Private gSkipCites As Long
Private gSkipBib As Boolean

Public Function Placeholder() As String
    Placeholder = "[…]"
End Function

Public Function NestedMsg() As String
    NestedMsg = "引用の中に別の引用が入っています（入れ子）。中の引用を削除し、Add/Edit で付け直してから Update してください。"
End Function

' 引用 tag の 8 桁（英小文字 + 数字）。文書の CC の tag・Document Variable と重ならないものを返す
' （VBA の Rnd は状態が 2^24 しか無いので衝突を確かめる。Randomize はセッションで 1 回 = 続けて呼んでも同じ種にならないように）
Public Function NewId(ByVal doc As Document) As String
    Dim i As Long, s As String, a As String, cc As ContentControl, used As Boolean
    a = "abcdefghijklmnopqrstuvwxyz0123456789"
    If Not gSeeded Then
        Randomize
        gSeeded = True
    End If
    Do
        s = ""
        For i = 1 To 8
            s = s & Mid$(a, Int(Rnd() * 36) + 1, 1)
        Next
        used = VarExists(doc, OR_CITE & s)
        If Not used Then
            For Each cc In doc.ContentControls
                If Right$(cc.Tag, 9) = "-" & s Then
                    used = True
                    Exit For
                End If
            Next
        End If
    Loop While used
    NewId = s
End Function

' ---------- Document Variables ----------
Public Function VarGet(ByVal doc As Document, ByVal name As String) As String
    Dim v As String
    On Error Resume Next
    v = doc.Variables(name).Value
    If Err.Number <> 0 Then v = ""
    Err.Clear
    VarGet = v
End Function

Public Function VarExists(ByVal doc As Document, ByVal name As String) As Boolean
    Dim v As String
    On Error Resume Next
    v = doc.Variables(name).Value
    VarExists = (Err.Number = 0)
    Err.Clear
End Function

Public Sub VarDel(ByVal doc As Document, ByVal name As String)
    On Error Resume Next
    doc.Variables(name).Delete
    Err.Clear
End Sub

Public Sub VarSet(ByVal doc As Document, ByVal name As String, ByVal value As String)
    VarDel doc, name
    doc.Variables.Add Name:=name, Value:=value
End Sub

Public Function PayloadGet(ByVal doc As Document, ByVal tag As String) As String
    Dim s As String, part As String, i As Long
    s = VarGet(doc, tag)
    If Len(s) = 0 Then Exit Function
    i = 2
    Do
        part = VarGet(doc, tag & "#" & i)
        If Len(part) = 0 Then Exit Do
        s = s & part
        i = i + 1
    Loop
    PayloadGet = s
End Function

Public Sub PayloadDel(ByVal doc As Document, ByVal tag As String)
    Dim i As Long
    VarDel doc, tag
    i = 2
    Do While VarExists(doc, tag & "#" & i)
        VarDel doc, tag & "#" & i
        i = i + 1
    Loop
End Sub

Public Sub PayloadSet(ByVal doc As Document, ByVal tag As String, ByVal payload As String)
    Dim i As Long, k As Long
    PayloadDel doc, tag
    doc.Variables.Add Name:=tag, Value:=Left$(payload, CHUNK)
    k = 2
    For i = CHUNK + 1 To Len(payload) Step CHUNK
        doc.Variables.Add Name:=tag & "#" & k, Value:=Mid$(payload, i, CHUNK)
        k = k + 1
    Next
End Sub

' ---------- 文書ごとの設定 ----------
Public Function CurrentStyle(ByVal doc As Document) As String
    CurrentStyle = VarGet(doc, OR_S_STYLE)
    If Len(CurrentStyle) = 0 Then CurrentStyle = "vancouver"
End Function

Public Function CurrentDoi(ByVal doc As Document) As Boolean
    CurrentDoi = (VarGet(doc, OR_S_DOI) = "true")
End Function

Public Function IsPinned(ByVal doc As Document) As Boolean
    Dim d As String
    d = VarGet(doc, OR_S_DOI)
    IsPinned = Len(VarGet(doc, OR_S_STYLE)) > 0 And (d = "true" Or d = "false")
End Function

Public Sub FontConf(ByVal doc As Document, ByRef fname As String, ByRef fsize As Double)
    Dim s As Double
    fname = Trim$(VarGet(doc, OR_S_FONT_NAME))
    If Len(fname) = 0 Then fname = OR_DEFAULT_FONT
    s = Val(VarGet(doc, OR_S_FONT_SIZE))
    If s > 0 Then fsize = s Else fsize = OR_DEFAULT_SIZE
End Sub

Public Function NumStr(ByVal x As Double) As String
    NumStr = Trim$(Str$(x))
End Function

' スタイル・DOI が未保存の文書で使う値: 引用が既にあれば従来既定（vancouver・付け足し無し）、
' 引用がまだ無い新規文書なら OpenRef app の既定。仮置きは引用に数えない
Public Sub EffectivePrefs(ByVal doc As Document, ByRef style As String, ByRef doi As Boolean, ByRef pinned As Boolean)
    Dim s As String, d As String, r() As String, appStyle As String, appDoi As Boolean, hadCites As Boolean, cc As ContentControl
    s = VarGet(doc, OR_S_STYLE)
    d = VarGet(doc, OR_S_DOI)
    If Len(s) > 0 And (d = "true" Or d = "false") Then
        style = s
        doi = (d = "true")
        pinned = True
        Exit Sub
    End If
    r = ORIpc.Post("/word/v2/prefs", "{}")
    If UBound(r) >= 1 Then
        appStyle = ORIpc.Unesc(ORIpc.Field(r(1), 1))
        appDoi = (ORIpc.Field(r(1), 2) = "1")
    End If
    For Each cc In doc.ContentControls
        If Left$(cc.Tag, Len(OR_CITE)) = OR_CITE Then
            hadCites = True
            Exit For
        End If
    Next
    If Len(s) > 0 Then
        style = s
    ElseIf hadCites Or Len(appStyle) = 0 Then
        style = "vancouver"
    Else
        style = appStyle
    End If
    If d = "true" Or d = "false" Then
        doi = (d = "true")
    ElseIf hadCites Then
        doi = False
    Else
        doi = appDoi
    End If
    pinned = False
End Sub

' ---------- 走査 ----------
Public Sub Scan(ByVal doc As Document, ByRef cites As Collection, ByRef pending As Collection, ByRef bibs As Collection, ByRef heads As Collection)
    Dim cc As ContentControl, t As String
    Set cites = New Collection
    Set pending = New Collection
    Set bibs = New Collection
    Set heads = New Collection
    For Each cc In doc.ContentControls
        t = cc.Tag
        If Left$(t, Len(OR_CITE)) = OR_CITE Then
            cites.Add cc
        ElseIf Left$(t, Len(OR_PENDING)) = OR_PENDING Then
            pending.Add cc
        ElseIf t = OR_BIB Then
            bibs.Add cc
        ElseIf t = OR_HEAD Then
            heads.Add cc
        End If
    Next
End Sub

Public Function IsOpenRefCC(ByVal cc As ContentControl) As Boolean
    If cc Is Nothing Then Exit Function
    IsOpenRefCC = (Left$(cc.Tag, Len(OR_PREFIX)) = OR_PREFIX)
End Function

' ---------- 変更履歴 ----------
' r の中に変更履歴の記録（挿入・削除・移動）があるか。端で接しているだけの記録（引用の直後に打った文字など）と、書式だけの記録
' （段落・文字書式 = 書き換えても失われない。ApplyFont が書式を戻す）は数えない。読めない記録があれば「ある」扱い（書き換えない側に倒す）
Public Function HasRevisions(ByVal r As Range) As Boolean
    Dim rv As Revision
    On Error GoTo Unsure
    For Each rv In r.Revisions
        ' WdRevisionType: 1 Insert・2 Delete・14 MovedFrom・15 MovedTo・20 / 21 ConflictInsert / Delete（数値 = Mac の型ライブラリに無い定数名を避ける）
        Select Case rv.Type
            Case 1, 2, 14, 15, 20, 21
                If rv.Range.Start < r.End And r.Start < rv.Range.End Then
                    HasRevisions = True
                    Exit Function
                End If
        End Select
    Next
    Exit Function
Unsure:
    HasRevisions = True
End Function

' 引数ありの Sub = マクロ一覧（ツール > マクロ）に出ない
Public Sub SkippedReset(Optional ByVal unused As Long = 0)
    gSkipCites = 0
    gSkipBib = False
End Sub

' 書き換えを見送ったものがあれば利用者向けの文言（無ければ空）
Public Function SkippedNote() As String
    Dim s As String
    If gSkipCites > 0 Then s = "引用 " & gSkipCites & " 箇所"
    If gSkipBib Then s = s & IIf(Len(s) > 0, "と", "") & "文献リスト"
    If Len(s) > 0 Then SkippedNote = "変更履歴が残っている" & s & "は書き換えていません（共著者の削除・挿入の記録を消さないため）。"
End Function

' 引用 CC（と extra）のうち OpenRef の CC の中にあるものがあれば、選択してエラー（外側を書き換えると中の引用が消えるので文書に触れる前に止める）
Public Sub CheckNested(ByVal cites As Collection, Optional ByVal extra As ContentControl = Nothing)
    Dim cc As ContentControl
    For Each cc In cites
        If IsOpenRefCC(cc.ParentContentControl) Then NestedFail cc
    Next
    If Not extra Is Nothing Then
        If IsOpenRefCC(extra.ParentContentControl) Then NestedFail extra
    End If
End Sub

Private Sub NestedFail(ByVal inner As ContentControl)
    Dim selected As Boolean
    On Error Resume Next
    inner.Range.Select
    selected = (Err.Number = 0)
    Err.Clear
    On Error GoTo 0
    If selected Then
        ORIpc.Fail NestedMsg() & "（入れ子の引用を選択しました）"
    Else
        ORIpc.Fail NestedMsg()
    End If
End Sub

' 引用の数（payload あり / なし）・文献の数（payload の "key" の異なり数）
Public Sub Summary(ByVal doc As Document, ByRef withData As Long, ByRef noData As Long, ByRef nItems As Long, ByRef nBib As Long, ByRef nHead As Long, ByRef nPending As Long)
    Dim cites As Collection, pending As Collection, bibs As Collection, heads As Collection, cc As ContentControl
    Dim p As String, keys As String, i As Long, j As Long, k As String
    Scan doc, cites, pending, bibs, heads
    keys = "|"
    For Each cc In cites
        p = PayloadGet(doc, cc.Tag)
        If Len(p) > 0 Then
            withData = withData + 1
            i = InStr(p, """key"":""")
            Do While i > 0
                j = InStr(i + 7, p, """")
                If j = 0 Then Exit Do
                k = Mid$(p, i + 7, j - i - 7)
                If InStr(keys, "|" & k & "|") = 0 Then
                    keys = keys & k & "|"
                    nItems = nItems + 1
                End If
                i = InStr(j, p, """key"":""")
            Loop
        Else
            noData = noData + 1
        End If
    Next
    nBib = bibs.Count
    nHead = heads.Count
    nPending = pending.Count
End Sub

' ---------- 旧版（Office JS アドイン）文書の取り込み ----------
' 引用 CC があるのに payload の変数が無い = 旧版の文書。WordOpenXML の webextension パート（Office の document settings）から
' connectd に refract-* を取り出してもらい、無い変数だけ書く。同じ文書はこのセッション中 1 回
Public Sub ImportLegacy(ByVal doc As Document)
    Dim cites As Collection, pending As Collection, bibs As Collection, heads As Collection, cc As ContentControl
    Dim missing As Boolean, x As String, blocks As String, a As Long, b As Long, blk As String, r() As String, i As Long
    Dim nm As String, v As String, key As String
    key = "|" & doc.FullName & "|"
    If InStr(gLegacyDone, key) > 0 Then Exit Sub
    Scan doc, cites, pending, bibs, heads
    For Each cc In cites
        If Not VarExists(doc, cc.Tag) Then
            missing = True
            Exit For
        End If
    Next
    If Not missing Then Exit Sub
    x = LegacyXml(doc)
    If Len(x) = 0 Then
        gLegacyDone = gLegacyDone & key
        Exit Sub
    End If
    ' 済みの印は取り込めてから付ける（connectd が落ちていて失敗したら、次のコマンドでもう一度試す）
    r = ORIpc.Post("/word/v2/legacy", "{""xml"":" & ORIpc.JStr(x) & "}")
    For i = 1 To UBound(r)
        If Left$(r(i), 2) = "V" & vbTab Then
            nm = ORIpc.Unesc(ORIpc.Field(r(i), 1))
            v = ORIpc.Unesc(ORIpc.Field(r(i), 2))
            If Left$(nm, Len(OR_PREFIX)) = OR_PREFIX And Len(v) > 0 And Not VarExists(doc, nm) Then
                If Left$(nm, Len(OR_CITE)) = OR_CITE Then
                    PayloadSet doc, nm, v
                Else
                    VarSet doc, nm, v
                End If
            End If
        End If
    Next
    gLegacyDone = gLegacyDone & key
End Sub

' 文書の webextension パートのうち refract- を含むもの（無ければ ""）
Public Function LegacyXml(ByVal doc As Document) As String
    Dim x As String, a As Long, b As Long, blk As String, out As String
    x = doc.WordOpenXML
    a = InStr(x, "<we:webextension ")
    Do While a > 0
        b = InStr(a, x, "</we:webextension>")
        If b = 0 Then Exit Do
        blk = Mid$(x, a, b - a + Len("</we:webextension>"))
        If InStr(blk, "refract-") > 0 Then out = out & blk
        a = InStr(b, x, "<we:webextension ")
    Loop
    LegacyXml = out
End Function

' ---------- 一括更新 ----------
' 文書内の全引用を初出順に番号付けし直し、文献リスト（無ければ見出しごと作る）を作り直す。戻り値 = 引用データのある引用の数（変更履歴の記録が残って書き換えを見送った分も含む。0 = 文書に触れない）。
' 仮置きには触れない。入れ子があれば文書に触れずにエラー
Public Function RenderAll(ByVal doc As Document) As Long
    Dim cites As Collection, pending As Collection, bibs As Collection, heads As Collection, cc As ContentControl
    Dim ccs() As ContentControl, pls() As String, n As Long, p As String, body As String, r() As String
    Dim texts() As String, runs() As String, k As Long, i As Long, bibText As String, bibRuns As String, gotBib As Boolean
    Dim fname As String, fsize As Double, head As ContentControl, bib As ContentControl
    Scan doc, cites, pending, bibs, heads
    CheckNested cites
    If cites.Count = 0 Then Exit Function
    ReDim ccs(1 To cites.Count)
    ReDim pls(0 To cites.Count - 1)
    For Each cc In cites
        p = PayloadGet(doc, cc.Tag)
        If Len(p) > 0 Then
            n = n + 1
            Set ccs(n) = cc
            pls(n - 1) = p
        End If
    Next
    If n = 0 Then Exit Function
    ReDim Preserve pls(0 To n - 1)
    body = "{""style"":" & ORIpc.JStr(CurrentStyle(doc)) & ",""doiAlways"":" & ORIpc.JBool(CurrentDoi(doc)) & ",""cites"":[" & Join(pls, ",") & "]}"
    r = ORIpc.Post("/word/v2/render", body, 120)
    ReDim texts(1 To n)
    ReDim runs(1 To n)
    For i = 1 To UBound(r)
        If Left$(r(i), 2) = "C" & vbTab Then
            k = k + 1
            If k <= n Then
                texts(k) = ORIpc.Unesc(ORIpc.Field(r(i), 1))
                runs(k) = ORIpc.Field(r(i), 2)
            End If
        ElseIf Left$(r(i), 2) = "B" & vbTab Then
            bibText = ORIpc.Unesc(ORIpc.Field(r(i), 1))
            bibRuns = ORIpc.Field(r(i), 2)
            gotBib = True
        End If
    Next
    If k <> n Or Not gotBib Then ORIpc.Fail "整形結果を受け取れませんでした（引用 " & n & " 件に対して " & k & " 件）。", "connectd /word/v2/render"
    FontConf doc, fname, fsize
    Application.ScreenUpdating = False
    For i = 1 To n
        ' 変更履歴の記録が中に残る引用は書き換えない（共著者の削除・挿入を黙って消さない。番号は承認 / 却下のあとの Update で揃う）
        If HasRevisions(ccs(i).Range) Then
            gSkipCites = gSkipCites + 1
        Else
            ApplyRich doc, ccs(i), texts(i), runs(i), fname, fsize, False
        End If
    Next
    ' 2 つ以上あるときは最後のもの
    If heads.Count > 0 Then Set head = heads(heads.Count)
    If bibs.Count > 0 Then Set bib = bibs(bibs.Count)
    ' 見出し（既存文書に素の "References" 段落があれば取り込む）
    If head Is Nothing Then
        If Not bib Is Nothing Then
            Set head = WrapLegacyReferences(doc)
            If head Is Nothing Then Set head = WrapParagraph(doc, InsertParagraphBefore(doc, bib.Range.Paragraphs(1), "References"))
        Else
            Set head = WrapParagraph(doc, AppendParagraph(doc, "References"))
        End If
        head.Tag = OR_HEAD
        head.Title = "References heading (OpenRef)"
    End If
    If bib Is Nothing Then
        Set bib = WrapParagraph(doc, AppendParagraph(doc, ""))
        bib.Tag = OR_BIB
        bib.Title = "References (OpenRef)"
    End If
    If Len(bibText) = 0 Then bibText = "(no items)"
    If HasRevisions(bib.Range) Then
        gSkipBib = True
    Else
        ApplyRich doc, bib, Replace(bibText, vbLf, vbCr), bibRuns, fname, fsize, False
    End If
    ApplyFont head.Range, fname, fsize, True
    Application.ScreenUpdating = True
    RenderAll = n
End Function

' CC の中身を text に置き換え、書式を既定（fname / fsize・黒）にしてから run の書式（i b u s d c）を当てる
Private Sub ApplyRich(ByVal doc As Document, ByVal cc As ContentControl, ByVal text As String, ByVal runs As String, ByVal fname As String, ByVal fsize As Double, ByVal bold As Boolean)
    Dim r As Range, parts() As String, f() As String, i As Long, s As Long, l As Long, rr As Range, startPos As Long
    cc.Range.Text = text
    Set r = cc.Range
    ApplyFont r, fname, fsize, bold
    If Len(runs) = 0 Then Exit Sub
    startPos = r.Start
    parts = Split(runs, ",")
    For i = 0 To UBound(parts)
        f = Split(parts(i), ":")
        If UBound(f) >= 2 Then
            s = CLng(f(0))
            l = CLng(f(1))
            If l > 0 Then
                Set rr = doc.Range(startPos + s, startPos + s + l)
                If InStr(f(2), "i") > 0 Then rr.Font.Italic = True
                If InStr(f(2), "b") > 0 Then rr.Font.Bold = True
                If InStr(f(2), "u") > 0 Then rr.Font.Underline = wdUnderlineSingle
                If InStr(f(2), "s") > 0 Then rr.Font.Superscript = True
                If InStr(f(2), "d") > 0 Then rr.Font.Subscript = True
                If InStr(f(2), "c") > 0 Then rr.Font.SmallCaps = True
            End If
        End If
    Next
End Sub

' 既定の書式（引用・文献リスト = 太字なし / 見出し = 太字）。insertHtml の代わりなので斜体・上付きなども毎回戻す
Private Sub ApplyFont(ByVal r As Range, ByVal fname As String, ByVal fsize As Double, ByVal bold As Boolean)
    With r.Font
        .Name = fname
        .Size = fsize
        .Color = wdColorBlack
        .Bold = bold
        .Italic = False
        .Underline = wdUnderlineNone
        .Superscript = False
        .Subscript = False
        .SmallCaps = False
    End With
End Sub

Private Function ParaText(ByVal p As Paragraph) As String
    Dim s As String
    s = p.Range.Text
    If Right$(s, 1) = vbCr Then s = Left$(s, Len(s) - 1)
    ParaText = s
End Function

' 文書末（空の最終段落の直前）に段落を足して返す（標準スタイル）。CC で包む段落を文書の最終段落にしない
' （最終段落の段落記号は CC に入れられない）= 末尾には空段落が 1 つ残る
Private Function AppendParagraph(ByVal doc As Document, ByVal text As String) As Paragraph
    Dim p As Paragraph, last As Paragraph
    Set last = doc.Paragraphs(doc.Paragraphs.Count)
    If Len(ParaText(last)) > 0 Or Not last.Range.ParentContentControl Is Nothing Then
        doc.Content.InsertParagraphAfter
        Set last = doc.Paragraphs(doc.Paragraphs.Count)
    End If
    last.Range.InsertParagraphBefore
    Set p = doc.Paragraphs(doc.Paragraphs.Count - 1)
    p.Style = doc.Styles(wdStyleNormal)
    If Len(text) > 0 Then p.Range.InsertBefore text
    Set AppendParagraph = doc.Paragraphs(doc.Paragraphs.Count - 1)
End Function

' before の段落の直前に段落を足して返す（標準スタイル）
Private Function InsertParagraphBefore(ByVal doc As Document, ByVal before As Paragraph, ByVal text As String) As Paragraph
    Dim r As Range, p As Paragraph
    Set r = before.Range
    r.InsertParagraphBefore
    Set p = r.Paragraphs(1)
    p.Style = doc.Styles(wdStyleNormal)
    If Len(text) > 0 Then p.Range.InsertBefore text
    Set InsertParagraphBefore = p
End Function

' 段落ごと包む block CC
Private Function WrapParagraph(ByVal doc As Document, ByVal p As Paragraph) As ContentControl
    Set WrapParagraph = doc.ContentControls.Add(wdContentControlRichText, p.Range)
End Function

' 旧文書の素の "References" 段落（最後のもの）を見出し CC にする。無ければ Nothing
Private Function WrapLegacyReferences(ByVal doc As Document) As ContentControl
    Dim p As Paragraph, legacy As Paragraph
    For Each p In doc.Paragraphs
        If LCase$(Trim$(ParaText(p))) = "references" Then
            If p.Range.ParentContentControl Is Nothing Then Set legacy = p
        End If
    Next
    If legacy Is Nothing Then Exit Function
    Set WrapLegacyReferences = WrapParagraph(doc, legacy)
End Function

' ---------- Add/Edit ----------
' カーソル位置の文脈: cite（引用の中）/ bib（文献リスト・見出し）/ pending-reusable（中身が […] だけの仮置き）/ pending（文字の打たれた仮置き）/ body
Public Sub Context(ByVal doc As Document, ByRef kind As String, ByRef cc As ContentControl)
    Dim t As String
    If Selection.StoryType <> wdMainTextStory Then ORIpc.Fail "本文にカーソルを置いてください。", "脚注・ヘッダー・テキストボックスの中には未対応です。"
    Set cc = Selection.Range.ParentContentControl
    kind = "body"
    If cc Is Nothing Then Exit Sub
    t = cc.Tag
    If Left$(t, Len(OR_CITE)) = OR_CITE Then
        kind = "cite"
    ElseIf t = OR_BIB Or t = OR_HEAD Then
        kind = "bib"
    ElseIf Left$(t, Len(OR_PENDING)) = OR_PENDING Then
        If cc.Range.Text = Placeholder() Then kind = "pending-reusable" Else kind = "pending"
    Else
        Set cc = Nothing
    End If
End Sub

' Context の kind だけ（Application.Run は ByRef を返さないので E2E はこちらを使う）
Public Function ContextKind(ByVal doc As Document) As String
    Dim kind As String, cc As ContentControl
    Context doc, kind, cc
    ContextKind = kind
End Function

Private Function PlaceholderAt(ByVal doc As Document, ByVal r As Range, ByVal tag As String) As ContentControl
    Dim cc As ContentControl
    Set cc = doc.ContentControls.Add(wdContentControlRichText, r)
    cc.Tag = tag
    cc.Title = "OpenRef citation (pending)"
    cc.Range.Text = Placeholder()
    Set PlaceholderAt = cc
End Function

Private Function AfterCC(ByVal doc As Document, ByVal cc As ContentControl) As Range
    Set AfterCC = doc.Range(cc.Range.End + 1, cc.Range.End + 1)
End Function

' 仮置き […] を選択範囲の末尾（replaceCC があればその直後）に入れる。直前の引用の境界で押すと
' 新しい CC がその引用の中に入ることがある → 親の直後に作り直す。それでも中なら消してエラー。最後にカーソルを仮置きの直後へ
Public Function InsertPlaceholder(ByVal doc As Document, ByVal replaceCC As ContentControl) As ContentControl
    Dim tag As String, r As Range, cc As ContentControl, parent As ContentControl
    tag = OR_PENDING & NewId(doc)
    If replaceCC Is Nothing Then
        Set r = Selection.Range
        r.Collapse wdCollapseEnd
    Else
        Set r = AfterCC(doc, replaceCC)
    End If
    Set cc = PlaceholderAt(doc, r, tag)
    Set parent = cc.ParentContentControl
    If IsOpenRefCC(parent) Then
        cc.Delete True
        Set cc = PlaceholderAt(doc, AfterCC(doc, parent), tag)
        If IsOpenRefCC(cc.ParentContentControl) Then
            cc.Delete True
            ORIpc.Fail "直前の引用の境界には挿入できません。引用の後ろに空白などを 1 文字入れてから Add/Edit を押してください。"
        End If
    End If
    If Not replaceCC Is Nothing Then UnwrapKeepingText doc, replaceCC
    SelectAfter doc, cc
    Set InsertPlaceholder = cc
End Function

' 仮置きに利用者が打った文字があるとき: 中の最初の […] を消してから CC だけ外す（文字は残す）
Private Sub UnwrapKeepingText(ByVal doc As Document, ByVal cc As ContentControl)
    Dim r As Range
    Set r = cc.Range
    With r.Find
        .ClearFormatting
        .Text = Placeholder()
        .MatchCase = True
        .MatchWildcards = False
        .Forward = True
        .Wrap = wdFindStop
        If .Execute Then r.Delete
    End With
    cc.Delete False
End Sub

' 仮置きを消す（キャンセル・確定の失敗）。中身が […] だけなら CC ごと、文字が打たれていれば […] だけ消して CC を外す
Public Sub RemovePlaceholder(ByVal doc As Document, ByVal tag As String)
    Dim cc As ContentControl, list As New Collection
    For Each cc In doc.ContentControls
        If cc.Tag = tag Then list.Add cc
    Next
    For Each cc In list
        If cc.Range.Text = Placeholder() Then
            cc.Delete True
        Else
            UnwrapKeepingText doc, cc
        End If
    Next
End Sub

Public Sub SelectAfter(ByVal doc As Document, ByVal cc As ContentControl)
    On Error Resume Next
    AfterCC(doc, cc).Select
    Err.Clear
End Sub

Private Function CCAlive(ByVal cc As ContentControl, ByVal tag As String) As Boolean
    Dim t As String
    On Error Resume Next
    t = cc.Tag
    CCAlive = (Err.Number = 0 And t = tag)
    Err.Clear
End Function

Private Sub NotFound(ByVal isNew As Boolean)
    If isNew Then
        ORIpc.Fail "挿入位置の仮置き […] が文書から消えています。", "もう一度 Add/Edit Citation を押してください。"
    Else
        ORIpc.Fail "編集中の引用が文書から見つかりません。", "ダイアログを開いている間に削除された可能性があります（文書は変更していません）。"
    End If
End Sub

' payload を引用として確定する。cc = 仮置き（追加）または引用（編集）。tag = ダイアログを開く前の tag。
' 未固定文書なら style / DOI を固定。編集で同じ tag の CC がほかにもある（コピー＆ペーストの複製）なら、編集した CC だけ新しい tag に分ける
Public Sub CommitCitation(ByVal doc As Document, ByVal cc As ContentControl, ByVal tag As String, ByVal payload As String)
    Dim isNew As Boolean, pinStyle As String, pinDoi As Boolean, pinned As Boolean, dupes As Long, c As ContentControl, citeTag As String
    Dim cites As Collection, pending As Collection, bibs As Collection, heads As Collection, myId As String
    isNew = (Left$(tag, Len(OR_PENDING)) = OR_PENDING)
    If Not CCAlive(cc, tag) Then NotFound isNew
    If Len(payload) = 0 Then ORIpc.Fail "引用する文献が選ばれていません。"
    EffectivePrefs doc, pinStyle, pinDoi, pinned
    myId = cc.ID
    For Each c In doc.ContentControls
        If c.Tag = tag And c.ID <> myId Then dupes = dupes + 1
    Next
    Scan doc, cites, pending, bibs, heads
    If isNew Then
        CheckNested cites, cc
    Else
        CheckNested cites
    End If
    If isNew Then
        citeTag = OR_CITE & Mid$(tag, Len(OR_PENDING) + 1)
    ElseIf dupes > 0 Then
        citeTag = OR_CITE & NewId(doc)
    Else
        citeTag = tag
    End If
    ' payload を先に保存し、保存できてから tag を付け替える（逆順だと失敗で payload の無い引用が残る）
    PayloadSet doc, citeTag, payload
    If Not pinned Then
        VarSet doc, OR_S_STYLE, pinStyle
        VarSet doc, OR_S_DOI, ORIpc.JBool(pinDoi)
    End If
    If citeTag <> tag Then
        cc.Tag = citeTag
        cc.Title = "OpenRef citation"
    End If
    RenderAll doc
    SelectAfter doc, cc
End Sub

' 引用を 1 個削除（編集で 0 件確定）。消すのは cc だけ。同じ tag の CC が残るなら payload は残す。
' 残る「payload のある引用」が 0 なら文献リストと見出しも消す
Public Sub DeleteCitation(ByVal doc As Document, ByVal cc As ContentControl, ByVal tag As String)
    Dim c As ContentControl, remaining As Long, sameLeft As Long, myId As String, olds As New Collection, t As String
    If Not CCAlive(cc, tag) Then NotFound False
    myId = cc.ID
    For Each c In doc.ContentControls
        t = c.Tag
        If c.ID <> myId Then
            If t = tag Then sameLeft = sameLeft + 1
            If Left$(t, Len(OR_CITE)) = OR_CITE Then
                If Len(VarGet(doc, t)) > 0 Then remaining = remaining + 1
            ElseIf t = OR_BIB Or t = OR_HEAD Then
                olds.Add c
            End If
        End If
    Next
    cc.Delete True
    If remaining = 0 Then
        ' 文献リスト・見出しに変更履歴の記録が残っていれば消さない（承認 / 却下のあと手で消す）
        For Each c In olds
            If HasRevisions(c.Range) Then gSkipBib = True
        Next
        If Not gSkipBib Then
            For Each c In olds
                c.Delete True
            Next
        End If
    End If
    If sameLeft = 0 Then PayloadDel doc, tag
    If remaining > 0 Then RenderAll doc
End Sub

' ---------- Settings and Tools ----------
Public Function ValidStyleId(ByVal id As String) As Boolean
    Dim i As Long, ch As String
    If Len(id) = 0 Or Len(id) > 200 Then Exit Function
    For i = 1 To Len(id)
        ch = Mid$(id, i, 1)
        If Not (ch Like "[a-z0-9]" Or (ch = "-" And i > 1)) Then Exit Function
    Next
    ValidStyleId = True
End Function

' 設定ダイアログの [適用]。入れ子があると後の更新が止まるので、保存する前に検査する
Public Sub ApplySettings(ByVal doc As Document, ByVal style As String, ByVal doi As Boolean, ByVal fname As String, ByVal fsize As Double)
    Dim cites As Collection, pending As Collection, bibs As Collection, heads As Collection
    Scan doc, cites, pending, bibs, heads
    CheckNested cites
    If Len(style) > 0 Then
        If Not ValidStyleId(style) Then ORIpc.Fail "スタイル id が不正です: " & style
        VarSet doc, OR_S_STYLE, style
    End If
    VarSet doc, OR_S_DOI, ORIpc.JBool(doi)
    fname = Trim$(fname)
    If Len(fname) = 0 Then fname = OR_DEFAULT_FONT
    If fsize <= 0 Then fsize = OR_DEFAULT_SIZE
    VarSet doc, OR_S_FONT_NAME, fname
    VarSet doc, OR_S_FONT_SIZE, NumStr(fsize)
End Sub

Private Function Overlaps(ByVal a As Range, ByVal b As Range) As Boolean
    Overlaps = (a.Start < b.End And b.Start < a.End)
End Function

' 文献リスト（と見出し）をカーソル段落の直後へ作り直す。新しいものを先に作り、旧いものは後で消す
' （途中で失敗しても文献リストが無くならない順）。選択の段落が旧い文献リスト・見出しと重なるなら何も変えずにエラー
Public Sub MoveBibliographyHere(ByVal doc As Document)
    Dim cites As Collection, pending As Collection, bibs As Collection, heads As Collection, c As ContentControl
    Dim olds As New Collection, legacy As Paragraph, prev As Paragraph, span As Range, lastP As Paragraph
    Dim hp As Paragraph, bp As Paragraph, head As ContentControl, bib As ContentControl
    Scan doc, cites, pending, bibs, heads
    CheckNested cites
    For Each c In bibs
        olds.Add c
    Next
    For Each c In heads
        olds.Add c
    Next
    ' 見出し CC が無い旧文書: 文献リスト直前の素の "References" 段落も旧い見出しとして扱う
    If bibs.Count > 0 And heads.Count = 0 Then
        Set prev = bibs(1).Range.Paragraphs(1).Previous
        If Not prev Is Nothing Then
            If LCase$(Trim$(ParaText(prev))) = "references" Then Set legacy = prev
        End If
    End If
    Set span = doc.Range(Selection.Paragraphs(1).Range.Start, Selection.Paragraphs(Selection.Paragraphs.Count).Range.End)
    For Each c In olds
        If Overlaps(span, c.Range) Then ORIpc.Fail "本文の段落にカーソルを置いてください。", "引用・文献リスト・References 見出し・仮置きの中には移動できません。"
    Next
    If Not legacy Is Nothing Then
        If Overlaps(span, legacy.Range) Then ORIpc.Fail "本文の段落にカーソルを置いてください。", "引用・文献リスト・References 見出し・仮置きの中には移動できません。"
    End If
    ' 変更履歴の記録が残る文献リスト・見出しは作り直せない（消すと記録が消える）= 文書に触れずに止める
    For Each c In olds
        If HasRevisions(c.Range) Then ORIpc.Fail "文献リストに変更履歴が残っているため移動できません。", "校閲 で変更を承認または却下してから、もう一度実行してください。"
    Next
    If Not legacy Is Nothing Then
        If HasRevisions(legacy.Range) Then ORIpc.Fail "文献リストに変更履歴が残っているため移動できません。", "校閲 で変更を承認または却下してから、もう一度実行してください。"
    End If
    Set lastP = Selection.Paragraphs(Selection.Paragraphs.Count)
    lastP.Range.InsertParagraphAfter
    Set hp = lastP.Next
    hp.Range.InsertBefore "References"
    Set hp = lastP.Next
    hp.Style = doc.Styles(wdStyleNormal)
    hp.Range.InsertParagraphAfter
    Set bp = hp.Next
    bp.Style = doc.Styles(wdStyleNormal)
    If bp.Next Is Nothing Then bp.Range.InsertParagraphAfter
    Set head = WrapParagraph(doc, hp)
    head.Tag = OR_HEAD
    head.Title = "References heading (OpenRef)"
    Set bib = WrapParagraph(doc, bp)
    bib.Tag = OR_BIB
    bib.Title = "References (OpenRef)"
    For Each c In olds
        c.Delete True
    Next
    If Not legacy Is Nothing Then legacy.Range.Delete
    RenderAll doc
End Sub

' 引用と文献リストを通常のテキストに変換し（CC だけ外す）、引用データ（refract- の変数）を文書から消す（旧 removeCodes）。
' 仮置きは […] だけなら CC ごと消し、文字が入っていれば […] だけ消して CC を外す
Public Sub RemoveCodes(ByVal doc As Document, ByRef nCites As Long, ByRef nBib As Long, ByRef nPending As Long, ByRef nVars As Long)
    Dim c As ContentControl, all As New Collection, t As String, v As Variable, names As New Collection, nm As Variant
    For Each c In doc.ContentControls
        If IsOpenRefCC(c) Then all.Add c
    Next
    For Each c In all
        t = c.Tag
        If Left$(t, Len(OR_CITE)) = OR_CITE Then
            c.Delete False
            nCites = nCites + 1
        ElseIf t = OR_BIB Or t = OR_HEAD Then
            c.Delete False
            nBib = nBib + 1
        ElseIf Left$(t, Len(OR_PENDING)) = OR_PENDING Then
            nPending = nPending + 1
            If c.Range.Text = Placeholder() Then
                c.Delete True
            Else
                UnwrapKeepingText doc, c
            End If
        End If
    Next
    For Each v In doc.Variables
        If Left$(v.Name, Len(OR_PREFIX)) = OR_PREFIX Then names.Add v.Name
    Next
    For Each nm In names
        VarDel doc, CStr(nm)
        If InStr(CStr(nm), "#") = 0 Then nVars = nVars + 1
    Next
End Sub
