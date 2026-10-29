# Paperpile array export: observed 2026-10-27

`library.json` retains the structure of a real, user-supplied Paperpile web
export. All textual record values have been replaced with test values, except
the format identifiers `PP_ARTICLE` and `application/pdf`. Dates and numeric
metadata are also replaced. This is a structure fixture, not a real library.
No original PDF or original bibliographic record is included.

Observed original: 3 journal articles; 2 attached PDFs; author counts 10/5/3;
empty `folders` and `labels`; no `notes` or `foldersNamed` field. The Paperpile
web application did not expose a version number in the supplied export.

The original JSON used `All Papers/<letter>/<filename>.pdf`, while the original
ZIP used `Paperpile files/<letter>/<filename>.pdf`. Tests preserve that exact
relative-layout relationship with replacement filenames. A test may create
tiny `%PDF-` header files: these only validate path resolution and hashing,
not PDF validity, annotations, extraction, or viewer behavior.

A second export of the same original three records was byte-identical, including
item and attachment IDs. This does not establish ID stability after edits or
across libraries. `tagged-library.json` retains the structure of a separate
three-record real export, with all values replaced: each has one `labels` ID
and the corresponding `labelsNamed` string. All three share the same label.
The committed fixture uses the replacement label `Fixture label`.
All `folders` arrays were empty even in the tagged export. No notes were present.

Not established: stable IDs after changes, full-library ZIP coverage,
folder/note formats, empty folders, supplementary PDFs, independent notes
or PDFs, custom organization settings, embedded annotation rendering.

The ignored `current_paperpile_real_export_preview` test accepts explicit
`OPENREF_TEST_PAPERPILE_JSON` and `OPENREF_TEST_PAPERPILE_FILES` paths. It is only
for the original 3-record/2-PDF sample. It never discovers a production DB.
