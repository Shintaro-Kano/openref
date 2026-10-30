use openref_core::{
    import::{
        self, apply,
        files::{AttachmentRoots, FileFingerprint},
        AttachmentMode, ImportSource,
    },
    Db,
};
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "openref-zotero-test-{}",
            openref_core::keys::new_key()
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.0.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn source() -> ImportSource {
    ImportSource {
        id: "chosen-zotero-library".into(),
        format: "zotero".into(),
    }
}
fn rdf(body: &str) -> String {
    format!(
        r#"<r:RDF xmlns:r="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:b="http://purl.org/net/biblio#" xmlns:d="http://purl.org/dc/elements/1.1/" xmlns:t="http://purl.org/dc/terms/" xmlns:z="http://www.zotero.org/namespaces/export#" xmlns:p="http://prismstandard.org/namespaces/1.2/basic/" xmlns:f="http://xmlns.com/foaf/0.1/" xmlns:l="http://purl.org/rss/1.0/modules/link/" xmlns:v="http://nwalsh.com/rdf/vCard#">{body}</r:RDF>"#
    )
}
// Synthetic format coverage derived from the official export vocabulary. This
// is not a claim that these bytes were exported by the Zotero desktop app.
const LIBRARY: &str = r##"
<b:Article r:about="#item_1">
 <z:itemType>journalArticle</z:itemType><d:title>日本語の論文</d:title><d:date>2024-03-02</d:date>
 <b:authors><r:Seq><r:li r:resource="#person2"/><r:li r:resource="#person1"/></r:Seq></b:authors>
 <b:editors><r:Seq><r:_1><f:Person><f:surname>Editor</f:surname><f:givenName>A</f:givenName></f:Person></r:_1></r:Seq></b:editors>
 <t:isPartOf><b:Part><d:title>Section</d:title><t:isPartOf r:resource="urn:issn:1234-5678"/></b:Part></t:isPartOf>
 <d:publisher><f:Organization><f:name>Publisher</f:name><v:adr><v:Address><v:locality>Tokyo</v:locality></v:Address></v:adr></f:Organization></d:publisher>
 <b:pages>1-9</b:pages><z:PMID>123456</z:PMID><d:subject>Manual</d:subject>
 <d:subject><z:AutomaticTag><r:value>Auto</r:value></z:AutomaticTag></d:subject>
 <t:isReferencedBy r:resource="#note1"/><t:isReferencedBy r:resource="#note2"/>
 <l:link r:resource="#pdf1"/><l:link r:resource="#pdf2"/><l:link r:resource="#html1"/>
 <d:relation r:resource="https://example.org/related"/><z:citationKey>Explicit2024</z:citationKey><z:unknown>preserve me</z:unknown>
</b:Article>
<f:Person r:about="#person1"><f:surname>First</f:surname><f:givenName>A</f:givenName></f:Person>
<f:Person r:about="#person2"><f:surname>Second</f:surname><f:givenName>B</f:givenName></f:Person>
<b:Journal r:about="urn:issn:1234-5678"><d:title>Fixture journal</d:title><t:alternative>F J</t:alternative><p:volume>12</p:volume><p:number>3</p:number><d:identifier>DOI 10.1234/fixture</d:identifier><d:identifier>ISSN 1234-5678</d:identifier></b:Journal>
<b:Memo r:about="#note1"><r:value>&lt;p&gt;Paragraph &amp;amp; 日本語&lt;/p&gt;&lt;ul&gt;&lt;li&gt;One&lt;/li&gt;&lt;li&gt;Two&lt;/li&gt;&lt;/ul&gt;</r:value></b:Memo>
<b:Memo r:about="#note2"><r:value>&lt;p&gt;Another note&lt;/p&gt;</r:value></b:Memo>
<z:Attachment r:about="#pdf1"><z:path r:resource="files/1/main.pdf"/><l:type>application/pdf</l:type><d:title>Main</d:title></z:Attachment>
<z:Attachment r:about="#pdf2"><z:path r:resource="files/2/supplement.pdf"/><l:type>application/pdf</l:type></z:Attachment>
<z:Attachment r:about="#html1"><z:path r:resource="files/3/snapshot.html"/><l:type>text/html</l:type></z:Attachment>
<z:Collection r:about="#collection1"><d:title>Parent/with slash</d:title><t:hasPart r:resource="#collection2"/><t:hasPart r:resource="#item_1"/></z:Collection>
<z:Collection r:about="#collection2"><d:title>Child</d:title><t:hasPart r:resource="#item_1"/></z:Collection>
<z:Collection r:about="#empty"><d:title>Empty</d:title></z:Collection>
<b:Memo r:about="#standalone"><r:value>Independent note</r:value></b:Memo>
<z:Attachment r:about="#standalonePdf"><z:path r:resource="files/4/independent.pdf"/><l:type>application/pdf</l:type><d:title>Independent PDF</d:title></z:Attachment>
<r:Description r:about="urn:unknown"><z:extension>unassigned original</z:extension></r:Description>
"##;

