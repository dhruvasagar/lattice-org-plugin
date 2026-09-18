//! OA.30 — agenda marks: which rows a bulk action acts on.
//!
//! Design: `lattice/docs/dev/architecture/plugin-multibuffer-views.md` §10.
//! Slice plan: `lattice/docs/dev/operations/slice-plans/org-agenda.md` OA.30.
//!
//! Emacs marks agenda rows with `m` and acts on the marked set with `x`
//! (`org-agenda-bulk-action`); evil-org-agenda keeps both letters. The host
//! half of this landed first: a guest could paint a mark into the gutter but
//! the host re-ran a decoration producer only when the registry or the buffer's
//! TEXT changed, and a mark is guest state over a read-only view — so a mark
//! would have painted once and then frozen. `host-services.refresh-decorations`
//! is what makes this side possible.
//!
//! ## A mark names a SOURCE line, not a view row
//!
//! An agenda row is an excerpt of a file. Its row number changes with every
//! refresh — `gr`, a filter, a span change, or a scan that found one more
//! entry — while the entry it shows does not. So a mark records
//! `path:line` (`excerpt-source`'s answer), and the paint resolves the other
//! way: for each row on screen, ask which source line it shows and look that
//! up. Marks therefore survive `gr`, which is the whole point of marking
//! several rows before acting on them.
//!
//! ## …and lives in the plugin store
//!
//! Chords run on the grammar seam, the paint on the decorations seam, and the
//! bulk menu on the transient seam — three `wasmtime::Store`s with three
//! separate memories. A `thread_local` mark set would be three drifting sets,
//! each internally consistent (`guest-state-does-not-cross-seams`). The store
//! is the one place all three see, and it is already how a capture finds its
//! state.
//!
//! The store is on disk, so marks outlive a restart. That is the same fact as
//! surviving `gr` rather than a separate decision: a mark names an ENTRY, and
//! the entry is still there tomorrow. It cannot paint anywhere unexpected —
//! the producer only ever marks a row the current view is showing — and `M`
//! clears the set. The alternative, clearing on boot, would need a notion of
//! session this seam does not have, to undo something the user did on purpose.

/// The store prefix every mark lives under. Not [`crate::capture_drafts`]'s,
/// so a drafts-picker scan never meets one.
pub const MARK_PREFIX: &str = "agenda-mark/";

/// The sign a marked row paints, before the host's `org.` namespacing.
pub const MARK_SIGN: &str = "agenda-mark";

/// The store key for the entry at `line` of `path`.
///
/// One direction only, and deliberately: the paint resolves view-ward — for
/// each row on screen, ask which source line it shows and look THAT up — so
/// nothing ever needs to read an entry back out of a key. A parser would be a
/// second spelling of this format with no caller to keep it honest.
pub fn mark_key(path: &str, line: u32) -> String {
    format!("{MARK_PREFIX}{path}:{line}")
}

/// The glyph a marked row paints, in BOTH palettes.
///
/// `>` is org's own `org-agenda-bulk-mark-char`, and it is one cell in an
/// ASCII font and a patched one alike — so the icon-degradation rule is
/// satisfied by the glyph rather than by a pair that has to be kept the same
/// width by hand. A nerd-font variant would look tidier and buy nothing: the
/// mark says "this row is selected", which is not a thing an icon says better.
pub const MARK_GLYPH: &str = ">";

/// The theme element the glyph is painted in, before namespacing.
pub const MARK_THEME_ELEMENT: &str = "org.agenda-mark";

/// How many marks the echo names, worded for one or many.
///
/// The count is what a bulk verb is about to act on, so it is stated rather
/// than implied — "3 rows" reads as a decision, "done" reads as a guess.
pub fn marked_count_label(n: usize) -> String {
    match n {
        1 => "1 row".to_string(),
        n => format!("{n} rows"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The format, pinned: prefix, path, `:`, line. The decorations producer
    /// builds a key this way for every row it paints and the chords build one
    /// the same way to store it, so a drift here is a mark that is written and
    /// never found.
    #[test]
    fn a_key_is_the_prefix_then_the_path_then_the_line() {
        assert_eq!(
            mark_key("/home/u/org/work.org", 41),
            "agenda-mark//home/u/org/work.org:41"
        );
    }

    /// A path may legally contain a colon. The line is the LAST field, so the
    /// key stays unambiguous without escaping the path.
    #[test]
    fn a_path_with_a_colon_keeps_the_line_last() {
        let key = mark_key("/tmp/odd:name/work.org", 7);
        assert!(key.ends_with(":7"), "{key}");
        assert_eq!(key.matches(':').count(), 2, "{key}");
    }

    /// Two different entries never share a key — including the pair a
    /// naive `path + line` concatenation would collide.
    #[test]
    fn a_line_boundary_cannot_collide() {
        assert_ne!(mark_key("/a/x.org", 11), mark_key("/a/x.org:1", 1));
    }

    #[test]
    fn the_count_reads_as_a_sentence() {
        assert_eq!(marked_count_label(1), "1 row");
        assert_eq!(marked_count_label(4), "4 rows");
    }
}
