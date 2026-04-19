//! Best-effort input template generator for AtCoder `tasks_print` HTML.
//!
//! This module parses `task.html` (the print view of a contest), extracts the
//! "入力" blocks, and heuristically infers `proconio::input!` declarations.
//! It also inserts small helper loops for common patterns like per-row lengths.
//!
//! The intent is to reduce boilerplate for typical A–D style inputs while
//! remaining conservative. When a pattern is ambiguous or unsupported, the
//! generator leaves a human-readable comment instead of guessing incorrectly.
//!
//! Type inference is based on constraints: if a variable is ever bounded by a
//! negative number, it is treated as `i64`; otherwise it defaults to `usize`.
//! Strings (S/T/U/X) and concatenated grid rows are inferred as `Chars`.
//! These heuristics are not perfect, but they usually match AtCoder style.

use crate::shell::Shell;
use anyhow::Context as _;
use camino::{Utf8Path, Utf8PathBuf};
use heck::KebabCase;
use regex::Regex;
use std::collections::HashMap;
use std::fs;

/// Minimal representation of a single task section (A, B, C, ...).
///
/// It only stores input blocks and constraint items needed for inference.
#[derive(Debug, Clone)]
struct TaskSection {
    letter: String,
    input_blocks: Vec<Vec<String>>,
    constraints_items: Vec<String>,
}

/// Remove HTML tags and decode a tiny subset of entities used in AtCoder pages.
///
/// This is intentionally shallow: it assumes the `tasks_print` HTML structure
/// and only decodes `&lt;`, `&gt;`, and `&amp;`.
fn strip_tags(html: &str) -> String {
    // Remove tags in a very rough way (AtCoder tasks_print is predictable enough).
    let re = Regex::new(r"(?s)<.*?>").expect("invalid regex");
    let mut s = re.replace_all(html, "").to_string();
    // Minimal HTML entity decoding we actually see in tasks_print.
    s = s.replace("&lt;", "<");
    s = s.replace("&gt;", ">");
    s = s.replace("&amp;", "&");
    s
}

/// Extract constraint bullet items from a task segment.
///
/// Prefers Japanese "制約" if present; otherwise uses English "Constraints".
/// Returns an empty list if no `<ul>` is found.
fn extract_constraints_items(seg: &str) -> Vec<String> {
    for key in ["制約", "Constraints"] {
        let re = Regex::new(&format!(
            r"(?s)<h3>{}</h3>.*?<ul>(.*?)</ul>",
            regex::escape(key)
        ))
        .unwrap();
        if let Some(cap) = re.captures(seg) {
            let ul = cap.get(1).unwrap().as_str();
            let li_re = Regex::new(r"(?s)<li>(.*?)</li>").unwrap();
            let mut items = Vec::new();
            for li in li_re.captures_iter(ul) {
                let txt = strip_tags(li.get(1).unwrap().as_str()).trim().to_string();
                if !txt.is_empty() {
                    items.push(txt);
                }
            }
            return items;
        }
    }
    Vec::new()
}

/// Detect lines like `case_i` that indicate testcase placeholders.
fn is_case_placeholder_line(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    l.contains("case") && (l.contains('_') || l.contains("\\mathrm"))
}

/// Detect lines like `query_i` that indicate query placeholders.
fn is_query_placeholder_line(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    l.contains("query") && (l.contains('_') || l.contains("\\mathrm") || l.contains("\\text"))
}

/// Normalize a constraint string for simple parsing.
///
/// Converts various comparison symbols and removes spaces, then normalizes
/// `\times`/`×` to `*` so we can parse numeric expressions uniformly.
fn normalize_constraint(s: &str) -> String {
    let mut t = s.to_string();
    t = t
        .replace("≤", "<=")
        .replace("≦", "<=")
        .replace("≧", ">=")
        .replace("≥", ">=");
    t = t.replace("\\leq", "<=").replace("\\le", "<=");
    t = t.replace("\\geq", ">=").replace("\\ge", ">=");
    t = t.replace("−", "-");
    t = t.replace("\\times", "*").replace("×", "*");
    t = t.replace(' ', "");
    t
}

/// Extract a base variable name from a token like `A_i` or `A_{i,j}`.
///
/// This drops subscripts and LaTeX noise and returns a snake-cased identifier.
fn base_var(tok: &str) -> Option<String> {
    let mut t = tok.to_string();
    t = t
        .replace("\\mathrm", "")
        .replace("\\text", "")
        .replace("\\rm", "");
    t = t.replace('{', "").replace('}', "");
    t = t.replace('|', "");
    t = t.replace('\\', "");
    if t.is_empty() {
        return None;
    }
    let start = t
        .char_indices()
        .find(|(_, c)| c.is_ascii_alphabetic())
        .map(|(i, _)| i)?;
    let t = &t[start..];
    let mut base = t;
    if let Some((b, _)) = base.split_once('_') {
        base = b;
    }
    if let Some((b, _)) = base.split_once('[') {
        base = b;
    }
    if base.is_empty() {
        return None;
    }
    Some(snake(base))
}

fn base_vars(tok: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in tok.split(',') {
        if let Some(b) = base_var(part) {
            out.push(b);
        }
    }
    out
}

/// Check whether a token looks like a purely numeric expression.
fn is_numeric_expr(tok: &str) -> bool {
    if tok.is_empty() {
        return false;
    }
    tok.chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '*' | '/' | '^' | '(' | ')'))
}

/// Check whether a numeric expression is negative after removing outer parens.
fn numeric_is_negative(tok: &str) -> bool {
    let mut t = tok.trim();
    loop {
        let t2 = t.trim_start_matches('(').trim_end_matches(')');
        if t2.len() == t.len() {
            break;
        }
        t = t2;
    }
    t.starts_with('-')
}

/// Determine which base variables should be treated as signed (`i64`).
///
/// Heuristic: if a constraint compares a variable against a negative bound,
/// or uses an absolute value like `|X|`, the base is treated as signed.
fn signed_bases(constraints: &[String]) -> std::collections::HashSet<String> {
    let mut signed = std::collections::HashSet::new();
    let op_re = Regex::new(r"(<=|>=|<|>)").unwrap();
    let abs_re = Regex::new(r"\|([^|]+)\|").unwrap();

    for item in constraints {
        for cap in abs_re.captures_iter(item) {
            for base in base_vars(cap.get(1).unwrap().as_str()) {
                signed.insert(base);
            }
        }

        let norm = normalize_constraint(item);
        let mut tokens = Vec::new();
        let mut ops = Vec::new();
        let mut last = 0usize;
        for m in op_re.find_iter(&norm) {
            tokens.push(norm[last..m.start()].to_string());
            ops.push(m.as_str().to_string());
            last = m.end();
        }
        tokens.push(norm[last..].to_string());
        if ops.is_empty() {
            continue;
        }

        for i in 0..ops.len() {
            let left = tokens[i].clone();
            let right = tokens[i + 1].clone();
            let left_vars = base_vars(&left);
            let right_vars = base_vars(&right);
            let left_num = is_numeric_expr(&left);
            let right_num = is_numeric_expr(&right);

            if !right_vars.is_empty() && left_num {
                if numeric_is_negative(&left) {
                    for var in right_vars {
                        signed.insert(var);
                    }
                }
            }
            if !left_vars.is_empty() && right_num {
                if numeric_is_negative(&right) {
                    for var in left_vars {
                        signed.insert(var);
                    }
                }
            }
        }
    }
    signed
}

