//! Document text extraction for RAG indexing — `/rag` funnels every
//! readable document through here before chunking. Plain text and code
//! read as-is; PDF, DOCX (Word), legacy DOC and EPUB get dedicated
//! extractors, so "most docs on disk" are one command away from memory.
//!
//! Pure Rust, offline, static-musl friendly: `pdf-extract` for PDF,
//! `zip` + `quick-xml` for the OOXML/EPUB containers, `cfb` for the
//! legacy Word binary's compound file. Extraction is best-effort by
//! design: a weird file degrades to whatever text could be pulled,
//! never to a crash.

use std::io::{Cursor, Read};
use std::path::Path;

use anyhow::{Context, Result};

/// Plain-text / code: read as-is (the allow-list /rag always had).
const TEXT_EXT: &[&str] = &[
    "md", "txt", "log", "json", "csv", "rst", "rs", "py", "js", "ts", "go", "c", "h", "cpp",
    "java", "rb", "sh", "sql", "yaml", "yml", "toml", "html", "css",
];

/// Binary formats with a dedicated extractor.
const RICH_EXT: &[&str] = &["pdf", "docx", "doc", "epub"];

/// Raw text stays small; rich documents can be fat on disk because
/// only their extracted text matters (and is capped).
const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_DOC_BYTES: u64 = 20 * 1024 * 1024;
const MAX_EXTRACT_CHARS: usize = 2 * 1024 * 1024;

#[derive(Debug)]
pub enum Extracted {
    Text(String),
    /// Recognized, but nothing indexable came out (worth reporting).
    Empty(String),
    /// Not a document /rag tries to read.
    Unsupported,
}

/// Lowercase extension without the dot, "" when there is none.
fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

pub fn is_supported(path: &Path) -> bool {
    let e = ext_of(path);
    TEXT_EXT.contains(&e.as_str()) || RICH_EXT.contains(&e.as_str())
}

/// Byte cap for reading the file, by type.
pub fn max_bytes(path: &Path) -> u64 {
    if TEXT_EXT.contains(&ext_of(path).as_str()) {
        MAX_TEXT_BYTES
    } else {
        MAX_DOC_BYTES
    }
}

/// Read + extract one document.
pub fn extract(path: &Path) -> Result<Extracted> {
    if std::fs::metadata(path)?.len() > max_bytes(path) {
        return Ok(Extracted::Empty(format!(
            "{} — larger than the indexing cap",
            path.display()
        )));
    }
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    match ext_of(path).as_str() {
        e if TEXT_EXT.contains(&e) => Ok(Extracted::Text(cap_text(
            String::from_utf8_lossy(&bytes).into_owned(),
        ))),
        "pdf" => pdf(&bytes),
        "docx" => docx(&bytes),
        "doc" => legacy_doc(&bytes),
        "epub" => epub(&bytes),
        _ => Ok(Extracted::Unsupported),
    }
}

// ---------- PDF ----------

fn pdf(bytes: &[u8]) -> Result<Extracted> {
    match pdf_extract::extract_text_from_mem(bytes) {
        Ok(t) => Ok(text_or_empty(
            cap_text(t),
            "PDF: no text layer found (it may be a scan) — OCR is not supported",
        )),
        Err(e) => Ok(Extracted::Empty(format!("PDF could not be parsed: {e}"))),
    }
}

// ---------- DOCX (Word 2007+: zip + word/document.xml) ----------

fn docx(bytes: &[u8]) -> Result<Extracted> {
    let xml = zip_read_one(bytes, |name| name == "word/document.xml").context("open DOCX")?;
    let Some(xml) = xml else {
        return Ok(Extracted::Empty("DOCX: no word/document.xml inside".into()));
    };
    Ok(text_or_empty(
        xml_text(&xml, &["p"], &[]),
        "DOCX: no text found inside",
    ))
}

// ---------- EPUB (zip + xhtml chapters) ----------

fn epub(bytes: &[u8]) -> Result<Extracted> {
    let mut out = String::new();
    let mut chapters = 0usize;
    for (name, data) in zip_entries(bytes).context("open EPUB")? {
        let n = name.to_lowercase();
        if !(n.ends_with(".xhtml") || n.ends_with(".html") || n.ends_with(".htm")) {
            continue;
        }
        chapters += 1;
        if chapters > 60 || out.len() > MAX_EXTRACT_CHARS {
            break;
        }
        out.push_str("\n\n");
        out.push_str(&xml_text(
            &data,
            &["p", "div", "h1", "h2", "h3"],
            &["style", "script"],
        ));
    }
    Ok(text_or_empty(
        cap_text(out),
        "EPUB: no readable chapters found inside",
    ))
}

// ---------- legacy DOC (Word 97: OLE compound file) ----------

