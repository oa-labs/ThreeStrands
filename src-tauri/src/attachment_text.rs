//! Plain text from attachments the user shares with Thread Chat.
//!
//! Attachments come from arbitrary senders, so every format is parsed under
//! fixed limits: the file size, the decompressed size of each Office part and
//! of the whole archive, the number of parts read, and the characters kept.
//! Callers run `extract` off the async runtime with a timeout, so a parser
//! that loops or panics on a hostile file fails only that question.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::sync::Mutex;
use std::time::Duration;

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::limits::MAX_ATTACHMENT_BYTES;

/// Most attachments one question may share.
pub(crate) const MAX_CHAT_ATTACHMENTS: usize = 4;
/// Characters of extracted text kept for each attachment.
pub(crate) const MAX_ATTACHMENT_TEXT_CHARS: usize = 24_000;
/// Longest one extraction may run before the question fails.
pub(crate) const ATTACHMENT_EXTRACT_TIMEOUT: Duration = Duration::from_secs(20);
/// Largest decompressed XML part read from an Office file.
const MAX_OFFICE_PART_BYTES: u64 = 16 * 1024 * 1024;
/// Largest total decompressed size read from one Office file.
const MAX_OFFICE_TOTAL_BYTES: u64 = 48 * 1024 * 1024;
/// Most slides or worksheets read from one Office file.
const MAX_OFFICE_PARTS: usize = 200;
/// Extracted attachments remembered for follow-up questions.
const CACHE_ENTRIES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentKind {
    Text,
    Pdf,
    Docx,
    Xlsx,
    Pptx,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtractedText {
    pub text: String,
    /// The file held more text than `MAX_ATTACHMENT_TEXT_CHARS`.
    pub truncated: bool,
}

/// The readable format of an attachment, judged by its extension and then
/// its declared type, or `None` when chat cannot read it.
pub(crate) fn kind_for(filename: &str, mime_type: &str) -> Option<DocumentKind> {
    let extension = filename.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase());
    let by_extension = match extension.as_deref() {
        Some("txt" | "text" | "csv" | "tsv" | "md" | "markdown" | "log") => Some(DocumentKind::Text),
        Some("pdf") => Some(DocumentKind::Pdf),
        Some("docx") => Some(DocumentKind::Docx),
        Some("xlsx") => Some(DocumentKind::Xlsx),
        Some("pptx") => Some(DocumentKind::Pptx),
        _ => None,
    };
    by_extension.or_else(|| {
        let essence = mime_type.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
        match essence.as_str() {
            "text/plain" | "text/csv" | "text/tab-separated-values" | "text/markdown" => Some(DocumentKind::Text),
            "application/pdf" => Some(DocumentKind::Pdf),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => Some(DocumentKind::Docx),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => Some(DocumentKind::Xlsx),
            "application/vnd.openxmlformats-officedocument.presentationml.presentation" => Some(DocumentKind::Pptx),
            _ => None,
        }
    })
}

/// Extracts the text of an attachment, keeping at most
/// `MAX_ATTACHMENT_TEXT_CHARS`. A file with no readable text is an error so
/// the user learns the AI could not see it.
pub(crate) fn extract(kind: DocumentKind, bytes: &[u8]) -> Result<ExtractedText, String> {
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err("it is too large to read".to_string());
    }
    let text = match kind {
        DocumentKind::Text => decode_text(bytes),
        DocumentKind::Pdf => pdf_text(bytes)?,
        DocumentKind::Docx => docx_text(bytes)?,
        DocumentKind::Xlsx => xlsx_text(bytes)?,
        DocumentKind::Pptx => pptx_text(bytes)?,
    };
    let text = tidy(&text);
    if text.is_empty() {
        return Err(match kind {
            DocumentKind::Pdf => "it has no readable text (it may be a scanned image)",
            _ => "it has no readable text",
        }
        .to_string());
    }
    let truncated = text.chars().count() > MAX_ATTACHMENT_TEXT_CHARS;
    let text = if truncated { text.chars().take(MAX_ATTACHMENT_TEXT_CHARS).collect() } else { text };
    Ok(ExtractedText { text, truncated })
}

fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(rest, u16::from_be_bytes);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

fn decode_utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let units = bytes.chunks_exact(2).map(|pair| unit([pair[0], pair[1]]));
    char::decode_utf16(units).map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER)).collect()
}