/// Map a base name to `i64` or `usize` based on constraints.
fn num_ty_for_base(base: &str, signed: &std::collections::HashSet<String>) -> &'static str {
    if signed.contains(&snake(base)) {
        "i64"
    } else {
        "usize"
    }
}

/// Map a raw token (possibly indexed) to `i64` or `usize`.
///
/// Uses `parse_indexed_token` to strip indices and then consults constraints.
fn num_ty_for_token(tok: &str, signed: &std::collections::HashSet<String>) -> &'static str {
    if let Some((base, _)) = parse_indexed_token(&normalize_line(tok)) {
        num_ty_for_base(&base, signed)
    } else {
        num_ty_for_base(tok, signed)
    }
}

/// Normalize a single input-format line for pattern matching.
///
/// This handles common AtCoder LaTeX patterns and whitespace quirks:
/// `A _ 1` → `A_1`, `\dots` → `\ldots`, and concatenated tokens like
/// `S_{1,1}S_{1,2}` → `S_{1,1} S_{1,2}`.
fn normalize_line(line: &str) -> String {
    let mut s = line.trim().to_string();
    s = s.replace("\\cdots", "\\ldots").replace("\\dots", "\\ldots");
    s = s.replace("\\vdots", " \\vdots ");
    s = s.replace("\\ldots", " \\ldots ");

    // Remove spaces around underscore (A _ 1 -> A_1)
    let underscore_re = Regex::new(r"\s*_\s*").unwrap();
    s = underscore_re.replace_all(&s, "_").to_string();

    // Tidy spaces inside braces/brackets
    let comma_re = Regex::new(r",\s+").unwrap();
    s = comma_re.replace_all(&s, ",").to_string();
    let brace_left_re = Regex::new(r"\{\s+").unwrap();
    s = brace_left_re.replace_all(&s, "{").to_string();
    let brace_right_re = Regex::new(r"\s+\}").unwrap();
    s = brace_right_re.replace_all(&s, "}").to_string();

    // Split concatenated tokens like S_{1,1}S_{1,2}, C[1][1]C[1][2], c_1c_2
    let brace_concat_re = Regex::new(r"\}([A-Za-z\\])").unwrap();
    s = brace_concat_re.replace_all(&s, "} $1").to_string();
    let bracket_concat_re = Regex::new(r"\]([A-Za-z\\])").unwrap();
    s = bracket_concat_re.replace_all(&s, "] $1").to_string();
    let digit_concat_re = Regex::new(r"([A-Za-z]_\d+)([A-Za-z\\])").unwrap();
    s = digit_concat_re.replace_all(&s, "$1 $2").to_string();

    // Collapse whitespace
    let ws_re = Regex::new(r"\s+").unwrap();
    s = ws_re.replace_all(&s, " ").to_string();
    s.trim().to_string()
}

/// Check if normalization increased token count (used to detect concatenation).
fn is_concat_hint(orig: &str, norm: &str) -> bool {
    let o = orig.split_whitespace().count();
    let n = norm.split_whitespace().count();
    n > o
}