#[test]
fn rdf_graph_preserves_creators_collections_notes_and_all_attachments() {
    let s = Scratch::new();
    let p = s.write("export.rdf", rdf(LIBRARY).as_bytes());
    let input = import::zotero_rdf::read_input(&p, source()).unwrap();
    assert_eq!(input.items.len(), 3);
    assert_eq!(input.folders.len(), 3);
    assert_eq!(
        input
            .folders
            .iter()
            .find(|f| f.id == "#collection1")
            .unwrap()
            .name,
        "Parent/with slash"
    );
    let item = input.items.iter().find(|i| i.id == "#item_1").unwrap();
    assert_eq!(item.csl["author"][0]["family"], "Second");
    assert_eq!(item.csl["author"][1]["family"], "First");
    assert_eq!(item.csl["editor"][0]["family"], "Editor");
    assert_eq!(item.csl["container-title"], "Fixture journal");
    assert_eq!(item.csl["DOI"], "10.1234/fixture");
    assert_eq!(item.csl["PMID"], "123456");
    assert_eq!(item.csl["volume"], "12");
    assert_eq!(item.csl["publisher-place"], "Tokyo");
    assert_eq!(item.csl["issued"]["date-parts"][0][2], 2);
    assert_eq!(item.notes.len(), 2);
    assert_eq!(item.attachments.len(), 3);
    assert_eq!(item.folder_ids.len(), 2);
    assert_eq!(item.tags.len(), 2);
    assert!(item.notes[0].content_md.contains("- One\n- Two"));
    assert!(item.notes[0].content_md.contains("Paragraph & 日本語"));
    assert!(item.raw.to_string().contains("preserve me"));
    assert!(input
        .items
        .iter()
        .any(|i| i.raw.to_string().contains("unassigned original")));
    assert!(item.warnings.iter().any(|w| w.contains("z:unknown")));
    assert!(item.csl.get("unknown").is_none());
    assert!(!item.stable_id);
}

