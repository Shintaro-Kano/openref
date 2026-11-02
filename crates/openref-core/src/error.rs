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
}

impl CoreError {
    pub fn wiki(code: &'static str, msg: impl Into<String>) -> Self {
        CoreError::Wiki { code, msg: msg.into() }
    }
}