/// Parse task sections from the `tasks_print` HTML.
///
/// Returns a list of `TaskSection` containing input `<pre>` blocks and
/// constraint items. Each block is a vector of trimmed, non-empty lines.
fn parse_task_sections(task_html: &str) -> Vec<TaskSection> {
    let span_re = Regex::new(r#"(?s)<span class="h2">\s*([A-Z])\s*-\s*([^<]+)</span>"#)
        .expect("invalid regex");
    let mut spans: Vec<(usize, usize, String, String)> = Vec::new();
    for cap in span_re.captures_iter(task_html) {
        let m = cap.get(0).unwrap();
        let letter = cap.get(1).unwrap().as_str().trim().to_string();
        let title = cap.get(2).unwrap().as_str().trim().to_string();
        spans.push((m.start(), m.end(), letter, title));
    }

    let mut out = Vec::new();
    let pre_re = Regex::new(r"(?s)<pre>(.*?)</pre>").expect("invalid regex");
    for idx in 0..spans.len() {
        let (start, _end, letter, _title) = spans[idx].clone();
        let end = if idx + 1 < spans.len() {
            spans[idx + 1].0
        } else {
            task_html.len()
        };
        let seg = &task_html[start..end];

        let in_pos = seg.find(r"<h3>入力</h3>");
        if in_pos.is_none() {
            continue;
        }
        let in_pos = in_pos.unwrap();
        let out_pos = seg.find(r"<h3>出力</h3>").unwrap_or(seg.len());
        let inp = &seg[in_pos..out_pos];

        let mut blocks: Vec<Vec<String>> = Vec::new();
        for cap in pre_re.captures_iter(inp) {
            let pre = cap.get(1).unwrap().as_str();
            let txt = strip_tags(pre);
            let lines: Vec<String> = txt
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .map(|l| l.to_string())
                .collect();
            blocks.push(lines);
        }
        let constraints_items = extract_constraints_items(seg);
        out.push(TaskSection {
            letter,
            input_blocks: blocks,
            constraints_items,
        });
    }
    out
}

/// Convert a symbol to a snake-case identifier suitable for Rust bindings.
fn snake(s: &str) -> String {
    let mut out = String::new();
    let mut prev_is_underscore = false;
    for ch in s.chars() {
        let c = if ch.is_ascii_alphanumeric() { ch } else { '_' };
        if c == '_' {
            if !prev_is_underscore {
                out.push('_');
            }
            prev_is_underscore = true;
        } else {
            out.push(c.to_ascii_lowercase());
            prev_is_underscore = false;
        }
    }
    out.trim_matches('_').to_string()
}

/// Convert a LaTeX-like symbol expression to a Rust-ish expression.
///
/// Examples: `N` → `n`, `N-1` → `n-1`, `5N` → `5*n`.
fn sym_expr(s: &str) -> String {
    // Convert common AtCoder latex-ish symbols to a Rust-ish expression: N-1, 5N, etc.
    let mut t = s.trim().replace(' ', "");
    t = t.replace('\\', "");
    if let Some((a, b)) = t.split_once('-') {
        if b.chars().all(|c| c.is_ascii_digit()) {
            return format!("{}-{}", snake(a), b);
        }
    }
    // 5N form
    let coef_re = Regex::new(r"^(\d+)([A-Za-z]+)$").unwrap();
    if let Some(cap) = coef_re.captures(&t) {
        return format!("{}*{}", &cap[1], snake(&cap[2]));
    }
    if t.chars().all(|c| c.is_ascii_alphabetic()) {
        return snake(&t);
    }
    t
}

/// Determine whether a symbol should be treated as a string (`Chars`).
fn is_string_symbol(sym: &str) -> bool {
    matches!(sym.to_ascii_uppercase().as_str(), "S" | "T" | "U" | "X")
}

/// Convert a 1-based/0-based indexed last term into a length expression.
fn len_expr(first_idx: &str, last_raw: &str) -> String {
    if first_idx == "0" {
        // if last is N-1, length is N; else (last+1)
        let mm = Regex::new(r"^([A-Za-z]+)-1$").unwrap();
        if let Some(c2) = mm.captures(last_raw) {
            snake(c2.get(1).unwrap().as_str())
        } else {
            format!("({})+1", sym_expr(last_raw))
        }
    } else {
        sym_expr(last_raw)
    }
}

/// Parse a single indexed token.
///
/// Supports underscore-form (`A_{1,2}`) and bracket-form (`C[1][2]`).
/// Returns `(base, indices)` when successful.
fn parse_indexed_token(token: &str) -> Option<(String, Vec<String>)> {
    let t = token.trim();
    // Bracket form: C[1][2]
    let bracket_re = Regex::new(r"^([A-Za-z]+)((?:\[[^\]]+\])+)$").unwrap();
    if let Some(cap) = bracket_re.captures(t) {
        let base = cap.get(1)?.as_str().to_string();
        let rest = cap.get(2)?.as_str();
        let idx_re = Regex::new(r"\[([^\]]+)\]").unwrap();
        let mut idxs = Vec::new();
        for c in idx_re.captures_iter(rest) {
            let idx = c.get(1)?.as_str().trim().to_string();
            if !idx.is_empty() {
                idxs.push(idx);
            }
        }
        if !idxs.is_empty() {
            return Some((base, idxs));
        }
    }

    // Underscore form: A_1, A_{1,2}
    let us_re = Regex::new(r"^([A-Za-z]+)_(?:\{)?(.+?)(?:\})?$").unwrap();
    if let Some(cap) = us_re.captures(t) {
        let base = cap.get(1)?.as_str().to_string();
        let idxs_raw = cap.get(2)?.as_str();
        let idxs = idxs_raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        if !idxs.is_empty() {
            return Some((base, idxs));
        }
    }
    None
}

/// Parse a 1D array line with ellipsis.
///
/// Handles `A_1 A_2 \ldots A_N`, `A_1 \ldots A_N`, and `A_0 ... A_{N-1}`.
/// Returns `(base, len_expr)` (the caller decides element type).
fn parse_1d_array_line(line: &str) -> Option<(String, String)> {
    // A_1 A_2 \ldots A_N  or A_1 \ldots A_N  or A_0 ... A_{N-1}
    let ln = line;
    // NOTE: Rust's `regex` crate does NOT support backreferences like \1.
    // Capture the base name multiple times and validate equality in code.
    let re_full = Regex::new(
        r"^([A-Za-z]+)_(?:\{)?(\d+)(?:\})?\s+([A-Za-z]+)_(?:\{)?(\d+)(?:\})?\s+\\ldots\s+([A-Za-z]+)_(?:\{)?(.+?)(?:\})?$",
    )
    .unwrap();
    if let Some(cap) = re_full.captures(&ln) {
        let base1 = cap.get(1)?.as_str();
        let first_idx = cap.get(2)?.as_str();
        let base2 = cap.get(3)?.as_str();
        let base3 = cap.get(5)?.as_str();
        if base1 != base2 || base1 != base3 {
            return None;
        }
        let last_raw = cap
            .get(6)?
            .as_str()
            .trim()
            .trim_matches('{')
            .trim_matches('}');
        let len_expr = len_expr(first_idx, last_raw);
        return Some((base1.to_string(), len_expr));
    }

    let re_short = Regex::new(
        r"^([A-Za-z]+)_(?:\{)?(\d+)(?:\})?\s+\\ldots\s+([A-Za-z]+)_(?:\{)?(.+?)(?:\})?$",
    )
    .unwrap();
    let cap = re_short.captures(&ln)?;
    let base1 = cap.get(1)?.as_str();
    let first_idx = cap.get(2)?.as_str();
    let base2 = cap.get(3)?.as_str();
    if base1 != base2 {
        return None;
    }
    let last_raw = cap
        .get(4)?
        .as_str()
        .trim()
        .trim_matches('{')
        .trim_matches('}');
    let len_expr = len_expr(first_idx, last_raw);
    Some((base1.to_string(), len_expr))
}

/// Parse a fixed 1D array line without ellipsis, e.g. `A_1 A_2 A_3`.
///
/// Returns `(base, length)`.
fn parse_fixed_indexed_line(line: &str) -> Option<(String, usize)> {
    // A_1 A_2 A_3 (no ellipsis)
    let toks: Vec<&str> = line.split_whitespace().collect();
    if toks.len() < 2 {
        return None;
    }
    let mut base: Option<String> = None;
    let mut first_idx: Option<i32> = None;
    let mut prev_idx: Option<i32> = None;
    for tok in &toks {
        let (b, idxs) = parse_indexed_token(tok)?;
        if idxs.len() != 1 {
            return None;
        }
        let idx = idxs[0].parse::<i32>().ok()?;
        if let Some(b0) = &base {
            if *b0 != b {
                return None;
            }
        } else {
            base = Some(b);
            first_idx = Some(idx);
        }
        if let Some(prev) = prev_idx {
            if idx != prev + 1 {
                return None;
            }
        }
        prev_idx = Some(idx);
    }
    let base = base?;
    let first_idx = first_idx?;
    let last_idx = prev_idx?;
    let len = (last_idx - first_idx + 1) as usize;
    Some((base, len))
}

/// Parse repeated N-tuples like `x_1 y_1 ...` ... `x_M y_M ...`.
///
/// Returns `(bases, count_expr, consumed_lines)` when it finds a matching
/// first and last line with a shared index.
fn parse_n_repeat(lines: &[String], idx: usize) -> Option<(Vec<String>, String, usize)> {
    fn parse_first_line(line: &str) -> Option<(Vec<String>, String)> {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() < 2 {
            return None;
        }
        let mut bases = Vec::new();
        let mut first_idx: Option<String> = None;
        for tok in toks {
            let (base, idxs) = parse_indexed_token(tok)?;
            if idxs.len() != 1 {
                return None;
            }
            let idx = idxs[0].clone();
            if let Some(prev) = &first_idx {
                if *prev != idx {
                    return None;
                }
            } else {
                if !idx.chars().all(|c| c.is_ascii_digit()) {
                    return None;
                }
                first_idx = Some(idx);
            }
            bases.push(base);
        }
        Some((bases, first_idx?))
    }

    fn parse_last_line(line: &str, bases: &[String]) -> Option<String> {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() != bases.len() {
            return None;
        }
        let mut last_idx: Option<String> = None;
        for (tok, base) in toks.into_iter().zip(bases.iter()) {
            let (b, idxs) = parse_indexed_token(tok)?;
            if &b != base || idxs.len() != 1 {
                return None;
            }
            let idx = idxs[0].clone();
            if let Some(prev) = &last_idx {
                if *prev != idx {
                    return None;
                }
            } else {
                last_idx = Some(idx);
            }
        }
        last_idx
    }

    let (bases, first_idx) = parse_first_line(lines.get(idx)?)?;

    let mut last_idx: Option<String> = None;
    let mut last_found: Option<usize> = None;
    let mut j = idx + 1;
    while j < lines.len() && j < idx + 12 {
        if lines[j].contains("\\vdots")
            || lines[j].contains("\\ldots")
            || lines[j].contains("\\cdots")
            || lines[j].contains("\\dots")
        {
            j += 1;
            continue;
        }
        if let Some(idx_expr) = parse_last_line(&lines[j], &bases) {
            last_idx = Some(idx_expr);
            last_found = Some(j);
            j += 1;
            continue;
        }
        if last_found.is_some() {
            break;
        }
        j += 1;
    }
    let last_idx = last_idx?;
    let count_expr = len_expr(&first_idx, &last_idx);
    let consumed = last_found.map(|lf| lf + 1 - idx).unwrap_or(1);
    Some((bases, count_expr, consumed))
}

/// Parse vertical scalars like `B_1`, `\vdots`, `B_N`.
///
/// Returns `(base, count_expr, consumed_lines)`.
fn parse_vertical_scalars(lines: &[String], idx: usize) -> Option<(String, String, usize)> {
    // B_1 \vdots B_N  -> b: [usize; n]
    let re = Regex::new(r"^([A-Za-z]+)_(?:\{)?1(?:\})?$").unwrap();
    let cap = re.captures(lines.get(idx)?)?;
    let base = cap.get(1)?.as_str();
    if base.eq_ignore_ascii_case("S") {
        return None;
    }
    let last_re = Regex::new(&format!(r"^{}_(?:\{{)?(.+?)(?:\}})?$", regex::escape(base))).unwrap();
    let mut last: Option<String> = None;
    let mut last_found: Option<usize> = None;
    let mut j = idx + 1;
    while j < lines.len() && j < idx + 8 {
        if lines[j].contains("\\vdots") {
            j += 1;
            continue;
        }
        if let Some(c2) = last_re.captures(&lines[j]) {
            last = Some(c2.get(1).unwrap().as_str().to_string());
            last_found = Some(j);
            break;
        }
        j += 1;
    }
    let last = last?;
    let count_expr = sym_expr(last.trim_matches('{').trim_matches('}'));
    let consumed = last_found.map(|lf| lf + 1 - idx).unwrap_or(1);
    Some((base.to_string(), count_expr, consumed))
}

/// Compact representation of a row with ellipsis, e.g. `A_{i,1} ... A_{i,W}`.
#[derive(Debug, Clone)]
struct RowPattern {
    base: String,
    prefix: Vec<String>,
    col_first: String,
    col_last: String,
}

/// Parse a row containing `\ldots` and indexed tokens.
fn parse_row_with_ellipsis(line: &str) -> Option<RowPattern> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    if !toks.iter().any(|t| *t == "\\ldots") {
        return None;
    }
    let first = toks.first()?;
    let last = toks.last()?;
    let (base1, idxs1) = parse_indexed_token(first)?;
    let (base2, idxs2) = parse_indexed_token(last)?;
    if base1 != base2 || idxs1.len() != idxs2.len() || idxs1.len() < 2 {
        return None;
    }
    let prefix1 = idxs1[..idxs1.len() - 1].to_vec();
    let prefix2 = idxs2[..idxs2.len() - 1].to_vec();
    if prefix1 != prefix2 {
        return None;
    }
    Some(RowPattern {
        base: base1,
        prefix: prefix1,
        col_first: idxs1.last()?.to_string(),
        col_last: idxs2.last()?.to_string(),
    })
}

