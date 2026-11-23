//! `:results value` — the built-in value wrappers (OB.8, design §5.2).
//!
//! Babel finds a block's value per language in elisp. Here a language record
//! names one of these wrappers (`value = "python"`), and the wrapper turns
//! the body into a script that runs it and prints its last expression's
//! value after a sentinel line. Only what follows the sentinel is the result,
//! so whatever the body itself printed is not.
//!
//! The body is embedded as hex, never as a string literal: no quote,
//! backslash or newline in it can end the literal and become wrapper code.

use serde::{Deserialize, Serialize};

/// A value wrapper a language record may name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Wrapper {
    Python,
    Ruby,
    Node,
    /// A shell's value is what it printed, as in babel's `ob-shell`: the
    /// body runs unwrapped.
    Sh,
}

impl Wrapper {
    /// The wrapper `name` names, or `None` for one that does not exist.
    pub(crate) fn named(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "python" => Some(Self::Python),
            "ruby" => Some(Self::Ruby),
            "node" => Some(Self::Node),
            "sh" => Some(Self::Sh),
            _ => None,
        }
    }

    /// The script for `body`, and whether its output carries a sentinel to
    /// cut at.
    pub(crate) fn wrap(self, body: &str, sentinel: &str) -> (String, bool) {
        let hex = hex(body);
        let script = match self {
            Self::Sh => return (body.to_string(), false),
            // The last statement, when it is an expression, is evaluated
            // for its value; a body that `return`s (babel's older python
            // rule) runs as a function instead.
            Self::Python => format!(
                "import ast as __ob_ast\n\
                 __ob_src = bytes.fromhex(\"{hex}\").decode(\"utf-8\")\n\
                 try:\n\
                 \x20   __ob_tree = __ob_ast.parse(__ob_src, \"<babel>\")\n\
                 \x20   __ob_last = None\n\
                 \x20   if __ob_tree.body and isinstance(__ob_tree.body[-1], __ob_ast.Expr):\n\
                 \x20       __ob_last = __ob_ast.Expression(__ob_tree.body.pop().value)\n\
                 \x20   exec(compile(__ob_tree, \"<babel>\", \"exec\"))\n\
                 \x20   __ob_value = eval(compile(__ob_last, \"<babel>\", \"eval\")) if __ob_last is not None else None\n\
                 except SyntaxError as __ob_e:\n\
                 \x20   if \"outside function\" not in str(__ob_e):\n\
                 \x20       raise\n\
                 \x20   __ob_def = \"def __ob_main():\\n\" + \"\".join(\"    \" + l + \"\\n\" for l in __ob_src.splitlines())\n\
                 \x20   exec(compile(__ob_def, \"<babel>\", \"exec\"))\n\
                 \x20   __ob_value = __ob_main()\n\
                 print(\"{sentinel}\")\n\
                 print(__ob_value)\n"
            ),
            // `eval` answers the last expression; a String is written as
            // itself, anything else as `inspect` writes it (babel's rule).
            Self::Ruby => format!(
                "__ob_src = [\"{hex}\"].pack(\"H*\").force_encoding(\"UTF-8\")\n\
                 __ob_value = eval(__ob_src, TOPLEVEL_BINDING, \"<babel>\")\n\
                 puts \"{sentinel}\"\n\
                 puts(__ob_value.is_a?(String) ? __ob_value : __ob_value.inspect)\n"
            ),
            // A direct `eval`, so the body keeps `require`.
            Self::Node => format!(
                "const __ob_src = Buffer.from(\"{hex}\", \"hex\").toString(\"utf8\");\n\
                 const __ob_value = eval(__ob_src);\n\
                 console.log(\"{sentinel}\");\n\
                 console.log(typeof __ob_value === \"string\" ? __ob_value : require(\"util\").inspect(__ob_value));\n"
            ),
        };
        (script, true)
    }
}

