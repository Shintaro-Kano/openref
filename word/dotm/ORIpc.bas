Attribute VB_Name = "ORIpc"
Option Explicit

' connectd（https://localhost:23120/word/v2/*）との通信。
' Word の sandbox の中から popen で curl を起こす（証明書の信頼も効く）。
' 日本語環境の Mac の VBA は文字列・popen の読み出し・Print # を MacJapanese で扱う（0x5C が ¥・バックスラッシュが 0x80）。
' → 応答は ASCII だけで、エスケープ文字は % （%% %t %n %r %uXXXX。バックスラッシュと ~ も %uXXXX）。
' 要求本文（JSON。非 ASCII は \uXXXX = JStr）はコード変換を通さずバイト列で書く（WriteAscii）。

Private Declare PtrSafe Function popen Lib "/usr/lib/libSystem.B.dylib" (ByVal command As String, ByVal mode As String) As LongPtr
Private Declare PtrSafe Function pclose Lib "/usr/lib/libSystem.B.dylib" (ByVal file As LongPtr) As Long
Private Declare PtrSafe Function fread Lib "/usr/lib/libSystem.B.dylib" (ByVal outStr As String, ByVal byteSize As LongPtr, ByVal items As LongPtr, ByVal file As LongPtr) As Long
Private Declare PtrSafe Function feof Lib "/usr/lib/libSystem.B.dylib" (ByVal file As LongPtr) As LongPtr

Public Const OR_BASE As String = "https://localhost:23120"
' 利用者に見せるエラー（Description = 文言・gErrDetail = 補足の 1 行）
Public Const OR_ERR As Long = vbObjectError + 1001
' connectd に繋がらない（ダイアログも出せない = MsgBox で見せる）
Public Const OR_ERR_CONN As Long = vbObjectError + 1002
Public gErrDetail As String

Private gSeq As Long
' E2E テスト用の差し替え先（空 = OR_BASE）
Private gBase As String

Private Function BaseUrl() As String
    If Len(gBase) > 0 Then BaseUrl = gBase Else BaseUrl = OR_BASE
End Function

Public Sub OR_TestSetBase(ByVal url As String)
    gBase = url
End Sub

Public Sub Fail(ByVal message As String, Optional ByVal detail As String = "")
    gErrDetail = detail
    Err.Raise OR_ERR, "OpenRef", message
End Sub

Private Function TmpDir() As String
    Dim p As String
    p = Environ("TMPDIR")
    If Len(p) = 0 Then p = Environ("HOME") & "/tmp"
    If Right$(p, 1) <> "/" Then p = p & "/"
    TmpDir = p
End Function

' 標準出力を返す。exitCode = シェルの終了コード
Private Function RunShell(ByVal cmd As String, ByRef exitCode As Long) As String
    Dim fp As LongPtr, chunk As String, got As Long, acc As String
    fp = popen(cmd, "r")
    If fp = 0 Then
        exitCode = -1
        Exit Function
    End If
    chunk = Space$(65536)
    Do While feof(fp) = 0
        got = fread(chunk, 1, 65536, fp)
        If got > 0 Then acc = acc & Left$(chunk, got)
    Loop
    exitCode = Int(pclose(fp) / 256) And 255
    RunShell = acc
End Function

' ASCII の文字列をそのままのバイト列でファイルへ（Print # は MacJapanese に変換するので使わない）
Private Sub WriteAscii(ByVal path As String, ByVal text As String)
    Dim u() As Byte, b() As Byte, i As Long, n As Long, fh As Integer
    u = text
    n = Int((UBound(u) + 1) / 2)
    If n = 0 Then Exit Sub
    ReDim b(0 To n - 1)
    For i = 0 To n - 1
        b(i) = u(i * 2)
    Next
    fh = FreeFile
    Open path For Binary Access Write As #fh
    Put #fh, , b
    Close #fh
End Sub

' POST（本文 = JSON・ASCII）。戻り値 = 応答の行（0 番目 = "OK"）。ERR 行なら Fail、繋がらなければ OR_ERR_CONN
Public Function Post(ByVal path As String, ByVal body As String, Optional ByVal timeoutSec As Long = 60) As String()
    Dim f As String, out As String, code As Long
    gSeq = gSeq + 1
    f = TmpDir() & "openref-req-" & Format$(Now, "hhnnss") & "-" & gSeq & ".json"
    WriteAscii f, body
    out = RunShell("/usr/bin/curl -sS -m " & timeoutSec & " -H 'X-OpenRef: word' -H 'Content-Type: application/json' --data-binary @'" & f & "' '" & BaseUrl() & path & "' 2>&1", code)
    On Error Resume Next
    Kill f
    On Error GoTo 0
    Post = Parse(out, code, path)
End Function

Public Function GetLines(ByVal path As String, Optional ByVal timeoutSec As Long = 15) As String()
    Dim out As String, code As Long
    out = RunShell("/usr/bin/curl -sS -m " & timeoutSec & " -H 'X-OpenRef: word' '" & BaseUrl() & path & "' 2>&1", code)
    GetLines = Parse(out, code, path)
End Function

' 行形式でない GET（/api/status の JSON など）。繋がらなければ OR_ERR_CONN
Public Function GetRaw(ByVal path As String, Optional ByVal timeoutSec As Long = 15) As String
    Dim out As String, code As Long
    out = RunShell("/usr/bin/curl -sS -f -m " & timeoutSec & " '" & BaseUrl() & path & "' 2>&1", code)
    If code <> 0 Then
        gErrDetail = path & " / curl " & code & " / " & Left$(Trim$(Replace(out, vbLf, " ")), 300)
        Err.Raise OR_ERR_CONN, "OpenRef", "OpenRef（connectd）に接続できません。"
    End If
    GetRaw = out
