//! The chooser: pick one `.org` file directly in a folder, guessing from the
//! buffer and the running clock.
//!
//! A capture group with `choose = "<dir>"` uses it to decide which file its
//! members capture into. The guess is computed fresh on every open, from two
//! things the user can see, and the menu shows it with where it came from:
//!
//! 1. **The buffer**, if it visits a file directly in the folder.
//! 2. **The clock**, if the running clock is in a file directly in the folder.
//!    You clock in to a meeting, and the next capture, made from a code repo,
//!    lands in the meeting's file.
//!
//! Otherwise there is no guess, and the picker opens when a choice is needed.
//! Nothing here is state: no "current file" is remembered between opens.
//!
//! **Direct children only.** `<dir>/archive/old.org` is not a choice, which is
//! what lets a folder keep finished files in a subfolder without a setting.
//!
//! Design: `docs/dev/architecture/org-clients.md` §3 in the lattice repo.

/// Where a guess came from, for the menu title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The file the buffer visits.
    Buffer,
    /// The file the running clock is in.
    Clock,
    /// Picked in the chooser.
    Chosen,
}

impl Source {
    /// The wire spelling, carried in the menu's open arguments.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Buffer => "buffer",
            Source::Clock => "clock",
            Source::Chosen => "chosen",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "buffer" => Some(Source::Buffer),
            "clock" => Some(Source::Clock),
            "chosen" => Some(Source::Chosen),
            _ => None,
        }
    }

    /// What the menu title says about it.
    pub fn label(self) -> &'static str {
        match self {
            Source::Buffer => "this buffer",
            Source::Clock => "clocked in",
            Source::Chosen => "chosen",
        }
    }
}

/// The folder as a path to compare against: `~` expanded, no trailing `/`.
pub fn folder(dir: &str) -> String {
    let expanded = crate::roam_scan::expand_tilde(dir.trim());
    let trimmed = expanded.trim_end_matches('/');
    if trimmed.is_empty() {
        expanded
    } else {
        trimmed.to_string()
    }
}

/// Whether `path` is an `.org` file directly in `folder` (already passed
/// through [`folder`]).
pub fn is_choice(folder: &str, path: &str) -> bool {
    let Some(name) = path
        .strip_prefix(folder)
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return false;
    };
    !name.is_empty() && !name.contains('/') && name.ends_with(".org") && !name.starts_with('.')
}

/// The guess for `folder`: the buffer's file, then the clock's.
pub fn guess(folder: &str, buffer: Option<&str>, clock: Option<&str>) -> Option<(String, Source)> {
    [(buffer, Source::Buffer), (clock, Source::Clock)]
        .into_iter()
        .find_map(|(path, source)| {
            path.filter(|p| is_choice(folder, p))
                .map(|p| (p.to_string(), source))
        })
}

/// The choices among `walked` (every file under the folder), sorted by name.
pub fn choices(folder: &str, walked: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = walked
        .into_iter()
        .filter(|p| is_choice(folder, p))
        .collect();
    out.sort();
    out
}

/// A choice as the user knows it: the file's name without `.org`.
pub fn label(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".org").unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = "/home/u/org/clients";

    #[test]
    fn only_org_files_directly_in_the_folder_are_choices() {
        assert!(is_choice(DIR, "/home/u/org/clients/acme.org"));
        assert!(
            !is_choice(DIR, "/home/u/org/clients/archive/old.org"),
            "a subfolder is where finished files go"
        );
        assert!(!is_choice(DIR, "/home/u/org/clients/notes.md"));
        assert!(!is_choice(DIR, "/home/u/org/clients/.hidden.org"));
        assert!(!is_choice(DIR, "/home/u/org/clientsx/acme.org"));
        assert!(!is_choice(DIR, "/home/u/org/acme.org"));
    }

    #[test]
    fn the_buffer_beats_the_clock() {
        let buffer = Some("/home/u/org/clients/acme.org");
        let clock = Some("/home/u/org/clients/globex.org");
        assert_eq!(
            guess(DIR, buffer, clock),
            Some(("/home/u/org/clients/acme.org".to_string(), Source::Buffer))
        );
    }

    #[test]
    fn the_clock_guesses_when_the_buffer_is_elsewhere() {
        // Working in a code repo, clocked in to a client meeting.
        let buffer = Some("/home/u/src/app/main.rs");
        let clock = Some("/home/u/org/clients/globex.org");
        assert_eq!(
            guess(DIR, buffer, clock),
            Some(("/home/u/org/clients/globex.org".to_string(), Source::Clock))
        );
    }

    #[test]
    fn no_guess_when_neither_is_in_the_folder() {
        assert_eq!(
            guess(
                DIR,
                Some("/home/u/src/app/main.rs"),
                Some("/home/u/org/todo.org")
            ),
            None
        );
        assert_eq!(guess(DIR, None, None), None);
    }

    #[test]
    fn choices_are_sorted_and_skip_subfolders() {
        let walked = vec![
            "/home/u/org/clients/globex.org".to_string(),
            "/home/u/org/clients/archive/initech.org".to_string(),
            "/home/u/org/clients/acme.org".to_string(),
        ];
        assert_eq!(
            choices(DIR, walked),
            vec![
                "/home/u/org/clients/acme.org".to_string(),
                "/home/u/org/clients/globex.org".to_string()
            ]
        );
    }

    #[test]
    fn a_trailing_slash_on_the_folder_is_ignored() {
        assert_eq!(folder("/home/u/org/clients/"), DIR);
        assert_eq!(label("/home/u/org/clients/acme.org"), "acme");
    }
}