/// Parse a row with fixed, explicit columns (no ellipsis).
fn parse_row_fixed(line: &str) -> Option<(String, String, usize)> {
    // Returns (base, row_idx, col_count)
    let toks: Vec<&str> = line.split_whitespace().collect();
    if toks.len() < 2 {
        return None;
    }
    let mut base: Option<String> = None;
    let mut row_idx: Option<String> = None;
    let mut prev_col: Option<i32> = None;
    for tok in &toks {
        let (b, idxs) = parse_indexed_token(tok)?;
        if idxs.len() != 2 {
            return None;
        }
        if let Some(b0) = &base {
            if *b0 != b {
                return None;
            }
        } else {
            base = Some(b);
        }
        if let Some(r0) = &row_idx {
            if *r0 != idxs[0] {
                return None;
            }
        } else {
            row_idx = Some(idxs[0].clone());
        }
        let col = idxs[1].parse::<i32>().ok()?;
        if let Some(prev) = prev_col {
            if col != prev + 1 {
                return None;
            }
        }
        prev_col = Some(col);
    }
    Some((base?, row_idx?, toks.len()))
}

/// Parse a grid of concatenated row-strings like `S_{1,1}S_{1,2}...`.
///
/// Returns `(base, type_expr, consumed_lines)` where the type is `[Chars; H]`.
fn parse_grid_row_block(
    lines: &[String],
    orig_lines: &[String],
    idx: usize,
    known_h: Option<&str>,
) -> Option<(String, String, usize)> {
    let row = parse_row_with_ellipsis(lines.get(idx)?)?;
    if row.prefix.len() != 1 {
        return None;
    }
    let concat = is_concat_hint(orig_lines.get(idx)?, lines.get(idx)?);
    if !concat && !row.base.eq_ignore_ascii_case("S") {
        return None;
    }
    let first_row = row.prefix[0].clone();
    let mut last_row: Option<String> = None;
    let mut last_found: Option<usize> = None;
    let mut j = idx + 1;
    while j < lines.len() && j < idx + 12 {
        if lines[j].contains("\\vdots") {
            j += 1;
            continue;
        }
        if let Some(r2) = parse_row_with_ellipsis(&lines[j]) {
            if r2.base == row.base && r2.prefix.len() == 1 {
                last_row = Some(r2.prefix[0].clone());
                last_found = Some(j);
                j += 1;
                continue;
            }
        }
        if last_found.is_some() {
            break;
        }
        j += 1;
    }
    let last_row = last_row?;
    let h_expr = known_h
        .map(|h| h.to_string())
        .unwrap_or_else(|| len_expr(&first_row, &last_row));
    let consumed = last_found.map(|lf| lf + 1 - idx).unwrap_or(1);
    Some((snake(&row.base), format!("[Chars; {}]", h_expr), consumed))
}

