//! The LaTeX transcoder, in ox-latex's shape: its preamble (class, default
//! packages, `\author` / `\date` / `\title`, `\hypersetup`), its sectioning
//! per class, labels on every headline so links are `\ref` / `\hyperref`,
//! `\footnote` at the first reference, and headlines below `H:` (or past
//! the class's last sectioning command) as `itemize` / `enumerate` items. A
//! `#+LATEX_HEADER` written for emacs compiles here (lattice's
//! `org-export.md` §1).
//!
//! It is written as the render walks, like the HTML and Markdown writers:
//! [`Latex::start`] writes the preamble and the text before the first
//! headline, [`Latex::headline`] one visited headline, [`Latex::finish`]
//! closes what is open. Each element is a block, blocks one blank line
//! apart, which is a paragraph break to LaTeX.

use super::common::{self, Definition, Footnotes, Outline};
use super::filter::{self, View};
use super::options::{builtin_classes, Depth, LatexClass, Options, SrcBackend, Tags, Tex};
use super::parse::objects::{Emphasis, Object};
use super::parse::{
    Align, Block, BlockKind, Checkbox, Doc, Element, ElementKind, Headline, Item, ListKind, Row,
    Table,
};
use super::IdFile;

/// What a LaTeX export takes from outside the tree.
pub(crate) struct Settings {
    /// Leave out the preamble and `\begin{document}`.
    pub(crate) body_only: bool,
}

/// The compilers a LaTeX export is written for: `#+LATEX_COMPILER`'s and
/// `org.export.latex-compiler`'s values.
const COMPILERS: &[&str] = &["pdflatex", "xelatex", "lualatex"];

/// ox-latex's default packages, in its order. Under pdflatex, `inputenc` and
/// `fontenc` read UTF-8 and give T1 fonts; xelatex and lualatex read UTF-8
/// themselves and take system fonts through `fontspec`.
fn default_packages(compiler: &str) -> String {
    let encoding = if compiler == "pdflatex" {
        "\\usepackage[utf8]{inputenc}\n\\usepackage[T1]{fontenc}\n"
    } else {
        "\\usepackage{fontspec}\n"
    };
    format!(
        "{encoding}\\usepackage{{graphicx}}\n\\usepackage{{longtable}}\n\\usepackage{{wrapfig}}\n\
         \\usepackage{{rotating}}\n\\usepackage[normalem]{{ulem}}\n\\usepackage{{amsmath}}\n\
         \\usepackage{{amssymb}}\n\\usepackage{{capt-of}}\n\\usepackage{{hyperref}}\n"
    )
}

/// The language names listings knows, lowercased, against the spelling it
/// takes: org's names for them first, as ox-latex's `org-latex-listings-langs`.
/// A language not here gets no `language=`, since listings stops the build on
/// one it does not know.
const LISTINGS_LANGUAGES: &[(&str, &str)] = &[
    ("emacs-lisp", "Lisp"),
    ("elisp", "Lisp"),
    ("lisp", "Lisp"),
    ("clojure", "Lisp"),
    ("scheme", "Lisp"),
    ("cc", "C++"),
    ("cpp", "C++"),
    ("c++", "C++"),
    ("cperl", "Perl"),
    ("latex", "[LaTeX]TeX"),
    ("shell-script", "bash"),
    ("shell", "bash"),
    ("sh", "bash"),
    ("bash", "bash"),
    ("zsh", "bash"),
    ("ocaml", "[Objective]Caml"),
    ("caml", "Caml"),
    ("sqlite", "SQL"),
    ("makefile", "make"),
    ("js", "Java"),
    ("ada", "Ada"),
    ("awk", "Awk"),
    ("c", "C"),
    ("cobol", "Cobol"),
    ("csh", "csh"),
    ("delphi", "Delphi"),
    ("eiffel", "Eiffel"),
    ("erlang", "erlang"),
    ("fortran", "Fortran"),
    ("gnuplot", "Gnuplot"),
    ("haskell", "Haskell"),
    ("html", "HTML"),
    ("java", "Java"),
    ("ksh", "ksh"),
    ("make", "make"),
    ("mathematica", "Mathematica"),
    ("matlab", "Matlab"),
    ("ml", "ML"),
    ("octave", "Octave"),
    ("pascal", "Pascal"),
    ("perl", "Perl"),
    ("php", "PHP"),
    ("postscript", "PostScript"),
    ("prolog", "Prolog"),
    ("python", "Python"),
    ("r", "R"),
    ("ruby", "Ruby"),
    ("sas", "SAS"),
    ("scilab", "Scilab"),
    ("sql", "SQL"),
    ("tcl", "tcl"),
    ("tex", "TeX"),
    ("vbscript", "VBScript"),
    ("verilog", "Verilog"),
    ("vhdl", "VHDL"),
    ("xml", "XML"),
    ("xslt", "XSLT"),
];

/// org's language names that are not Pygments' lexer names, as ox-latex's
/// `org-latex-minted-langs`; any other is passed through, Pygments knowing
/// most.
const MINTED_LANGUAGES: &[(&str, &str)] = &[
    ("emacs-lisp", "common-lisp"),
    ("elisp", "common-lisp"),
    ("cc", "c++"),
    ("cperl", "perl"),
    ("shell-script", "bash"),
    ("shell", "bash"),
    ("caml", "ocaml"),
    ("sqlite", "sql"),
];

/// The package a source-block backend needs, which emacs leaves to the
/// user's packages and a first export then fails without.
fn src_package(backend: SrcBackend) -> &'static str {
    match backend {
        SrcBackend::Verbatim => "",
        SrcBackend::Listings => "\\usepackage{listings}\n",
        SrcBackend::Minted => "\\usepackage{minted}\n",
    }
}

/// The class's header with `options` in its `\documentclass[…]`, when
/// given, as `#+LATEX_CLASS_OPTIONS` does in ox-latex.
fn class_header(header: &str, options: &str) -> String {
    let options = options.trim();
    let Some(at) = header.find("\\documentclass") else {
        return header.to_string();
    };
    if options.is_empty() {
        return header.to_string();
    }
    let after = at + "\\documentclass".len();
    let rest = &header[after..];
    let keep = match rest.strip_prefix('[').and_then(|r| r.find(']')) {
        Some(close) => &rest[close + 2..],
        None => rest,
    };
    format!("{}{options}{keep}", &header[..after])
}

/// The preamble's packages and header lines put in the class's header, as
/// ox-latex's `org-splice-latex-header`: at its `[DEFAULT-PACKAGES]`,
/// `[PACKAGES]` and `[EXTRA]` when it has them, left out at its
/// `[NO-DEFAULT-PACKAGES]`, `[NO-PACKAGES]` and `[NO-EXTRA]`, else after it
/// in that order.
fn splice(header: &str, defaults: &str, packages: &str, extra: &str) -> String {
    let mut out = header.to_string();
    let mut tail = String::new();
    for (key, text) in [
        ("DEFAULT-PACKAGES", defaults),
        ("PACKAGES", packages),
        ("EXTRA", extra),
    ] {
        let place = format!("[{key}]");
        let no = format!("[NO-{key}]");
        if out.contains(&no) {
            out = out.replace(&format!("{no}\n"), "").replace(&no, "");
        } else if out.contains(&place) {
            out = out.replace(&place, text.trim_end_matches('\n'));
        } else {
            tail.push_str(text);
        }
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&tail);
    out
}

/// The document-wide state of one LaTeX export.
pub(crate) struct Latex {
    outline: Outline,
    footnotes: Footnotes,
    class: LatexClass,
    compiler: String,
    src_backend: SrcBackend,
    /// The list-item headlines (below `H:`) open around the next one: their
    /// level and whether the list is `enumerate`.
    deep: Vec<(usize, bool)>,
    /// How many `enumerate`s the element being written is in, which names
    /// the counter an item's `[@n]` sets.
    enumerate_depth: usize,
    body_only: bool,
    /// The next section [`Latex::index`] collects footnotes from, once it
    /// has started.
    collect_next: Option<Vec<usize>>,
}

/// Text: LaTeX's ten specials escaped, `...` typeset, `--` kept from
/// ligaturing when `-:` is off, and a `[` where LaTeX would read an
/// optional argument (a line's start, after `\\` or `\item`) braced.
fn text(t: &str, o: &Options, out: &mut String) {
    let mut rest = t;
    while let Some(c) = rest.chars().next() {
        if o.special_strings {
            if let Some(r) = rest.strip_prefix("...") {
                out.push_str("\\ldots{}");
                rest = r;
                continue;
            }
            if let Some(r) = rest.strip_prefix("\\-") {
                out.push_str("\\-");
                rest = r;
                continue;
            }
        } else if let Some(r) = rest.strip_prefix("--") {
            out.push_str("-{}-");
            rest = r;
            continue;
        }
        match c {
            '\n' if o.preserve_breaks => out.push_str("\\\\\n"),
            '[' if opens_argument(out) => out.push_str("{[}"),
            c => escape_char(c, out),
        }
        rest = &rest[c.len_utf8()..];
    }
}

/// Whether a `[` written next would be read as an optional argument.
fn opens_argument(out: &str) -> bool {
    let tail = out.trim_end_matches(' ');
    tail.is_empty() || tail.ends_with('\n') || tail.ends_with("\\\\") || tail.ends_with("\\item")
}