fn legacy_doc(bytes: &[u8]) -> Result<Extracted> {
    let mut cfb = match cfb::CompoundFile::open(Cursor::new(bytes)) {
        Ok(c) => c,
        Err(_) => {
            return Ok(Extracted::Empty(
                "legacy .doc: not a readable Word binary (if it is really a \
                 .docx, rename the extension)"
                    .into(),
            ))
        }
    };
    let mut stream = Vec::new();
    match cfb.open_stream("WordDocument") {
        Ok(mut s) => {
            let _ = s.read_to_end(&mut stream);
        }
        Err(_) => {
            return Ok(Extracted::Empty(
                "legacy .doc: no WordDocument stream".into(),
            ))
        }
    }
    Ok(text_or_empty(
        legacy_word_text(&stream),
        "legacy .doc: no text could be pulled",
    ))
}

/// Best-effort: the classic FIB points at the text range (fcMin/fcMax),
/// but piece tables and fast-saves make it ambiguous — so decode the two
/// candidate charsets and keep whichever looks like real text, falling
/// back to a printable-run scan of the whole stream.
fn legacy_word_text(stream: &[u8]) -> String {
    if stream.len() < 0x20 {
        return String::new();
    }
    let fcmin =
        u32::from_le_bytes([stream[0x18], stream[0x19], stream[0x1A], stream[0x1B]]) as usize;
    let fcmax =
        u32::from_le_bytes([stream[0x1C], stream[0x1D], stream[0x1E], stream[0x1F]]) as usize;
    if fcmax > fcmin && fcmax <= stream.len() && fcmax - fcmin > 64 {
        let region = &stream[fcmin..fcmax];
        let latin = latin1(region);
        let utf16 = utf16le(region);
        let (a, da) = score_text(&latin);
        let (b, db) = score_text(&utf16);
        if da > 0.0 || db > 0.0 {
            return cap_text(if db > da { b } else { a });
        }
    }
    cap_text(printable_runs(stream))
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

fn utf16le(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() / 2);
    let (chunks, _) = bytes.as_chunks::<2>();
    for pair in chunks {
        let c = u16::from_le_bytes([pair[0], pair[1]]);
        if let Some(ch) = char::from_u32(c as u32) {
            s.push(ch);
        }
    }
    s
}

/// (text, letter density) — lets the legacy-DOC reader pick the codec
/// that actually carried text.
fn score_text(s: &str) -> (String, f32) {
    let letters = s.chars().filter(|c| c.is_alphanumeric()).count();
    let density = if s.is_empty() {
        0.0
    } else {
        letters as f32 / s.len() as f32
    };
    (s.to_string(), density)
}

fn printable_runs(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut run = String::new();
    for &b in bytes {
        if (0x20..0x7f).contains(&b) {
            run.push(b as char);
        } else {
            if run.len() >= 6 {
                out.push_str(&run);
                out.push('\n');
            }
            run.clear();
        }
    }
    out
}

// ---------- zip helpers ----------

fn zip_entries(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).context("open zip container")?;
    let mut out = Vec::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        if f.is_dir() {
            continue;
        }
        let name = f.name().map_err(|e| anyhow::anyhow!("{e}"))?.to_string();
        let mut data = Vec::new();
        if f.size() <= 8 * 1024 * 1024 {
            f.read_to_end(&mut data)?;
        }
        out.push((name, data));
    }
    Ok(out)
}

fn zip_read_one(bytes: &[u8], want: fn(&str) -> bool) -> Result<Option<Vec<u8>>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).context("open zip container")?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        let name = f.name().map_err(|e| anyhow::anyhow!("{e}"))?.to_string();
        if want(&name) && !f.is_dir() {
            let mut data = Vec::new();
            f.read_to_end(&mut data)?;
            return Ok(Some(data));
        }
    }
    Ok(None)
}

// ---------- XML/HTML text ----------