/// Parse a numeric matrix block with ellipsis in each row.
///
/// Returns `(base, width_expr, height_expr, consumed_lines)`.
fn parse_matrix_block(
    lines: &[String],
    idx: usize,
    known_h: Option<&str>,
    known_w: Option<&str>,
) -> Option<(String, String, String, usize)> {
    let row = parse_row_with_ellipsis(lines.get(idx)?)?;
    if row.prefix.len() != 1 {
        return None;
    }
    let first_row = row.prefix[0].clone();
    let w_expr = known_w
        .map(|w| w.to_string())
        .unwrap_or_else(|| len_expr(&row.col_first, &row.col_last));

    let mut last_row: Option<String> = None;
    let mut last_found: Option<usize> = None;
    let mut j = idx + 1;
    while j < lines.len() && j < idx + 12 {
        if lines[j].contains("\\vdots") {
            j += 1;
            continue;
        }
        if let Some(r2) = parse_row_with_ellipsis(&lines[j]) {
            if r2.base == row.base && r2.prefix.len() == 1 {
                last_row = Some(r2.prefix[0].clone());
                last_found = Some(j);
                j += 1;
                continue;
            }
        }
        if last_found.is_some() {
            break;
        }
        j += 1;
    }
    let last_row = last_row?;
    let h_expr = known_h
        .map(|h| h.to_string())
        .unwrap_or_else(|| len_expr(&first_row, &last_row));
    let consumed = last_found.map(|lf| lf + 1 - idx).unwrap_or(1);
    Some((row.base, w_expr, h_expr, consumed))
}

/// Parse a numeric matrix with explicit rows and columns (no ellipsis).
///
/// Returns `(base, width, height, consumed_lines)`.
fn parse_matrix_fixed_block(lines: &[String], idx: usize) -> Option<(String, usize, usize, usize)> {
    let (base, row_idx, col_count) = parse_row_fixed(lines.get(idx)?)?;
    let mut row_count = 1usize;
    let mut j = idx + 1;
    while j < lines.len() {
        if let Some((b2, r2, c2)) = parse_row_fixed(&lines[j]) {
            if b2 == base && c2 == col_count {
                if let (Ok(r0), Ok(r1)) = (row_idx.parse::<i32>(), r2.parse::<i32>()) {
                    if r1 == r0 + row_count as i32 {
                        row_count += 1;
                        j += 1;
                        continue;
                    }
                }
            }
        }
        break;
    }
    if row_count < 2 {
        return None;
    }
    let consumed = row_count;
    Some((base, col_count, row_count, consumed))
}

/// Parse a 3D array like `S_{f,h,1} ... S_{f,h,W}`.
///
/// Returns `(base, width_expr, height_expr, depth_expr, consumed_lines)`.
fn parse_3d_array_block(
    lines: &[String],
    idx: usize,
) -> Option<(String, String, String, String, usize)> {
    let row = parse_row_with_ellipsis(lines.get(idx)?)?;
    if row.prefix.len() != 2 {
        return None;
    }
    let first_f = row.prefix[0].clone();
    let first_h = row.prefix[1].clone();
    let w_expr = len_expr(&row.col_first, &row.col_last);

    let mut last_f: Option<String> = None;
    let mut last_h: Option<String> = None;
    let mut last_found: Option<usize> = None;
    let mut j = idx + 1;
    while j < lines.len() && j < idx + 32 {
        if lines[j].contains("\\vdots") {
            j += 1;
            continue;
        }
        if let Some(r2) = parse_row_with_ellipsis(&lines[j]) {
            if r2.base == row.base && r2.prefix.len() == 2 {
                last_f = Some(r2.prefix[0].clone());
                last_h = Some(r2.prefix[1].clone());
                last_found = Some(j);
                j += 1;
                continue;
            }
        }
        if last_found.is_some() {
            break;
        }
        j += 1;
    }
    let last_f = last_f?;
    let last_h = last_h?;
    let f_expr = len_expr(&first_f, &last_f);
    let h_expr = len_expr(&first_h, &last_h);
    let consumed = last_found.map(|lf| lf + 1 - idx).unwrap_or(1);
    Some((row.base, w_expr, h_expr, f_expr, consumed))
}

