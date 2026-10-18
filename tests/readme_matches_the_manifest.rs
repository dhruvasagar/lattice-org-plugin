//! The README's seam table matches the manifest's `provides`.
//!
//! The README claimed **thirteen** seams in its prose and heading while listing
//! fifteen in its table, and the manifest — the only thing that decides what
//! this plugin actually registers — said fifteen. The discrepancy was noticed
//! in lattice's launch plan months before it was fixed, and was deliberately
//! left uncorrected there because neither number could be trusted from outside
//! this repo.
//!
//! A count in prose is exactly the kind of claim that goes stale silently: the
//! seam it forgot still works, so nothing fails, and the only cost is that the
//! document overselling the plugin now undersells it. So the list is read from
//! the manifest and compared, in both directions.
//!
//! This test lives in the COMPONENT package on purpose. It needs `toml` and
//! nothing else — no host crates — so it runs for anyone who cloned this repo,
//! which is the audience the README is for.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::Path;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read(name: &str) -> String {
    let path = repo_root().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// The manifest's TOML: the `MANIFEST` byte string in `src/lib.rs`.
fn manifest_text() -> String {
    let lib = read("src/lib.rs");
    let (_, rest) = lib
        .split_once("const MANIFEST: &[u8] = br#\"")
        .expect("src/lib.rs declares the embedded `MANIFEST`");
    rest.split_once("\"#;")
        .expect("the manifest literal closes")
        .0
        .to_owned()
}

/// The seams this plugin registers, from the manifest that decides it.
fn declared_seams() -> BTreeSet<String> {
    let manifest: toml::Value = toml::from_str(&manifest_text()).expect("the manifest parses");
    manifest
        .get("provides")
        .and_then(|v| v.as_array())
        .expect("the manifest has a `provides` array")
        .iter()
        .map(|v| {
            v.as_str()
                .expect("every `provides` entry is a string")
                .to_owned()
        })
        .collect()
}

/// The first column of the README's `| seam | what org contributes |` table.
fn readme_table_seams() -> BTreeSet<String> {
    let readme = read("README.md");
    let (_, rest) = readme
        .split_once("| seam | what org contributes |")
        .expect("README has the seam table");
    rest.lines()
        // `rest` begins with the tail of the header line (empty) and then the
        // `|---|---|` separator; the rows start after both.
        .skip_while(|l| !l.trim_start().starts_with("|---"))
        .skip(1)
        .take_while(|l| l.trim_start().starts_with('|'))
        .filter_map(|l| {
            let cell = l.trim().trim_start_matches('|').split('|').next()?;
            let name = cell.trim().trim_matches('`');
            (!name.is_empty()).then(|| name.to_owned())
        })
        .collect()
}

fn report(what: &str, declared: &BTreeSet<String>, found: &BTreeSet<String>) {
    let missing: Vec<&String> = declared.difference(found).collect();
    let extra: Vec<&String> = found.difference(declared).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "{what} disagrees with the manifest's `provides` ({} seams).\n  \
         missing from the README: {missing:?}\n  \
         in the README but not declared: {extra:?}",
        declared.len()
    );
}

#[test]
fn the_readme_table_lists_every_declared_seam() {
    let declared = declared_seams();
    assert!(declared.len() >= 10, "parsed `provides` as {declared:?}");
    report("the README's seam table", &declared, &readme_table_seams());
}

#[test]
fn the_readme_does_not_state_a_stale_seam_count_in_prose() {
    let readme = read("README.md");
    let n = declared_seams().len();
    let words = [
        "",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
    ];
    let correct = words.get(n).copied().unwrap_or("");
    // The heading counts every seam; the opening line counts the others beside
    // `language`. Both are prose and both went stale.
    let others = words.get(n - 1).copied().unwrap_or("");
    assert!(
        readme
            .to_lowercase()
            .contains(&format!("{correct} seams from one component")),
        "the heading should read `{correct} seams from one component` — \
         the manifest declares {n}"
    );
    assert!(
        readme.to_lowercase().contains(&format!("{others} others")),
        "the opening should say `and by now of {others} others` — the manifest \
         declares {n} seams, of which `language` is one"
    );
}
