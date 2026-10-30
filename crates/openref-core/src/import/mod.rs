//! Library migration. Adapters produce normalized data; planning is read-only.
//! The Paperpile and Zotero adapters feed the one `apply` path (there is no other import route).
pub mod apply;
pub mod decisions;
pub mod files;
pub mod inventory;
mod note_html;
pub mod paperpile;
pub mod plan;
mod prepare;
mod provenance;
mod storage;
pub mod zotero_rdf;

pub use prepare::{prepare_request, read_input, Prepared, Request};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct ImportProgress {
    pub phase: &'static str,
    pub completed: usize,
    pub total: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportSource {
    /// Persistent identity selected by the user, never inferred from a filename.
    pub id: String,
    pub format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportInput {
    pub source: ImportSource,
    /// Explicit confirmation that legacy custom.paperpile.id values belong to
    /// this source library. Never enabled merely because the IDs look alike.
    #[serde(default)]
    pub adopt_legacy_paperpile_ids: bool,
    pub input_sha256: String,
    pub adapter_version: String,
    pub items: Vec<ImportItem>,
    /// A graph, not slash-delimited paths. Includes empty folders.
    pub folders: Vec<ImportFolder>,
    /// この入力に固有の注意（入力の中身による）。最終報告の warnings に入り、complete を false にする
    pub warnings: Vec<String>,
    /// 対応の要らない注意: 形式（adapter の版）の固定の注意と、元レコードに保持しただけのものの件数の集計。
    /// 事前確認・最終報告に notices として出し、complete には数えない。対応が要るもの（取り込めない・曖昧・不一致）は warnings
    #[serde(default)]
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportItem {
    pub id: String,
    /// True only when verified against the source application's actual export.
    /// Otherwise the ID is scoped to this exact input digest.
    pub stable_id: bool,
    pub csl: Value,
    #[serde(default)]
    pub date_added: Option<String>,
    pub raw: Value,
    pub folder_ids: Vec<String>,
    pub tags: Vec<ImportTag>,
    pub notes: Vec<ImportNote>,
    pub attachments: Vec<ImportAttachment>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportFolder {
    pub id: String,
    #[serde(default)]
    pub stable_id: bool,
    pub name: String,
    pub parent_id: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportTag {
    pub id: String,
    #[serde(default)]
    pub stable_id: bool,
    pub name: String,
    pub color: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportNote {
    pub id: String,
    #[serde(default)]
    pub stable_id: bool,
    pub content_md: String,
    pub raw: Value,
    pub date_added: Option<String>,
    pub date_modified: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportAttachment {
    pub id: String,
    #[serde(default)]
    pub stable_id: bool,
    pub path: String,
    /// Exact layout alternatives established from an actual export, never a
    /// basename search. All alternatives are checked for ambiguity together.
    #[serde(default)]
    pub alternative_paths: Vec<String>,
    pub media_type: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentMode {
    #[default]
    Copy,
    Link,
}