/// Parse variable-length rows with fixed prefixes and a trailing list.
///
/// Supported forms include:
/// - `L_i a_{i,1} ... a_{i,L_i}`
/// - `P_i C_i f_{i,1} ... f_{i,C_i}`
///
/// Returns `(prefix_bases, len_base, elem_base, count_expr, consumed_lines)`.
fn parse_varlen_rows(
    lines: &[String],
    idx: usize,
) -> Option<(Vec<String>, String, String, String, usize)> {
    /// Parsed shape of one row: fixed prefix columns and a variable-length tail.
    struct VarlenRow {
        prefix_bases: Vec<String>,
        row_idx: String,
        len_base: String,
        elem_base: String,
    }

    fn parse_row(line: &str) -> Option<VarlenRow> {
        if !line.contains("\\ldots") {
            return None;
        }
        let toks: Vec<&str> = line
            .split_whitespace()
            .filter(|t| *t != "\\ldots")
            .collect();
        if toks.len() < 3 {
            return None;
        }

        let parsed = toks
            .iter()
            .map(|t| parse_indexed_token(t))
            .collect::<Option<Vec<_>>>()?;

        let first_elem_pos = parsed.iter().position(|(_, idxs)| idxs.len() == 2)?;
        if first_elem_pos == 0 {
            return None;
        }

        let (elem_base, elem_idxs) = &parsed[first_elem_pos];
        if elem_idxs.len() != 2 || !elem_idxs[1].chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let row_idx = elem_idxs[0].clone();

        let mut prefix_bases = Vec::new();
        for (base, idxs) in parsed.iter().take(first_elem_pos) {
            if idxs.len() != 1 || idxs[0] != row_idx {
                return None;
            }
            prefix_bases.push(base.clone());
        }

        let (last_base, last_idxs) = parsed.last()?;
        if last_base != elem_base || last_idxs.len() != 2 || last_idxs[0] != row_idx {
            return None;
        }
        let len_base = base_var(&last_idxs[1])?;
        if !prefix_bases
            .iter()
            .any(|b| base_var(b).as_deref() == Some(len_base.as_str()))
        {
            return None;
        }

        Some(VarlenRow {
            prefix_bases,
            row_idx,
            len_base,
            elem_base: elem_base.clone(),
        })
    }

    let first = parse_row(lines.get(idx)?)?;
    if !first.row_idx.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut last_row: Option<String> = None;
    let mut last_found: Option<usize> = None;

    let mut j = idx + 1;
    while j < lines.len() && j < idx + 16 {
        if lines[j].contains("\\vdots") {
            j += 1;
            continue;
        }
        if let Some(row) = parse_row(&lines[j]) {
            if row.prefix_bases == first.prefix_bases
                && row.len_base == first.len_base
                && row.elem_base == first.elem_base
            {
                last_row = Some(row.row_idx);
                last_found = Some(j);
                j += 1;
                continue;
            }
        }
        if last_found.is_some() {
            break;
        }
        j += 1;
    }

    let last_row = last_row?;
    let count_expr = len_expr(&first.row_idx, &last_row);
    let consumed = last_found.map(|lf| lf + 1 - idx).unwrap_or(1);
    Some((
        first.prefix_bases,
        first.len_base,
        first.elem_base,
        count_expr,
        consumed,
    ))
}

/// Parse a vertical grid like `S_1` ... `S_H` (each row is a string).
fn parse_grid_lines(
    lines: &[String],
    idx: usize,
    known_h: Option<&str>,
) -> Option<(String, String, usize)> {
    // S_1 \vdots S_H  -> s: [Chars; h]
    let re = Regex::new(r"^([A-Za-z]+)_(?:\{)?1(?:\})?$").unwrap();
    let cap = re.captures(lines.get(idx)?)?;
    let base = cap.get(1)?.as_str();
    if !base.eq_ignore_ascii_case("S") {
        return None;
    }
    let last_re = Regex::new(r"^S_(?:\{)?(.+?)(?:\})?$").unwrap();
    let mut last: Option<String> = None;
    let mut last_found: Option<usize> = None;
    let mut j = idx + 1;
    while j < lines.len() && j < idx + 8 {
        if lines[j].contains("\\vdots") {
            j += 1;
            continue;
        }
        if let Some(c2) = last_re.captures(&lines[j]) {
            last = Some(c2.get(1).unwrap().as_str().to_string());
            last_found = Some(j);
            break;
        }
        j += 1;
    }
    let last = last?;
    let h_expr = known_h
        .map(|h| h.to_string())
        .unwrap_or_else(|| sym_expr(last.trim_matches('{').trim_matches('}')));
    let consumed = last_found.map(|lf| lf + 1 - idx).unwrap_or(1);
    Some((snake(base), format!("[Chars; {}]", h_expr), consumed))
}

/// Output of the input inference step.
///
/// `decls` are `input!` fields, `needs_chars` enables `marker::Chars`, and
/// `extra_lines` are additional loops or post-processing lines.
struct GuessResult {
    decls: Vec<String>,
    needs_chars: bool,
    extra_lines: Vec<String>,
}

