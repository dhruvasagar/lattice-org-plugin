//! OR.11a — `${field}`: what a roam template knows about the node it is making.
//!
//! Design: `docs/dev/architecture/org-roam.md` §6.3.
//!
//! ## Two placeholder syntaxes, because they answer different questions
//!
//! `%` interpolates the **capture context** — when you pressed the key, what
//! buffer you were in, what you typed ([`crate::capture`] owns all of it).
//! `${}` interpolates the **node being created**: its title, its slug, its id.
//! Neither can express the other. `%^{Title}` would prompt for a title the
//! picker already asked for, and `${a}` has no meaning in a plain org capture
//! that creates no node.
//!
//! So they coexist rather than compete, and a template uses both — the
//! reference corpus's `pkos-concept.org` opens
//!
//! ```org
//! #+Title: ${title}
//! …
//! :QUESTION: %^{What one question does this answer?}
//! ```
//!
//! ## This is a correctness fix, not a nicety
//!
//! Ten of the eleven templates in the reference corpus contain `${title}`.
//! Without expansion they do not degrade — they write the characters
//! `${title}` into the user's new note, as the first line, as the title. Every
//! note made from a template is then wrong in the one field that names it.
//!
//! ## Unknown fields survive verbatim
//!
//! [`crate::capture`]'s rule for unknown `%x`, for its reason: a template is
//! user text, and a `${autor}` that did not expand can be found and fixed,
//! while one that vanished cannot. A typo must not silently eat a line.

/// What a roam template may interpolate about the node being created.
#[derive(Debug, Clone, Default)]
pub struct Node<'a> {
    /// The title the user typed into the picker.
    pub title: &'a str,
    /// The filename slug derived from it — `roam_find::slug`'s output.
    pub slug: &'a str,
    /// The freshly-minted `:ID:`.
    pub id: &'a str,
    /// CD.7: a link back to the capture this note was started from, or empty
    /// when there is none (or the option is off).
    pub origin: &'a str,
}

