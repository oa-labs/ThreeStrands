//! Runs the real binary in PDF worker mode, the way the app spawns it.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

/// A one-page PDF with a correct cross-reference table.
fn pdf(text: &str) -> Vec<u8> {
    let stream = format!("BT /F1 12 Tf 72 700 Td ({text}) Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>".to_string(),
        format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

fn worker() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_threestrands"));
    command
        .arg("--threestrands-pdf-worker")
        .env_clear()
        .env("THREESTRANDS_PDF_WORKER", "1")
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

fn run(input: &[u8]) -> (Option<i32>, Vec<u8>) {
    let mut child = worker().spawn().unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    (child.wait().unwrap().code(), out)
}

#[test]
fn the_binary_extracts_a_pdf_in_worker_mode() {
    let (code, out) = run(&pdf("Worker says hello"));
    assert_eq!(code, Some(0));
    let length = u32::from_be_bytes(out[..4].try_into().unwrap()) as usize;
    assert_eq!(out.len(), 4 + length, "one framed page and nothing else");
    assert!(String::from_utf8_lossy(&out[4..]).contains("Worker says hello"));
}

#[test]
fn the_binary_reports_unreadable_and_empty_input() {
    assert_eq!(run(b"%PDF-1.4\nnot really a pdf"), (Some(11), Vec::new()));
    assert_eq!(run(b""), (Some(11), Vec::new()));
}