/// One character, escaped if it is one of LaTeX's specials.
fn escape_char(c: char, out: &mut String) {
    match c {
        '#' | '$' | '%' | '&' | '_' | '{' | '}' => {
            out.push('\\');
            out.push(c);
        }
        '~' => out.push_str("\\textasciitilde{}"),
        '^' => out.push_str("\\textasciicircum{}"),
        '\\' => out.push_str("\\textbackslash{}"),
        c => out.push(c),
    }
}

/// `t` escaped for a place that is not running text: `\texttt`, a
/// timestamp, a keyword. `--` is kept from ligaturing.
fn escaped(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(c) = rest.chars().next() {
        if let Some(r) = rest.strip_prefix("--") {
            out.push_str("-{}-");
            rest = r;
            continue;
        }
        escape_char(c, &mut out);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// A URL or path as `\url`, `\href` and `\includegraphics` take it: only
/// `%`, `#` and `\` need care.
fn url(u: &str) -> String {
    let mut out = String::with_capacity(u.len());
    for c in u.chars() {
        match c {
            '%' | '#' => {
                out.push('\\');
                out.push(c);
            }
            '\\' => out.push_str("\\textbackslash{}"),
            c => out.push(c),
        }
    }
    out
}

/// An entity as LaTeX writes it: a math command in `\(…\)`, a text command,
/// or the character itself, which `inputenc` takes.
fn entity(name: &str, utf8: &str) -> String {
    let math = |cmd: &str| format!("\\({cmd}\\)");
    // The capital Greek letters LaTeX has no command for are Latin capitals.
    let upright = |l: &str| format!("\\(\\mathrm{{{l}}}\\)");
    let text_command = |cmd: &str| cmd.to_string();
    match name {
        "alpha" | "beta" | "gamma" | "delta" | "epsilon" | "varepsilon" | "zeta" | "eta"
        | "theta" | "iota" | "kappa" | "lambda" | "mu" | "nu" | "xi" | "pi" | "rho" | "sigma"
        | "tau" | "upsilon" | "phi" | "varphi" | "chi" | "psi" | "omega" | "Gamma" | "Delta"
        | "Theta" | "Lambda" | "Xi" | "Pi" | "Sigma" | "Upsilon" | "Phi" | "Psi" | "Omega"
        | "approx" | "ast" | "cap" | "cdot" | "cdots" | "cup" | "div" | "downarrow" | "ell"
        | "emptyset" | "equiv" | "exists" | "forall" | "ge" | "geq" | "in" | "infty" | "int"
        | "le" | "leq" | "leftarrow" | "leftrightarrow" | "Leftarrow" | "Leftrightarrow" | "mp"
        | "nabla" | "ne" | "neq" | "ni" | "notin" | "partial" | "pm" | "prime" | "prod"
        | "rightarrow" | "Rightarrow" | "sim" | "star" | "subset" | "sum" | "supset" | "times"
        | "to" | "uparrow" | "checkmark" => math(&format!("\\{name}")),
        "omicron" => math("o"),
        "prop" => math("\\propto"),
        // An argument it takes, and here has none.
        "sqrt" => math("\\sqrt{}"),
        "quot" => text_command("\\textquotedbl{}"),
        "infin" => math("\\infty"),
        "darr" => math("\\downarrow"),
        "uarr" => math("\\uparrow"),
        "larr" => math("\\leftarrow"),
        "rarr" => math("\\rightarrow"),
        "harr" => math("\\leftrightarrow"),
        "minus" => math("-"),
        "sub" => math("\\subset"),
        "sup" => math("\\supset"),
        "Alpha" => upright("A"),
        "Beta" => upright("B"),
        "Chi" => upright("X"),
        "Epsilon" => upright("E"),
        "Eta" => upright("H"),
        "Iota" => upright("I"),
        "Kappa" => upright("K"),
        "Mu" => upright("M"),
        "Nu" => upright("N"),
        "Omicron" => upright("O"),
        "Rho" => upright("P"),
        "Tau" => upright("T"),
        "Zeta" => upright("Z"),
        "AA" | "aa" | "AE" | "ae" | "OE" | "oe" => format!("\\{name}{{}}"),
        "Dagger" => text_command("\\ddag{}"),
        "dagger" => text_command("\\dag{}"),
        "amp" => text_command("\\&"),
        "lt" => text_command("\\textless{}"),
        "gt" => text_command("\\textgreater{}"),
        "vbar" | "vert" => text_command("\\textbar{}"),
        "bullet" => text_command("\\textbullet{}"),
        "cent" => text_command("\\textcent{}"),
        "copy" => text_command("\\textcopyright{}"),
        "reg" => text_command("\\textregistered{}"),
        "trade" => text_command("\\texttrademark{}"),
        "deg" => text_command("\\textdegree{}"),
        "euro" => text_command("\\texteuro{}"),
        "pound" => text_command("\\pounds{}"),
        "yen" => text_command("\\textyen{}"),
        "para" => text_command("\\P{}"),
        "sect" => text_command("\\S{}"),
        "iexcl" => text_command("\\textexclamdown{}"),
        "iquest" => text_command("\\textquestiondown{}"),
        "laquo" => text_command("\\guillemotleft{}"),
        "raquo" => text_command("\\guillemotright{}"),
        "ldquo" => text_command("``"),
        "rdquo" => text_command("''"),
        "lsquo" => text_command("`"),
        "rsquo" => text_command("'"),
        "mdash" => text_command("---"),
        "ndash" => text_command("--"),
        "dots" | "hellip" | "ldots" => text_command("\\ldots{}"),
        "middot" => text_command("\\textperiodcentered{}"),
        "nbsp" => text_command("~"),
        "shy" => text_command("\\-"),
        "emsp" => text_command("\\hspace*{1em}"),
        "ensp" => text_command("\\hspace*{.5em}"),
        "thinsp" => text_command("\\,"),
        "frac12" => text_command("\\textonehalf{}"),
        "frac14" => text_command("\\textonequarter{}"),
        "frac34" => text_command("\\textthreequarters{}"),
        _ => {
            let mut out = String::new();
            utf8.chars().for_each(|c| escape_char(c, &mut out));
            out
        }
    }
}

/// `\includegraphics` of `path` with `options`.
fn graphic(options: &[String], path: &str) -> String {
    if options.is_empty() {
        format!("\\includegraphics{{{}}}", url(path))
    } else {
        format!("\\includegraphics[{}]{{{}}}", options.join(","), url(path))
    }
}

/// `body` as a `verbatim` environment.
fn verbatim(body: &str) -> String {
    let body = body.trim_end_matches('\n');
    format!("\\begin{{verbatim}}\n{body}\n\\end{{verbatim}}")
}

/// Append a block and the blank line after it.
fn push_block(out: &mut String, block: &str) {
    if block.is_empty() {
        return;
    }
    out.push_str(block);
    out.push_str("\n\n");
}

/// The backends whose raw output LaTeX passes through.
fn is_raw_backend(backend: &str) -> bool {
    matches!(backend.to_ascii_lowercase().as_str(), "latex" | "tex")
}

/// `pdflang`'s value for the file's `#+LANGUAGE`.
fn language(code: &str) -> &str {
    match code {
        "" | "en" => "English",
        "de" => "German",
        "es" => "Spanish",
        "fr" => "French",
        "it" => "Italian",
        "nl" => "Dutch",
        "pt" => "Portuguese",
        "sv" => "Swedish",
        other => other,
    }
}

/// `objects` without what is written once in a document: footnote
/// references and targets (a radio target keeps its text).
fn once_only_removed(objects: &[Object]) -> Vec<Object> {
    let mut out = Vec::with_capacity(objects.len());
    for o in objects {
        match o {
            Object::FootnoteReference { .. } | Object::Target(_) => {}
            Object::RadioTarget(contents) => out.extend(once_only_removed(contents)),
            Object::Emphasis { kind, contents } => out.push(Object::Emphasis {
                kind: *kind,
                contents: once_only_removed(contents),
            }),
            o => out.push(o.clone()),
        }
    }
    out
}

impl Latex {
    /// The writer for a numbered outline. An unknown `#+LATEX_CLASS` fails
    /// the export, as in emacs.
    pub(crate) fn new(outline: Outline, settings: Settings, view: &mut View) -> Latex {
        let o = &view.options;
        let class_name = o.latex_class.trim().to_string();
        let class = o
            .latex_classes
            .iter()
            .find(|c| c.name == class_name)
            .cloned();
        let compiler = o.latex_compiler.trim().to_string();
        let src_backend = SrcBackend::parse(&o.latex_src_backend);
        let known: Vec<&str> = o.latex_classes.iter().map(|c| c.name.as_str()).collect();
        let why = match &class {
            None => Some(format!(
                "org: no LaTeX class `{class_name}` — use one of {}, or add it to \
                 org.export.latex-classes",
                known.join(", ")
            )),
            Some(c) if c.sections.is_empty() => Some(format!(
                "org: LaTeX class `{class_name}` has no sections in \
                 org.export.latex-classes"
            )),
            _ if !COMPILERS.contains(&compiler.as_str()) => Some(format!(
                "org: no LaTeX compiler `{compiler}` — use pdflatex, xelatex or lualatex"
            )),
            _ if src_backend.is_none() => Some(format!(
                "org: org.export.latex-src-block-backend is `{}` — use verbatim, \
                 listings or minted",
                o.latex_src_backend.trim()
            )),
            _ => None,
        };
        if let Some(why) = why {
            view.fail(why);
        }
        let class = class
            .filter(|c| !c.sections.is_empty())
            .unwrap_or_else(|| builtin_classes().swap_remove(0));
        Latex {
            outline,
            footnotes: Footnotes::default(),
            class,
            compiler,
            src_backend: src_backend.unwrap_or(SrcBackend::Verbatim),
            deep: Vec::new(),
            enumerate_depth: 0,
            body_only: settings.body_only,
            collect_next: None,
        }
    }

    /// Collect the footnote definitions of the next sections, about
    /// `budget` bytes of them: `true` once every one is. A footnote is
    /// written where it is first referenced, which may come before its
    /// definition, so all of them are known before the body.
    pub(crate) fn index(&mut self, doc: &Doc, budget: usize) -> bool {
        let next = self.collect_next.get_or_insert_with(|| {
            self.footnotes.collect(&doc.preamble);
            if doc.headlines.is_empty() {
                Vec::new()
            } else {
                vec![0]
            }
        });
        let mut spent = 0usize;
        while !next.is_empty() && spent < budget {
            let h = common::at(doc, next);
            self.footnotes.collect(&h.section);
            spent += 64 + 16 * h.section.len();
            common::advance(doc, next, true);
        }
        next.is_empty()
    }

    /// Write the preamble and the text before the first headline.
    pub(crate) fn begin(&mut self, doc: &Doc, view: &mut View, out: &mut String) {
        if !self.body_only {
            self.preamble(view, out);
        }
        let preamble = filter::elements(&doc.preamble, view);
        let body = self.blocks(&preamble, view);
        push_block(out, &body);
    }

    /// Everything before the body: ox-latex's preamble and title block.
    fn preamble(&mut self, view: &mut View, out: &mut String) {
        let o = view.options.clone();
        out.push_str(&format!("% Intended LaTeX compiler: {}\n", self.compiler));
        let mut extra = String::new();
        for line in o.latex_header.iter().chain(&o.latex_header_extra) {
            extra.push_str(line);
            extra.push('\n');
        }
        out.push_str(&splice(
            &class_header(&self.class.header, &o.latex_class_options),
            &default_packages(&self.compiler),
            src_package(self.src_backend),
            &extra,
        ));
        let mut author = String::new();
        if o.with_author {
            let a = filter::objects(&o.author, view);
            self.objects(&a, view, &mut author);
            if o.with_email && !o.email.is_empty() {
                author.push_str(&format!("\\thanks{{{}}}", escaped(&o.email)));
            }
        }
        let mut date = String::new();
        if o.with_date {
            if o.date.is_empty() {
                date.push_str("\\today");
            } else {
                let d = filter::objects(&o.date, view);
                self.objects(&d, view, &mut date);
            }
        }
        let mut title = String::new();
        let t = filter::objects(&o.title, view);
        self.objects(&t, view, &mut title);
        let mut subtitle = String::new();
        let st = filter::objects(&o.subtitle, view);
        self.objects(&st, view, &mut subtitle);
        let full_title = if subtitle.is_empty() {
            title.clone()
        } else {
            format!("{title}\\\\\\medskip\n\\large {subtitle}")
        };
        out.push_str(&format!(
            "\\author{{{author}}}\n\\date{{{date}}}\n\\title{{{full_title}}}\n"
        ));
        // The PDF's metadata is text, not markup: what the reader shows in
        // its document properties.
        let pdf = |objects: &[Object], view: &mut View| {
            escaped(&filter::plain(&filter::objects(objects, view)))
        };
        let pdf_author = if o.with_author {
            pdf(&o.author, view)
        } else {
            String::new()
        };
        let (pdf_title, pdf_subject) = (pdf(&o.title, view), pdf(&o.subtitle, view));
        out.push_str(&format!(
            "\\hypersetup{{\n pdfauthor={{{pdf_author}}},\n pdftitle={{{pdf_title}}},\n \
             pdfkeywords={{}},\n pdfsubject={{{pdf_subject}}},\n pdfcreator={{lattice}},\n \
             pdflang={{{}}}}}\n",
            language(&o.language)
        ));
        if let Depth::To(n) = o.num {
            out.push_str(&format!("\\setcounter{{secnumdepth}}{{{n}}}\n"));
        }
        out.push_str("\\begin{document}\n\n");
        if o.with_title {
            out.push_str("\\maketitle\n");
        }
        match o.toc {
            Depth::Off => {}
            Depth::On => out.push_str("\\tableofcontents\n"),
            Depth::To(n) => out.push_str(&format!(
                "\\setcounter{{tocdepth}}{{{n}}}\n\\tableofcontents\n"
            )),
        }
        if o.with_title || o.toc != Depth::Off {
            out.push('\n');
        }
    }

    /// Close the list-item headlines open at `level` or deeper.
    fn close_deep(&mut self, level: usize, out: &mut String) {
        while let Some(&(l, numbered)) = self.deep.last() {
            if l < level {
                break;
            }
            self.deep.pop();
            if numbered {
                self.enumerate_depth -= 1;
            }
            push_block(
                out,
                if numbered {
                    "\\end{enumerate}"
                } else {
                    "\\end{itemize}"
                },
            );
        }
    }

    /// One visited headline, already filtered (`filter::own`).
    pub(crate) fn headline(&mut self, h: &Headline, view: &mut View, out: &mut String) {
        if common::is_footnote_section(h) {
            // Its definitions were collected; the heading is not exported,
            // as in ox.
            return;
        }
        let level = self.outline.level(h);
        let anchor = view.targets().headline_anchor(h.line);
        let numbered = self.outline.is_numbered(&anchor);
        let mut title = String::new();
        self.objects(&h.title, view, &mut title);
        let title = title.trim_end().to_string();
        let full = self.headline_text(h, &title, view).trim_end().to_string();
        let deepest = view.options.headline_levels.min(self.class.sections.len());
        if level > deepest {
            // ox-latex's low-level headline: an item, its list opened by the
            // first sibling and closed after the last.
            self.close_deep(level + 1, out);
            if !matches!(self.deep.last(), Some(&(l, _)) if l == level) {
                push_block(
                    out,
                    if numbered {
                        "\\begin{enumerate}"
                    } else {
                        "\\begin{itemize}"
                    },
                );
                if numbered {
                    self.enumerate_depth += 1;
                }
                self.deep.push((level, numbered));
            }
            let body = self.body(h, view);
            let relax = if full.starts_with('[') { "\\relax" } else { "" };
            let mut item = format!("\\item{relax} {full}\n\\label{{{anchor}}}");
            if !body.is_empty() {
                item.push('\n');
                item.push_str(&body);
            }
            push_block(out, &item);
            return;
        }
        self.close_deep(0, out);
        let command = self.class.sections[level - 1].clone();
        let star = if numbered { "" } else { "*" };
        // The title again for the contents and running heads, without what
        // must be written once (a footnote, a target's label). Braced, so a
        // `]` in it does not end the optional argument.
        let mut short = String::new();
        self.objects(&once_only_removed(&h.title), view, &mut short);
        let short = short.trim_end().to_string();
        // A short title when the full one carries a keyword or tags, as
        // ox-latex, or something the contents must not repeat.
        let optional = if numbered && full != short && !short.is_empty() {
            format!("[{{{short}}}]")
        } else {
            String::new()
        };
        let mut heading = format!("\\{command}{star}{optional}{{{full}}}\n\\label{{{anchor}}}");
        if !numbered && self.outline.is_listed(&anchor) {
            heading.push_str(&format!(
                "\n\\addcontentsline{{toc}}{{{command}}}{{{short}}}"
            ));
        }
        push_block(out, &heading);
        let body = self.body(h, view);
        push_block(out, &body);
    }

    /// A headline's planning, properties and section.
    fn body(&mut self, h: &Headline, view: &mut View) -> String {
        let mut body = self.meta(h);
        let section = self.blocks(&h.section, view);
        if !section.is_empty() {
            if !body.is_empty() {
                body.push_str("\n\n");
            }
            body.push_str(&section);
        }
        body
    }

    /// The headline's text as ox-latex formats it: the keyword bold sans,
    /// the priority framed, the tags right-aligned in small caps.
    fn headline_text(&self, h: &Headline, title: &str, view: &View) -> String {
        let o = &view.options;
        let mut out = String::new();
        if o.todo {
            if let Some(kw) = &h.todo {
                out.push_str(&format!("{{\\bfseries\\sffamily {}}} ", escaped(kw)));
            }
        }
        if o.priority {
            if let Some(p) = h.priority {
                out.push_str(&format!("\\framebox{{\\#{p}}} "));
            }
        }
        out.push_str(title);
        if o.tags != Tags::Off && !h.tags.is_empty() {
            let tags: Vec<String> = h.tags.iter().map(|t| escaped(t)).collect();
            out.push_str(&format!("\\hfill{{}}\\textsc{{{}}}", tags.join(":")));
        }
        out
    }

    /// Planning and properties, when the options kept them.
    fn meta(&self, h: &Headline) -> String {
        let mut out = String::new();
        let p = &h.planning;
        if !p.is_empty() {
            let mut parts = Vec::new();
            for (kw, stamp) in [
                ("SCHEDULED:", &p.scheduled),
                ("DEADLINE:", &p.deadline),
                ("CLOSED:", &p.closed),
            ] {
                if let Some(stamp) = stamp {
                    parts.push(format!("\\textbf{{{kw}}} \\textit{{{}}}", escaped(stamp)));
                }
            }
            out.push_str(&format!("\\noindent{}\\\\", parts.join(" ")));
        }
        if !h.properties.is_empty() {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            let body: String = h
                .properties
                .iter()
                .map(|(k, v)| format!("{k}: {v}\n"))
                .collect();
            out.push_str(&verbatim(&body));
        }
        out
    }

    /// Elements as blocks, one blank line apart.
    fn blocks(&mut self, list: &[Element], view: &mut View) -> String {
        let mut out = String::new();
        for e in list {
            let block = self.element(e, view);
            if block.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&block);
        }
        out
    }

    /// A named element's label, for links to it.
    fn label(e: &Element, view: &View) -> Option<String> {
        e.affiliated
            .name
            .as_ref()
            .map(|n| format!("\\label{{{}}}", view.targets().name_anchor(n)))
    }

    /// The element's `\caption`, its lines joined, its label inside.
    fn caption(&mut self, e: &Element, view: &mut View) -> Option<String> {
        let caption = self.caption_text(e, view)?;
        Some(format!(
            "\\caption{{{}{caption}}}",
            Self::label(e, view).unwrap_or_default(),
        ))
    }

    /// The element's caption lines, joined and written.
    fn caption_text(&mut self, e: &Element, view: &mut View) -> Option<String> {
        let mut caption = String::new();
        for (i, c) in e.affiliated.caption.iter().enumerate() {
            if i > 0 {
                caption.push(' ');
            }
            let c = filter::objects(c, view);
            self.objects(&c, view, &mut caption);
        }
        if caption.trim().is_empty() {
            return None;
        }
        Some(caption.trim().to_string())
    }

    /// One element, as a block (no blank lines around it).
    fn element(&mut self, e: &Element, view: &mut View) -> String {
        match &e.kind {
            ElementKind::Paragraph(objects) => match common::standalone_image(objects) {
                Some(path) => self.figure(e, path, view),
                None => {
                    let mut p = String::new();
                    self.objects(objects, view, &mut p);
                    let label = Self::label(e, view)
                        .map(|l| format!("{l}\n"))
                        .unwrap_or_default();
                    label + p.trim_end()
                }
            },
            ElementKind::PlainList(list) => self.list(list.kind, &list.items, view),
            ElementKind::Table(t) => self.table(e, t, view),
            ElementKind::TableEl(lines) | ElementKind::FixedWidth(lines) => {
                verbatim(&lines.join("\n"))
            }
            ElementKind::Block(b) => self.block(e, b, view),
            ElementKind::Drawer { contents, .. } => self.blocks(contents, view),
            ElementKind::Keyword { key, value } => match key.as_str() {
                "LATEX" | "TEX" => value.clone(),
                "TOC" => {
                    view.options.ignore("#+TOC".to_string());
                    String::new()
                }
                _ => String::new(),
            },
            // Collected in `start`; written at the first reference.
            ElementKind::FootnoteDefinition { .. } | ElementKind::Comment(_) => String::new(),
            ElementKind::HorizontalRule => "\\noindent\\rule{\\linewidth}{0.5pt}".to_string(),
            ElementKind::LatexEnvironment(tex) => match view.options.tex {
                Tex::On => tex.trim_end().to_string(),
                Tex::Verbatim => verbatim(tex),
                Tex::Off => String::new(),
            },
            ElementKind::Clock(value) => {
                format!(
                    "\\noindent\\textbf{{CLOCK:}} \\textit{{{}}}",
                    escaped(value)
                )
            }
        }
    }

    /// A paragraph that is one image: a `figure` when it has a caption,
    /// else centred, as ox-latex.
    /// `#+ATTR_LATEX:`'s `:width`, `:height` and `:options` size it; with
    /// neither size, `org.export.latex-image-width` does.
    fn figure(&mut self, e: &Element, path: &str, view: &mut View) -> String {
        let attributes = common::attributes(e, "latex");
        let width = common::attribute(&attributes, "width");
        let height = common::attribute(&attributes, "height");
        let mut options = Vec::new();
        match (width, height) {
            (Some(w), _) if !w.is_empty() => options.push(format!("width={w}")),
            (None, None) => {
                let w = view.options.latex_image_width.trim();
                if !w.is_empty() {
                    options.push(format!("width={w}"));
                }
            }
            _ => {}
        }
        if let Some(h) = height.filter(|h| !h.is_empty()) {
            options.push(format!("height={h}"));
        }
        if let Some(more) = common::attribute(&attributes, "options").filter(|o| !o.is_empty()) {
            options.push(more.to_string());
        }
        let image = graphic(&options, path);
        match self.caption(e, view) {
            Some(caption) => {
                format!("\\begin{{figure}}[htbp]\n\\centering\n{image}\n{caption}\n\\end{{figure}}")
            }
            None => format!("\\begin{{center}}\n{image}\n\\end{{center}}"),
        }
    }

    fn list(&mut self, kind: ListKind, items: &[Item], view: &mut View) -> String {
        let env = match kind {
            ListKind::Unordered => "itemize",
            ListKind::Ordered => "enumerate",
            ListKind::Descriptive => "description",
        };
        if kind == ListKind::Ordered {
            self.enumerate_depth += 1;
        }
        let mut out = format!("\\begin{{{env}}}");
        for item in items {
            out.push('\n');
            if let (ListKind::Ordered, Some(n)) = (kind, item.counter) {
                // The counter for this depth: enumi, enumii, enumiii, enumiv.
                let counter = ["i", "ii", "iii", "iv"]
                    .get(self.enumerate_depth.saturating_sub(1))
                    .unwrap_or(&"iv");
                out.push_str(&format!(
                    "\\setcounter{{enum{counter}}}{{{}}}\n",
                    n.saturating_sub(1)
                ));
            }
            out.push_str("\\item");
            match (item.checkbox, kind, &item.tag) {
                (Some(c), _, _) => out.push_str(match c {
                    Checkbox::On => "[$\\boxtimes$]",
                    Checkbox::Off => "[$\\square$]",
                    Checkbox::Partial => "[$\\boxminus$]",
                }),
                (None, ListKind::Descriptive, Some(tag)) => {
                    let mut term = String::new();
                    self.objects(tag, view, &mut term);
                    out.push_str(&format!("[{{{}}}]", term.trim()));
                }
                _ => {}
            }
            out.push(' ');
            let body = self.blocks(&item.contents, view);
            // A body that opens with `[` would be read as the item's
            // optional argument.
            match body.strip_prefix('[') {
                Some(rest) => {
                    out.push_str("{[}");
                    out.push_str(rest);
                }
                None => out.push_str(&body),
            }
        }
        if kind == ListKind::Ordered {
            self.enumerate_depth -= 1;
        }
        out.push_str(&format!("\n\\end{{{env}}}"));
        out
    }

    /// A `tabular`: alignment from the cookies, else right for a column of
    /// numbers; `\hline` for each rule. In a `table` float with its caption
    /// above when it has one, else centred.
    fn table(&mut self, e: &Element, t: &Table, view: &mut View) -> String {
        let columns = t
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::Cells(c) => Some(c.len()),
                Row::Rule => None,
            })
            .max()
            .unwrap_or(0);
        if columns == 0 {
            return String::new();
        }
        let attributes = common::attributes(e, "latex");
        let environment = common::attribute(&attributes, "environment")
            .filter(|v| !v.is_empty())
            .unwrap_or("tabular")
            .to_string();
        let computed: String = (0..columns)
            .map(|i| match t.align.get(i).copied().flatten() {
                Some(Align::Left) => 'l',
                Some(Align::Center) => 'c',
                Some(Align::Right) => 'r',
                None if common::numeric_column(t, i) => 'r',
                None => 'l',
            })
            .collect();
        let spec = common::attribute(&attributes, "align")
            .filter(|v| !v.is_empty())
            .map_or(computed, str::to_string);
        // The width-taking environments take it before the columns.
        let width = match environment.as_str() {
            "tabularx" | "tabulary" | "tabu" | "tabu*" => {
                let w = common::attribute(&attributes, "width")
                    .filter(|v| !v.is_empty())
                    .unwrap_or("\\linewidth");
                format!("{{{w}}}")
            }
            _ => String::new(),
        };
        let longtable = environment == "longtable";
        let mut tabular = format!("\\begin{{{environment}}}{width}{{{spec}}}\n");
        if longtable {
            // A longtable is its own float: the caption, or the label alone,
            // is its first row.
            let head = match self.caption(e, view) {
                Some(caption) => Some(caption),
                None => Self::label(e, view),
            };
            if let Some(head) = head {
                tabular.push_str(&head);
                tabular.push_str("\\\\\n");
            }
        }
        for r in &t.rows {
            match r {
                Row::Rule => tabular.push_str("\\hline\n"),
                Row::Cells(cells) => {
                    let mut line = Vec::with_capacity(columns);
                    for i in 0..columns {
                        let mut cell = String::new();
                        if let Some(c) = cells.get(i) {
                            self.objects(c, view, &mut cell);
                        }
                        line.push(cell.trim().to_string());
                    }
                    tabular.push_str(&line.join(" & "));
                    tabular.push_str(" \\\\\n");
                }
            }
        }
        tabular.push_str(&format!("\\end{{{environment}}}"));
        if longtable {
            return tabular;
        }
        match self.caption(e, view) {
            Some(caption) => {
                format!("\\begin{{table}}[htbp]\n{caption}\n\\centering\n{tabular}\n\\end{{table}}")
            }
            None => {
                let label = Self::label(e, view)
                    .map(|l| format!("{l}\n"))
                    .unwrap_or_default();
                format!("\\begin{{center}}\n{label}{tabular}\n\\end{{center}}")
            }
        }
    }

    /// A source block, as `org.export.latex-src-block-backend` writes one.
    fn src(&mut self, e: &Element, language: &str, body: &str, view: &mut View) -> String {
        let lang = language.to_ascii_lowercase();
        match self.src_backend {
            SrcBackend::Verbatim => {
                let code = verbatim(body);
                match self.caption(e, view) {
                    // A captioned block floats, so the caption has a home.
                    Some(caption) => {
                        format!("\\begin{{figure}}[htbp]\n{code}\n{caption}\n\\end{{figure}}")
                    }
                    None => code,
                }
            }
            // The options are the listing's own, not an `\lstset`, which
            // would carry one block's caption to the next.
            SrcBackend::Listings => {
                let body = body.trim_end_matches('\n');
                let mut options = Vec::new();
                if let Some((_, name)) = LISTINGS_LANGUAGES.iter().find(|(org, _)| *org == lang) {
                    options.push(format!("language={{{name}}}"));
                }
                if let Some(caption) = self.caption_text(e, view) {
                    options.push(format!("caption={{{caption}}}"));
                }
                if let Some(name) = &e.affiliated.name {
                    options.push(format!("label={{{}}}", view.targets().name_anchor(name)));
                }
                let options = if options.is_empty() {
                    String::new()
                } else {
                    format!("[{}]", options.join(","))
                };
                format!("\\begin{{lstlisting}}{options}\n{body}\n\\end{{lstlisting}}")
            }
            SrcBackend::Minted => {
                let body = body.trim_end_matches('\n');
                let lexer = match MINTED_LANGUAGES.iter().find(|(org, _)| *org == lang) {
                    Some((_, lexer)) => (*lexer).to_string(),
                    None if lang.is_empty() => "text".to_string(),
                    None => lang,
                };
                let code = format!("\\begin{{minted}}{{{lexer}}}\n{body}\n\\end{{minted}}");
                match self.caption(e, view) {
                    Some(caption) => {
                        format!("\\begin{{listing}}[htbp]\n{code}\n{caption}\n\\end{{listing}}")
                    }
                    None => code,
                }
            }
        }
    }

    fn block(&mut self, e: &Element, b: &Block, view: &mut View) -> String {
        match &b.kind {
            BlockKind::Src { language } => self.src(e, language, &b.body, view),
            BlockKind::Example => verbatim(&b.body),
            BlockKind::Export { backend } if is_raw_backend(backend) => {
                b.body.trim_end_matches('\n').to_string()
            }
            BlockKind::Export { .. } | BlockKind::Comment => String::new(),
            BlockKind::Verse => {
                let objects = super::parse::objects::parse(b.body.trim_end_matches('\n'));
                let objects = filter::objects(&objects, view);
                let mut verse = String::new();
                self.objects(&objects, view, &mut verse);
                // Each line ends in `\\`; its indentation is kept as space,
                // and a blank line between stanzas as a gap.
                let lines: Vec<String> = verse
                    .split('\n')
                    .map(|line| {
                        if line.trim().is_empty() {
                            return "\\vspace*{1em}".to_string();
                        }
                        let spaces = line.len() - line.trim_start_matches(' ').len();
                        if spaces > 0 {
                            format!("\\hspace*{{{spaces}em}}{}", &line[spaces..])
                        } else {
                            line.to_string()
                        }
                    })
                    .collect();
                format!("\\begin{{verse}}\n{}\n\\end{{verse}}", lines.join("\\\\\n"))
            }
            BlockKind::Quote => self.environment("quote", &b.contents, view),
            BlockKind::Center => self.environment("center", &b.contents, view),
            // A special block is an environment of its name, as in ox-latex.
            BlockKind::Special(name) => self.environment(name, &b.contents, view),
        }
    }

    fn environment(&mut self, name: &str, contents: &[Element], view: &mut View) -> String {
        let inner = self.blocks(contents, view);
        format!("\\begin{{{name}}}\n{inner}\n\\end{{{name}}}")
    }

    /// Objects, as LaTeX.
    pub(crate) fn objects(&mut self, list: &[Object], view: &mut View, out: &mut String) {
        for o in list {
            self.object(o, view, out);
        }
    }

    fn object(&mut self, obj: &Object, view: &mut View, out: &mut String) {
        match obj {
            Object::Text(t) => text(t, &view.options, out),
            Object::Emphasis { kind, contents } => {
                out.push_str(match kind {
                    Emphasis::Bold => "\\textbf{",
                    Emphasis::Italic => "\\emph{",
                    Emphasis::Underline => "\\uline{",
                    Emphasis::Strike => "\\sout{",
                });
                self.objects(contents, view, out);
                out.push('}');
            }
            Object::Verbatim(t) | Object::Code(t) => {
                out.push_str(&format!("\\texttt{{{}}}", escaped(t)));
            }
            Object::Link {
                path,
                description,
                target,
            } => self.link(path, description, target.as_deref(), view, out),
            Object::FootnoteReference { label, definition } => {
                let (n, count) = self.footnotes.reference(label.as_deref(), definition);
                if count > 1 {
                    out.push_str(&format!("\\footnotemark[{n}]"));
                    return;
                }
                let body = match self.footnotes.take(n) {
                    Some(Definition::Elements(elements)) => {
                        let elements = filter::elements(&elements, view);
                        self.blocks(&elements, view)
                    }
                    Some(Definition::Inline(objects)) => {
                        let objects = filter::objects(&objects, view);
                        let mut p = String::new();
                        self.objects(&objects, view, &mut p);
                        p
                    }
                    None => String::new(),
                };
                out.push_str(&format!("\\footnote{{{}}}", body.trim()));
            }
            Object::Entity { name, utf8, .. } => out.push_str(&entity(name, utf8)),
            Object::LatexFragment(t) => match view.options.tex {
                Tex::On => out.push_str(t),
                Tex::Verbatim => out.push_str(&format!("\\texttt{{{}}}", escaped(t))),
                Tex::Off => {}
            },
            Object::Subscript { contents, .. } => {
                out.push_str("\\textsubscript{");
                self.objects(contents, view, out);
                out.push('}');
            }
            Object::Superscript { contents, .. } => {
                out.push_str("\\textsuperscript{");
                self.objects(contents, view, out);
                out.push('}');
            }
            // The newline after it is the text's own.
            Object::LineBreak => out.push_str("\\\\"),
            Object::Timestamp(t) => out.push_str(&format!("\\textit{{{}}}", escaped(t))),
            Object::Target(t) => {
                out.push_str(&format!("\\label{{{}}}", view.targets().target_anchor(t)));
            }
            Object::RadioTarget(contents) => {
                let name = filter::plain(contents);
                out.push_str(&format!(
                    "\\label{{{}}}",
                    view.targets().target_anchor(&name)
                ));
                self.objects(contents, view, out);
            }
            Object::StatisticsCookie(c) => text(c, &view.options, out),
            // The filter expands every macro before this sees the tree.
            Object::Macro { .. } => {}
            Object::ExportSnippet { backend, value } => {
                if is_raw_backend(backend) {
                    out.push_str(value);
                }
            }
        }
    }

    /// A link, as ox-latex writes it: `\ref` to a numbered headline or a
    /// target when there is no description, `\hyperref` otherwise, and
    /// `\url` / `\href` outside the document.
    fn link(
        &mut self,
        path: &str,
        description: &[Object],
        target: Option<&str>,
        view: &mut View,
        out: &mut String,
    ) {
        let mut desc = String::new();
        self.objects(description, view, &mut desc);
        if let Some(anchor) = target {
            if !desc.is_empty() {
                out.push_str(&format!("\\hyperref[{anchor}]{{{desc}}}"));
            } else if self.outline.is_numbered(anchor) {
                out.push_str(&format!("\\ref{{{anchor}}}"));
            } else if let Some(title) = self.outline.label(anchor) {
                // An unnumbered headline: its title, linked.
                let title = title.to_string();
                let mut t = String::new();
                text(&title, &view.options, &mut t);
                out.push_str(&format!("\\hyperref[{anchor}]{{{t}}}"));
            } else {
                out.push_str(&format!("\\ref{{{anchor}}}"));
            }
            return;
        }
        if let Some(image) = common::inline_image(path, description) {
            let width = view.options.latex_image_width.trim();
            let options = if width.is_empty() {
                Vec::new()
            } else {
                vec![format!("width={width}")]
            };
            out.push_str(&graphic(&options, image));
            return;
        }
        if let Some(id) = path.strip_prefix("id:") {
            // Another file's node: no label of this document reaches it, so
            // its file is linked, or the description stands alone.
            match view.host().id_file(id) {
                IdFile::In(file) if desc.is_empty() => {
                    out.push_str(&format!("\\url{{{}}}", url(&file)));
                }
                IdFile::In(file) => {
                    out.push_str(&format!("\\href{{{}}}{{{desc}}}", url(&file)));
                }
                IdFile::NoIndex | IdFile::Unknown => {
                    if desc.is_empty() {
                        text(path, &view.options, out);
                    } else {
                        out.push_str(&desc);
                    }
                }
            }
            return;
        }
        let href = if common::is_file_path(path) {
            let file = path.strip_prefix("file:").unwrap_or(path);
            file.split_once("::").map_or(file, |(f, _)| f).to_string()
        } else {
            path.to_string()
        };
        if desc.is_empty() {
            out.push_str(&format!("\\url{{{}}}", url(&href)));
        } else {
            out.push_str(&format!("\\href{{{}}}{{{desc}}}", url(&href)));
        }
    }

    /// Close what is open and end the document.
    pub(crate) fn finish(&mut self, out: &mut String) {
        self.close_deep(0, out);
        while out.ends_with("\n\n") {
            out.pop();
        }
        if !self.body_only {
            out.push_str("\\end{document}\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::rc::Rc;

    use super::super::html::Stylesheet;
    use super::super::options::{LatexClass, Options};
    use super::super::pipeline::{Backend, Configured, Request, Run, Step};
    use super::super::{Host, IdFile};
    use crate::lattice::plugin_host::types::Effect;
    use crate::time_format::When;

    /// A host with no files and one roam node, `n1`, in `/notes/sub/n1.org`.
    struct Notes;

    impl Host for Notes {
        fn read_file(&self, _: &str) -> Result<String, String> {
            Err("no files".to_string())
        }
        fn id_file(&self, id: &str) -> IdFile {
            match id {
                "n1" => IdFile::In("/notes/sub/n1.org".to_string()),
                _ => IdFile::Unknown,
            }
        }
        fn output_buffer(&self, _: Backend) -> Option<(u32, String)> {
            None
        }
    }

    /// `text` exported to LaTeX from `/notes/plan.org`, or the echo that
    /// stopped it.
    fn export_with(text: &str, body_only: bool) -> Result<String, String> {
        configured_export(text, body_only, Options::default())
    }

    /// `text` exported over `options`, as the plugin's options give them.
    fn configured_export(text: &str, body_only: bool, options: Options) -> Result<String, String> {
        let request = Request {
            buffer: 1,
            source: "/notes/plan.org".to_string(),
            target: "/notes/plan.tex".to_string(),
            backend: Backend::Latex,
            subtree: false,
            body_only,
            line: 0,
            sink: crate::export::sink::Sink::File,
        };
        let mut run = Run::new(
            1,
            request,
            Some(text.to_string()),
            vec!["TODO NEXT | DONE".to_string()],
            When { local_secs: 0 },
            Rc::new(Notes),
            Configured {
                options,
                stylesheet: Stylesheet::None,
            },
        )
        .map_err(|e| format!("{e:?}"))?;
        loop {
            if let Step::Done(effects) = run.step() {
                return effects
                    .iter()
                    .find_map(|e| match e {
                        Effect::WriteToFile(w) => Some(w.text.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| format!("{effects:?}"));
            }
        }
    }

    fn export(text: &str) -> String {
        export_with(text, false).unwrap_or_else(|e| panic!("{e}"))
    }

    fn body(text: &str) -> String {
        export_with(text, true).unwrap_or_else(|e| panic!("{e}"))
    }

    /// Check `tex`'s grouping as TeX would: every brace closed, every
    /// `\begin{x}` closed by its `\end{x}` in order, `verbatim` bodies read
    /// as text. Gives back the labels it defines and the ones it refers to.
    fn well_formed(tex: &str) -> (HashSet<String>, Vec<String>) {
        let (mut labels, mut refs) = (HashSet::new(), Vec::new());
        let mut envs: Vec<String> = Vec::new();
        let mut depth = 0i32;
        let mut rest = tex;
        // The argument of the command `rest` starts at, after `\name` and an
        // optional `[...]`.
        fn argument(after: &str) -> Option<(&str, &str)> {
            let inner = after.strip_prefix('{')?;
            let end = inner.find('}')?;
            Some((&inner[..end], &inner[end + 1..]))
        }
        while let Some(c) = rest.chars().next() {
            if let Some(r) = rest.strip_prefix("\\begin{verbatim}") {
                let end = r
                    .find("\\end{verbatim}")
                    .unwrap_or_else(|| panic!("verbatim never ends\n{tex}"));
                rest = &r[end + "\\end{verbatim}".len()..];
                continue;
            }
            if let Some(r) = rest.strip_prefix("\\begin") {
                let (name, after) = argument(r).unwrap_or_else(|| panic!("bad \\begin\n{tex}"));
                envs.push(name.to_string());
                rest = after;
                continue;
            }
            if let Some(r) = rest.strip_prefix("\\end") {
                let (name, after) = argument(r).unwrap_or_else(|| panic!("bad \\end\n{tex}"));
                assert_eq!(envs.pop().as_deref(), Some(name), "\\end{{{name}}}\n{tex}");
                rest = after;
                continue;
            }
            for (cmd, defines) in [("\\label", true), ("\\ref", false)] {
                if let Some((name, _)) = rest.strip_prefix(cmd).and_then(argument) {
                    if defines {
                        assert!(labels.insert(name.to_string()), "label {name} twice\n{tex}");
                    } else {
                        refs.push(name.to_string());
                    }
                }
            }
            if let Some(r) = rest.strip_prefix("\\hyperref[") {
                refs.push(r[..r.find(']').unwrap_or(0)].to_string());
            }
            match c {
                // An escaped character is text.
                '\\' => {
                    let next = rest[1..].chars().next().map_or(0, char::len_utf8);
                    rest = &rest[1 + next..];
                    continue;
                }
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    assert!(depth >= 0, "a `}}` closes nothing\n{tex}");
                }
                _ => {}
            }
            rest = &rest[c.len_utf8()..];
        }
        assert_eq!(depth, 0, "a `{{` is never closed\n{tex}");
        assert!(envs.is_empty(), "{envs:?} never end\n{tex}");
        (labels, refs)
    }

    /// Compare the export of `testdata/NAME.org` with `testdata/NAME.tex`;
    /// `UPDATE_GOLDEN=1` rewrites the `.tex` instead. Every golden is also
    /// well-formed, and each of its references lands on a label.
    fn golden(name: &str) {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/export/parse/testdata");
        let org = std::fs::read_to_string(dir.join(format!("{name}.org"))).expect("corpus file");
        let got = export(&org);
        let (labels, refs) = well_formed(&got);
        for r in &refs {
            assert!(labels.contains(r), "{name}: {r} is labelled nowhere\n{got}");
        }
        let path = dir.join(format!("{name}.tex"));
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::write(&path, &got).expect("write golden");
            return;
        }
        let want = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            got == want,
            "{name}.tex differs (UPDATE_GOLDEN=1 rewrites it)\n--- got\n{got}"
        );
    }

    #[test]
    fn the_corpus_exports_as_its_goldens() {
        for name in ["headlines", "lists", "blocks", "misc", "objects"] {
            golden(name);
        }
    }

    /// ox-latex's preamble: the class, the default packages, the header
    /// lines, the title block, the contents.
    #[test]
    fn the_preamble_is_ox_latexs() {
        let tex = export(
            "#+TITLE: The Plan\n#+AUTHOR: Ada\n#+LATEX_CLASS_OPTIONS: [a4paper]\n\
             #+LATEX_HEADER: \\usepackage{tikz}\n* One\n",
        );
        assert!(tex.starts_with("% Intended LaTeX compiler: pdflatex\n\\documentclass[a4paper]{article}\n\\usepackage[utf8]{inputenc}\n"), "{tex}");
        assert!(tex.contains("\\usepackage{hyperref}\n\\usepackage{tikz}\n\\author{Ada}\n\\date{\\today}\n\\title{The Plan}\n"), "{tex}");
        assert!(tex.contains(" pdftitle={The Plan},\n"), "{tex}");
        assert!(
            tex.contains("\\begin{document}\n\n\\maketitle\n\\tableofcontents\n\n\\section{One}\n"),
            "{tex}"
        );
        assert!(tex.ends_with("\\end{document}\n"), "{tex}");
        well_formed(&tex);
    }

    #[test]
    fn the_class_picks_the_sectioning_and_an_unknown_one_fails() {
        let tex = body("#+LATEX_CLASS: report\n* Part\n** Chapter\n*** Section\n");
        assert!(tex.starts_with("\\part{Part}\n"), "{tex}");
        assert!(
            tex.contains("\\chapter{Chapter}\n") && tex.contains("\\section{Section}\n"),
            "{tex}"
        );
        let err = export_with("#+LATEX_CLASS: memoir\n* A\n", false).unwrap_err();
        assert!(err.contains("no LaTeX class `memoir`"), "{err}");
    }

    /// Unnumbered headlines are starred, and still in the contents.
    #[test]
    fn unnumbered_headlines_are_starred_and_listed() {
        let tex = export("#+OPTIONS: num:nil\n* One\n");
        assert!(tex.contains("\\section*{One}\n\\label{"), "{tex}");
        assert!(
            tex.contains("\\addcontentsline{toc}{section}{One}"),
            "{tex}"
        );
        let tex = export("#+OPTIONS: num:nil toc:nil\n* One\n");
        assert!(!tex.contains("addcontentsline"), "{tex}");
    }

    /// Below `H:`, headlines are items of lists that open with the first
    /// sibling and close after the last, nested by level.
    #[test]
    fn deep_headlines_are_list_items() {
        let tex = body("#+OPTIONS: H:1\n* Top\n** A\n*** Under\n** B\n* Next\n");
        let want = "\\section{Top}\n\\label{";
        assert!(tex.starts_with(want), "{tex}");
        let at = |s: &str| tex.find(s).unwrap_or_else(|| panic!("{s} in\n{tex}"));
        assert!(at("\\begin{enumerate}") < at("\\item A"));
        assert!(at("\\item A") < at("\\item Under") && at("\\item Under") < at("\\item B"));
        assert!(at("\\item B") < at("\\section{Next}"));
        assert_eq!(tex.matches("\\begin{enumerate}").count(), 2, "{tex}");
        well_formed(&tex);
    }

    #[test]
    fn the_headline_carries_keyword_priority_and_tags_with_a_short_title() {
        let tex = body("#+OPTIONS: pri:t\n* TODO [#A] Ship it :work:\n");
        assert!(
            tex.starts_with("\\section[{Ship it}]{{\\bfseries\\sffamily TODO} \\framebox{\\#A} Ship it\\hfill{}\\textsc{work}}\n"),
            "{tex}"
        );
        well_formed(&tex);
    }

    /// The ten specials are text, and a `[` where LaTeX would read an
    /// optional argument is braced.
    #[test]
    fn specials_are_escaped() {
        let tex = body("#+OPTIONS: ^:nil\n50% of $5 & #1 a_b {x} ~ ^ =a\\b=\n");
        assert_eq!(
            tex,
            "50\\% of \\$5 \\& \\#1 a\\_b \\{x\\} \\textasciitilde{} \\textasciicircum{} \\texttt{a\\textbackslash{}b}\n"
        );
        let tex = body("[x] at the start\n\n- [y] in an item\n");
        assert!(tex.starts_with("{[}x] at the start\n"), "{tex}");
        assert!(tex.contains("\\item {[}y] in an item"), "{tex}");
        well_formed(&tex);
    }

    /// Links are ox-latex's: `\ref` to a numbered headline, `\hyperref` with
    /// a description or to an unnumbered one, `\url` and `\href` outward.
    #[test]
    fn links_are_refs_inside_and_urls_outside() {
        let tex = body(
            "* One\n[[*One]] [[*One][the first]] [[https://x.org/a%20b]] [[https://x.org][site]] [[id:n1][node]]\n\
             * Two\n:PROPERTIES:\n:UNNUMBERED: t\n:END:\n[[*Two]]\n",
        );
        assert!(tex.contains("\\ref{"), "{tex}");
        assert!(tex.contains("]{the first}"), "{tex}");
        assert!(tex.contains("\\url{https://x.org/a\\%20b}"), "{tex}");
        assert!(tex.contains("\\href{https://x.org}{site}"), "{tex}");
        assert!(tex.contains("\\href{/notes/sub/n1.org}{node}"), "{tex}");
        assert!(tex.contains("]{Two}"), "{tex}");
        let (labels, refs) = well_formed(&tex);
        assert!(refs.iter().all(|r| labels.contains(r)), "{tex}");
    }

    /// A footnote is written at its first reference, even when it is defined
    /// after; a later reference is a mark.
    #[test]
    fn footnotes_are_written_where_first_referenced() {
        let tex = body("a[fn:x] b[fn:x] c[fn::inline]\n\n* Footnotes\n[fn:x] The note.\n");
        assert_eq!(
            tex,
            "a\\footnote{The note.} b\\footnotemark[1] c\\footnote{inline}\n"
        );
    }

    #[test]
    fn tables_are_tabulars() {
        let tex = body("| a | 1 |\n|---+---|\n| b | 22 |\n");
        assert_eq!(
            tex,
            "\\begin{center}\n\\begin{tabular}{lr}\na & 1 \\\\\n\\hline\nb & 22 \\\\\n\\end{tabular}\n\\end{center}\n"
        );
        let tex = body("#+CAPTION: Numbers\n#+NAME: tab\n| a |\n");
        assert!(
            tex.starts_with("\\begin{table}[htbp]\n\\caption{\\label{")
                && tex.contains("}Numbers}\n\\centering\n\\begin{tabular}{l}\n"),
            "{tex}"
        );
    }

    #[test]
    fn lists_are_environments() {
        // Two blank lines end an org list.
        let tex = body("- [X] done\n- [ ] open\n\n\n1. one\n2. [@5] five\n\n\n- term :: desc\n");
        assert!(
            tex.contains("\\item[$\\boxtimes$] done\n\\item[$\\square$] open"),
            "{tex}"
        );
        assert!(
            tex.contains("\\begin{enumerate}\n\\item one\n\\setcounter{enumi}{4}\n\\item five"),
            "{tex}"
        );
        assert!(
            tex.contains("\\begin{description}\n\\item[{term}] desc"),
            "{tex}"
        );
        well_formed(&tex);
    }

    #[test]
    fn blocks_raw_latex_and_entities() {
        let tex = body(
            "#+begin_src rust\nlet x = {1};\n#+end_src\n#+begin_export latex\n\\newpage\n#+end_export\n\
             #+LATEX: \\clearpage\n@@latex:\\LaTeX{}@@ \\alpha \\deg \\nbsp x\n#+begin_quote\nq\n#+end_quote\n",
        );
        assert!(
            tex.starts_with(
                "\\begin{verbatim}\nlet x = {1};\n\\end{verbatim}\n\n\\newpage\n\n\\clearpage\n\n"
            ),
            "{tex}"
        );
        assert!(
            tex.contains("\\LaTeX{} \\(\\alpha\\) \\textdegree{} ~ x"),
            "{tex}"
        );
        assert!(tex.contains("\\begin{quote}\nq\n\\end{quote}"), "{tex}");
        well_formed(&tex);
    }

    fn with(f: impl FnOnce(&mut Options)) -> Options {
        let mut o = Options::default();
        f(&mut o);
        o
    }

    fn configured(text: &str, options: Options) -> String {
        configured_export(text, false, options).unwrap_or_else(|e| panic!("{e}"))
    }

    fn configured_body(text: &str, options: Options) -> String {
        configured_export(text, true, options).unwrap_or_else(|e| panic!("{e}"))
    }

    /// The compiler picks the encoding packages and is named in the file;
    /// the file's keyword overrides the option, and an unknown one stops the
    /// export.
    #[test]
    fn the_compiler_picks_the_encoding_packages() {
        let xe = with(|o| o.latex_compiler = "xelatex".to_string());
        let tex = configured("x\n", xe.clone());
        assert!(
            tex.starts_with("% Intended LaTeX compiler: xelatex\n"),
            "{tex}"
        );
        assert!(
            tex.contains("\\usepackage{fontspec}\n\\usepackage{graphicx}"),
            "{tex}"
        );
        assert!(!tex.contains("inputenc"), "{tex}");
        let tex = configured("#+LATEX_COMPILER: pdflatex\nx\n", xe);
        assert!(tex.contains("\\usepackage[utf8]{inputenc}"), "{tex}");
        let err = export_with("#+LATEX_COMPILER: latex\nx\n", false).unwrap_err();
        assert!(err.contains("no LaTeX compiler `latex`"), "{err}");
    }

    /// The option's header lines, then the file's `#+LATEX_HEADER` and
    /// `#+LATEX_HEADER_EXTRA` lines, after the default packages.
    #[test]
    fn header_lines_come_after_the_packages_in_order() {
        let o = with(|o| o.latex_header = vec!["\\usepackage{a}".to_string()]);
        let tex = configured(
            "#+LATEX_HEADER_EXTRA: \\usepackage{c}\n#+LATEX_HEADER: \\usepackage{b}\nx\n",
            o,
        );
        assert!(
            tex.contains("\\usepackage{hyperref}\n\\usepackage{a}\n\\usepackage{b}\n\\usepackage{c}\n\\author"),
            "{tex}"
        );
    }

    /// A declared class sections as it says, with its header's placeholders
    /// placing or leaving out the preamble's parts.
    #[test]
    fn a_declared_class_sections_and_places_its_preamble() {
        let mut o = with(|o| o.latex_class = "memoir".to_string());
        o.add_classes([LatexClass {
            name: "memoir".to_string(),
            header:
                "\\documentclass[10pt]{memoir}\n[NO-DEFAULT-PACKAGES]\n\\usepackage{x}\n[EXTRA]"
                    .to_string(),
            sections: vec!["chapter".to_string(), "section".to_string()],
        }]);
        let tex = configured(
            "#+LATEX_CLASS_OPTIONS: [a5paper]\n#+LATEX_HEADER: \\usepackage{y}\n* A\n** B\n",
            o,
        );
        assert!(
            tex.contains(
                "\\documentclass[a5paper]{memoir}\n\\usepackage{x}\n\\usepackage{y}\n\\author"
            ),
            "{tex}"
        );
        assert!(!tex.contains("graphicx"), "{tex}");
        assert!(
            tex.contains("\\chapter{A}") && tex.contains("\\section{B}"),
            "{tex}"
        );
        let err = export_with("#+LATEX_CLASS: memoir\nx\n", false).unwrap_err();
        assert!(err.contains("article, report, book"), "{err}");
    }

    /// `listings` and `minted` write their environments and add their
    /// package; a language listings does not know gets none, and a backend
    /// that is none of the three stops the export, naming the option.
    #[test]
    fn source_blocks_follow_the_backend() {
        let listings = with(|o| o.latex_src_backend = "listings".to_string());
        let text = "#+NAME: f\n#+CAPTION: Fn\n#+begin_src emacs-lisp\n(f)\n#+end_src\n\n#+begin_src rust\nfn f() {}\n#+end_src\n";
        let tex = configured(text, listings);
        assert!(
            tex.contains("\\usepackage{hyperref}\n\\usepackage{listings}\n"),
            "{tex}"
        );
        assert!(
            tex.contains("\\begin{lstlisting}[language={Lisp},caption={Fn},label={org")
                && tex.contains("}]\n(f)\n\\end{lstlisting}"),
            "{tex}"
        );
        assert!(
            tex.contains("\\begin{lstlisting}\nfn f() {}\n\\end{lstlisting}"),
            "{tex}"
        );
        let minted = with(|o| o.latex_src_backend = "minted".to_string());
        let tex = configured(text, minted);
        assert!(tex.contains("\\usepackage{minted}\n"), "{tex}");
        assert!(
            tex.contains("\\begin{listing}[htbp]\n\\begin{minted}{common-lisp}\n(f)\n\\end{minted}\n\\caption{\\label{org")
                && tex.contains("}Fn}\n\\end{listing}"),
            "{tex}"
        );
        assert!(
            tex.contains("\\begin{minted}{rust}\nfn f() {}\n\\end{minted}"),
            "{tex}"
        );
        let bad = with(|o| o.latex_src_backend = "pygments".to_string());
        let err = configured_export(text, false, bad).unwrap_err();
        assert!(
            err.contains("org.export.latex-src-block-backend is `pygments`"),
            "{err}"
        );
    }

    /// `#+ATTR_LATEX:` sizes an image, and the option's width is the
    /// default; empty draws it at its own size.
    #[test]
    fn attr_latex_and_the_option_size_an_image() {
        let tex = body("#+ATTR_LATEX: :width 5cm :options angle=90\n[[./a.png]]\n");
        assert!(
            tex.contains("\\includegraphics[width=5cm,angle=90]{./a.png}"),
            "{tex}"
        );
        let tex = body("#+ATTR_LATEX: :height 2cm\n[[./a.png]]\n");
        assert!(
            tex.contains("\\includegraphics[height=2cm]{./a.png}"),
            "{tex}"
        );
        let natural = with(|o| o.latex_image_width = String::new());
        let tex = configured_body("[[./a.png]]\n", natural);
        assert!(tex.contains("\\includegraphics{./a.png}"), "{tex}");
    }

    /// `#+ATTR_LATEX:` picks a table's environment and alignment; a
    /// longtable carries its caption as its first row and floats alone.
    #[test]
    fn attr_latex_picks_the_table_environment() {
        let tex =
            body("#+CAPTION: Big\n#+ATTR_LATEX: :environment longtable :align |l|r|\n| a | 1 |\n");
        assert_eq!(
            tex,
            "\\begin{longtable}{|l|r|}\n\\caption{Big}\\\\\na & 1 \\\\\n\\end{longtable}\n"
        );
        let tex = body("#+ATTR_LATEX: :environment tabularx :align lX\n| a | b |\n");
        assert!(
            tex.contains("\\begin{tabularx}{\\linewidth}{lX}\n"),
            "{tex}"
        );
    }

    /// A 1x1 grey PNG.
    const PIXEL: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x00, 0x00, 0x00, 0x00, 0x3a,
        0x7e, 0x9b, 0x55, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0x0f, 0x00, 0x01, 0x01, 0x01, 0x00, 0xb1, 0x38, 0xf6, 0x14, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    /// The goldens, and the source blocks under listings (minted needs a
    /// shell escape), compile. ox loads `wrapfig` and `capt-of` for what
    /// this writer never writes, so they are stubbed where they are not
    /// installed; a file needing another package that is not installed is
    /// skipped, naming it, as is everything where there is no `latexmk`.
    #[test]
    fn the_exports_compile() {
        let installed = |package: &str| {
            std::process::Command::new("kpsewhich")
                .arg(format!("{package}.sty"))
                .output()
                .is_ok_and(|o| o.status.success())
        };
        let latexmk = std::process::Command::new("latexmk").arg("-v").output();
        if latexmk.is_err() || !installed("hyperref") {
            eprintln!("SKIP: no latexmk or kpsewhich");
            return;
        }
        let dir = std::env::temp_dir().join(format!("org-export-compile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        let stubbed = ["wrapfig", "capt-of"];
        for package in stubbed {
            if !installed(package) {
                std::fs::write(dir.join(format!("{package}.sty")), "")
                    .unwrap_or_else(|e| panic!("{e}"));
            }
        }
        let listings = with(|o| o.latex_src_backend = "listings".to_string());
        let mut files: Vec<(String, String)> = Vec::new();
        for name in ["blocks", "headlines", "lists", "misc", "objects"] {
            let org = std::fs::read_to_string(format!("src/export/parse/testdata/{name}.org"))
                .unwrap_or_else(|e| panic!("{e}"));
            // A special block is an environment the user defines, in ox as
            // here; the corpus's one is defined as a user would.
            let org = format!("#+LATEX_HEADER: \\newenvironment{{note}}{{}}{{}}\n{org}");
            files.push((name.to_string(), export(&org)));
            if name == "blocks" {
                files.push((
                    format!("{name}-listings"),
                    configured(&org, listings.clone()),
                ));
            }
        }
        for (name, tex) in files {
            let missing: Vec<&str> = tex
                .lines()
                .filter_map(|l| l.strip_prefix("\\usepackage"))
                .filter_map(|l| l.rsplit_once('{').map(|(_, p)| p.trim_end_matches('}')))
                .filter(|p| !stubbed.contains(p) && !installed(p))
                .collect();
            if !missing.is_empty() {
                eprintln!("SKIP: {name}.tex needs {}", missing.join(", "));
                continue;
            }
            // The images the file draws, as one pixel each.
            for image in tex.split("\\includegraphics").skip(1) {
                let Some(path) = image.split_once('{').and_then(|(_, p)| p.split_once('}')) else {
                    continue;
                };
                if !path.0.starts_with('/') && !path.0.contains("..") {
                    std::fs::write(dir.join(path.0), PIXEL).unwrap_or_else(|e| panic!("{e}"));
                }
            }
            let path = dir.join(format!("{name}.tex"));
            std::fs::write(&path, tex).unwrap_or_else(|e| panic!("{e}"));
            let out = std::process::Command::new("latexmk")
                .args([
                    "-pdf",
                    "-interaction=nonstopmode",
                    "-halt-on-error",
                    "-quiet",
                ])
                .arg(format!("-outdir={}", dir.display()))
                .arg(&path)
                .current_dir(&dir)
                .output()
                .unwrap_or_else(|e| panic!("{e}"));
            assert!(
                out.status.success(),
                "{name}.tex does not compile; see {}",
                dir.join(format!("{name}.log")).display()
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `]` in a short title does not end the optional argument, and a
    /// footnote in the title is written once, in the section itself.
    #[test]
    fn a_short_title_is_braced_and_holds_no_footnote() {
        let tex = body("* TODO Fix a[i] indexing\n* Note[fn:: aside] here\n");
        assert!(
            tex.contains(
                "\\section[{Fix a[i] indexing}]{{\\bfseries\\sffamily TODO} Fix a[i] indexing}"
            ),
            "{tex}"
        );
        assert!(
            tex.contains("\\section[{Note here}]{Note\\footnote{aside} here}"),
            "{tex}"
        );
        assert_eq!(tex.matches("\\footnote").count(), 1, "{tex}");
    }

    /// A `CUSTOM_ID` LaTeX could not take as a label is one it can, the same
    /// at the label and at every reference.
    #[test]
    fn an_unsafe_custom_id_is_a_safe_label() {
        let tex = body(
            "* A\n:PROPERTIES:\n:CUSTOM_ID: a b%{x}\n:END:\n* B\n[[#a b%{x}][there]] [[#a b%{x}]]\n",
        );
        let (labels, refs) = well_formed(&tex);
        assert_eq!(refs.len(), 2, "{tex}");
        assert!(refs.iter().all(|r| labels.contains(r)), "{tex}");
        assert!(!tex.contains("a b%"), "{tex}");
    }

    /// The PDF's metadata is plain text, not the title block's markup.
    #[test]
    fn the_pdf_metadata_is_plain_text() {
        let tex =
            export("#+TITLE: The *Plan*\n#+AUTHOR: Ada /L/\n#+EMAIL: a@b.c\n#+OPTIONS: email:t\n");
        assert!(
            tex.contains("\\author{Ada \\emph{L}\\thanks{a@b.c}}"),
            "{tex}"
        );
        assert!(
            tex.contains(" pdfauthor={Ada L},\n pdftitle={The Plan},"),
            "{tex}"
        );
    }

    #[test]
    fn entities_that_need_more_are_given_it() {
        let tex = body("\\sqrt \\prop \\quot\n");
        assert_eq!(tex, "\\(\\sqrt{}\\) \\(\\propto\\) \\textquotedbl{}\n");
    }

    #[test]
    fn a_captioned_image_is_a_figure() {
        let tex = body("#+CAPTION: A pic\n[[./img.png]]\n\n[[./bare.png]]\n");
        assert!(
            tex.starts_with("\\begin{figure}[htbp]\n\\centering\n\\includegraphics[width=.9\\linewidth]{./img.png}\n\\caption{A pic}\n\\end{figure}\n"),
            "{tex}"
        );
        assert!(tex.contains("\\begin{center}\n\\includegraphics[width=.9\\linewidth]{./bare.png}\n\\end{center}"), "{tex}");
    }
}
