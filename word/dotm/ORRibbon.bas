Attribute VB_Name = "ORRibbon"
Option Explicit

' リボン「OpenRef」タブ（customUI14.xml）のコールバックと起動時の処理。
' コールバックの引数は (ByVal control As IRibbonControl)（Paperpile の Startup テンプレートと同じ形）

Private gRibbon As IRibbonUI

' Word の起動時（Startup のテンプレートを読んだとき）。vbaData.xml に載っていないと呼ばれない（build-word-dotm.py が載せる）
Public Sub AutoExec()
    On Error Resume Next
    ' テンプレート自体は変更しない = 終了時に「OpenRef.dotm への変更を保存しますか」を出さない
    ThisDocument.Saved = True
    On Error GoTo 0
#If OPENREF_E2E Then
    E2E.RunAll
#End If
End Sub

Public Sub OR_RibbonLoad(ByVal ribbon As IRibbonUI)
    Set gRibbon = ribbon
End Sub

Public Sub OR_AddEdit(ByVal control As IRibbonControl)
    ORCmd.OpenRefAddEdit
End Sub

Public Sub OR_Update(ByVal control As IRibbonControl)
    ORCmd.OpenRefUpdate
End Sub

Public Sub OR_Style(ByVal control As IRibbonControl)
    ORCmd.OpenRefCitationStyle
End Sub

Public Sub OR_DocSettings(ByVal control As IRibbonControl)
    ORCmd.OpenRefDocumentSettings
End Sub

Public Sub OR_MoveBib(ByVal control As IRibbonControl)
    ORCmd.OpenRefMoveBibliography
End Sub

Public Sub OR_RemoveCodes(ByVal control As IRibbonControl)
    ORCmd.OpenRefRemoveCitationCodes
End Sub

Public Sub OR_Diagnostics(ByVal control As IRibbonControl)
    ORCmd.OpenRefShowDiagnostics
End Sub
