//! Where a finished export goes: a file beside the source, a buffer, or a
//! file that is then opened (lattice's `org-export.md` §5).
//!
//! **The buffer sink re-exports in place.** `open-synthetic-buffer` ignores
//! its seed text for a buffer that exists, so a second export into
//! `*Org HTML Export*` replaces the text with one whole-text `apply-edit`
//! (one undo step) and then shows the buffer. That needs the buffer's id,
//! which only the activation of the minor the buffer opens with hands over.
//! Each backend's buffer has its own minor, so the event's mode name says
//! which buffer it is: the event comes before the editor publishes that the
//! mode is on, so the buffer cannot be read yet to tell them apart. The ids
//! live in the plugin's store, so a reload of the plugin still finds the
//! buffers it made.
//!
//! A buffer the user killed leaves a stale id; its `apply-edit` is dropped
//! and the `display-buffer` after it seeds a new one, so posting both is
//! right either way.
//!
//! **An export never takes the window.** It lands seconds after its chord,
//! a compile much later, and the user is typing elsewhere by then; so every
//! open in lattice is a `display-buffer` into `org.export.open-in`, and a
//! pane already showing the output is reused (lattice's `org-export.md` §5).

use super::pipeline::{file_name, Backend};
use super::Host;
use crate::lattice::plugin_host::types::{
    ApplyEditPayload, DisplayBufferPayload, DisplaySource, DisplayTarget, Edit, EditKind, Effect,
    FileAnchor, OpenSyntheticBufferPayload, Position, Range, WriteToFilePayload,
};

/// Every backend, for the minors registered per backend.
pub(crate) const BACKENDS: [Backend; 3] = [Backend::Html, Backend::Markdown, Backend::Latex];

/// The minor `backend`'s export buffer carries, whose activation tells the
/// plugin the buffer's id.
pub(crate) fn output_mode(backend: Backend) -> &'static str {
    match backend {
        Backend::Html => "org-export-html-output-mode",
        Backend::Markdown => "org-export-markdown-output-mode",
        Backend::Latex => "org-export-latex-output-mode",
    }
}

/// The backend whose export buffer `mode` is the minor of.
pub(crate) fn output_backend(mode: &str) -> Option<Backend> {
    BACKENDS.into_iter().find(|b| output_mode(*b) == mode)
}

/// Where an export's output goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Sink {
    /// Beside the source (`h h`).
    File,
    /// Into `*Org HTML Export*` or its peer (`h H`), shown at `place`.
    Buffer { place: Place },
    /// Beside the source, then opened (`h o`).
    Open(Viewer),
    /// LaTeX beside the source, then compiled to PDF (`l p`), and the PDF
    /// opened as [`Sink::Open`] when `open` is set (`l o`).
    Pdf { open: Option<Viewer> },
}

/// How an export that opens its output shows it (`org.export.open-with`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Viewer {
    /// The system's handler for the file, a window of its own.
    System,
    /// In lattice, beside the source, focus kept.
    Editor(Place),
}

/// Where in lattice an export is shown (`org.export.open-in`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Place {
    Split,
    Vsplit,
    Tab,
}

impl Place {
    fn target(self) -> DisplayTarget {
        match self {
            Place::Split => DisplayTarget::Split,
            Place::Vsplit => DisplayTarget::Vsplit,
            Place::Tab => DisplayTarget::Tab,
        }
    }
}

/// `org.export.open-in` as a place, or why the value is not one.
pub(crate) fn place(value: &str) -> Result<Place, String> {
    match value.trim() {
        "" | "vsplit" => Ok(Place::Vsplit),
        "split" => Ok(Place::Split),
        "tab" => Ok(Place::Tab),
        other => Err(format!(
            "org: org.export.open-in is `{other}` — use vsplit, split or tab"
        )),
    }
}

/// `org.export.open-with` (and, for the editor, `org.export.open-in`) as a
/// viewer, or why a value is not one.
pub(crate) fn viewer(open_with: &str, open_in: &str) -> Result<Viewer, String> {
    match open_with.trim() {
        "" | "editor" => Ok(Viewer::Editor(place(open_in)?)),
        "system" => Ok(Viewer::System),
        other => Err(format!(
            "org: org.export.open-with is `{other}` — use editor or system"
        )),
    }
}

/// What shows `path` for `viewer`.
pub(crate) fn show(viewer: Viewer, path: &str) -> Effect {
    match viewer {
        Viewer::System => Effect::OpenExternalUri(path.to_string()),
        Viewer::Editor(place) => Effect::DisplayBuffer(DisplayBufferPayload {
            source: DisplaySource::File(path.to_string()),
            target: place.target(),
        }),
    }
}

