//! Cross-export IDs are hints for review, never authority to merge or recreate.
use super::{
    files::digest,
    plan::{self, ItemAction},
    ImportFolder, ImportInput, ImportItem, ImportTag,
};
use crate::Result;
use rusqlite::Connection;
use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Default)]
pub(crate) struct CrossExport {
    items: HashMap<String, BTreeSet<String>>,
    folders: HashSet<String>,
    tags: HashSet<String>,
    targets: HashMap<(String, String, String), BTreeSet<String>>,
}
fn item_tokens(item: &ImportItem) -> Vec<String> {
    let mut bibliography = item.csl.clone();
    if let Some(fields) = bibliography.as_object_mut() {
        fields.remove("custom");
        fields.remove("citation-key");
    }
    let mut tokens = vec![
        format!("id:{}", item.id),
        format!("csl:{}", digest(bibliography.to_string().as_bytes())),
    ];
    for (kind, value) in [
        ("doi", plan::doi(&item.csl)),
        ("pmid", plan::pmid(&item.csl)),
        ("description", plan::descriptive(&item.csl)),
    ] {
        if let Some(value) = value {
            tokens.push(format!("{kind}:{value}"));
        }
    }
    tokens
}
impl CrossExport {
    pub fn read(conn: &Connection, input: &ImportInput) -> Result<Self> {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='import_objects')",
            [],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(Self::default());
        }
        let mut objects = HashMap::new();
        let mut stmt = conn.prepare("SELECT kind,scope,object_id,target_key FROM import_objects WHERE source_id=?1 AND scope!='' AND scope!=?2 AND kind IN ('item','folder','tag')")?;
        for row in stmt.query_map(
            rusqlite::params![input.source.id, input.input_sha256],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        )? {
            let (kind, scope, id, key) = row?;
            objects.insert((kind, scope, id), key);
        }
        if objects.is_empty() {
            return Ok(Self::default());
        }
        let mut guard = Self::default();
        let mut seen = HashSet::new();
        let mut runs=conn.prepare("SELECT input_sha256,input_json FROM import_runs WHERE source_id=?1 AND input_sha256!=?2")?;
        for row in runs.query_map(
            rusqlite::params![input.source.id, input.input_sha256],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )? {
            let (sha, raw) = row?;
            if !seen.insert(sha.clone()) {
                continue;
            }
            let prior: ImportInput = serde_json::from_str(&raw)?;
            for item in &prior.items {
                for tag in &item.tags {
                    if objects.contains_key(&("tag".into(), sha.clone(), tag.id.clone())) {
                        let key = objects
                            .get(&("tag".into(), sha.clone(), tag.id.clone()))
                            .unwrap();
                        guard
                            .targets
                            .entry(("tag".into(), tag.id.clone(), tag.name.clone()))
                            .or_default()
                            .insert(key.clone());
                        guard.tags.insert(format!("id:{}", tag.id));
                        guard.tags.insert(format!("name:{}", tag.name));
                    }
                }
                if let Some(key) = objects.get(&("item".into(), sha.clone(), item.id.clone())) {
                    for token in item_tokens(item) {
                        guard.items.entry(token).or_default().insert(key.clone());
                    }
                }
            }
            for folder in prior.folders {
                if objects.contains_key(&("folder".into(), sha.clone(), folder.id.clone())) {
                    let key = objects
                        .get(&("folder".into(), sha.clone(), folder.id.clone()))
                        .unwrap();
                    guard
                        .targets
                        .entry(("folder".into(), folder.id.clone(), folder.name.clone()))
                        .or_default()
                        .insert(key.clone());
                    guard.folders.insert(format!("id:{}", folder.id));
                    guard.folders.insert(format!("name:{}", folder.name));
                }
            }
        }
        Ok(guard)
    }
    pub fn action(
        &self,
        item: &ImportItem,
        mut live: impl FnMut(&str) -> Result<bool>,
    ) -> Result<Option<ItemAction>> {
        if item.stable_id {
            return Ok(None);
        }
        let candidates: BTreeSet<_> = item_tokens(item)
            .iter()
            .filter_map(|t| self.items.get(t))
            .flatten()
            .cloned()
            .collect();
        if candidates.is_empty() {
            return Ok(None);
        }
        let mut deleted = false;
        for key in &candidates {
            deleted |= !live(key)?;
        }
        let reason=if deleted { "別の書出しに削除済み対象の対応候補があります。自動再作成しません" } else { "別の書出しの取込済み文献に対応する可能性があります。編集・所属・ノートを保護するため、対応の確認まで保留します。同一入力で不足分を補完できます" }.into();
        Ok(Some(if deleted {
            ItemAction::Blocked {
                candidates: candidates.into_iter().collect(),
                reason,
            }
        } else {
            ItemAction::Review {
                candidates: candidates.into_iter().collect(),
                reason,
            }
        }))
    }
    pub fn target(&self, kind: &str, id: &str, name: &str) -> Option<&str> {
        let candidates = self
            .targets
            .get(&(kind.to_string(), id.to_string(), name.to_string()))?;
        (candidates.len() == 1).then(|| candidates.iter().next().unwrap().as_str())
    }
    pub fn tag_needs_review(&self, tag: &ImportTag) -> bool {
        !tag.stable_id
            && (self.tags.contains(&format!("id:{}", tag.id))
                || self.tags.contains(&format!("name:{}", tag.name)))
    }
    pub fn folder_needs_review(&self, folder: &ImportFolder) -> bool {
        !folder.stable_id
            && (self.folders.contains(&format!("id:{}", folder.id))
                || self.folders.contains(&format!("name:{}", folder.name)))
    }
}
