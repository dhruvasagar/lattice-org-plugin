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

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

/// The value in `output`: the lines after the LAST sentinel line. `None` when
/// the sentinel never came — the body failed before it — and the caller
/// keeps the whole output, which is what says why.
pub(crate) fn after_sentinel(output: &[String], sentinel: &str) -> Option<Vec<String>> {
    let at = output.iter().rposition(|l| l == sentinel)?;
    Some(output[at + 1..].to_vec())
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
    fn the_value_is_what_follows_the_last_sentinel() {
        let out: Vec<String> = ["noise", "S", "printed S", "S", "42"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(after_sentinel(&out, "S"), Some(vec!["42".to_string()]));
        assert_eq!(after_sentinel(&out[..1], "S"), None);
    }

    #[test]
    fn wrappers_are_named_as_the_config_names_them() {
        assert_eq!(Wrapper::named("Python"), Some(Wrapper::Python));
        assert_eq!(Wrapper::named("perl"), None);
    }
}