/// The output buffer's name, emacs's.
pub(crate) fn buffer_name(backend: Backend) -> &'static str {
    match backend {
        Backend::Html => "*Org HTML Export*",
        Backend::Markdown => "*Org MD Export*",
        Backend::Latex => "*Org LATEX Export*",
    }
}

/// The output buffer's major. There is no LaTeX major, so LaTeX is text.
fn buffer_major(backend: Backend) -> &'static str {
    match backend {
        Backend::Html => "html-mode",
        Backend::Markdown => "markdown-mode",
        Backend::Latex => "text-mode",
    }
}

fn store_key(backend: Backend) -> String {
    format!("export/output/{}", backend.extension())
}

/// What a finished export posts: the write or the buffer, any open, and the
/// echo that says it is done (`done` is `Exported X`, possibly with what
/// was left out).
pub(crate) fn effects(
    sink: Sink,
    backend: Backend,
    target: &str,
    output: String,
    done: Effect,
    host: &dyn Host,
) -> Vec<Effect> {
    match sink {
        Sink::File | Sink::Open(_) => {
            let mut out = vec![Effect::WriteToFile(WriteToFilePayload {
                path: target.to_string(),
                anchor: FileAnchor::Whole,
                text: output,
                cut: None,
                // The output goes beside its source, whose directory exists.
                create_parents: false,
                // On disk is the point: a browser, a forge or a compiler reads
                // it.
                save: true,
            })];
            if let Sink::Open(viewer) = sink {
                out.push(show(viewer, target));
            }
            out.push(done);
            out
        }
        // The `.tex` must be on disk before the compiler starts, which this
        // effect cannot promise; `pdf::begin` writes it.
        Sink::Pdf { .. } => vec![done],
        Sink::Buffer { place } => {
            let mut out = Vec::new();
            if let Some((id, current)) = host.output_buffer(backend) {
                out.push(Effect::ApplyEdit(ApplyEditPayload {
                    target: id,
                    edit: Edit {
                        range: Range {
                            start: Position { line: 0, byte: 0 },
                            end: end_of(&current),
                        },
                        kind: EditKind::Replace(output.clone()),
                    },
                    // `some` would move the focused buffer's caret, whatever
                    // `target` names.
                    cursor: None,
                }));
            }
            out.push(Effect::DisplayBuffer(DisplayBufferPayload {
                source: DisplaySource::Synthetic(OpenSyntheticBufferPayload {
                    name: buffer_name(backend).to_string(),
                    mode_id: buffer_major(backend).to_string(),
                    content: Some(output),
                    cursor: None,
                    activate_minor: Some(output_mode(backend).to_string()),
                }),
                target: place.target(),
            }));
            out.push(done);
            out
        }
    }
}

/// Where `text` ends, as a position.
fn end_of(text: &str) -> Position {
    let line = text.matches('\n').count() as u32;
    let byte = text.rsplit_once('\n').map_or(text, |(_, last)| last).len() as u32;
    Position { line, byte }
}

/// The name an export's echo gives its output.
pub(crate) fn target_label(sink: Sink, backend: Backend, target: &str) -> &str {
    match sink {
        Sink::Buffer { .. } => buffer_name(backend),
        Sink::File | Sink::Open(_) | Sink::Pdf { .. } => file_name(target),
    }
}

/// `backend`'s output minor went on in `buffer`: that is its buffer now.
pub(crate) fn on_output_mode(backend: Backend, buffer: u32) {
    let _ = crate::lattice::plugin_host::host_services::store_put(
        &store_key(backend),
        &buffer.to_le_bytes(),
    );
}

