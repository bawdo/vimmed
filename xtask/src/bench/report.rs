//! Threshold verdicts and the Linux stage budget table.

use super::Platform;
use super::stats::{max, median, nearest_rank, sorted};
use marks::names;
use std::collections::BTreeMap;
use std::fmt::Write;

pub const COLD_MEDIAN_MS: f64 = 250.0;
pub const COLD_P95_MS: f64 = 400.0;
pub const KEY_P95_MS: f64 = 16.0;
pub const KEY_MIN_SAMPLES: usize = 500;
pub const GLASS_PERIODS: f64 = 2.5;

/// Linux stage budgets in ms: (stage, from mark, to mark, budget). T0 is 0 ms.
pub const STAGES: [(&str, Option<&str>, &str, f64); 5] = [
    ("launcher", None, names::WINDOW_MAIN, 65.0),
    ("prep", Some(names::WINDOW_MAIN), names::FONTS_LOADED, 20.0),
    (
        "toolkit",
        Some(names::FONTS_LOADED),
        names::WINDOW_CREATED,
        40.0,
    ),
    (
        "renderer",
        Some(names::WINDOW_CREATED),
        names::FIRST_PAINT,
        80.0,
    ),
    (
        "hand-off",
        Some(names::FIRST_PAINT),
        names::FIRST_FRAME,
        40.0,
    ),
];

fn verdict(ok: bool) -> &'static str {
    if ok { "PASS" } else { "FAIL" }
}

/// Median duration of each stage as (stage, median_ms, budget_ms). Stages whose
/// marks are missing are left out. Runs are paired when both marks have the same
/// number of samples, otherwise medians are subtracted.
pub fn stages(marks: &BTreeMap<String, Vec<f64>>) -> Vec<(String, f64, f64)> {
    let mut out = Vec::new();
    for (stage, from, to, budget) in STAGES {
        let Some(to_v) = marks.get(to).filter(|v| !v.is_empty()) else {
            continue;
        };
        let ms = match from {
            None => median(to_v),
            Some(f) => {
                let Some(from_v) = marks.get(f).filter(|v| !v.is_empty()) else {
                    continue;
                };
                if from_v.len() == to_v.len() {
                    let d: Vec<f64> = to_v.iter().zip(from_v).map(|(t, f)| t - f).collect();
                    median(&d)
                } else {
                    median(to_v) - median(from_v)
                }
            }
        };
        out.push((stage.to_string(), ms, budget));
    }
    out
}

/// `key_draw_ms` grouped by input kind, by redraw kind and by both, from the
/// `key_draw_kind_ms.<input>.<redraw>` samples. Empty when a log has none.
fn draw_by_kind(samples: &BTreeMap<String, Vec<f64>>) -> String {
    let mut by_input: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    let mut by_redraw: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    let mut by_both: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for (name, v) in samples {
        if let Some((input, redraw)) = marks::parse_key_draw_kind(name) {
            by_input.entry(input).or_default().extend(v);
            by_redraw.entry(redraw).or_default().extend(v);
            by_both
                .entry(format!("{input}.{redraw}"))
                .or_default()
                .extend(v);
        }
    }
    let mut s = String::new();
    if by_both.is_empty() {
        return s;
    }
    let mut table = |title: &str, rows: Vec<(&str, &Vec<f64>)>| {
        let _ = writeln!(s, "{} by {title}", names::KEY_DRAW_MS);
        for (kind, v) in rows {
            let sv = sorted(v);
            let _ = writeln!(
                s,
                "  {kind}: n={} median {:.1} p95 {:.1} max {:.1}",
                v.len(),
                nearest_rank(&sv, 0.5),
                nearest_rank(&sv, 0.95),
                max(v),
            );
        }
    };
    table("input", by_input.iter().map(|(k, v)| (*k, v)).collect());
    table("redraw", by_redraw.iter().map(|(k, v)| (*k, v)).collect());
    table(
        "input and redraw",
        by_both.iter().map(|(k, v)| (k.as_str(), v)).collect(),
    );
    s
}