/// Expand `${title}` / `${slug}` / `${id}` in `body`.
///
/// Runs **before** [`crate::capture::expand_with`], never instead of it: a
/// template carrying both syntaxes needs both passes, and doing `${}` first
/// means a title containing a literal `%U` is inserted as text rather than
/// being re-read as a capture placeholder. That ordering is the safe one —
/// user data must not become template syntax.
///
/// `$` that does not open a `${` is literal, so a template mentioning a shell
/// variable or a price is untouched.
pub fn expand_fields(body: &str, node: &Node<'_>) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let Some(close) = after.find('}') else {
            // An unterminated `${` is the rest of the template, verbatim.
            // Scanning on would silently swallow everything after a typo.
            out.push_str(&rest[at..]);
            return out;
        };
        let name = &after[..close];
        match name {
            "title" => out.push_str(node.title),
            "slug" => out.push_str(node.slug),
            "id" => out.push_str(node.id),
            "origin" => out.push_str(node.origin),
            // Verbatim, braces included — see the module note.
            _ => {
                out.push_str("${");
                out.push_str(name);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node<'a>(title: &'a str, slug: &'a str, id: &'a str) -> Node<'a> {
        Node {
            title,
            slug,
            id,
            origin: "",
        }
    }

    /// CD.7: `${origin}` is the reference it was given, and empty without one.
    #[test]
    fn origin_expands_to_the_reference() {
        let with = Node {
            origin: "[[id:P][Parent]]",
            ..node("T", "t", "I")
        };
        assert_eq!(
            expand_fields("From: ${origin}", &with),
            "From: [[id:P][Parent]]"
        );
        assert_eq!(
            expand_fields("From: ${origin}", &node("T", "t", "I")),
            "From: "
        );
    }

    #[test]
    fn the_three_fields_expand() {
        let n = node("Rust Async", "rust_async", "ABC-123");
        assert_eq!(
            expand_fields("#+title: ${title}", &n),
            "#+title: Rust Async"
        );
        assert_eq!(expand_fields("${slug}.org", &n), "rust_async.org");
        assert_eq!(expand_fields(":ID: ${id}", &n), ":ID: ABC-123");
    }

    #[test]
    fn several_fields_in_one_body_all_expand() {
        let n = node("Rust Async", "rust_async", "ABC-123");
        assert_eq!(
            expand_fields(":ID: ${id}\n#+title: ${title}\n# ${slug}", &n),
            ":ID: ABC-123\n#+title: Rust Async\n# rust_async"
        );
    }

    /// An unknown field survives with its braces. A typo must be findable.
    #[test]
    fn an_unknown_field_is_left_verbatim() {
        let n = node("T", "t", "I");
        assert_eq!(expand_fields("${autor}", &n), "${autor}");
        assert_eq!(expand_fields("a ${x} b ${title}", &n), "a ${x} b T");
    }

    /// An unterminated `${` takes the rest of the template with it, verbatim —
    /// rather than scanning to a `}` on some later line and eating everything
    /// between, which is what a typo would otherwise cost.
    #[test]
    fn an_unterminated_field_does_not_eat_the_template() {
        let n = node("T", "t", "I");
        assert_eq!(
            expand_fields("#+title: ${title\nbody stays", &n),
            "#+title: ${title\nbody stays"
        );
    }

    /// A bare `$` is not a placeholder. Templates mention prices and shell
    /// variables.
    #[test]
    fn a_dollar_that_opens_nothing_is_literal() {
        let n = node("T", "t", "I");
        assert_eq!(
            expand_fields("costs $5 and $HOME", &n),
            "costs $5 and $HOME"
        );
        assert_eq!(expand_fields("$", &n), "$");
    }

    #[test]
    fn a_body_with_no_fields_is_unchanged() {
        let n = node("T", "t", "I");
        assert_eq!(expand_fields("* plain\nbody\n", &n), "* plain\nbody\n");
        assert_eq!(expand_fields("", &n), "");
    }

    /// `%` placeholders are NOT this pass's business — they must survive it
    /// untouched so `capture::expand_with` can do its half afterwards.
    #[test]
    fn capture_placeholders_pass_through_untouched() {
        let n = node("Rust", "rust", "I");
        assert_eq!(
            expand_fields("#+title: ${title}\n:Q: %^{Question}\n%U\n%?", &n),
            "#+title: Rust\n:Q: %^{Question}\n%U\n%?"
        );
    }

    /// A title containing template syntax is inserted as TEXT. This is why
    /// `${}` runs before `%`: user data must not become template syntax.
    #[test]
    fn a_title_is_data_not_syntax() {
        let n = node("100% done", "100_done", "I");
        assert_eq!(expand_fields("#+title: ${title}", &n), "#+title: 100% done");
    }
}

/// Guarantee the node's `:ID:` — org-roam's own rule, not the template's job.
///
/// `org-roam-capture.el` (`org-roam-capture--setup-target-location`):
///
/// ```elisp
/// (if-let ((id (org-entry-get p "ID")))
///     (setf (org-roam-node-id org-roam-capture--node) id)
///   (org-entry-put p "ID" (org-roam-node-id org-roam-capture--node)))
/// ```
///
/// Two halves, and both matter:
///
/// - **An existing `:ID:` is ADOPTED, never replaced.** A template that writes
///   `:ID: ${id}` itself, or a target file that already carries one, keeps the
///   id it has — the node record follows the file rather than the file being
///   rewritten to match a freshly-minted id. Overwriting would silently
///   re-identify a note every link in the corpus already points at.
/// - **An absent one is WRITTEN.** An `:ID:` is not template content; it is what
///   makes the file a node at all. org-roam's own default template carries none
///   and still produces indexable notes, which is the clearest statement that
///   this belongs to the create flow.
///
/// Without this a template that forgets `${id}` produces a file, no error, and a
/// note that never appears in `find-node` — a silent failure whose only symptom
/// is an absence.
///
/// ## Placement
///
/// A file-level drawer must be the **first element in the file**. So: if the
/// body already opens with a `:PROPERTIES:` drawer the id joins it; otherwise a
/// fresh drawer is prepended above everything, `#+title:` included. A drawer
/// sitting further down (after the keywords, say) is NOT the file's property
/// block and is left exactly where the template put it — it is the user's
/// content, and quietly relocating it would be a second surprise on top of the
/// one this fixes.
pub fn ensure_id(body: &str, id: &str) -> String {
    if file_level_id(body).is_some() {
        return body.to_string();
    }
    let mut lines = body.lines();
    if lines.next().map(str::trim) == Some(":PROPERTIES:") {
        // Opens with the file's drawer — put the id first inside it, which is
        // where `org-entry-put` places a new property.
        let rest: Vec<&str> = body.lines().skip(1).collect();
        let mut out = String::from(":PROPERTIES:\n");
        out.push_str(&format!(":ID:       {id}\n"));
        out.push_str(&rest.join("\n"));
        if body.ends_with('\n') {
            out.push('\n');
        }
        return out;
    }
    format!(":PROPERTIES:\n:ID:       {id}\n:END:\n{body}")
}

