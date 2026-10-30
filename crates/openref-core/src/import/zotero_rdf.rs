//! Zotero RDF/XML library reader. No network access, external entities or DB.
//! Independently implemented from the documented RDF output vocabulary, not
//! from the simplified Connector item JSON converter.
use super::{
    files::digest, ImportAttachment, ImportFolder, ImportInput, ImportItem, ImportNote,
    ImportSource, ImportTag,
};
use crate::{CoreError, Result};
use roxmltree::Node;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const NS: &[(&str, &str)] = &[
    (RDF, "rdf"),
    ("http://purl.org/net/biblio#", "bib"),
    ("http://purl.org/dc/elements/1.1/", "dc"),
    ("http://purl.org/dc/terms/", "dcterms"),
    ("http://prismstandard.org/namespaces/1.2/basic/", "prism"),
    ("http://xmlns.com/foaf/0.1/", "foaf"),
    ("http://nwalsh.com/rdf/vCard#", "vcard"),
    ("http://www.w3.org/2006/vcard/ns#", "vcard2"),
    ("http://purl.org/rss/1.0/modules/link/", "link"),
    ("http://www.zotero.org/namespaces/export#", "z"),
];
fn name(ns: Option<&str>, local: &str) -> String {
    match NS.iter().find(|(uri, _)| Some(*uri) == ns) {
        Some((_, prefix)) => format!("{prefix}:{local}"),
        None => format!("{{{}}}{local}", ns.unwrap_or("")),
    }
}
fn uri_name(uri: &str) -> String {
    NS.iter()
        .find_map(|(ns, prefix)| uri.strip_prefix(ns).map(|s| format!("{prefix}:{s}")))
        .unwrap_or_else(|| uri.into())
}
fn bad(message: impl Into<String>) -> CoreError {
    CoreError::BadInput(format!("Zotero RDF: {}", message.into()))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Term {
    Literal(String),
    Resource(String),
}
#[derive(Default, Clone, Serialize)]
struct Resource {
    properties: BTreeMap<String, Vec<Term>>,
    original_xml: Vec<String>,
}
#[derive(Default)]
struct Graph {
    nodes: BTreeMap<String, Resource>,
    read: RefCell<BTreeSet<(String, String)>>,
}
impl Graph {
    fn add(&mut self, id: &str, p: String, value: Term) {
        let values = self
            .nodes
            .entry(id.into())
            .or_default()
            .properties
            .entry(p)
            .or_default();
        if !values.contains(&value) {
            values.push(value);
        }
    }
    fn values(&self, id: &str, p: &str) -> Vec<Term> {
        self.read.borrow_mut().insert((id.into(), p.into()));
        self.nodes
            .get(id)
            .and_then(|r| r.properties.get(p))
            .cloned()
            .unwrap_or_default()
    }
    fn literals(&self, id: &str, p: &str) -> Vec<String> {
        self.values(id, p)
            .into_iter()
            .filter_map(|t| {
                if let Term::Literal(v) = t {
                    Some(v)
                } else {
                    None
                }
            })
            .collect()
    }
    fn text(&self, id: &str, p: &str) -> Option<String> {
        self.literals(id, p)
            .into_iter()
            .find(|s| !s.trim().is_empty())
    }
    fn refs(&self, id: &str, p: &str) -> Vec<String> {
        self.values(id, p)
            .into_iter()
            .filter_map(|t| {
                if let Term::Resource(v) = t {
                    Some(v)
                } else {
                    None
                }
            })
            .collect()
    }
    fn has_type(&self, id: &str, t: &str) -> bool {
        self.refs(id, "rdf:type").iter().any(|v| uri_name(v) == t)
    }
    fn kind(&self, id: &str) -> Option<String> {
        self.text(id, "z:itemType").or_else(|| {
            for (t, kind) in [
                ("bib:Memo", "note"),
                ("z:Attachment", "attachment"),
                ("bib:Book", "book"),
                ("bib:BookSection", "bookSection"),
                ("bib:Article", "journalArticle"),
                ("bib:Thesis", "thesis"),
                ("bib:Document", "document"),
            ] {
                if self.has_type(id, t) {
                    return Some(kind.into());
                }
            }
            None
        })
    }
    fn parse_subject(&mut self, node: Node<'_, '_>, depth: usize) -> Result<String> {
        if depth > 128 {
            return Err(bad("入れ子が深すぎます"));
        }
        let identities = ["about", "ID", "nodeID"]
            .iter()
            .filter(|a| node.attribute((RDF, **a)).is_some())
            .count();
        if identities > 1 {
            return Err(bad("resource の ID が複数指定されています"));
        }
        let id = if let Some(s) = node.attribute((RDF, "about")) {
            s.into()
        } else if let Some(s) = node.attribute((RDF, "ID")) {
            format!("#{s}")
        } else if let Some(s) = node.attribute((RDF, "nodeID")) {
            format!("_:{s}")
        } else {
            format!("_:anon{}", node.range().start)
        };
        self.nodes
            .entry(id.clone())
            .or_default()
            .original_xml
            .push(node.document().input_text()[node.range()].into());
        let tag = node.tag_name();
        if tag.namespace() != Some(RDF) || tag.name() != "Description" {
            self.add(
                &id,
                "rdf:type".into(),
                Term::Resource(format!("{}{}", tag.namespace().unwrap_or(""), tag.name())),
            );
        }
        self.parse_properties(&id, node, depth)?;
        Ok(id)
    }
    fn parse_properties(&mut self, id: &str, node: Node<'_, '_>, depth: usize) -> Result<()> {
        let mut ordinal = 0;
        for prop in node.children().filter(Node::is_element) {
            let tag = prop.tag_name();
            let p = if tag.namespace() == Some(RDF) && tag.name() == "li" {
                ordinal += 1;
                format!("rdf:_{ordinal}")
            } else {
                name(tag.namespace(), tag.name())
            };
            let children: Vec<_> = prop.children().filter(Node::is_element).collect();
            let term = if let Some(v) = prop.attribute((RDF, "resource")) {
                if !children.is_empty() {
                    return Err(bad("resource と入れ子が同時に存在します"));
                }
                Term::Resource(v.into())
            } else if let Some(v) = prop.attribute((RDF, "nodeID")) {
                Term::Resource(format!("_:{v}"))
            } else {
                match prop.attribute((RDF, "parseType")) {
                    Some("Resource") => {
                        if depth >= 128 {
                            return Err(bad("入れ子が深すぎます"));
                        }
                        let child = format!("_:anon{}", prop.range().start);
                        self.nodes.entry(child.clone()).or_default();
                        self.parse_properties(&child, prop, depth + 1)?;
                        Term::Resource(child)
                    }
                    Some("Literal") => {
                        let source = prop.document().input_text();
                        let value = prop
                            .children()
                            .map(|n| &source[n.range()])
                            .collect::<String>();
                        Term::Literal(value)
                    }
                    Some(other) => return Err(bad(format!("未対応の rdf:parseType={other}"))),
                    None if children.len() == 1 => {
                        Term::Resource(self.parse_subject(children[0], depth + 1)?)
                    }
                    None if children.len() > 1 => {
                        return Err(bad("1つのプロパティに複数の resource があります"))
                    }
                    None => Term::Literal(
                        prop.children()
                            .filter_map(|n| n.text().filter(|_| n.is_text()))
                            .collect(),
                    ),
                }
            };
            self.add(id, p, term);
        }
        Ok(())
    }
    // Keep the RDF subgraph outside CSL. Do not walk item relations into the
    // entire library, or recurse through cycles. Every node is retained once
    // at least, including otherwise unassigned extension data below.
    fn raw(&self, id: &str, claimed: &mut BTreeSet<String>) -> Value {
        let mut pending = vec![id.to_string()];
        let mut visited = BTreeSet::new();
        let mut nodes = BTreeMap::new();
        while let Some(key) = pending.pop() {
            if !visited.insert(key.clone()) {
                continue;
            }
            if let Some(node) = self.nodes.get(&key) {
                for (p, terms) in &node.properties {
                    if matches!(p.as_str(), "dc:relation" | "dcterms:hasPart" | "rdf:type") {
                        continue;
                    }
                    for t in terms {
                        if let Term::Resource(r) = t {
                            pending.push(r.clone());
                        }
                    }
                }
                claimed.insert(key.clone());
                nodes.insert(key, node);
            }
        }
        json!({"resource":id,"resources":nodes})
    }
}

pub fn read_input(path: &Path, source: ImportSource) -> Result<ImportInput> {
    if source.format != "zotero" {
        return Err(bad("移行元形式が一致しません"));
    }
    let bytes = std::fs::read(path)?;
    let xml =
        std::str::from_utf8(&bytes).map_err(|_| bad("UTF-8 の RDF/XML を指定してください"))?;
    // roxmltree rejects DTD by default. Never fetch schemas/entities/URLs.
    let doc = roxmltree::Document::parse(xml).map_err(|e| bad(e.to_string()))?;
    let root = doc.root_element();
    if root.tag_name().namespace() != Some(RDF) || root.tag_name().name() != "RDF" {
        return Err(bad("rdf:RDF ルートがありません"));
    }
    if root.descendants().any(|n| {
        n.attribute(("http://www.w3.org/XML/1998/namespace", "base"))
            .is_some()
    }) {
        return Err(bad(
            "xml:base を持つ入力は未対応です。元の Zotero RDF で書き出してください",
        ));
    }
    let mut graph = Graph::default();
    for node in root.children().filter(Node::is_element) {
        graph.parse_subject(node, 0)?;
    }
    let collection_ids: BTreeSet<_> = graph
        .nodes
        .keys()
        .filter(|id| graph.has_type(id, "z:Collection"))
        .cloned()
        .collect();
    // A typed Book used only as a container is not a library item. Explicit
    // itemType still permits a real standalone book also referenced by a chapter.
    let containers: BTreeSet<_> = graph
        .nodes
        .keys()
        .flat_map(|id| graph.refs(id, "dcterms:isPartOf"))
        .collect();
    let kinds: BTreeMap<_, _> = graph
        .nodes
        .keys()
        .filter(|id| !containers.contains(*id) || graph.text(id, "z:itemType").is_some())
        .filter_map(|id| graph.kind(id).map(|kind| (id.clone(), kind)))
        .collect();
    let mut parents: BTreeMap<String, String> = BTreeMap::new();
    let mut memberships: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let notices = vec![
        "Zotero RDF の登録・更新日時は保持できないため、OpenRef の取込日時を使います".into(),
        "RDF の resource ID は同一入力内で照合します。別の書出しやライブラリでの安定 ID とは扱いません".into(),
        "公式 RDF 出力処理と公開テスト形式を根拠に実装しています。Zotero アプリでの書出し操作と PDF 注釈表示は未検証です".into(),
    ];
    let mut warnings = Vec::new();
    for id in &collection_ids {
        for child in graph.refs(id, "dcterms:hasPart") {
            if collection_ids.contains(&child) {
                if parents.insert(child.clone(), id.clone()).is_some() {
                    return Err(bad(format!("コレクション {child} に複数の親があります")));
                }
            } else if kinds.contains_key(&child) {
                memberships.entry(child).or_default().push(id.clone());
            } else {
                warnings.push(format!(
                    "コレクション {id} の参照先 {child} が入力にありません。元情報を保持します"
                ));
            }
        }
    }
    for id in &collection_ids {
        let mut seen = BTreeSet::new();
        let mut current = Some(id);
        while let Some(c) = current {
            if !seen.insert(c) {
                return Err(bad("コレクション階層に循環があります"));
            }
            current = parents.get(c);
        }
    }
    let mut owned = BTreeSet::new();
    let mut owners = BTreeMap::new();
    for id in kinds.keys() {
        for (p, expected) in [
            ("dcterms:isReferencedBy", "note"),
            ("link:link", "attachment"),
        ] {
            for child in graph.refs(id, p) {
                if kinds.get(&child).is_some_and(|v| v == expected) {
                    if child == *id || matches!(kinds[id].as_str(), "note" | "attachment") {
                        return Err(bad("子ノート・子添付の参照が循環または未対応の入れ子です"));
                    }
                    if owners
                        .insert(child.clone(), id.clone())
                        .is_some_and(|prior| prior != *id)
                    {
                        return Err(bad(format!(
                            "子オブジェクト {child} が複数文献から参照されています"
                        )));
                    }
                    owned.insert(child);
                }
            }
        }
    }
    let mut claimed = BTreeSet::new();
    let folders = collection_ids
        .iter()
        .map(|id| ImportFolder {
            id: id.clone(),
            stable_id: false,
            name: graph.text(id, "dc:title").unwrap_or_else(|| id.clone()),
            parent_id: parents.get(id).cloned(),
            raw: graph.raw(id, &mut claimed),
        })
        .collect();
    let mut items = Vec::new();
    for (id, kind) in &kinds {
        if owned.contains(id) {
            continue;
        }
        let mut item_warnings = Vec::new();
        let csl = bibliography(&graph, id, kind, &mut item_warnings)?;
        let mut notes = Vec::new();
        let mut attachments = Vec::new();
        if kind == "note" {
            notes.push(note(&graph, id, &mut item_warnings));
            item_warnings.push("独立ノートを専用の文書として保持します".into());
        }
        if kind == "attachment" {
            attachments.push(attachment(&graph, id));
            item_warnings.push("独立添付を文書として保持します。論文書誌は補いません".into());
        }
        for child in graph.refs(id, "dcterms:isReferencedBy") {
            if kinds.get(&child).is_some_and(|v| v == "note") {
                notes.push(note(&graph, &child, &mut item_warnings));
            } else {
                item_warnings.push(format!("ノート参照 {child} が未解決または未対応です"));
            }
        }
        for child in graph.refs(id, "link:link") {
            if kinds.get(&child).is_some_and(|v| v == "attachment") {
                attachments.push(attachment(&graph, &child));
            } else {
                item_warnings.push(format!("添付参照 {child} が未解決または未対応です"));
            }
        }
        // A child can carry its own collection membership. Retain it on the
        // parent item, since OpenRef's folders belong to bibliographic items.
        let mut folder_ids = memberships.get(id).cloned().unwrap_or_default();
        for child in notes
            .iter()
            .map(|n| &n.id)
            .chain(attachments.iter().map(|a| &a.id))
        {
            if let Some(ids) = memberships.get(child) {
                folder_ids.extend(ids.clone());
            }
        }
        folder_ids.sort();
        folder_ids.dedup();
        let mut tags = tags(&graph, id, &mut item_warnings);
        for child in notes
            .iter()
            .map(|n| &n.id)
            .chain(attachments.iter().map(|a| &a.id))
        {
            for tag in tags_for_child(&graph, child, &mut item_warnings) {
                if !tags.iter().any(|t| t.id == tag.id) {
                    tags.push(tag);
                }
            }
        }
        items.push(ImportItem {
            id: id.clone(),
            stable_id: false,
            csl,
            date_added: None,
            raw: graph.raw(id, &mut claimed),
            folder_ids,
            tags,
            notes,
            attachments,
            warnings: item_warnings,
        });
    }
    if items.is_empty() && !graph.nodes.is_empty() {
        return Err(bad("対応する文献・ノート・添付がありません"));
    }
    for item in &mut items {
        let node_ids: Vec<_> = item.raw["resources"]
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        let read = graph.read.borrow();
        for id in node_ids {
            if let Some(node) = graph.nodes.get(&id) {
                let unused: Vec<_> = node
                    .properties
                    .keys()
                    .filter(|p| !read.contains(&(id.clone(), (*p).clone())))
                    .cloned()
                    .collect();
                if !unused.is_empty() {
                    item.warnings.push(format!(
                        "{id}: UI/引用に未反映の RDF プロパティを元情報に保持: {}",
                        unused.join(", ")
                    ));
                }
            }
        }
    }
    let unassigned: BTreeMap<_, _> = graph
        .nodes
        .iter()
        .filter(|(id, _)| !claimed.contains(*id))
        .collect();
    if !unassigned.is_empty() {
        warnings.push(format!(
            "文献に結び付かない RDF resource {} 件を原情報に保持しました",
            unassigned.len()
        ));
        if let Some(item) = items.first_mut() {
            item.raw["unassigned_resources"] = serde_json::to_value(unassigned)?;
        }
    }
    Ok(ImportInput {
        source,
        adopt_legacy_paperpile_ids: false,
        input_sha256: digest(&bytes),
        adapter_version: "zotero-rdf-v1".into(),
        items,
        folders,
        warnings,
        notices,
    })
}

fn csl_type(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "journalArticle" => "article-journal",
        "magazineArticle" => "article-magazine",
        "newspaperArticle" => "article-newspaper",
        "book" => "book",
        "bookSection" => "chapter",
        "conferencePaper" => "paper-conference",
        "thesis" => "thesis",
        "report" => "report",
        "webpage" => "webpage",
        "blogPost" => "post-weblog",
        "forumPost" => "post",
        "letter" | "email" | "instantMessage" => "personal_communication",
        "manuscript" => "manuscript",
        "interview" => "interview",
        "film" | "videoRecording" | "tvBroadcast" => "motion_picture",
        "audioRecording" | "radioBroadcast" | "podcast" => "song",
        "artwork" => "graphic",
        "map" => "map",
        "computerProgram" => "software",
        "dataset" => "dataset",
        "preprint" => "document", // CSL の article は型カタログに無く編集できない。document なら publisher・genre・number が欄にある
        "presentation" => "speech",
        "patent" => "patent",
        "statute" => "legislation",
        "bill" => "bill",
        "case" => "legal_case",
        "hearing" => "hearing",
        "encyclopediaArticle" => "entry-encyclopedia",
        "dictionaryEntry" => "entry-dictionary",
        "document" | "note" | "attachment" => "document",
        _ => return None,
    })
}
fn date(value: &str) -> Value {
    // Preserve free-form/ranges as literal instead of guessing locale or era.
    let candidate = value.split('T').next().unwrap_or(value);
    let parts: Option<Vec<u32>> = candidate
        .split('-')
        .map(|s| s.parse::<u32>().ok())
        .collect();
    if let Some(p) = parts {
        if (1..=3).contains(&p.len())
            && candidate.split('-').next().is_some_and(|s| s.len() == 4)
            && p[0] > 0
            && (p.len() < 2 || (1..=12).contains(&p[1]))
            && (p.len() < 3 || (1..=31).contains(&p[2]))
        {
            return json!({"date-parts":[p]});
        }
    }
    json!({"literal":value})
}
fn put(csl: &mut Value, field: &str, value: Option<String>) {
    if let Some(value) = value.filter(|s| !s.trim().is_empty()) {
        csl[field] = value.into();
    }
}
fn bibliography(g: &Graph, id: &str, kind: &str, warnings: &mut Vec<String>) -> Result<Value> {
    let t = csl_type(kind).unwrap_or_else(|| {
        warnings.push(format!("未対応の種別 {kind} を document として保持します"));
        "document"
    });
    let mut csl = json!({"type":t});
    put(&mut csl, "title", g.text(id, "dc:title"));
    if kind == "note" && csl.get("title").is_none() {
        csl["title"] = json!("独立ノート");
    }
    if csl.get("title").is_none() {
        warnings.push("題名は出力されていません。元レコードを保持します".into());
    }
    for (predicate, field) in [
        ("dcterms:abstract", "abstract"),
        ("bib:pages", "page"),
        ("prism:edition", "edition"),
        ("z:language", "language"),
        ("dc:rights", "rights"),
        ("dc:type", "genre"),
        ("dc:source", "source"),
        ("z:PMID", "PMID"),
        ("z:PMCID", "PMCID"),
        ("z:numberOfPages", "number-of-pages"),
        ("z:numVolumes", "number-of-volumes"),
        ("z:shortTitle", "title-short"),
        ("z:archive", "archive"),
        ("dc:coverage", "archive_location"),
        ("z:libraryCatalog", "source"),
        ("z:reportNumber", "number"),
        ("z:reportType", "genre"),
        ("z:thesisType", "genre"),
    ] {
        put(&mut csl, field, g.text(id, predicate));
    }
    if let Some(v) = g
        .text(id, "dc:date")
        .or_else(|| g.text(id, "dcterms:issued"))
    {
        csl["issued"] = date(&v);
    }
    if let Some(v) = g.text(id, "dcterms:dateSubmitted") {
        csl["accessed"] = date(&v);
    }
    // The exporter puts DOI/ISBN/ISSN/volume/issue on the container, sometimes
    // behind a bib:Part and followed by a bib:Series. Follow the graph, not text.
    let mut queue = vec![id.to_string()];
    let mut visited = BTreeSet::new();
    while let Some(current) = queue.pop() {
        if !visited.insert(current.clone()) {
            return Err(bad(format!(
                "文献 {id} のコンテナ参照に循環または重複があります"
            )));
        }
        if visited.len() > 128 {
            return Err(bad("コンテナ参照が深すぎます"));
        }
        if !g.nodes.contains_key(&current) {
            warnings.push(format!("コンテナ {current} が入力にありません"));
            continue;
        }
        let is_part = g.has_type(&current, "bib:Part");
        let is_series = g.has_type(&current, "bib:Series");
        if current != id {
            if is_part {
                put(&mut csl, "section", g.text(&current, "dc:title"));
            } else if is_series {
                put(&mut csl, "collection-title", g.text(&current, "dc:title"));
                put(
                    &mut csl,
                    "collection-number",
                    g.text(&current, "dc:identifier"),
                );
            } else {
                put(&mut csl, "container-title", g.text(&current, "dc:title"));
                put(
                    &mut csl,
                    "container-title-short",
                    g.text(&current, "dcterms:alternative"),
                );
            }
        }
        for (p, field) in [("prism:volume", "volume"), ("prism:number", "issue")] {
            if csl.get(field).is_none() {
                put(&mut csl, field, g.text(&current, p));
            }
        }
        if !is_series {
            for term in g.values(&current, "dc:identifier") {
                match term {
                    Term::Literal(v) => {
                        let mut mapped = false;
                        for prefix in ["DOI", "PMID", "PMCID", "ISBN", "ISSN"] {
                            if let Some(rest) = v
                                .strip_prefix(&format!("{prefix} "))
                                .or_else(|| v.strip_prefix(&format!("{prefix}:")))
                            {
                                if let Some(existing) = csl.get(prefix).and_then(Value::as_str) {
                                    if existing != rest.trim() {
                                        return Err(bad(format!("文献 {id} の {prefix} が矛盾しています。入力の識別子を確認してください")));
                                    }
                                } else {
                                    put(&mut csl, prefix, Some(rest.trim().into()));
                                }
                                mapped = true;
                                break;
                            }
                        }
                        if !mapped {
                            warnings.push(format!("未変換の識別子を元情報に保持: {v}"));
                        }
                    }
                    Term::Resource(r) => {
                        if g.has_type(&r, "dcterms:URI") {
                            put(&mut csl, "URL", g.text(&r, "rdf:value"));
                        } else {
                            warnings.push(format!("未対応の識別子 resource {r} を保持しました"));
                        }
                    }
                }
            }
        }
        queue.extend(g.refs(&current, "dcterms:isPartOf"));
    }
    for publisher in g.refs(id, "dc:publisher") {
        put(&mut csl, "publisher", g.text(&publisher, "foaf:name"));
        for address in g.refs(&publisher, "vcard:adr") {
            put(
                &mut csl,
                "publisher-place",
                g.text(&address, "vcard:locality"),
            );
        }
    }
    for conf in g.refs(id, "bib:presentedAt") {
        put(&mut csl, "event-title", g.text(&conf, "dc:title"));
    }
    for (p, field) in [
        ("bib:authors", "author"),
        ("bib:editors", "editor"),
        ("bib:contributors", "contributor"),
        ("z:translators", "translator"),
        ("z:bookAuthors", "container-author"),
        ("z:directors", "director"),
        ("z:interviewers", "interviewer"),
        ("z:recipients", "recipient"),
        ("z:composers", "composer"),
        ("z:inventors", "author"),
        ("z:programmers", "author"),
        ("z:artists", "author"),
        ("z:presenters", "author"),
        ("z:podcasters", "host"),
        ("z:performers", "author"),
        ("z:interviewees", "author"),
        ("z:cartographers", "author"),
        ("z:sponsors", "author"),
        ("z:seriesEditors", "collection-editor"),
        ("z:scriptwriters", "script-writer"),
        ("z:producers", "producer"),
        ("z:executiveProducers", "executive-producer"),
        ("z:wordsBy", "author"),
        ("z:reviewedAuthors", "reviewed-author"),
        ("z:creators", "author"),
        ("z:chairs", "chair"),
        ("z:castMembers", "performer"),
        ("z:guests", "guest"),
        ("z:hosts", "host"),
        ("z:narrators", "narrator"),
        ("z:originalCreators", "original-author"),
        ("z:organizers", "organizer"),
        ("z:seriesCreators", "series-creator"),
    ] {
        let mut people = Vec::new();
        for seq in g.refs(id, p) {
            let Some(node) = g.nodes.get(&seq) else {
                warnings.push(format!("著者順序の参照 {seq} がありません"));
                continue;
            };
            let mut positions: Vec<_> = node
                .properties
                .keys()
                .filter_map(|key| {
                    key.strip_prefix("rdf:_")
                        .and_then(|n| n.parse::<usize>().ok())
                        .map(|n| (n, key))
                })
                .collect();
            positions.sort_by_key(|(n, _)| *n);
            for (_, key) in positions {
                for person in g.refs(&seq, key) {
                    let family = g.text(&person, "foaf:surname");
                    let given = g.text(&person, "foaf:givenName");
                    if g.has_type(&person, "foaf:Organization") {
                        if let Some(n) = g.text(&person, "foaf:name") {
                            people.push(json!({"literal":n}));
                        }
                    } else if let Some(f) = family {
                        if let Some(v) = given {
                            people.push(json!({"family":f,"given":v}));
                        } else {
                            people.push(json!({"family":f}));
                            warnings.push("姓のみの著者は団体著者との区別が RDF に出力されません。表記を保持します".into());
                        }
                    } else if let Some(n) = g.text(&person, "foaf:name") {
                        people.push(json!({"literal":n}));
                    } else {
                        warnings.push(format!(
                            "著者 {person} の名前を取得できません。元情報を保持します"
                        ));
                    }
                }
            }
        }
        if !people.is_empty() {
            if let Some(existing) = csl[field].as_array_mut() {
                existing.extend(people);
            } else {
                csl[field] = json!(people);
            }
        }
    }
    // Extra is deliberately not parsed into fabricated citation keys or IDs.
    // Only an explicit exported citationKey property is eligible for citation.
    if let Some(key) = g.text(id, "z:citationKey") {
        csl["citation-key"] = json!(key);
    }
    Ok(csl)
}
fn tags_for_child(g: &Graph, id: &str, warnings: &mut Vec<String>) -> Vec<ImportTag> {
    tags(g, id, warnings)
}
fn tags(g: &Graph, id: &str, warnings: &mut Vec<String>) -> Vec<ImportTag> {
    let mut out = Vec::new();
    for term in g.values(id, "dc:subject") {
        let (name, automatic, raw) = match term {
            Term::Literal(v) => (v, false, json!({"manual":true})),
            Term::Resource(r) if g.has_type(&r, "z:AutomaticTag") => {
                let Some(v) = g.text(&r, "rdf:value") else {
                    warnings.push(format!("自動タグ {r} に名前がありません"));
                    continue;
                };
                (v, true, json!({"resource":r,"automatic":true}))
            }
            Term::Resource(r) => {
                warnings.push(format!(
                    "dc:subject {r} はタグとして変換できません。元情報を保持します"
                ));
                continue;
            }
        };
        if name.trim().is_empty() {
            continue;
        }
        let tag_id = format!("tag:{}", digest(name.as_bytes()));
        if out.iter().any(|t: &ImportTag| t.id == tag_id) {
            continue;
        }
        out.push(ImportTag {
            id: tag_id,
            stable_id: false,
            name: name.clone(),
            color: None,
            raw: json!({"name":name,"automatic":automatic,"source":raw}),
        });
    }
    out
}
fn attachment(g: &Graph, id: &str) -> ImportAttachment {
    let path = g
        .refs(id, "z:path")
        .into_iter()
        .next()
        .or_else(|| g.text(id, "z:path"))
        .unwrap_or_default();
    ImportAttachment {
        id: id.into(),
        stable_id: false,
        path,
        alternative_paths: vec![],
        media_type: g.text(id, "link:type"),
        raw: json!({"resource":id,"properties":g.nodes.get(id)}),
    }
}
fn note(g: &Graph, id: &str, warnings: &mut Vec<String>) -> ImportNote {
    let html = g.text(id, "rdf:value").unwrap_or_else(|| {
        warnings.push(format!("ノート {id} の本文がありません"));
        String::new()
    });
    let (content_md, losses) = super::note_html::to_markdown(&html);
    warnings.extend(losses.into_iter().map(|w| format!("ノート {id}: {w}")));
    ImportNote {
        id: id.into(),
        stable_id: false,
        content_md,
        raw: json!({"resource":id,"html":html,"properties":g.nodes.get(id)}),
        date_added: None,
        date_modified: None,
    }
}