pub fn summarise(samples: &BTreeMap<String, Vec<f64>>, platform: Platform) -> String {
    let mut s = String::new();

    for (name, v) in samples {
        // Shown grouped below, not one line per kind.
        if marks::parse_key_draw_kind(name).is_some() {
            continue;
        }
        let sv = sorted(v);
        let _ = writeln!(
            s,
            "{name}: n={} median {:.1} p95 {:.1} p99 {:.1} max {:.1}",
            v.len(),
            nearest_rank(&sv, 0.5),
            nearest_rank(&sv, 0.95),
            nearest_rank(&sv, 0.99),
            max(v),
        );
    }
    s.push('\n');
    let kinds = draw_by_kind(samples);
    if !kinds.is_empty() {
        s.push_str(&kinds);
        s.push('\n');
    }

    if let Some(v) = samples.get(names::FIRST_FRAME).filter(|v| !v.is_empty()) {
        let sv = sorted(v);
        let med = nearest_rank(&sv, 0.5);
        let p95 = nearest_rank(&sv, 0.95);
        let _ = writeln!(
            s,
            "cold start: median {med:.1} ms (<= {COLD_MEDIAN_MS:.0} {})",
            verdict(med <= COLD_MEDIAN_MS)
        );
        let _ = writeln!(
            s,
            "cold start: p95 {p95:.1} ms (<= {COLD_P95_MS:.0} {})",
            verdict(p95 <= COLD_P95_MS)
        );
    }

    if let Some(v) = samples.get(names::KEY_MS) {
        if v.len() < KEY_MIN_SAMPLES {
            let _ = writeln!(
                s,
                "keystroke: INSUFFICIENT ({} < {KEY_MIN_SAMPLES})",
                v.len()
            );
        } else {
            let p95 = nearest_rank(&sorted(v), 0.95);
            let _ = writeln!(
                s,
                "keystroke: p95 {p95:.1} ms (<= {KEY_P95_MS:.0} {})",
                verdict(p95 <= KEY_P95_MS)
            );
        }
    }

    if let Some(v) = samples.get(names::KEY_GLASS_MS).filter(|v| !v.is_empty()) {
        let p95 = nearest_rank(&sorted(v), 0.95);
        match samples
            .get(names::REFRESH_HZ)
            .and_then(|h| h.first())
            .copied()
        {
            Some(hz) if hz > 0.0 => {
                let periods = p95 * hz / 1000.0;
                let _ = writeln!(
                    s,
                    "on-glass: p95 {p95:.1} ms, {periods:.2} refresh periods (<= {GLASS_PERIODS} {}) at {hz:.0} Hz",
                    verdict(periods <= GLASS_PERIODS)
                );
            }
            _ => {
                let _ = writeln!(s, "on-glass: p95 {p95:.1} ms (refresh_hz missing)");
            }
        }
    }

    if platform == Platform::Linux {
        let st = stages(samples);
        if !st.is_empty() {
            s.push_str("\nstage budget (median ms)\n");
            let mut total = 0.0;
            let mut budget_total = 0.0;
            for (name, ms, budget) in &st {
                total += ms;
                budget_total += budget;
                let flag = if ms > budget { "OVER" } else { "ok" };
                let _ = writeln!(s, "{name} {ms:.1} / {budget:.0} {flag}");
            }
            let _ = writeln!(s, "total {total:.1} / {budget_total:.0}");
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pairs: &[(&str, Vec<f64>)]) -> BTreeMap<String, Vec<f64>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn cold_start_verdicts() {
        let a = summarise(&m(&[("first_frame", vec![240.0])]), Platform::Macos);
        let b = summarise(&m(&[("first_frame", vec![260.0])]), Platform::Macos);
        assert!(
            a.contains("cold start: median 240.0 ms (<= 250 PASS)"),
            "{a}"
        );
        assert!(b.contains("(<= 250 FAIL)"), "{b}");
    }

    #[test]
    fn keystroke_needs_500_samples() {
        let s = summarise(&m(&[("key_ms", vec![5.0; 499])]), Platform::Macos);
        assert!(s.contains("keystroke: INSUFFICIENT (499 < 500)"), "{s}");
        let s = summarise(&m(&[("key_ms", vec![5.0; 500])]), Platform::Macos);
        assert!(s.contains("keystroke: p95 5.0 ms (<= 16 PASS)"), "{s}");
    }

    #[test]
    fn glass_reported_in_refresh_periods() {
        let s = summarise(
            &m(&[("key_glass_ms", vec![40.0]), ("refresh_hz", vec![60.0])]),
            Platform::Macos,
        );
        assert!(s.contains("2.40 refresh periods (<= 2.5 PASS)"), "{s}");
    }

    #[test]
    fn bench_summary_groups_draw_by_kind() {
        let mut samples = BTreeMap::new();
        super::super::parse_log(
            "key_draw_ms,1.000\nkey_draw_kind_ms.text.lines,1.000\n\
             key_draw_ms,30.000\nkey_draw_kind_ms.enter.lines,30.000\n\
             key_draw_ms,2.000\nkey_draw_kind_ms.text.lines,2.000\n\
             key_draw_ms,0.500\nkey_draw_kind_ms.arrow.status,0.500\n\
             key_draw_ms,4.000\nkey_draw_kind_ms.enter.all,4.000\n",
            &mut samples,
        );
        let s = summarise(&samples, Platform::Macos);
        assert!(s.contains("key_draw_ms: n=5 median 2.0"), "{s}");
        assert!(s.contains("key_draw_ms by input\n"), "{s}");
        assert!(
            s.contains("  enter: n=2 median 4.0 p95 30.0 max 30.0\n"),
            "{s}"
        );
        assert!(
            s.contains("  text: n=2 median 1.0 p95 2.0 max 2.0\n"),
            "{s}"
        );
        assert!(s.contains("key_draw_ms by redraw\n"), "{s}");
        assert!(
            s.contains("  lines: n=3 median 2.0 p95 30.0 max 30.0\n"),
            "{s}"
        );
        assert!(s.contains("  status: n=1 median 0.5"), "{s}");
        assert!(s.contains("key_draw_ms by input and redraw\n"), "{s}");
        assert!(s.contains("  enter.all: n=1 median 4.0"), "{s}");
        assert!(
            !s.contains("key_draw_kind_ms."),
            "kind samples only appear grouped: {s}"
        );
    }

    #[test]
    fn bench_summary_without_kinds_is_unchanged() {
        let samples = m(&[
            ("key_draw_ms", vec![1.0, 2.0, 3.0]),
            ("first_frame", vec![200.0]),
        ]);
        let s = summarise(&samples, Platform::Macos);
        assert_eq!(
            s,
            "first_frame: n=1 median 200.0 p95 200.0 p99 200.0 max 200.0\n\
             key_draw_ms: n=3 median 2.0 p95 3.0 p99 3.0 max 3.0\n\
             \n\
             cold start: median 200.0 ms (<= 250 PASS)\n\
             cold start: p95 200.0 ms (<= 400 PASS)\n"
        );
    }

    #[test]
    fn linux_stage_table_flags_over_budget() {
        let marks = m(&[
            ("window_main", vec![70.0]),
            ("fonts_loaded", vec![85.0]),
            ("window_created", vec![120.0]),
            ("first_paint", vec![190.0]),
            ("first_frame", vec![220.0]),
        ]);
        let s = summarise(&marks, Platform::Linux);
        assert!(s.contains("launcher 70.0 / 65 OVER"), "{s}");
        assert!(s.contains("prep 15.0 / 20 ok"), "{s}");
        assert!(s.contains("total 220.0 / 245"), "{s}");
        let mac = summarise(&marks, Platform::Macos);
        assert!(!mac.contains("stage budget"), "{mac}");
    }
}