/// Infer input declarations from a list of input-format lines.
///
/// This is the main heuristic engine. It tries specialized parsers first
/// (grids, matrices, repeated pairs, etc.), then falls back to scalar lines.
/// Signedness is inferred from constraints and passed via `signed`.
fn guess_input_from_lines(
    lines: &[String],
    signed: &std::collections::HashSet<String>,
) -> GuessResult {
    let mut decls: Vec<String> = Vec::new();
    let mut needs_chars = false;
    let mut extra_lines: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut known_h: Option<String> = None;
    let mut known_w: Option<String> = None;

    let norm_lines: Vec<String> = lines.iter().map(|l| normalize_line(l)).collect();
    let concat_hints: Vec<bool> = lines
        .iter()
        .zip(norm_lines.iter())
        .map(|(o, n)| is_concat_hint(o, n))
        .collect();

    let t_is_testcases = lines
        .iter()
        .any(|l| l.to_ascii_lowercase().contains("case"));

    let mut i = 0usize;
    while i < lines.len() {
        let ln = &norm_lines[i];
        let orig = &lines[i];
        let concat_hint = concat_hints[i];
        if is_case_placeholder_line(ln) || is_query_placeholder_line(ln) || ln.contains("\\vdots") {
            i += 1;
            continue;
        }

        if let Some((prefix_bases, len_base, elem_base, count_expr, consumed)) =
            parse_varlen_rows(&norm_lines, i)
        {
            let elem_name = snake(&elem_base);
            let elem_ty = num_ty_for_base(&elem_base, signed);
            for base in &prefix_bases {
                let name = snake(base);
                let ty = num_ty_for_base(base, signed);
                if seen.insert(name.clone()) {
                    extra_lines.push(format!(
                        "let mut {name}: Vec<{ty}> = Vec::with_capacity({count_expr});"
                    ));
                }
            }
            if seen.insert(elem_name.clone()) {
                extra_lines.push(format!(
                    "let mut {elem_name}: Vec<Vec<{elem_ty}>> = Vec::with_capacity({count_expr});"
                ));
            }

            let len_var = format!("{}_i", snake(&len_base));
            let row_var = format!("{}_row", elem_name);
            let mut input_fields = Vec::new();
            let mut push_lines = Vec::new();
            for base in &prefix_bases {
                let name = snake(base);
                let var = format!("{name}_i");
                let ty = if base_var(base).as_deref() == Some(len_base.as_str()) {
                    "usize".to_string()
                } else {
                    num_ty_for_base(base, signed).to_string()
                };
                input_fields.push(format!("{var}: {ty}"));
                push_lines.push(format!("    {name}.push({var});"));
            }
            input_fields.push(format!("{row_var}: [{elem_ty}; {len_var}]"));

            extra_lines.push(format!("for _ in 0..{count_expr} {{"));
            extra_lines.push(format!("    input! {{ {} }};", input_fields.join(", ")));
            for line in push_lines {
                extra_lines.push(line);
            }
            extra_lines.push(format!("    {elem_name}.push({row_var});"));
            extra_lines.push("}".to_string());
            i += consumed;
            continue;
        }

        if let Some((name, ty, consumed)) =
            parse_grid_row_block(&norm_lines, &lines, i, known_h.as_deref())
        {
            needs_chars = true;
            if seen.insert(name.clone()) {
                decls.push(format!("{name}: {ty},"));
            }
            i += consumed;
            continue;
        }
        if let Some((base, w_expr, h_expr, f_expr, consumed)) = parse_3d_array_block(&norm_lines, i)
        {
            let name = snake(&base);
            let elem_ty = num_ty_for_base(&base, signed);
            let ty = format!("[[[{elem_ty}; {w_expr}]; {h_expr}]; {f_expr}]");
            if seen.insert(name.clone()) {
                decls.push(format!("{name}: {ty},"));
            }
            i += consumed;
            continue;
        }
        if let Some((base, w_expr, h_expr, consumed)) =
            parse_matrix_block(&norm_lines, i, known_h.as_deref(), known_w.as_deref())
        {
            let name = snake(&base);
            let elem_ty = num_ty_for_base(&base, signed);
            let ty = format!("[[{elem_ty}; {w_expr}]; {h_expr}]");
            if seen.insert(name.clone()) {
                decls.push(format!("{name}: {ty},"));
            }
            i += consumed;
            continue;
        }
        if let Some((base, col_count, row_count, consumed)) =
            parse_matrix_fixed_block(&norm_lines, i)
        {
            let name = snake(&base);
            let elem_ty = num_ty_for_base(&base, signed);
            let ty = format!("[[{elem_ty}; {col_count}]; {row_count}]");
            if seen.insert(name.clone()) {
                decls.push(format!("{name}: {ty},"));
            }
            i += consumed;
            continue;
        }
        if let Some((name, ty, consumed)) = parse_grid_lines(&norm_lines, i, known_h.as_deref()) {
            needs_chars = true;
            if seen.insert(name.clone()) {
                decls.push(format!("{name}: {ty},"));
            }
            i += consumed;
            continue;
        }
        if let Some((bases, count_expr, consumed)) = parse_n_repeat(&norm_lines, i) {
            let name = snake(&bases.concat());
            let tys = bases
                .iter()
                .map(|b| num_ty_for_base(b, signed))
                .collect::<Vec<_>>()
                .join(", ");
            let ty = format!("[({tys}); {count_expr}]");
            if seen.insert(name.clone()) {
                decls.push(format!("{name}: {ty},"));
            }
            i += consumed;
            continue;
        }
        if let Some((base, count_expr, consumed)) = parse_vertical_scalars(&norm_lines, i) {
            let name = snake(&base);
            let elem_ty = num_ty_for_base(&base, signed);
            let ty = format!("[{elem_ty}; {count_expr}]");
            if seen.insert(name.clone()) {
                decls.push(format!("{name}: {ty},"));
            }
            i += consumed;
            continue;
        }
        if let Some((base, len_expr)) = parse_1d_array_line(ln) {
            let name = snake(&base);
            let ty = if concat_hint {
                "Chars".to_string()
            } else {
                let elem_ty = num_ty_for_base(&base, signed);
                format!("[{elem_ty}; {len_expr}]")
            };
            if concat_hint {
                needs_chars = true;
            }
            if seen.insert(name.clone()) {
                decls.push(format!("{name}: {ty},"));
            }
            i += 1;
            continue;
        }
        if let Some((base, len)) = parse_fixed_indexed_line(ln) {
            let name = snake(&base);
            let elem_ty = num_ty_for_base(&base, signed);
            let ty = format!("[{elem_ty}; {len}]");
            if seen.insert(name.clone()) {
                decls.push(format!("{name}: {ty},"));
            }
            i += 1;
            continue;
        }

        // scalar line like "N M"
        if ln.contains(' ')
            && !ln.contains("\\ldots")
            && !ln.contains("\\cdots")
            && !ln.contains("\\dots")
            && !ln.contains('_')
            && !ln.contains('{')
            && !ln.contains('}')
        {
            for tok in ln.split_whitespace() {
                let name = snake(tok);
                let ty = num_ty_for_base(tok, signed);
                if seen.insert(name.clone()) {
                    decls.push(format!("{name}: {ty},"));
                }
                if name == "h" {
                    known_h = Some("h".to_string());
                }
                if name == "w" {
                    known_w = Some("w".to_string());
                }
            }
            i += 1;
            continue;
        }

        // scalar tokens with subscripts like "S_x S_y"
        if ln.contains(' ') && ln.contains('_') && !ln.contains("\\ldots") {
            let mut ok = true;
            let mut candidates: Vec<(String, String)> = Vec::new();
            for tok in ln.split_whitespace() {
                if let Some((base, idxs)) = parse_indexed_token(tok) {
                    if idxs.len() != 1 {
                        ok = false;
                        break;
                    }
                    let name = snake(&format!("{}_{}", base, idxs[0]));
                    let ty = num_ty_for_base(&base, signed);
                    candidates.push((name, ty.to_string()));
                } else if tok.chars().all(|c| c.is_ascii_alphanumeric()) {
                    let name = snake(tok);
                    let ty = num_ty_for_base(tok, signed);
                    candidates.push((name, ty.to_string()));
                } else {
                    ok = false;
                    break;
                }
            }
            if ok {
                for (name, ty) in candidates {
                    if seen.insert(name.clone()) {
                        decls.push(format!("{name}: {ty},"));
                    }
                }
                i += 1;
                continue;
            }
        }

        // single symbol line
        if !ln.contains(' ')
            && !ln.contains("\\ldots")
            && !ln.contains("\\cdots")
            && !ln.contains("\\dots")
        {
            let sym = ln.trim();
            let name = snake(sym);
            let ty = if sym.eq_ignore_ascii_case("T") && t_is_testcases {
                "usize".to_string()
            } else if is_string_symbol(sym) {
                needs_chars = true;
                "Chars".to_string()
            } else {
                num_ty_for_base(sym, signed).to_string()
            };
            if seen.insert(name.clone()) {
                decls.push(format!("{name}: {ty},"));
            }
            i += 1;
            continue;
        }

        decls.push(format!("/* TODO: {orig} */"));
        i += 1;
    }

    GuessResult {
        decls,
        needs_chars,
        extra_lines,
    }
}