/// The guest's output buffer for `backend` and its text, when it is still
/// open.
pub(crate) fn guest_output_buffer(backend: Backend) -> Option<(u32, String)> {
    use crate::lattice::plugin_host::host_services;
    let bytes = host_services::store_get(&store_key(backend))?;
    let id = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
    match host_services::buffer_text(id) {
        Some(text) => Some((id, text)),
        None => {
            let _ = host_services::store_delete(&store_key(backend));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::IdFile;
    use crate::lattice::plugin_host::types::{EchoLevel, EchoPayload};

    /// A host whose HTML output buffer, if any, is the one given.
    struct Output(Option<(u32, String)>);

    impl Host for Output {
        fn read_file(&self, _: &str) -> Result<String, String> {
            Err(String::new())
        }
        fn id_file(&self, _: &str) -> IdFile {
            IdFile::NoIndex
        }
        fn output_buffer(&self, backend: Backend) -> Option<(u32, String)> {
            (backend == Backend::Html).then(|| self.0.clone()).flatten()
        }
    }

    fn at(p: &Position) -> (u32, u32) {
        (p.line, p.byte)
    }

    fn done() -> Effect {
        Effect::Echo(EchoPayload {
            level: EchoLevel::Info,
            text: "Exported".to_string(),
        })
    }

    fn run(sink: Sink, output: &str, host: &Output) -> Vec<Effect> {
        effects(
            sink,
            Backend::Html,
            "/a/b.html",
            output.to_string(),
            done(),
            host,
        )
    }

    #[test]
    fn open_with_is_editor_or_system_and_open_in_says_where() {
        assert_eq!(viewer("", ""), Ok(Viewer::Editor(Place::Vsplit)));
        assert_eq!(viewer("editor", "tab"), Ok(Viewer::Editor(Place::Tab)));
        assert_eq!(viewer("editor", "split"), Ok(Viewer::Editor(Place::Split)));
        assert_eq!(viewer("system", "nonsense"), Ok(Viewer::System));
        assert!(viewer("browser", "").unwrap_err().contains("`browser`"));
        assert!(viewer("editor", "window").unwrap_err().contains("`window`"));
    }

    fn displayed_file(e: &Effect) -> Option<(&str, DisplayTarget)> {
        match e {
            Effect::DisplayBuffer(DisplayBufferPayload {
                source: DisplaySource::File(path),
                target,
            }) => Some((path, *target)),
            _ => None,
        }
    }

    #[test]
    fn a_file_sink_writes_and_an_open_sink_then_opens() {
        let out = run(Sink::File, "x", &Output(None));
        assert!(matches!(&out[..], [Effect::WriteToFile(w), Effect::Echo(_)] if w.save));
        let out = run(
            Sink::Open(Viewer::Editor(Place::Vsplit)),
            "x",
            &Output(None),
        );
        let [Effect::WriteToFile(_), shown, Effect::Echo(_)] = &out[..] else {
            panic!("{out:?}");
        };
        assert_eq!(
            displayed_file(shown),
            Some(("/a/b.html", DisplayTarget::Vsplit))
        );
        let out = run(Sink::Open(Viewer::System), "x", &Output(None));
        assert!(matches!(&out[1], Effect::OpenExternalUri(u) if u == "/a/b.html"));
    }

    /// A first export opens a seeded buffer; once its id is known, the next
    /// replaces its whole text and then shows it.
    #[test]
    fn a_buffer_sink_seeds_then_replaces_in_place() {
        let buffer = Sink::Buffer { place: Place::Tab };
        let out = run(buffer, "<p>one</p>\n", &Output(None));
        let [Effect::DisplayBuffer(DisplayBufferPayload {
            source: DisplaySource::Synthetic(o),
            target: DisplayTarget::Tab,
        }), Effect::Echo(_)] = &out[..]
        else {
            panic!("{out:?}");
        };
        assert_eq!(o.name, "*Org HTML Export*");
        assert_eq!(o.mode_id, "html-mode");
        assert_eq!(
            o.activate_minor.as_deref(),
            Some("org-export-html-output-mode")
        );

        let current = "<p>one</p>\nsecond line".to_string();
        let out = run(buffer, "<p>two</p>\n", &Output(Some((7, current))));
        let [Effect::ApplyEdit(e), Effect::DisplayBuffer(_), Effect::Echo(_)] = &out[..] else {
            panic!("{out:?}");
        };
        assert_eq!(e.target, 7);
        assert_eq!(at(&e.edit.range.start), (0, 0));
        assert_eq!(at(&e.edit.range.end), (1, 11));
        assert!(matches!(&e.edit.kind, EditKind::Replace(t) if t == "<p>two</p>\n"));
        assert!(e.cursor.is_none());
    }

    #[test]
    fn each_backends_minor_names_it() {
        for b in BACKENDS {
            assert_eq!(output_backend(output_mode(b)), Some(b));
        }
        assert_eq!(output_backend("org-export-mode"), None);
    }

    #[test]
    fn each_backend_has_emacss_buffer() {
        assert_eq!(buffer_name(Backend::Markdown), "*Org MD Export*");
        assert_eq!(buffer_name(Backend::Latex), "*Org LATEX Export*");
        assert_eq!(buffer_major(Backend::Latex), "text-mode");
        assert_eq!(at(&end_of("a\nb\n")), (2, 0));
        assert_eq!(at(&end_of("")), (0, 0));
    }
}