#[test]
fn zotero_copy_apply_and_repeat_preserve_all_counts_and_bytes() {
    let s = Scratch::new();
    let p = s.write("export.rdf", rdf(LIBRARY).as_bytes());
    let files = [
        s.write("files/1/main.pdf", b"%PDF-1.4 main"),
        s.write("files/2/supplement.pdf", b"%PDF-1.4 supplement"),
        s.write("files/4/independent.pdf", b"%PDF-1.4 independent"),
    ];
    let original: Vec<_> = files
        .iter()
        .map(|p| FileFingerprint::read(p).unwrap())
        .collect();
    let input = import::zotero_rdf::read_input(&p, source()).unwrap();
    let roots = AttachmentRoots::new(std::slice::from_ref(&s.0)).unwrap();
    let db_path = s.0.join("app/openref.sqlite");
    for _ in 0..2 {
        let plan = apply::prepare(&db_path, &input, &roots, AttachmentMode::Copy).unwrap();
        assert_eq!(plan.counts.available_pdfs, 3);
        assert_eq!(plan.counts.unsupported_attachments, 1);
        let report = apply::execute(
            &db_path,
            &p,
            &input,
            &plan,
            &roots,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(report.state, "finished");
        assert!(!report.complete);
    }
    let db = Db::open(&db_path).unwrap();
    let (key, csl): (String, String) = db
        .conn
        .query_row(
            "SELECT key,csl FROM items WHERE json_extract(csl,'$.\"citation-key\"') IS NOT NULL",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        openref_core::bibtex::citekey(&serde_json::from_str(&csl).unwrap(), &key),
        "Explicit2024"
    );
    for (table, count) in [
        ("items", 3),
        ("attachments", 3),
        ("notes", 3),
        ("tags", 2),
        ("collections", 3),
    ] {
        let actual: i64 = db
            .conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(actual, count, "{table}");
    }
    let paths: Vec<String> = db
        .conn
        .prepare("SELECT path FROM attachments")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for (source, fp) in files.iter().zip(&original) {
        fp.verify(source).unwrap();
        assert!(paths
            .iter()
            .any(|p| FileFingerprint::read(Path::new(p)).unwrap() == *fp));
    }
}

#[test]
fn malformed_graph_external_entities_and_collection_cycles_are_rejected() {
    let s = Scratch::new();
    for body in [
        r##"<z:Collection r:about="#a"><t:hasPart r:resource="#b"/></z:Collection><z:Collection r:about="#b"><t:hasPart r:resource="#a"/></z:Collection>"##,
        r##"<b:Book r:about="#a"><t:isPartOf r:resource="#a"/></b:Book>"##,
        r##"<b:Book r:about="#a"><d:title r:parseType="Collection"/></b:Book>"##,
    ] {
        let p = s.write("bad.rdf", rdf(body).as_bytes());
        assert!(import::zotero_rdf::read_input(&p, source()).is_err());
    }
    let xxe = format!(
        "<!DOCTYPE rdf [<!ENTITY secret SYSTEM 'file:///etc/passwd'>]>{}",
        rdf("<b:Book r:about='a'><d:title>&secret;</d:title></b:Book>")
    );
    let p = s.write("xxe.rdf", xxe.as_bytes());
    assert!(import::zotero_rdf::read_input(&p, source()).is_err());
}

#[test]
#[ignore = "explicit path to the public official Zotero test artifact; no production DB"]
fn official_zotero_rdf_artifact() {
    let p = std::env::var_os("OPENREF_TEST_ZOTERO_RDF").expect("set official zotero_rdf.xml path");
    let input = import::zotero_rdf::read_input(Path::new(&p), source()).unwrap();
    assert_eq!(input.items.len(), 2);
    let book = input
        .items
        .iter()
        .find(|i| i.csl["type"] == "book")
        .unwrap();
    assert_eq!(book.notes.len(), 2);
    assert_eq!(book.csl["ISBN"], "1-4214-0283-1");
    let web = input
        .items
        .iter()
        .find(|i| i.csl["type"] == "webpage")
        .unwrap();
    assert_eq!(web.csl["URL"], "http://example.com");
}

#[test]
fn book_containers_are_not_items_but_explicit_standalone_books_are() {
    for identifier in [
        "",
        "<d:identifier>ISBN 1234567890</d:identifier>",
        "<d:identifier>DOI 10.1234/chapter</d:identifier>",
    ] {
        let s = Scratch::new();
        let p=s.write("chapter.rdf",rdf(&format!(r##"<b:BookSection r:about="#chapter"><z:itemType>bookSection</z:itemType><d:title>Chapter</d:title><t:isPartOf r:nodeID="book"/></b:BookSection><b:Book r:nodeID="book"><d:title>Container</d:title>{identifier}</b:Book><b:Book r:about="#standalone"><z:itemType>book</z:itemType><d:title>Standalone</d:title></b:Book>"##)).as_bytes());
        let input = import::zotero_rdf::read_input(&p, source()).unwrap();
        assert_eq!(input.items.len(), 2);
        assert!(input.items.iter().all(|i| i.id != "_:book"));
        assert_eq!(
            input.items.iter().find(|i| i.id == "#chapter").unwrap().csl["container-title"],
            "Container"
        );
        let plan = apply::prepare(
            &s.0.join("app/library.sqlite"),
            &input,
            &AttachmentRoots::new(&[]).unwrap(),
            AttachmentMode::Copy,
        )
        .unwrap();
        assert_eq!(plan.counts.new_items, 2);
        assert_eq!(plan.counts.review, 0);
    }
}

#[test]
fn n8_html_notes_keep_scientific_plain_text_without_markdown_escapes() {
    let s = Scratch::new();
    let path=s.write("plain.rdf",rdf(r##"<b:Memo r:about="#n"><r:value>&lt;p&gt;p&amp;lt;0.05; HU &amp;gt; 30; a*b; snake_case; [ref]&lt;/p&gt;</r:value></b:Memo>"##).as_bytes());
    let input = import::zotero_rdf::read_input(&path, source()).unwrap();
    assert_eq!(
        input.items[0].notes[0].content_md,
        "p<0.05; HU > 30; a*b; snake_case; [ref]"
    );
}

#[test]
fn n9_primary_creators_and_series_editors_are_preserved() {
    for (role, kind, field) in [
        ("inventors", "patent", "author"),
        ("programmers", "computerProgram", "author"),
        ("artists", "artwork", "author"),
        ("presenters", "presentation", "author"),
        ("podcasters", "podcast", "host"),
        ("performers", "audioRecording", "author"),
        ("seriesEditors", "book", "collection-editor"),
    ] {
        let s = Scratch::new();
        let body = format!(
            r##"<r:Description r:about="#i"><z:itemType>{kind}</z:itemType><d:title>Roles</d:title><z:{role}><r:Seq><r:li><f:Person><f:surname>First</f:surname></f:Person></r:li><r:li><f:Person><f:surname>Second</f:surname></f:Person></r:li></r:Seq></z:{role}></r:Description>"##
        );
        let path = s.write("roles.rdf", rdf(&body).as_bytes());
        let input = import::zotero_rdf::read_input(&path, source()).unwrap();
        assert_eq!(input.items[0].csl[field][0]["family"], "First", "{role}");
        assert_eq!(input.items[0].csl[field][1]["family"], "Second", "{role}");
    }
}
