use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("入力を解釈できません: {0}")]
    BadInput(String),
    #[error("重複: 既存 item {existing_key}（{field} が一致）")]
    Duplicate {
        existing_key: String,
        field: &'static str,
    },
    #[error("not found: {0}")]
    NotFound(String),
    /// 取得中に状態が変わった（別経路で添付済み等）。前置きを付けずそのまま表示する
    #[error("{0}")]
    Conflict(String),
    /// データディレクトリの解決・移行（datadir.rs）。前置きを付けずそのまま表示する
    #[error("{0}")]
    DataDir(String),
    /// OpenNote の状態（無効・未設定・索引の作り直し中）など。前置きを付けずそのまま表示する（msg）。
    /// code = 機械可読の印（`crate::wiki::codes`）。画面は code で分岐し、msg（人と agent が読む文）を照合しない
    #[error("{msg}")]
    Wiki { code: &'static str, msg: String },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// エージェント（MCP / CLI）向けの手がかり（MCP の呼び方・引数名・tool 名）を添えた失敗。Display（= MCP / CLI が返す文）は
    /// `{err}（{hint}）` = 手がかりを文に直に書いていたときと同じ字。GUI は `without_hint` で外して err だけを利用者に見せる
    /// （利用者には意味の無い「root には … をそのまま渡す」を toast に出さない）。作るのは `CoreError::hint` だけ
    #[error("{err}（{hint}）")]
    Hinted { err: Box<CoreError>, hint: &'static str },
}

impl CoreError {
    pub fn wiki(code: &'static str, msg: impl Into<String>) -> Self {
        CoreError::Wiki { code, msg: msg.into() }
    }

    /// エージェント向けの手がかりを添える（Display の末尾に「（hint）」= MCP / CLI の文。GUI は without_hint で外す）
    pub fn hint(self, hint: &'static str) -> Self {
        CoreError::Hinted { err: Box::new(self), hint }
    }

    /// エージェント向けの手がかりを外した失敗（GUI が利用者に見せる物）。手がかりの無い失敗はそのまま
    pub fn without_hint(self) -> Self {
        match self {
            CoreError::Hinted { err, .. } => err.without_hint(),
            e => e,
        }
    }

    /// `CoreError::Wiki` の code（手がかりを添えていても中を見る）。他は None
    pub fn code(&self) -> Option<&'static str> {
        match self {
            CoreError::Wiki { code, .. } => Some(code),
            CoreError::Hinted { err, .. } => err.code(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手がかりを添えても MCP / CLI の文（Display）は直に書いたときと同じ字・GUI の文（without_hint）は手がかりを含まない・code は中を見る
    #[test]
    fn hint_is_agent_only() {
        let e = CoreError::BadInput("履歴に無い本棚です: /a/b".into()).hint("root には shelf_status の root をそのまま渡す");
        assert_eq!(e.to_string(), "入力を解釈できません: 履歴に無い本棚です: /a/b（root には shelf_status の root をそのまま渡す）");
        assert_eq!(e.without_hint().to_string(), "入力を解釈できません: 履歴に無い本棚です: /a/b");
        let w = CoreError::wiki("unknown_root", "履歴に無いワークスペースです: /v").hint("root には wiki_status の workspaces の root をそのまま渡す");
        assert_eq!(w.to_string(), "履歴に無いワークスペースです: /v（root には wiki_status の workspaces の root をそのまま渡す）");
        assert_eq!(w.code(), Some("unknown_root"));
        assert!(matches!(w.without_hint(), CoreError::Wiki { code: "unknown_root", ref msg } if msg == "履歴に無いワークスペースです: /v"));
        assert_eq!(CoreError::NotFound("x".into()).without_hint().to_string(), "not found: x");
    }
}