/// Pull readable text out of XML/HTML: text nodes in, newline per
/// paragraph-ish element, style/script blocks skipped.
fn xml_text(raw: &[u8], paras: &[&str], skip: &[&str]) -> String {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_reader(raw);
    let mut out = String::new();
    let mut skip_depth = 0usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = e.name().local_name().as_ref().to_string();
                if skip.contains(&name.as_str()) {
                    skip_depth += 1;
                }
            }
            Ok(Event::End(e)) => {
                let name = e.name().local_name().as_ref().to_string();
                if skip.contains(&name.as_str()) {
                    skip_depth = skip_depth.saturating_sub(1);
                }
                if skip_depth == 0 && paras.contains(&name.as_str()) && !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            Ok(Event::Text(t)) => {
                if skip_depth == 0 {
                    out.push_str(&quick_xml::escape::unescape(t.as_ref()).unwrap_or_default());
                }
            }
            Ok(Event::CData(t)) => {
                if skip_depth == 0 {
                    out.push_str(&t.into_inner());
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    cap_text(out)
}

fn text_or_empty(t: String, msg: &str) -> Extracted {
    if t.trim().is_empty() {
        Extracted::Empty(msg.to_string())
    } else {
        Extracted::Text(t)
    }
}

fn cap_text(t: String) -> String {
    if t.len() <= MAX_EXTRACT_CHARS {
        return t;
    }
    let mut end = MAX_EXTRACT_CHARS;
    while end > 0 && !t.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated]", &t[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zipped(name: &str, content: &[u8]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            w.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut w, content).unwrap();
        }
        buf.into_inner()
    }

    fn tmpfile(name: &str, bytes: Vec<u8>) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("dct-extract-{}-{name}", std::process::id()));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn support_matrix_and_caps() {
        assert!(is_supported(Path::new("a/report.PDF")));
        assert!(is_supported(Path::new("a/book.epub")));
        assert!(is_supported(Path::new("a/letter.docX")));
        assert!(is_supported(Path::new("a/old.doc")));
        assert!(is_supported(Path::new("server.rs")));
        assert!(!is_supported(Path::new("a/photo.jpg")));
        assert!(!is_supported(Path::new("archive.tar.gz")));
        assert_eq!(max_bytes(Path::new("a.md")), MAX_TEXT_BYTES);
        assert_eq!(max_bytes(Path::new("a.pdf")), MAX_DOC_BYTES);
    }

    #[test]
    fn text_extract_reads() {
        let p = tmpfile("notes.md", b"# Notes\n\nthe kettle is electric".to_vec());
        match extract(&p).unwrap() {
            Extracted::Text(t) => assert!(t.contains("the kettle is electric")),
            _ => panic!("plain text must extract"),
        }
        let p = tmpfile("image.bin", vec![0u8; 8]);
        assert!(matches!(extract(&p).unwrap(), Extracted::Unsupported));
    }

    #[test]
    fn docx_extract() {
        let xml = br#"<w:document xmlns:w="x"><w:body><w:p><w:r><w:t>The zigfruit</w:t></w:r><w:r><w:t> repeater hums</w:t></w:r></w:p><w:p><w:r><w:t>on port 6060</w:t></w:r></w:p></w:body></w:document>"#;
        let p = tmpfile("spec.docx", zipped("word/document.xml", xml));
        match extract(&p).unwrap() {
            Extracted::Text(t) => {
                assert!(t.contains("The zigfruit repeater hums"));
                assert!(t.contains("on port 6060"));
            }
            other => panic!("docx must extract, got {other:?}"),
        }
    }

    #[test]
    fn epub_extract() {
        let c1 = br#"<html><body><style>.x{}</style><p>Alpha chapter</p><p>brewing notes</p></body></html>"#;
        let c2 = br#"<html><body><p>Beta chapter</p></body></html>"#;
        let epub_bytes;
        {
            use std::io::Write;
            let mut buf = Cursor::new(Vec::new());
            let mut w = zip::ZipWriter::new(&mut buf);
            w.start_file("mimetype", zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(b"application/epub+zip").unwrap();
            w.start_file("ch1.xhtml", zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(c1).unwrap();
            w.start_file("ch2.xhtml", zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(c2).unwrap();
            w.finish().unwrap();
            epub_bytes = buf.into_inner();
        }
        let p = tmpfile("book.epub", epub_bytes);
        match extract(&p).unwrap() {
            Extracted::Text(t) => {
                assert!(t.contains("Alpha chapter"));
                assert!(t.contains("brewing notes"));
                assert!(t.contains("Beta chapter"));
                assert!(!t.contains(".x{}"), "style must be skipped");
            }
            other => panic!("epub must extract, got {other:?}"),
        }
    }

    #[test]
    fn docx_without_document_xml_reports_empty() {
        let p = tmpfile("broken.docx", zipped("not-what-we-expected.xml", b"<x/>"));
        match extract(&p).unwrap() {
            Extracted::Empty(msg) => assert!(msg.contains("no word/document.xml")),
            other => panic!("expected empty report, got {other:?}"),
        }
    }

    #[test]
    fn legacy_doc_decoders_score_real_text() {
        let mut stream = vec![0u8; 0x20];
        stream.extend_from_slice(b"\x00\x01the zigfruit repeater hums on port 6060\x00\x02");
        let t = printable_runs(&stream);
        assert!(t.contains("the zigfruit repeater hums on port 6060"));
        let mut u16bytes = Vec::new();
        for c in "帷幕 adapter45".encode_utf16() {
            u16bytes.extend_from_slice(&c.to_le_bytes());
        }
        let (t16, d16) = score_text(&utf16le(&u16bytes));
        assert!(t16.contains("帷幕"));
        assert!(d16 > 0.0);
    }

    #[test]
    fn xml_text_skips_style_and_breaks_paragraphs() {
        let raw = br#"<doc><p>alpha</p><p>beta</p><script>bad()</script><p>gamma</p></doc>"#;
        let t = xml_text(raw, &["p"], &["script"]);
        assert!(t.contains("alpha"));
        assert!(t.contains("beta"));
        assert!(t.contains("gamma"));
        assert!(!t.contains("bad()"));
        assert!(t.contains('\n'));
    }
}