/// The `:ID:` of the file-level drawer — the one that opens the file — or
/// `None`.
///
/// Deliberately only the FIRST element: a `:PROPERTIES:` block further down
/// belongs to a headline (or to nothing), and treating it as the file's would
/// make a headline's id suppress the file node's.
fn file_level_id(body: &str) -> Option<String> {
    let mut lines = body.lines();
    if lines.next().map(str::trim) != Some(":PROPERTIES:") {
        return None;
    }
    for line in lines {
        let t = line.trim();
        if t == ":END:" {
            return None;
        }
        if let Some(rest) = t.strip_prefix(":ID:") {
            let id = rest.trim();
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod ensure_id_tests {
    use super::*;

    #[test]
    fn a_body_with_no_drawer_gets_one_at_the_very_top() {
        let out = ensure_id("#+title: T\n\n* Heading\n", "ABC");
        assert!(
            out.starts_with(":PROPERTIES:\n:ID:       ABC\n:END:\n#+title: T"),
            "the drawer must precede the keywords — a file-level drawer is the \
             FIRST element or it is not the file's: {out}"
        );
    }

    #[test]
    fn a_body_that_opens_with_a_drawer_gains_the_id_inside_it() {
        let out = ensure_id(":PROPERTIES:\n:TYPE: book\n:END:\n#+title: T\n", "ABC");
        assert_eq!(
            out,
            ":PROPERTIES:\n:ID:       ABC\n:TYPE: book\n:END:\n#+title: T\n"
        );
    }

    /// The half that is easy to get backwards: an id already present is
    /// ADOPTED, not replaced. Overwriting would silently re-identify a note
    /// every existing link points at.
    #[test]
    fn an_existing_id_is_left_alone() {
        let body = ":PROPERTIES:\n:ID:       KEEP-ME\n:END:\n#+title: T\n";
        assert_eq!(ensure_id(body, "FRESH"), body);
    }

    /// A template that spells it itself (`:ID: ${id}`) has already been
    /// expanded by `expand_fields` before this runs, so it looks exactly like
    /// the case above and must not produce a second `:ID:`.
    #[test]
    fn a_template_that_writes_its_own_id_gets_no_second_one() {
        let expanded = expand_fields(
            ":PROPERTIES:\n:ID: ${id}\n:END:\n#+title: ${title}\n",
            &Node {
                title: "T",
                slug: "t",
                id: "ABC",
                origin: "",
            },
        );
        let out = ensure_id(&expanded, "ABC");
        assert_eq!(out.matches(":ID:").count(), 1, "exactly one id: {out}");
    }

    /// **The reported case.** A drawer AFTER the keywords is not the file's
    /// property block, so the id still needs a drawer of its own at the top —
    /// and the template's own drawer stays exactly where it was written.
    #[test]
    fn a_drawer_below_the_keywords_does_not_count_as_the_files() {
        let body = "#+Title: test123\n#+Filetags: :source:book:\n:PROPERTIES:\n:TYPE: source\n:END:\n\n* Why\n";
        let out = ensure_id(body, "ABC");
        assert!(out.starts_with(":PROPERTIES:\n:ID:       ABC\n:END:\n#+Title: test123"));
        assert!(
            out.contains(":TYPE: source"),
            "the template's own drawer is left where it put it: {out}"
        );
        assert_eq!(out.matches(":PROPERTIES:").count(), 2);
    }

    #[test]
    fn an_empty_id_line_does_not_count_as_present() {
        let out = ensure_id(":PROPERTIES:\n:ID:\n:END:\n", "ABC");
        assert!(out.contains(":ID:       ABC"), "{out}");
    }
}