/// Normalizes line endings, drops control characters other than tab and
/// newline, trims trailing spaces, and collapses runs of blank lines.
fn tidy(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(normalized.len());
    let mut blank_run = 0;
    for line in normalized.split('\n') {
        let line: String = line.chars().filter(|ch| *ch == '\t' || !ch.is_control()).collect();
        let line = line.trim_end();
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

fn pdf_text(bytes: &[u8]) -> Result<String, String> {
    let pages = pdf_extract::extract_text_from_mem_by_pages(bytes).map_err(|error| {
        let detail = error.to_string();
        if detail.to_ascii_lowercase().contains("encrypt") {
            "it is password protected".to_string()
        } else {
            "it isn't a readable PDF".to_string()
        }
    })?;
    let mut out = String::new();
    for (index, page) in pages.iter().enumerate() {
        if page.trim().is_empty() {
            continue;
        }
        if pages.len() > 1 {
            out.push_str(&format!("[Page {}]\n", index + 1));
        }
        out.push_str(page.trim());
        out.push_str("\n\n");
        if out.chars().count() > MAX_ATTACHMENT_TEXT_CHARS {
            break;
        }
    }
    Ok(out)
}

/// An Office file opened with a budget for how much it may decompress.
struct OfficeArchive {
    archive: zip::ZipArchive<Cursor<Vec<u8>>>,
    remaining: u64,
}

impl OfficeArchive {
    fn open(bytes: &[u8]) -> Result<Self, String> {
        let archive = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).map_err(|_| "it isn't a readable Office file".to_string())?;
        Ok(Self { archive, remaining: MAX_OFFICE_TOTAL_BYTES })
    }

    fn names(&self) -> Vec<String> {
        self.archive.file_names().map(str::to_string).collect()
    }

    /// Reads one part as text, or `None` when the archive lacks it.
    fn part(&mut self, name: &str) -> Result<Option<String>, String> {
        let entry = match self.archive.by_name(name) {
            Ok(entry) => entry,
            Err(zip::result::ZipError::FileNotFound) => return Ok(None),
            Err(_) => return Err("it isn't a readable Office file".to_string()),
        };
        let limit = MAX_OFFICE_PART_BYTES.min(self.remaining);
        let mut data = Vec::new();
        entry
            .take(limit + 1)
            .read_to_end(&mut data)
            .map_err(|_| "it isn't a readable Office file".to_string())?;
        if data.len() as u64 > limit {
            return Err("it expands to more data than can be read".to_string());
        }
        self.remaining -= data.len() as u64;
        Ok(Some(String::from_utf8_lossy(&data).into_owned()))
    }

    fn required(&mut self, name: &str) -> Result<String, String> {
        self.part(name)?.ok_or_else(|| "it isn't a readable Office file".to_string())
    }
}

/// Parts named `{prefix}{number}.xml`, in numeric order.
fn numbered_parts(names: &[String], prefix: &str) -> Vec<String> {
    let mut parts: Vec<(u32, String)> = names
        .iter()
        .filter_map(|name| {
            let number = name.strip_prefix(prefix)?.strip_suffix(".xml")?.parse().ok()?;
            Some((number, name.clone()))
        })
        .collect();
    parts.sort();
    parts.into_iter().map(|(_, name)| name).collect()
}

fn xml_error() -> String {
    "it isn't a readable Office file".to_string()
}

fn push_reference(out: &mut String, reference: &str) {
    if let Some(number) = reference.strip_prefix('#') {
        let value = match number.strip_prefix('x').or_else(|| number.strip_prefix('X')) {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => number.parse().ok(),
        };
        if let Some(ch) = value.and_then(char::from_u32) {
            out.push(ch);
        }
        return;
    }
    out.push_str(match reference {
        "lt" => "<",
        "gt" => ">",
        "amp" => "&",
        "apos" => "'",
        "quot" => "\"",
        _ => "",
    });
}

/// Text from WordprocessingML or DrawingML: runs inside `text_tag`, a line
/// break after each `paragraph_tag`, and tabs and breaks as whitespace.
fn markup_text(xml: &str, text_tag: &str, paragraph_tag: &str) -> Result<String, String> {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event().map_err(|_| xml_error())? {
            Event::Start(element) if element.local_name().as_ref() == text_tag => in_text = true,
            Event::End(element) if element.local_name().as_ref() == text_tag => in_text = false,
            Event::End(element) if element.local_name().as_ref() == paragraph_tag => out.push('\n'),
            Event::Empty(element) => match element.local_name().as_ref() {
                "tab" => out.push('\t'),
                "br" | "cr" => out.push('\n'),
                _ => {}
            },
            Event::Text(text) if in_text => out.push_str(&text),
            Event::CData(text) if in_text => out.push_str(&text),
            Event::GeneralRef(reference) if in_text => push_reference(&mut out, &reference),
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

fn docx_text(bytes: &[u8]) -> Result<String, String> {
    let mut archive = OfficeArchive::open(bytes)?;
    let document = archive.required("word/document.xml")?;
    markup_text(&document, "t", "p")
}

fn pptx_text(bytes: &[u8]) -> Result<String, String> {
    let mut archive = OfficeArchive::open(bytes)?;
    let slides = numbered_parts(&archive.names(), "ppt/slides/slide");
    if slides.is_empty() {
        return Err(xml_error());
    }
    let mut out = String::new();
    for (index, slide) in slides.iter().take(MAX_OFFICE_PARTS).enumerate() {
        let xml = archive.required(slide)?;
        out.push_str(&format!("[Slide {}]\n", index + 1));
        out.push_str(markup_text(&xml, "t", "p")?.trim());
        out.push_str("\n\n");
        if out.chars().count() > MAX_ATTACHMENT_TEXT_CHARS {
            break;
        }
    }
    Ok(out)
}

fn attribute(element: &quick_xml::events::BytesStart<'_>, name: &str) -> Option<String> {
    element
        .try_get_attribute(name)
        .ok()
        .flatten()
        .and_then(|value| value.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok().map(|value| value.into_owned()))
}

/// Shared strings, each the concatenation of its `t` runs.
fn shared_strings(xml: &str) -> Result<Vec<String>, String> {
    let mut reader = Reader::from_str(xml);
    let mut strings = Vec::new();
    let mut current = String::new();
    let mut in_text = false;
    // Phonetic runs repeat the reading of East Asian text, not its content.
    let mut in_phonetic = false;
    loop {
        match reader.read_event().map_err(|_| xml_error())? {
            Event::Start(element) => match element.local_name().as_ref() {
                "si" => current.clear(),
                "t" => in_text = true,
                "rPh" => in_phonetic = true,
                _ => {}
            },
            Event::End(element) => match element.local_name().as_ref() {
                "si" => strings.push(std::mem::take(&mut current)),
                "t" => in_text = false,
                "rPh" => in_phonetic = false,
                _ => {}
            },
            Event::Empty(element) if element.local_name().as_ref() == "si" => strings.push(String::new()),
            Event::Text(text) if in_text && !in_phonetic => current.push_str(&text),
            Event::GeneralRef(reference) if in_text && !in_phonetic => push_reference(&mut current, &reference),
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(strings)
}

/// Worksheet names and their part paths, in workbook order.
fn worksheets(archive: &mut OfficeArchive) -> Result<Vec<(String, String)>, String> {
    let mut targets = HashMap::new();
    if let Some(rels) = archive.part("xl/_rels/workbook.xml.rels")? {
        let mut reader = Reader::from_str(&rels);
        loop {
            match reader.read_event().map_err(|_| xml_error())? {
                Event::Start(element) | Event::Empty(element) if element.local_name().as_ref() == "Relationship" => {
                    if let (Some(id), Some(target)) = (attribute(&element, "Id"), attribute(&element, "Target")) {
                        let path = match target.strip_prefix('/') {
                            Some(absolute) => absolute.to_string(),
                            None => format!("xl/{target}"),
                        };
                        targets.insert(id, path);
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
    }
    let workbook = archive.required("xl/workbook.xml")?;
    let mut reader = Reader::from_str(&workbook);
    let mut sheets = Vec::new();
    loop {
        match reader.read_event().map_err(|_| xml_error())? {
            Event::Start(element) | Event::Empty(element) if element.local_name().as_ref() == "sheet" => {
                let name = attribute(&element, "name").unwrap_or_default();
                let path = attribute(&element, "r:id").and_then(|id| targets.get(&id).cloned());
                if let Some(path) = path {
                    sheets.push((name, path));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if sheets.is_empty() {
        // Some writers omit relationships; fall back to the sheet parts.
        sheets = numbered_parts(&archive.names(), "xl/worksheets/sheet")
            .into_iter()
            .enumerate()
            .map(|(index, path)| (format!("Sheet{}", index + 1), path))
            .collect();
    }
    Ok(sheets)
}

/// One worksheet as tab-separated rows of cell values.
fn worksheet_text(xml: &str, strings: &[String]) -> Result<String, String> {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell_type = String::new();
    let mut value = String::new();
    let mut in_value = false;
    let mut in_inline = false;
    loop {
        match reader.read_event().map_err(|_| xml_error())? {
            Event::Start(element) => match element.local_name().as_ref() {
                "row" => row.clear(),
                "c" => {
                    cell_type = attribute(&element, "t").unwrap_or_default();
                    value.clear();
                }
                "v" => in_value = true,
                "is" => in_inline = true,
                "t" if in_inline => in_value = true,
                _ => {}
            },
            Event::End(element) => match element.local_name().as_ref() {
                "v" | "t" => in_value = false,
                "is" => in_inline = false,
                "c" => {
                    let shown = match cell_type.as_str() {
                        "s" => value.trim().parse::<usize>().ok().and_then(|index| strings.get(index).cloned()).unwrap_or_default(),
                        "b" => if value.trim() == "1" { "TRUE".to_string() } else { "FALSE".to_string() },
                        _ => value.clone(),
                    };
                    row.push(shown.replace(['\t', '\n', '\r'], " "));
                }
                "row" => {
                    while row.last().is_some_and(|cell| cell.is_empty()) {
                        row.pop();
                    }
                    if !row.is_empty() {
                        out.push_str(&row.join("\t"));
                        out.push('\n');
                    }
                }
                _ => {}
            },
            Event::Text(text) if in_value => value.push_str(&text),
            Event::GeneralRef(reference) if in_value => push_reference(&mut value, &reference),
            Event::Eof => break,
            _ => {}
        }
        if out.chars().count() > MAX_ATTACHMENT_TEXT_CHARS {
            break;
        }
    }
    Ok(out)
}

fn xlsx_text(bytes: &[u8]) -> Result<String, String> {
    let mut archive = OfficeArchive::open(bytes)?;
    let strings = match archive.part("xl/sharedStrings.xml")? {
        Some(xml) => shared_strings(&xml)?,
        None => Vec::new(),
    };
    let sheets = worksheets(&mut archive)?;
    if sheets.is_empty() {
        return Err(xml_error());
    }
    let mut out = String::new();
    for (name, path) in sheets.into_iter().take(MAX_OFFICE_PARTS) {
        let Some(xml) = archive.part(&path)? else { continue };
        out.push_str(&format!("[Sheet: {name}]\n"));
        out.push_str(worksheet_text(&xml, &strings)?.trim_end());
        out.push_str("\n\n");
        if out.chars().count() > MAX_ATTACHMENT_TEXT_CHARS {
            break;
        }
    }
    Ok(out)
}

/// An extracted attachment with the filename and type it was read under.
pub(crate) type CachedAttachment = (String, String, ExtractedText);

/// Recently extracted attachments, keyed by message and attachment id, so
/// follow-up questions don't download and parse the same file again. It
/// lives in memory only.
#[derive(Default)]
pub(crate) struct ExtractCache {
    entries: Mutex<Vec<((String, String), CachedAttachment)>>,
}

impl ExtractCache {
    pub(crate) fn get(&self, message_id: &str, attachment_id: &str) -> Option<CachedAttachment> {
        let entries = self.entries.lock().ok()?;
        entries
            .iter()
            .find(|((message, attachment), _)| message == message_id && attachment == attachment_id)
            .map(|(_, cached)| cached.clone())
    }

    pub(crate) fn insert(&self, message_id: &str, attachment_id: &str, cached: CachedAttachment) {
        let Ok(mut entries) = self.entries.lock() else { return };
        entries.retain(|((message, attachment), _)| !(message == message_id && attachment == attachment_id));
        if entries.len() >= CACHE_ENTRIES {
            entries.remove(0);
        }
        entries.push(((message_id.to_string(), attachment_id.to_string()), cached));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn office(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in parts {
            writer.start_file(*name, SimpleFileOptions::default()).unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    /// A PDF whose pages each draw the given content stream with Helvetica,
    /// with a correct cross-reference table.
    fn pdf(page_streams: &[&str]) -> Vec<u8> {
        let page_count = page_streams.len();
        // Objects: 1 catalog, 2 pages, 3 font, then a page and a content
        // stream for each page.
        let kids: Vec<String> = (0..page_count).map(|index| format!("{} 0 R", 4 + index * 2)).collect();
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            format!("<< /Type /Pages /Kids [{}] /Count {page_count} >>", kids.join(" ")),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        ];
        for (index, stream) in page_streams.iter().enumerate() {
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
                5 + index * 2
            ));
            objects.push(format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()));
        }
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
        out
    }

    #[test]
    fn recognizes_formats_by_extension_before_declared_type() {
        assert_eq!(kind_for("Q3 Report.PDF", "application/octet-stream"), Some(DocumentKind::Pdf));
        assert_eq!(kind_for("notes.md", "application/octet-stream"), Some(DocumentKind::Text));
        assert_eq!(kind_for("budget.xlsx", ""), Some(DocumentKind::Xlsx));
        assert_eq!(kind_for("deck.pptx", ""), Some(DocumentKind::Pptx));
        assert_eq!(kind_for("letter.docx", ""), Some(DocumentKind::Docx));
        assert_eq!(kind_for("attachment", "text/csv; charset=utf-8"), Some(DocumentKind::Text));
        assert_eq!(kind_for("scan", "application/pdf"), Some(DocumentKind::Pdf));
    }

    #[test]
    fn rejects_formats_chat_cannot_read() {
        for (name, mime) in [
            ("photo.jpg", "image/jpeg"),
            ("page.html", "text/html"),
            ("invite.ics", "text/calendar"),
            ("legacy.doc", "application/msword"),
            ("macro.xlsm", "application/octet-stream"),
            ("archive.zip", "application/zip"),
        ] {
            assert_eq!(kind_for(name, mime), None, "{name}");
        }
    }

    #[test]
    fn decodes_utf8_and_utf16_text_and_tidies_it() {
        let utf8 = extract(DocumentKind::Text, "\u{FEFF}Line one\r\nLine two  \r\n\r\n\r\n\r\nDone\u{0007}".as_bytes()).unwrap();
        assert_eq!(utf8.text, "Line one\nLine two\n\nDone");
        assert!(!utf8.truncated);

        let mut utf16 = vec![0xFF, 0xFE];
        for unit in "Größe\tok".encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(extract(DocumentKind::Text, &utf16).unwrap().text, "Größe\tok");
    }

    #[test]
    fn keeps_text_up_to_the_limit_and_marks_longer_files_truncated() {
        let below = "a".repeat(MAX_ATTACHMENT_TEXT_CHARS - 1);
        let exact = "a".repeat(MAX_ATTACHMENT_TEXT_CHARS);
        let above = "a".repeat(MAX_ATTACHMENT_TEXT_CHARS + 1);
        let below = extract(DocumentKind::Text, below.as_bytes()).unwrap();
        assert_eq!((below.text.chars().count(), below.truncated), (MAX_ATTACHMENT_TEXT_CHARS - 1, false));
        let exact = extract(DocumentKind::Text, exact.as_bytes()).unwrap();
        assert_eq!((exact.text.chars().count(), exact.truncated), (MAX_ATTACHMENT_TEXT_CHARS, false));
        let above = extract(DocumentKind::Text, above.as_bytes()).unwrap();
        assert_eq!((above.text.chars().count(), above.truncated), (MAX_ATTACHMENT_TEXT_CHARS, true));
    }

    #[test]
    fn refuses_files_over_the_attachment_size_limit() {
        let exact = vec![b'a'; MAX_ATTACHMENT_BYTES];
        assert!(extract(DocumentKind::Text, &exact).is_ok());
        let above = vec![b'a'; MAX_ATTACHMENT_BYTES + 1];
        assert_eq!(extract(DocumentKind::Text, &above).unwrap_err(), "it is too large to read");
    }

    #[test]
    fn reports_empty_files_instead_of_sharing_nothing() {
        assert_eq!(extract(DocumentKind::Text, b" \n\r\n ").unwrap_err(), "it has no readable text");
    }

    #[test]
    fn extracts_pdf_text_from_single_and_multi_page_documents() {
        let single = extract(DocumentKind::Pdf, &pdf(&["BT /F1 12 Tf 72 700 Td (Invoice total: 1,250 USD) Tj ET"])).unwrap();
        assert!(single.text.contains("Invoice total: 1,250 USD"), "{}", single.text);
        assert!(!single.text.contains("[Page"));

        let multi = extract(
            DocumentKind::Pdf,
            &pdf(&[
                "BT /F1 12 Tf 72 700 Td [(Due) -250 (date)] TJ ET",
                "BT /F1 12 Tf 72 700 Td (Signed by both parties) Tj ET",
            ]),
        )
        .unwrap();
        assert!(multi.text.contains("[Page 1]") && multi.text.contains("[Page 2]"), "{}", multi.text);
        assert!(multi.text.contains("Signed by both parties"), "{}", multi.text);
    }

    #[test]
    fn reports_pdfs_without_text_and_malformed_pdfs() {
        let image_only = pdf(&["q 100 0 0 100 0 0 cm Q"]);
        assert_eq!(
            extract(DocumentKind::Pdf, &image_only).unwrap_err(),
            "it has no readable text (it may be a scanned image)"
        );
        assert!(extract(DocumentKind::Pdf, b"%PDF-1.4\nnot really a pdf").is_err());
        assert!(extract(DocumentKind::Pdf, b"").is_err());
    }

    #[test]
    fn extracts_word_paragraphs_tabs_breaks_and_entities() {
        let document = r#"<?xml version="1.0"?><w:document xmlns:w="w"><w:body>
            <w:p><w:r><w:t>Scope &amp; terms</w:t></w:r></w:p>
            <w:p><w:r><w:t xml:space="preserve">Start: </w:t></w:r><w:r><w:t>May 1</w:t><w:tab/><w:t>End: June 30</w:t><w:br/><w:t>Fee &#8364;500</w:t></w:r></w:p>
            <w:p><w:pPr><w:rPr><w:b/></w:rPr></w:pPr></w:p>
        </w:body></w:document>"#;
        let text = extract(DocumentKind::Docx, &office(&[("word/document.xml", document)])).unwrap().text;
        assert_eq!(text, "Scope & terms\nStart: May 1\tEnd: June 30\nFee €500");
    }

    #[test]
    fn extracts_slides_in_numeric_order() {
        let slide = |text: &str| format!(r#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p><a:p><a:r><a:t>Owner: Sam</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#);
        let (one, two, ten) = (slide("Roadmap"), slide("Budget"), slide("Questions"));
        let bytes = office(&[
            ("ppt/slides/slide10.xml", ten.as_str()),
            ("ppt/slides/slide2.xml", two.as_str()),
            ("ppt/slides/slide1.xml", one.as_str()),
            ("ppt/slides/_rels/slide1.xml.rels", "<Relationships/>"),
        ]);
        let text = extract(DocumentKind::Pptx, &bytes).unwrap().text;
        let order: Vec<usize> = ["Roadmap", "Budget", "Questions"].iter().map(|word| text.find(word).unwrap()).collect();
        assert!(order[0] < order[1] && order[1] < order[2], "{text}");
        assert!(text.starts_with("[Slide 1]\nRoadmap\nOwner: Sam"), "{text}");
    }

    #[test]
    fn extracts_spreadsheet_cells_by_sheet_with_shared_and_inline_strings() {
        let workbook = r#"<workbook xmlns:r="r"><sheets><sheet name="Summary" sheetId="1" r:id="rId2"/><sheet name="Raw &amp; Notes" sheetId="2" r:id="rId1"/></sheets></workbook>"#;
        let rels = r#"<Relationships><Relationship Id="rId1" Target="worksheets/sheet2.xml"/><Relationship Id="rId2" Target="/xl/worksheets/sheet1.xml"/></Relationships>"#;
        let strings = r#"<sst><si><t>Item</t></si><si><r><t>Total </t></r><r><t>cost</t></r><rPh><t>ignored</t></rPh></si><si/></sst>"#;
        let summary = r#"<worksheet><sheetData>
            <row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>
            <row r="2"><c r="A2" t="inlineStr"><is><t>Laptops</t></is></c><c r="B2"><v>4200.5</v></c><c r="C2" t="b"><v>1</v></c></row>
            <row r="3"><c r="A3" t="s"><v>2</v></c></row>
        </sheetData></worksheet>"#;
        let raw = r#"<worksheet><sheetData><row r="1"><c r="A1" t="str"><v>a &lt; b</v></c></row></sheetData></worksheet>"#;
        let bytes = office(&[
            ("xl/workbook.xml", workbook),
            ("xl/_rels/workbook.xml.rels", rels),
            ("xl/sharedStrings.xml", strings),
            ("xl/worksheets/sheet1.xml", summary),
            ("xl/worksheets/sheet2.xml", raw),
        ]);
        let text = extract(DocumentKind::Xlsx, &bytes).unwrap().text;
        assert_eq!(text, "[Sheet: Summary]\nItem\tTotal cost\nLaptops\t4200.5\tTRUE\n\n[Sheet: Raw & Notes]\na < b");
    }

    #[test]
    fn finds_worksheets_when_the_workbook_has_no_relationships() {
        let bytes = office(&[
            ("xl/workbook.xml", "<workbook><sheets/></workbook>"),
            ("xl/worksheets/sheet1.xml", r#"<worksheet><sheetData><row><c t="inlineStr"><is><t>Only sheet</t></is></c></row></sheetData></worksheet>"#),
        ]);
        assert_eq!(extract(DocumentKind::Xlsx, &bytes).unwrap().text, "[Sheet: Sheet1]\nOnly sheet");
    }

    #[test]
    fn rejects_malformed_office_files() {
        assert_eq!(extract(DocumentKind::Docx, b"PK not a zip").unwrap_err(), "it isn't a readable Office file");
        assert_eq!(extract(DocumentKind::Docx, &office(&[("other.xml", "<a/>")])).unwrap_err(), "it isn't a readable Office file");
        assert_eq!(extract(DocumentKind::Pptx, &office(&[("ppt/presentation.xml", "<a/>")])).unwrap_err(), "it isn't a readable Office file");
        assert!(extract(DocumentKind::Docx, &office(&[("word/document.xml", "<w:p><w:t>unclosed</w:p>")])).is_err());
    }

    #[test]
    fn refuses_office_parts_that_expand_past_the_part_limit() {
        let (open, close) = ("<w:document><w:t>", "</w:t></w:document>");
        let filler = MAX_OFFICE_PART_BYTES as usize - open.len() - close.len();
        let part = |count: usize| format!("{open}{}{close}", "a".repeat(count));

        let below = part(filler - 1);
        assert!(extract(DocumentKind::Docx, &office(&[("word/document.xml", &below)])).is_ok());
        let exact = part(filler);
        assert_eq!(exact.len() as u64, MAX_OFFICE_PART_BYTES);
        assert!(extract(DocumentKind::Docx, &office(&[("word/document.xml", &exact)])).unwrap().truncated);

        // Highly compressible, so the archive is small but the part is not.
        let bomb = office(&[("word/document.xml", &part(filler + 1))]);
        assert!(bomb.len() < 1024 * 1024);
        assert_eq!(extract(DocumentKind::Docx, &bomb).unwrap_err(), "it expands to more data than can be read");
    }

    #[test]
    fn refuses_office_files_that_expand_past_the_total_limit() {
        let large = "a".repeat((MAX_OFFICE_TOTAL_BYTES / 3) as usize - 1);
        let bytes = office(&[("one.xml", &large), ("two.xml", &large), ("three.xml", &large), ("small.xml", "ab"), ("fits.xml", "abc")]);
        let mut archive = OfficeArchive::open(&bytes).unwrap();
        for name in ["one.xml", "two.xml", "three.xml"] {
            assert!(archive.part(name).is_ok(), "{name}");
        }
        // Three bytes of budget remain: two fit, then the rest does not.
        assert_eq!(archive.part("small.xml").unwrap().as_deref(), Some("ab"));
        assert_eq!(archive.part("fits.xml").unwrap_err(), "it expands to more data than can be read");
    }

    #[test]
    fn cache_returns_recent_extractions_and_evicts_the_oldest() {
        let cache = ExtractCache::default();
        let text = |value: &str| ("file.txt".to_string(), "text/plain".to_string(), ExtractedText { text: value.to_string(), truncated: false });
        for index in 0..CACHE_ENTRIES {
            cache.insert(&format!("m{index}"), "a", text(&index.to_string()));
        }
        assert_eq!(cache.get("m0", "a"), Some(text("0")));
        cache.insert("m-new", "a", text("new"));
        assert_eq!(cache.get("m0", "a"), None);
        assert_eq!(cache.get("m1", "a"), Some(text("1")));
        assert_eq!(cache.get("m-new", "a"), Some(text("new")));
        assert_eq!(cache.get("m1", "b"), None);
    }
}
