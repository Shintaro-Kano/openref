pub mod annotate;
pub mod backup;
pub mod bibtex;
pub mod citestyle;
pub mod collections;
pub mod commands;
pub mod datadir;
pub mod db;
pub mod drive_layout;
pub mod edit;
pub mod error;
pub mod fulltext;
mod http;
pub mod import;
pub mod ingest;
pub mod kbmanifest;
pub mod keys;
pub mod model;
pub mod mount;
pub mod pdffetch;
pub mod pdfingest;
pub mod pdftools;
pub mod resolver;
pub mod search;
pub mod sync;
pub mod tags;
pub mod types;
pub mod update;
pub mod zotero;

pub use db::Db;
pub use error::CoreError;

pub type Result<T> = std::result::Result<T, CoreError>;

/// 環境変数 `OPENREF_<name>` を読む（env の読み出しはすべてここを通す）。
/// 値が空文字なら未設定とみなす（`OPENREF_DB=""` で空パスの DB を開かない）。
/// 改名前の `REFRACT_<name>` への fallback は無い
pub fn env_os(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(format!("OPENREF_{name}")).filter(|v| !v.is_empty())
}