/// Render a single task section into a Rust `main` template.
///
/// The output is a minimal skeleton with `input!` and optional loops for
/// testcases or queries. It aims for readability rather than completeness.
fn render_section(task: &TaskSection) -> anyhow::Result<String> {
    let all_lines: Vec<String> = task.input_blocks.iter().flatten().cloned().collect();
    let has_cases = all_lines.iter().any(|l| is_case_placeholder_line(l));
    let has_queries = all_lines.iter().any(|l| is_query_placeholder_line(l));

    let first = task
        .input_blocks
        .first()
        .with_context(|| format!("{}: missing input format <pre>", task.letter))?;
    let signed = signed_bases(&task.constraints_items);
    let GuessResult {
        decls,
        needs_chars,
        extra_lines,
    } = guess_input_from_lines(first, &signed);
    let mut out: Vec<String> = Vec::new();
    if needs_chars {
        out.push("use proconio::{input, marker::Chars};".to_string());
    } else {
        out.push("use proconio::input;".to_string());
    }
    out.push(String::new());
    out.push("fn main() {".to_string());

    if !has_cases && !has_queries {
        out.push("    input! {".to_string());
        for d in &decls {
            out.push(format!("        {d}"));
        }
        out.push("    };".to_string());
        for l in &extra_lines {
            out.push(format!("    {l}"));
        }
        out.push("}".to_string());
        return Ok(out.join("\n"));
    }

    // Header
    out.push("    input! {".to_string());
    for d in decls {
        out.push(format!("        {d}"));
    }
    out.push("    };".to_string());
    for l in extra_lines {
        out.push(format!("    {l}"));
    }
    out.push(String::new());

    if has_cases {
        if task.input_blocks.len() >= 2 {
            let GuessResult {
                decls: case_decls,
                needs_chars: case_needs_chars,
                extra_lines: case_extra_lines,
            } = guess_input_from_lines(&task.input_blocks[1], &signed);
            if case_needs_chars && !needs_chars {
                out[0] = "use proconio::{input, marker::Chars};".to_string();
            }
            out.push("    for _ in 0..t {".to_string());
            out.push("        input! {".to_string());
            for d in case_decls {
                out.push(format!("            {d}"));
            }
            out.push("        };".to_string());
            for l in case_extra_lines {
                out.push(format!("        {l}"));
            }
            out.push("        /* solve testcase */".to_string());
            out.push("    }".to_string());
            out.push("}".to_string());
            return Ok(out.join("\n"));
        }
        out.push("    for _ in 0..t {".to_string());
        out.push("        input! { /* per-testcase fields */ };".to_string());
        out.push("        /* solve testcase */".to_string());
        out.push("    }".to_string());
        out.push("}".to_string());
        return Ok(out.join("\n"));
    }

    // Queries
    out.push("    for _ in 0..q {".to_string());
    let mut qtypes: Vec<(i32, Vec<String>)> = Vec::new();
    let mut sym_types: Vec<(String, Vec<String>)> = Vec::new();
    let mut sym_name: Option<String> = None;
    let mut mixed_symbol = false;
    for b in task.input_blocks.iter().skip(1) {
        if b.len() != 1 {
            continue;
        }
        let toks: Vec<&str> = b[0].split_whitespace().collect();
        if toks.is_empty() {
            continue;
        }
        if let Ok(qt) = toks[0].parse::<i32>() {
            let rest = toks[1..].iter().map(|s| s.to_string()).collect();
            qtypes.push((qt, rest));
            continue;
        }
        let name = snake(toks[0]);
        if let Some(prev) = &sym_name {
            if *prev != name {
                mixed_symbol = true;
            }
        } else {
            sym_name = Some(name.clone());
        }
        let rest = toks[1..].iter().map(|s| s.to_string()).collect();
        sym_types.push((name, rest));
    }
    if !qtypes.is_empty() {
        qtypes.sort_by_key(|x| x.0);
        out.push("        input! { qt: usize };".to_string());
        out.push("        match qt {".to_string());
        for (qt, toks) in qtypes {
            if toks.is_empty() {
                out.push(format!("            {qt} => {{}},"));
            } else {
                let inner = toks
                    .iter()
                    .map(|t| format!("{}: {}", snake(t), num_ty_for_token(t, &signed)))
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push(format!("            {qt} => {{ input! {{ {inner} }}; }},"));
            }
        }
        out.push("            _ => unreachable!(),".to_string());
        out.push("        }".to_string());
    } else if !sym_types.is_empty() && !mixed_symbol {
        if sym_types.len() == 1 {
            let toks = &sym_types[0].1;
            let mut all = vec![sym_name.unwrap_or_else(|| "t".to_string())];
            all.extend(toks.iter().map(|t| snake(t)));
            let inner = all
                .iter()
                .map(|t| format!("{t}: {}", num_ty_for_token(t, &signed)))
                .collect::<Vec<_>>()
                .join(", ");
            out.push(format!("        input! {{ {inner} }};"));
        } else {
            let qt_name = sym_name.unwrap_or_else(|| "t".to_string());
            let qt_ty = num_ty_for_token(&qt_name, &signed);
            out.push(format!("        input! {{ {qt_name}: {qt_ty} }};"));
            out.push(format!("        match {qt_name} {{"));
            for (idx, (_, toks)) in sym_types.iter().enumerate() {
                let qt = idx + 1;
                if toks.is_empty() {
                    out.push(format!("            {qt} => {{}},"));
                } else {
                    let inner = toks
                        .iter()
                        .map(|t| format!("{}: {}", snake(t), num_ty_for_token(t, &signed)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    out.push(format!("            {qt} => {{ input! {{ {inner} }}; }},"));
                }
            }
            out.push("            _ => unreachable!(),".to_string());
            out.push("        }".to_string());
        }
    } else {
        out.push("        /* TODO: per-query fields */".to_string());
    }
    out.push("        /* process query */".to_string());
    out.push("    }".to_string());
    out.push("}".to_string());
    Ok(out.join("\n"))
}

/// Generate templates for all tasks found in `dest_dir/task.html`.
///
/// Returns a map of destination source paths to generated file contents.
pub(crate) fn generate_template(
    dest_dir: &Utf8Path,
    shell: &mut Shell,
) -> anyhow::Result<Option<HashMap<Utf8PathBuf, String>>> {
    let task_path = dest_dir.join("task.html");
    if !task_path.exists() {
        return Ok(None);
    }
    let html =
        fs::read_to_string(&task_path).with_context(|| format!("failed to read {task_path}"))?;
    let sections = parse_task_sections(&html);
    let src_dir = dest_dir.join("src").join("bin");
    let mut out: HashMap<Utf8PathBuf, String> = HashMap::new();
    for task in &sections {
        let src_path = src_dir
            .join(task.letter.to_kebab_case())
            .with_extension("rs");
        match render_section(task) {
            Ok(content) => {
                out.insert(src_path, content);
            }
            Err(err) => {
                shell.warn(format!("render_section failed at {}: {err}", task.letter))?;
            }
        }
    }
    Ok(Some(out))
}

#[cfg(test)]
#[path = "input_template_tests.rs"]
mod tests;