End Function

Private Function Parse(ByVal out As String, ByVal code As Long, ByVal path As String) As String()
    Dim lines() As String, f() As String, first As String
    If code <> 0 Or (Left$(out, 2) <> "OK" And Left$(out, 3) <> "ERR") Then
        first = Trim$(Replace(Replace(out, vbCr, " "), vbLf, " "))
        If Len(first) > 300 Then first = Left$(first, 300) & "..."
        If code <> 0 Then
            gErrDetail = path & " / curl " & code & IIf(Len(first) > 0, " / " & first, "")
            Err.Raise OR_ERR_CONN, "OpenRef", "OpenRef（connectd）に接続できません。"
        End If
        ' 繋がったが v2 の応答でない = connectd が古い（Word テンプレートだけ新しい）
        Fail "OpenRef（connectd）の応答を解釈できません。OpenRef を更新してください。", path & " / " & first
    End If
    lines = Split(out, vbLf)
    If Left$(lines(0), 3) = "ERR" Then
        f = Split(lines(0), vbTab)
        If UBound(f) >= 2 Then
            Fail Unesc(f(1)), Unesc(f(2))
        ElseIf UBound(f) = 1 Then
            Fail Unesc(f(1))
        Else
            Fail "OpenRef（connectd）でエラーが起きました。", path
        End If
    End If
    Parse = lines
End Function

' 行の欄（タブ区切り）。足りない欄は ""
Public Function Field(ByVal rowText As String, ByVal i As Long) As String
    Dim f() As String
    f = Split(rowText, vbTab)
    If i <= UBound(f) Then Field = f(i)
End Function

' JSON の文字列リテラル（ASCII だけ）
Public Function JStr(ByVal s As String) As String
    Dim i As Long, c As Long, n As Long, pos As Long, buf As String, piece As String
    n = Len(s)
    buf = Space$(n * 6 + 2)
    pos = 1
    Mid$(buf, pos, 1) = """"
    pos = pos + 1
    For i = 1 To n
        c = AscW(Mid$(s, i, 1)) And &HFFFF&
        If c = 34 Then
            piece = "\"""
        ElseIf c = 92 Then
            piece = "\\"
        ElseIf c >= 32 And c <= 126 Then
            piece = Mid$(s, i, 1)
        Else
            piece = "\u" & Right$("000" & Hex$(c), 4)
        End If
        Mid$(buf, pos, Len(piece)) = piece
        pos = pos + Len(piece)
    Next
    Mid$(buf, pos, 1) = """"
    JStr = Left$(buf, pos)
End Function

Public Function JBool(ByVal b As Boolean) As String
    If b Then JBool = "true" Else JBool = "false"
End Function

' 行形式の欄のエスケープを戻す（%% %t %n %r %uXXXX）
Public Function Unesc(ByVal s As String) As String
    Dim n As Long, i As Long, j As Long, pos As Long, buf As String, piece As String, nx As String, code As Long
    If InStr(s, "%") = 0 Then
        Unesc = s
        Exit Function
    End If
    n = Len(s)
    buf = Space$(n)
    pos = 1
    i = 1
    Do While i <= n
        j = InStr(i, s, "%")
        If j = 0 Then j = n + 1
        If j > i Then
            Mid$(buf, pos, j - i) = Mid$(s, i, j - i)
            pos = pos + (j - i)
        End If
        If j > n Then Exit Do
        nx = LCase$(Mid$(s, j + 1, 1))
        If nx = "u" And j + 5 <= n Then
            code = CLng("&H" & Mid$(s, j + 2, 4))
            If code < 0 Then code = code + 65536
            piece = ChrW(code)
            i = j + 6
        ElseIf nx = "n" Then
            piece = vbLf
            i = j + 2
        ElseIf nx = "t" Then
            piece = vbTab
            i = j + 2
        ElseIf nx = "r" Then
            piece = vbCr
            i = j + 2
        ElseIf nx = "%" Then
            piece = "%"
            i = j + 2
        Else
            piece = "%"
            i = j + 1
        End If
        Mid$(buf, pos, 1) = piece
        pos = pos + 1
    Loop
    Unesc = Left$(buf, pos - 1)
End Function

' アプリを LaunchServices で開く（前面に出す）。パスが ASCII でなければ何もしない（popen の引数が MacJapanese に変換されるため）。失敗しても何もしない
Public Sub OpenApp(ByVal appPath As String)
    Dim i As Long, code As Long, out As String
    If Len(appPath) = 0 Or InStr(appPath, "'") > 0 Then Exit Sub
    For i = 1 To Len(appPath)
        If AscW(Mid$(appPath, i, 1)) > 126 Or AscW(Mid$(appPath, i, 1)) < 32 Then Exit Sub
    Next
    On Error Resume Next
    out = RunShell("/usr/bin/open -a '" & appPath & "' 2>&1", code)
    If code <> 0 Then LogLine "warn", "open -a " & appPath & " -> " & code & " " & Left$(out, 200)
    Err.Clear
End Sub

' connectd のログへ（失敗しても何もしない）
Public Sub LogLine(ByVal level As String, ByVal message As String)
    Dim r() As String
    On Error Resume Next
    r = Post("/word/v2/log", "{""level"":" & JStr(level) & ",""message"":" & JStr(message) & "}", 5)
End Sub
