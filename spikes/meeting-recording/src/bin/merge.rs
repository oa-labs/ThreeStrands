//! Merges a microphone ("You") and a system-audio ("Others") transcript,
//! dropping microphone segments that are speaker echo: a mic segment that
//! overlaps system segments in time and whose words are mostly found in them.
//!
//!   merge <mic.tsv> <system.tsv> [threshold]
//!
//! TSV rows: start_seconds, end_seconds, text (from `transcribe --tsv`).

use std::collections::HashMap;

struct Seg {
    start: f64,
    end: f64,
    text: String,
}

fn load(path: &str) -> anyhow::Result<Vec<Seg>> {
    Ok(std::fs::read_to_string(path)?
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            Some(Seg {
                start: parts.next()?.parse().ok()?,
                end: parts.next()?.parse().ok()?,
                text: parts.next()?.to_string(),
            })
        })
        .collect())
}

fn tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Share of the mic segment's words (as a multiset) that also occur in the
/// overlapping system text. Echo scores near 1; the user's own words near 0;
/// the user talking over remote audio lands in between and is kept.
fn echo_score(mic: &[String], system: &[String]) -> f64 {
    if mic.is_empty() {
        return 0.0;
    }
    let mut available: HashMap<&str, usize> = HashMap::new();
    for w in system {
        *available.entry(w).or_default() += 1;
    }
    let mut common = 0;
    for w in mic {
        if let Some(n) = available.get_mut(w.as_str()) {
            if *n > 0 {
                *n -= 1;
                common += 1;
            }
        }
    }
    common as f64 / mic.len() as f64
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(
        args.len() >= 2,
        "usage: merge <mic.tsv> <system.tsv> [threshold]"
    );
    let mic = load(&args[0])?;
    let system = load(&args[1])?;
    let threshold: f64 = args.get(2).and_then(|t| t.parse().ok()).unwrap_or(0.6);
    // Slack for the acoustic path and VAD boundary differences.
    const SLACK: f64 = 1.0;

    let mut merged: Vec<(f64, &str, String, Option<f64>)> = Vec::new();
    let mut dropped = 0;
    for m in &mic {
        let overlapping: Vec<String> = system
            .iter()
            .filter(|s| s.start < m.end + SLACK && m.start < s.end + SLACK)
            .flat_map(|s| tokens(&s.text))
            .collect();
        let score = echo_score(&tokens(&m.text), &overlapping);
        if score >= threshold {
            dropped += 1;
            println!("drop  {score:.2} [{:>6.2}] {}", m.start, m.text);
        } else {
            merged.push((m.start, "You", m.text.clone(), Some(score)));
        }
    }
    for s in &system {
        merged.push((s.start, "Others", s.text.clone(), None));
    }
    merged.sort_by(|a, b| a.0.total_cmp(&b.0));
    println!(
        "--- merged (threshold {threshold}, dropped {dropped} of {} mic segments)",
        mic.len()
    );
    for (start, who, text, score) in merged {
        let score = score.map(|s| format!(" (echo {s:.2})")).unwrap_or_default();
        println!("[{start:>6.2}] {who:<6}{score} {text}");
    }
    Ok(())
}
