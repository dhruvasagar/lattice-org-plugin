//! NOT COMPILED — parked pending a decision. `tests/disabled/` is not a cargo
//! test target (cargo discovers `tests/*.rs`, and a subdirectory without
//! `main.rs` is ignored), so this file is preserved rather than deleted.
//!
//! These are org's conceal-rule tests, previously a `#[cfg(test)] mod` inside
//! `src/lib.rs`. They cannot live there any more: they need `lattice-syntax`,
//! and cargo resolves `[dev-dependencies]` as part of the BUILD graph, so a
//! lattice path dependency there stops `cargo build --release --target
//! wasm32-wasip2` for anyone without a lattice checkout.
//!
//! They cannot simply move here either. This package would have to link the
//! component to reach org's declared rules, and the component is a `cdylib`
//! whose wit-bindgen imports are only satisfied inside the wasm host — adding
//! `rlib` makes cargo build the cdylib for the host too, which fails to link:
//!
//!     Undefined symbols for architecture arm64
//!
//! Every cheaper route was tried and does not work: optional dev-dependencies
//! are not a thing, `[target.'cfg(...)'.dev-dependencies]` is resolved anyway,
//! and an OPTIONAL path dependency is resolved anyway too.
//!
//! The fix is to give the rule data a home both sides can reach — a small
//! dependency-free crate in this repo holding the patterns, which the component
//! turns into WIT `ConcealRule`s and this test compiles through the host's real
//! engine. That is a change to where org's rules live, so it is Dhruva's call
//! rather than a side effect of a CI change.
//! org's conceal rules, compiled and evaluated by the HOST's own engine.
//!
//! Moved out of `src/lib.rs` when the component's dependencies became
//! crates.io versions: these tests need `lattice-syntax`, a path dependency on
//! a lattice checkout, and cargo resolves `[dev-dependencies]` as part of the
//! BUILD graph — so leaving them there would have meant the component could not
//! be built by anyone without that checkout, which is the bug the split fixed.
//!
//! They still run the real evaluator rather than a re-implementation. That is
//! the point of them: a hand-rolled union in the plugin could agree with itself
//! forever while disagreeing with the thing that actually renders the buffer.

use lattice_org_plugin::declared_conceal_rules;
use lattice_syntax::conceal::{compile_rules, conceal_spans};

/// Compile org's declared rules through the HOST's own compiler.
///
/// These tests run the real evaluator rather than a re-implementation
/// of it. That matters more than convenience: a hand-rolled union in this crate
/// could agree with itself forever while disagreeing with the thing
/// that actually renders the buffer.
fn compiled() -> Vec<lattice_syntax::conceal::ConcealRule> {
    let declared: Vec<(String, Vec<u32>, Option<String>)> = declared_conceal_rules()
        .into_iter()
        .map(|r| (r.pattern, r.hide, r.slot))
        .collect();
    let (ok, errs) = compile_rules(&declared, None);
    assert!(
        errs.is_empty(),
        "org must ship rules the host accepts: {errs:?}"
    );
    assert_eq!(ok.len(), declared.len());
    ok
}

/// What the user actually sees, via the host's span union.
fn rendered(line: &str) -> String {
    let rules = compiled();
    let mut out = String::new();
    let mut at = 0usize;
    for (s, e) in conceal_spans(&rules, line) {
        out.push_str(&line[at..s as usize]);
        at = e as usize;
    }
    out.push_str(&line[at..]);
    out
}

/// The registration gate, asserted here rather than discovered as a
/// `warn` in someone's editor: every rule org ships must compile,
/// name only groups its pattern has, and hide at least one that is
/// not group 0.
#[test]
fn every_rule_org_ships_is_one_the_host_accepts() {
    let _ = compiled();
    for r in declared_conceal_rules() {
        assert!(!r.hide.is_empty(), "{}", r.pattern);
        assert!(!r.hide.contains(&0), "{}", r.pattern);
    }
}

#[test]
fn a_described_link_collapses_to_its_description() {
    assert_eq!(
        rendered("* See [[id:6F398E54][Project Kickoff]] before Friday."),
        "* See Project Kickoff before Friday."
    );
}

