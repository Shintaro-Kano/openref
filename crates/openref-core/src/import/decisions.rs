//! Explicit, input-bound decisions. A decision never overrides a tombstone or
//! identifier conflict; every target is checked again inside the writer tx.
use super::{
    files::{digest, AttachmentRoots},
    inventory,
    plan::{self, AttachmentPlan, ExistingLibrary, ImportPlan, ItemAction},
    ImportInput, ImportItem,
};
use crate::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub csl_sha256: String,
    pub key: String,
    pub title: String,
    pub doi: Option<String>,
    pub pmid: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Selection {
    pub item_key: String,
    pub csl_sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemChoice {
    pub object_id: String,
    pub item_key: String,
    /// The candidate shown when the user chose it, not a newly generated preview.
    #[serde(default)]
    pub csl_sha256: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileChoice {
    pub object_id: String,
    pub attachment_id: String,
    pub path: PathBuf,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decisions {
    pub source_id: String,
    pub input_sha256: String,
    #[serde(default)]
    pub items: Vec<ItemChoice>,
    #[serde(default)]
    pub files: Vec<FileChoice>,
}
fn invalid() -> CoreError {
    CoreError::Conflict(
        "選択した候補または入力が変更されています。事前確認から選び直してください".into(),
    )
}
pub(crate) fn describe(plan: &mut ImportPlan, library: &ExistingLibrary) {
    for item in &mut plan.items {
        item.file_options = item
            .attachments
            .iter()
            .flat_map(|a| match a {
                AttachmentPlan::Ambiguous { id, candidates } => candidates
                    .iter()
                    .map(|path| FileChoice {
                        object_id: item.object_id.clone(),
                        attachment_id: id.clone(),
                        path: path.clone(),
                    })
                    .collect(),
                _ => Vec::new(),
            })
            .collect();
        if let ItemAction::Review { candidates, .. } = &item.action {
            item.candidate_details = library
                .items
                .iter()
                .filter(|i| !i.deleted && candidates.contains(&i.key))
                .map(|i| Candidate {
                    csl_sha256: digest(i.csl.to_string().as_bytes()),
                    key: i.key.clone(),
                    title: crate::model::s(&i.csl, "title").unwrap_or("").into(),
                    doi: plan::doi(&i.csl),
                    pmid: plan::pmid(&i.csl),
                })
                .collect();
        }
    }
}
/// Resolves a freshly generated preview, using only a private DB snapshot.
/// Mutates the plan only on success; never modifies the source DB or files.
pub fn resolve(
    db: &Path,
    input: &ImportInput,
    roots: &AttachmentRoots,
    plan: &mut ImportPlan,
    choices: &Decisions,
    cancel: &AtomicBool,
) -> Result<()> {
    if choices.source_id != input.source.id
        || choices.input_sha256 != input.input_sha256
        || plan.input_model_sha256 != digest(&serde_json::to_vec(input)?)
    {
        return Err(invalid());
    }
    let (library, _, _) = inventory::read_import(db, input)?;
    let mut next = plan.clone();
    let mut seen = HashSet::new();
    for choice in &choices.items {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Conflict("事前確認を中止しました".into()));
        }
        if !seen.insert(&choice.object_id) {
            return Err(invalid());
        }
        let source = input
            .items
            .iter()
            .find(|i| i.id == choice.object_id)
            .ok_or_else(invalid)?;
        let planned = next
            .items
            .iter_mut()
            .find(|i| i.object_id == choice.object_id)
            .ok_or_else(invalid)?;
        if choice.item_key == "@new" {
            // A completed explicit-new decision becomes its exact provenance mapping on retry.
            if let ItemAction::AddMissing { item_key } = &planned.action {
                if library.objects.iter().any(|o| {
                    o.source_id == input.source.id
                        && o.object_id == source.id
                        && o.item_key == *item_key
                        && (o.input_sha256.is_none()
                            || o.input_sha256.as_deref() == Some(&input.input_sha256))
                }) {
                    continue;
                }
            }
            if !planned.allow_new || !matches!(&planned.action, ItemAction::Review { .. }) {
                return Err(invalid());
            }
            next.counts.review -= 1;
            next.counts.new_items += 1;
            planned.action = ItemAction::New;
            planned.register_new = true;
            planned.warnings.push("入力内の重複候補を新規として登録します。適用時に同一識別子の文献があれば対応を再確認します".into());
            continue;
        }
        let target = library
            .items
            .iter()
            .find(|i| i.key == choice.item_key && !i.deleted)
            .ok_or_else(invalid)?;
        // Already applied choices may be supplied again with the same input.
        let mapped = library.objects.iter().any(|o| {
            o.source_id == input.source.id
                && o.object_id == source.id
                && (o.input_sha256.is_none()
                    || o.input_sha256.as_deref() == Some(&input.input_sha256))
                && o.item_key == target.key
        });
        if !mapped
            && choice.csl_sha256.as_deref()
                != Some(digest(target.csl.to_string().as_bytes()).as_str())
        {
            return Err(invalid());
        }
        match &planned.action {
            ItemAction::Review { candidates, .. }
                if candidates.contains(&target.key)
                    && planned.candidate_details.iter().any(|c| {
                        c.key == target.key
                            && c.csl_sha256 == digest(target.csl.to_string().as_bytes())
                    }) =>
            {
                next.counts.review -= 1;
                next.counts.existing_items += 1;
            }
            ItemAction::AddMissing { item_key } if mapped && item_key == &target.key => {}
            _ => return Err(invalid()),
        }
        if plan::conflicts(&source.csl, &target.csl) {
            return Err(invalid());
        }
        planned.selection = Some(Selection {
            item_key: target.key.clone(),
            csl_sha256: digest(target.csl.to_string().as_bytes()),
        });
        planned.action = ItemAction::AddMissing {
            item_key: target.key.clone(),
        };
        planned.warnings.push(
            "利用者が選択した既存文献へ不足分を追加します。識別子と対象を適用直前にも確認します"
                .into(),
        );
        if library
            .objects
            .iter()
            .any(|o| o.source_id == input.source.id && o.item_key == target.key)
        {
            planned.warnings.push("過去の編集・削除を保護するため、ノート・タグ・所属の対応は保留します。書誌の不足分とPDFだけを追加します".into());
        }
    }
    let mut seen = HashSet::new();
    for choice in &choices.files {
        if !seen.insert((&choice.object_id, &choice.attachment_id)) {
            return Err(invalid());
        }
        let planned = next
            .items
            .iter_mut()
            .find(|i| i.object_id == choice.object_id)
            .ok_or_else(invalid)?;
        let attachment = planned
            .attachments
            .iter_mut()
            .find(|a| matches!(a,AttachmentPlan::Ambiguous{id,..} if id==&choice.attachment_id))
            .ok_or_else(invalid)?;
        let AttachmentPlan::Ambiguous { candidates, .. } = attachment else {
            unreachable!()
        };
        if !candidates.contains(&choice.path) {
            return Err(invalid());
        }
        let (path, fingerprint) = roots.inspect_controlled(&choice.path, cancel, &mut |_| {})?;
        next.counts.ambiguous_pdfs -= 1;
        next.counts.available_pdfs += 1;
        if next.attachment_mode == super::AttachmentMode::Copy {
            next.counts.copy_bytes_upper_bound += fingerprint.bytes;
        }
        *attachment = AttachmentPlan::Available {
            id: choice.attachment_id.clone(),
            path,
            fingerprint,
        };
        planned.selected_files.push(choice.attachment_id.clone());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(CoreError::Conflict("事前確認を中止しました".into()));
    }
    *plan = next;
    Ok(())
}
pub(crate) fn recheck(
    conn: &Connection,
    item: &ImportItem,
    action: &ItemAction,
    selection: &Selection,
) -> Result<ItemAction> {
    let blocked = || ItemAction::Blocked {
        candidates: vec![selection.item_key.clone()],
        reason: "選択した文献が変更・削除されたか、照合結果が変わりました。再確認してください"
            .into(),
    };
    let current: Option<(String, bool)> = conn
        .query_row(
            "SELECT csl,deleted_at IS NOT NULL FROM items WHERE key=?1",
            [&selection.item_key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((raw, false)) = current else {
        return Ok(blocked());
    };
    let csl: serde_json::Value = serde_json::from_str(&raw)?;
    if digest(csl.to_string().as_bytes()) != selection.csl_sha256
        || plan::conflicts(&item.csl, &csl)
    {
        return Ok(blocked());
    }
    let eligible = match action {
        ItemAction::Review { candidates, .. } => candidates.contains(&selection.item_key),
        ItemAction::AddMissing { item_key } => item_key == &selection.item_key,
        _ => false,
    };
    // All strong identifiers must point at this target, even for a weak match.
    let candidates = plan::identifier_candidates(conn, &item.csl, None)?;
    if !eligible
        || candidates
            .iter()
            .any(|(key, _, deleted)| key != &selection.item_key || *deleted)
    {
        return Ok(blocked());
    }
    Ok(ItemAction::AddMissing {
        item_key: selection.item_key.clone(),
    })
}