impl Wrapper {
    /// The program a session's interpreter runs, which `{driver}` in a
    /// record's `session-args` stands for (OB.15, design §6.3). It reads one
    /// block per line, each a [`Self::session_line`], and runs it where the
    /// last one left its state. `None` for sh, which reads its commands from
    /// stdin itself.
    ///
    /// A driver, not the interpreter's own REPL: a REPL prints prompts, and
    /// node's echoes every expression's value, and none of that is the block's
    /// output.
    pub(crate) fn driver(self) -> Option<&'static str> {
        match self {
            Self::Sh => None,
            Self::Python => Some(
                "import sys\n\
                 for __ob_l in sys.stdin:\n\
                 \x20   exec(bytes.fromhex(__ob_l.strip()).decode(\"utf-8\"))\n",
            ),
            Self::Ruby => Some(
                "STDOUT.sync = true; STDERR.sync = true; \
                 STDIN.each_line { |__ob_l| \
                 eval([__ob_l.strip].pack(\"H*\").force_encoding(\"UTF-8\"), TOPLEVEL_BINDING, \"<babel>\") }",
            ),
            // `runInThisContext`, so a `let` in one block is there in the next.
            Self::Node => Some(
                "require(\"readline\").createInterface({ input: process.stdin }).on(\"line\", \
                 (l) => require(\"vm\").runInThisContext(Buffer.from(l.trim(), \"hex\").toString(\"utf8\")));",
            ),
        }
    }

    /// `body` as the one line a session is sent: it runs the body, prints
    /// its value after `value` when that is given (`:results value`), and
    /// then prints `end` on stdout AND on stderr. Each stream is read on its
    /// own, so a line the body wrote to stderr may arrive after one it wrote
    /// later to stdout; when `end` has come on both, everything the body
    /// wrote has. An error in the body is printed, not fatal: the session
    /// outlives a block that fails.
    pub(crate) fn session_line(self, body: &str, value: Option<&str>, end: &str) -> String {
        let program = match self {
            // `command eval`: a syntax error in a plain `eval` exits a
            // non-interactive shell, and the session with it.
            Self::Sh => {
                let octal: String = body.bytes().map(|b| format!("\\{b:03o}")).collect();
                return format!("command eval \"$(printf '{octal}')\"; echo {end}; echo {end} >&2");
            }
            Self::Python => {
                let run = match value {
                    Some(v) => self.wrap(body, v).0,
                    None => body.to_string(),
                };
                // The traceback starts below this frame, which is no part of
                // the user's code.
                format!(
                    "import sys as __ob_sys\n\
                     try:\n\
                     \x20   exec(compile(bytes.fromhex(\"{}\").decode(\"utf-8\"), \"<babel>\", \"exec\"))\n\
                     except Exception as __ob_e:\n\
                     \x20   import traceback as __ob_tb\n\
                     \x20   __ob_tb.print_exception(type(__ob_e), __ob_e, __ob_e.__traceback__.tb_next)\n\
                     print(\"{end}\", flush=True)\n\
                     print(\"{end}\", file=__ob_sys.stderr, flush=True)\n",
                    hex(&run)
                )
            }
            Self::Ruby => {
                let show = value.map_or(String::new(), |v| {
                    format!(
                        "  puts \"{v}\"\n  puts(__ob_v.is_a?(String) ? __ob_v : __ob_v.inspect)\n"
                    )
                });
                format!(
                    "begin\n\
                     \x20 __ob_v = eval([\"{}\"].pack(\"H*\").force_encoding(\"UTF-8\"), TOPLEVEL_BINDING, \"<babel>\")\n\
                     {show}\
                     rescue StandardError, ScriptError => __ob_e\n\
                     \x20 STDERR.puts \"#{{__ob_e.class}}: #{{__ob_e.message}}\"\n\
                     end\n\
                     puts \"{end}\"\n\
                     STDERR.puts \"{end}\"\n",
                    hex(body)
                )
            }
            Self::Node => {
                let show = value.map_or(String::new(), |v| {
                    format!(
                        "console.log(\"{v}\"); console.log(typeof v === \"string\" ? v : require(\"util\").inspect(v)); "
                    )
                });
                format!(
                    "(() => {{ try {{ const v = require(\"vm\").runInThisContext(Buffer.from(\"{}\", \"hex\").toString(\"utf8\"), {{ filename: \"<babel>\" }}); {show}}} \
                     catch (e) {{ console.error(e && e.stack ? e.stack : String(e)); }} \
                     console.log(\"{end}\"); console.error(\"{end}\"); }})()",
                    hex(body)
                )
            }
        };
        hex(&program)
    }
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_is_embedded_as_hex_and_cannot_escape() {
        let body = "x = \"\"\"\nprint('\\\\')\n";
        for w in [Wrapper::Python, Wrapper::Ruby, Wrapper::Node] {
            let (script, cut) = w.wrap(body, "SENT");
            assert!(cut);
            assert!(
                !script.contains("print('"),
                "{w:?}: the body is not in the clear"
            );
            assert!(script.contains(&hex(body)), "{w:?}");
            assert!(script.contains("SENT"), "{w:?}");
        }
        assert_eq!(
            Wrapper::Sh.wrap("echo hi", "SENT"),
            ("echo hi".to_string(), false)
        );
    }

    #[test]
    fn a_session_line_is_one_line_with_the_body_never_in_the_clear() {
        let body = "x = \"a\nb\"\n# it's\n";
        for w in [Wrapper::Sh, Wrapper::Python, Wrapper::Ruby, Wrapper::Node] {
            let line = w.session_line(body, Some("VAL"), "END");
            assert!(!line.contains('\n'), "{w:?}: one line");
            assert!(!line.contains("it's"), "{w:?}: not in the clear");
            assert_eq!(w.driver().is_some(), w != Wrapper::Sh, "{w:?}");
        }
    }

    /// The driver and its lines through the real interpreter: state carries
    /// from block to block, a failing block does not end the session, and
    /// `end` comes on both streams after each. Skipped for an interpreter
    /// that is not on this host.
    #[test]
    fn each_driver_keeps_state_across_blocks_and_survives_an_error() {
        use std::io::Write as _;
        use std::process::{Command, Stdio};
        let cases: [(Wrapper, &str, Vec<&str>, [&str; 3]); 4] = [
            (
                Wrapper::Sh,
                "sh",
                vec![],
                ["x=41", "echo \"unterminated", "echo $((x + 1))"],
            ),
            (
                Wrapper::Python,
                "python3",
                vec!["-u", "-c"],
                ["x = 41", "raise ValueError('no')", "x + 1"],
            ),
            (
                Wrapper::Ruby,
                "ruby",
                vec!["-e"],
                ["x = 41", "raise 'no'", "x + 1"],
            ),
            (
                Wrapper::Node,
                "node",
                vec!["-e"],
                ["let x = 41", "throw new Error('no')", "x + 1"],
            ),
        ];
        for (w, program, mut args, [set, fail, read]) in cases {
            let driver = w.driver().map(str::to_string);
            if let Some(d) = &driver {
                args.push(d);
            }
            let Ok(mut child) = Command::new(program)
                .args(&args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            else {
                eprintln!("skipping {program}: not on this host");
                continue;
            };
            let value = (w != Wrapper::Sh).then_some("VAL");
            {
                // Closed at the end of this scope: the driver's end of input.
                let mut stdin = child.stdin.take().unwrap();
                for body in [set, fail, read] {
                    writeln!(stdin, "{}", w.session_line(body, value, "END")).unwrap();
                }
            }
            let out = child.wait_with_output().unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(
                stdout.matches("END\n").count(),
                3,
                "{program}: {stdout}\n{stderr}"
            );
            assert_eq!(stderr.matches("END\n").count(), 3, "{program}: {stderr}");
            assert!(
                stdout.trim_end().ends_with("42\nEND"),
                "{program}: {stdout}\n{stderr}"
            );
            assert!(
                stderr.contains("no") || stderr.contains("nterminated"),
                "{program}: {stderr}"
            );
        }
    }

    #[test]
    fn wrappers_are_named_as_the_config_names_them() {
        assert_eq!(Wrapper::named("Python"), Some(Wrapper::Python));
        assert_eq!(Wrapper::named("perl"), None);
    }
}
