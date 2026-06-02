//! A tiny, dependency-free TOML syntax highlighter for the egui editor.
//!
//! Pure Rust (no syntect → no `onig` C dep, keeping cross-compile clean, in the
//! spirit of the rustls/boa choices). Handles comments, strings (incl.
//! triple-quoted multi-line bodies, carried across lines), `[section]` headers,
//! and keys. Slicing only ever happens at ASCII delimiters / char boundaries.

use eframe::egui::{
    text::{LayoutJob, TextFormat},
    Color32, FontId,
};

struct Palette {
    font: FontId,
    normal: Color32,
    key: Color32,
    string: Color32,
    comment: Color32,
    section: Color32,
}

fn palette() -> Palette {
    Palette {
        font: FontId::monospace(13.0),
        normal: Color32::from_gray(0xCC),
        key: Color32::from_rgb(0x6E, 0xAA, 0xFF),
        string: Color32::from_rgb(0x8C, 0xC8, 0x8C),
        comment: Color32::from_rgb(0x78, 0x80, 0x8C),
        section: Color32::from_rgb(0xC8, 0x96, 0xE6),
    }
}

pub fn toml_highlight(text: &str) -> LayoutJob {
    let pal = palette();
    let mut job = LayoutJob::default();
    // Delimiter of an open multi-line string carried across line boundaries.
    let mut multiline: Option<char> = None;

    for line in text.split_inclusive('\n') {
        if let Some(q) = multiline {
            let triple: String = std::iter::repeat(q).take(3).collect();
            if let Some(pos) = line.find(&triple) {
                let end = pos + 3;
                seg(&mut job, &line[..end], pal.string, &pal.font);
                multiline = None;
                scan_value(&mut job, &line[end..], &pal, &mut multiline);
            } else {
                seg(&mut job, line, pal.string, &pal.font);
            }
            continue;
        }
        scan_line(&mut job, line, &pal, &mut multiline);
    }
    job
}

fn seg(job: &mut LayoutJob, text: &str, color: Color32, font: &FontId) {
    if text.is_empty() {
        return;
    }
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font.clone(),
            color,
            ..Default::default()
        },
    );
}

fn scan_line(job: &mut LayoutJob, line: &str, pal: &Palette, multiline: &mut Option<char>) {
    let ws_end = line.len() - line.trim_start().len();
    seg(job, &line[..ws_end], pal.normal, &pal.font);
    let body = &line[ws_end..];
    if body.is_empty() {
        return;
    }

    if body.starts_with('#') {
        seg(job, body, pal.comment, &pal.font);
        return;
    }

    if body.starts_with('[') {
        if let Some(rb) = body.find(']') {
            let mut end = rb + 1;
            if body[end..].starts_with(']') {
                end += 1; // [[array]]
            }
            seg(job, &body[..end], pal.section, &pal.font);
            scan_value(job, &body[end..], pal, multiline);
        } else {
            seg(job, body, pal.section, &pal.font);
        }
        return;
    }

    // key = value: color a leading key when `=` precedes any quote or comment.
    if let Some(eq) = key_eq(body) {
        seg(job, &body[..eq], pal.key, &pal.font);
        seg(job, "=", pal.normal, &pal.font);
        scan_value(job, &body[eq + 1..], pal, multiline);
        return;
    }

    scan_value(job, body, pal, multiline);
}

/// Byte index of a `=` that introduces a value (before any `"`, `'`, or `#`).
fn key_eq(s: &str) -> Option<usize> {
    for (i, ch) in s.char_indices() {
        match ch {
            '=' => return Some(i),
            '"' | '\'' | '#' => return None,
            _ => {}
        }
    }
    None
}

fn scan_value(job: &mut LayoutJob, s: &str, pal: &Palette, multiline: &mut Option<char>) {
    for (i, ch) in s.char_indices() {
        if ch == '#' {
            seg(job, &s[..i], pal.normal, &pal.font);
            seg(job, &s[i..], pal.comment, &pal.font);
            return;
        }
        if ch == '"' || ch == '\'' {
            seg(job, &s[..i], pal.normal, &pal.font);
            let q3: String = std::iter::repeat(ch).take(3).collect();
            let rest = &s[i..];
            if rest.starts_with(&q3) {
                if let Some(rel) = s[i + 3..].find(&q3) {
                    let end = i + 3 + rel + 3;
                    seg(job, &s[i..end], pal.string, &pal.font);
                    scan_value(job, &s[end..], pal, multiline);
                } else {
                    seg(job, &s[i..], pal.string, &pal.font);
                    *multiline = Some(ch);
                }
            } else if let Some(rel) = s[i + 1..].find(ch) {
                let end = i + 1 + rel + 1;
                seg(job, &s[i..end], pal.string, &pal.font);
                scan_value(job, &s[end..], pal, multiline);
            } else {
                seg(job, &s[i..], pal.string, &pal.font);
            }
            return;
        }
    }
    seg(job, s, pal.normal, &pal.font);
}