/// A link whose only text IS its target has nothing left to show
/// once the target is hidden, so the target stays. Emacs draws the
/// same line.
#[test]
fn a_bare_link_keeps_its_target() {
    assert_eq!(
        rendered("see [[https://example.com]] ok"),
        "see https://example.com ok"
    );
}

#[test]
fn two_links_on_one_line_both_collapse() {
    assert_eq!(rendered("[[id:A][one]] and [[id:B][two]]"), "one and two");
}

#[test]
fn a_file_link_collapses_like_any_other() {
    assert_eq!(
        rendered("[[file:img/diagram.png][a wiring diagram]]"),
        "a wiring diagram"
    );
}

#[test]
fn a_malformed_link_is_left_entirely_alone() {
    let line = "[[id:6F39][unterminated";
    assert_eq!(rendered(line), line);
}

/// The property the retracted "declaration order matters" claim was
/// actually worried about, asserted as what is true: the two
/// patterns cannot both match one link. `[^]]+` stops at the first
/// `]`, so the bare pattern never reaches a described link's closing
/// `]]`.
///
/// Pinned in the crate that OWNS the patterns, because it is a
/// property of how they are written — a future edit that made them
/// overlap would reintroduce exactly the failure the old rule
/// feared, and nothing else would catch it.
#[test]
fn the_two_patterns_are_disjoint_by_construction() {
    let rules = compiled();
    let with_desc = "[[id:6F39][Project Kickoff]]";
    assert!(rules[0].pattern().find(with_desc).is_some());
    assert!(
        rules[1].pattern().find(with_desc).is_none(),
        "the bare pattern must not match a described link"
    );

    let plain = "[[https://example.com]]";
    assert!(rules[1].pattern().find(plain).is_some());
    assert!(rules[0].pattern().find(plain).is_none());
}

/// Order-independence, from the side that declares the rules.
#[test]
fn declaring_them_the_other_way_round_renders_the_same() {
    let mut declared: Vec<(String, Vec<u32>, Option<String>)> = declared_conceal_rules()
        .into_iter()
        .map(|r| (r.pattern, r.hide, r.slot))
        .collect();
    declared.reverse();
    let (reversed, _) = compile_rules(&declared, None);
    for line in [
        "[[id:6F39][Project Kickoff]]",
        "see [[https://example.com]] ok",
        "[[id:A][one]] and [[id:B][two]]",
    ] {
        assert_eq!(
            conceal_spans(&compiled(), line),
            conceal_spans(&reversed, line),
            "{line}"
        );
    }
}

/// **OL.1: the part of a link that stays on screen is styled.**
///
/// Reported as links rendering plain. Conceal made that worse rather than
/// milder — it hides the brackets and the target, so what remains is bare
/// prose with nothing saying `<CR>` under the cursor would go anywhere.
///
/// Asserted DISJOINT from the concealed ranges, which is the whole point:
/// a style covering only the hidden bytes would look correct in a span
/// dump and paint nothing at all.
#[test]
fn ol1_the_visible_part_of_a_link_is_styled() {
    use lattice_syntax::conceal::conceal_style_spans;
    let rules = compiled();

    let line = "see [[id:6F39][Project Kickoff]] ok";
    let hidden = conceal_spans(&rules, line);
    let styled = conceal_style_spans(&rules, line);
    assert_eq!(styled.len(), 1, "one styled run: {styled:?}");
    let (s, e, style) = styled[0];
    assert_eq!(
        &line[s as usize..e as usize],
        "Project Kickoff",
        "the DESCRIPTION survives conceal, so it is what gets styled"
    );
    assert_eq!(style, lattice_syntax::Style::Link);
    for (hs, he) in &hidden {
        assert!(
            e <= *hs || s >= *he,
            "styled {s}..{e} overlaps hidden {hs}..{he} — it would paint \
             bytes that are about to disappear"
        );
    }

    // The bare form styles the target, which is what it shows.
    let line = "see [[https://example.com]] ok";
    let styled = conceal_style_spans(&rules, line);
    assert_eq!(styled.len(), 1, "{styled:?}");
    let (s, e, style) = styled[0];
    assert_eq!(&line[s as usize..e as usize], "https://example.com");
    assert_eq!(style, lattice_syntax::Style::Url);

    // Prose is untouched — the rules match links, not bracket-ish text.
    assert!(conceal_style_spans(&rules, "no links on this line").is_empty());
}
