//! The fixture corpus: three months of one person's org files, written
//! relative to today so the agenda, the clock and the dailies always have a
//! "now" to be about.
//!
//! The layout is the usual one: `org/` holds the agenda files (projects, a
//! calendar, habits, and a daily file for each of the last ninety days) and
//! `roam/` holds the notes. Every fact a task is graded on is planted here,
//! next to a distractor that a careless answer would pick up instead.

use std::path::{Path, PathBuf};

/// Today as the guest computes it: local, days since the epoch.
pub fn today() -> i64 {
    let utc = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    let offset = i64::from(chrono::Local::now().offset().local_minus_utc());
    (utc + offset).div_euclid(86_400)
}

/// `(YYYY-MM-DD, Day)` for `today() + offset`.
pub fn date(offset: i64) -> (String, &'static str) {
    let days = today() + offset;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    // 1970-01-01 was a Thursday.
    const NAMES: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    (
        format!("{y:04}-{m:02}-{d:02}"),
        NAMES[days.rem_euclid(7) as usize],
    )
}

/// An active stamp, `<2026-11-12 Thu 09:30 +1d>`; `rest` starts with a space.
pub fn active(offset: i64, rest: &str) -> String {
    let (day, name) = date(offset);
    format!("<{day} {name}{rest}>")
}

/// An inactive stamp with a time, `[2026-11-12 Thu 09:30]`.
pub fn inactive(offset: i64, time: &str) -> String {
    let (day, name) = date(offset);
    format!("[{day} {name} {time}]")
}

/// One `CLOCK:` line, `offset` days from today, `from`–`to` the same day.
fn clock(offset: i64, from: &str, to: &str) -> String {
    let minutes = |t: &str| {
        let (h, m) = t.split_once(':').unwrap_or(("0", "0"));
        h.parse::<i64>().unwrap_or(0) * 60 + m.parse::<i64>().unwrap_or(0)
    };
    let span = minutes(to) - minutes(from);
    format!(
        "CLOCK: {}--{} => {:2}:{:02}\n",
        inactive(offset, from),
        inactive(offset, to),
        span / 60,
        span % 60
    )
}

/// What the clock task expects: each project with time in the last seven
/// days (today and the six before), in minutes. Mobile app's time is all
/// older than that, and one Hiring span sits a day outside the window.
pub const CLOCKED: &[(&str, u32)] = &[("Website redesign", 225), ("Hiring", 135)];

fn projects() -> String {
    let mut s = String::from("#+title: Projects\n\n");
    s.push_str("* Website redesign :proj:\n:LOGBOOK:\n");
    s.push_str(&clock(-1, "09:00", "11:30"));
    s.push_str(&clock(-3, "14:00", "15:15"));
    s.push_str(&clock(-12, "10:00", "12:00"));
    s.push_str(&clock(-30, "13:00", "16:00"));
    s.push_str(":END:\n");
    s.push_str(&format!(
        "** TODO Draft the landing page copy\nSCHEDULED: {}\n",
        active(2, "")
    ));
    s.push_str(&format!(
        "** DONE Pick the colour palette\nCLOSED: {}\n",
        inactive(-20, "16:40")
    ));
    s.push_str("* Hiring :proj:\n:LOGBOOK:\n");
    s.push_str(&clock(-2, "13:00", "14:30"));
    s.push_str(&clock(-6, "09:00", "09:45"));
    s.push_str(&clock(-7, "10:00", "11:00"));
    s.push_str(&clock(-45, "10:00", "12:30"));
    s.push_str(":END:\n");
    s.push_str("** WAITING Reply from the recruiter\n");
    s.push_str(&format!(
        "** TODO Write the job post\nDEADLINE: {}\n",
        active(5, "")
    ));
    s.push_str("* Mobile app :proj:\n:LOGBOOK:\n");
    s.push_str(&clock(-40, "09:00", "12:00"));
    s.push_str(&clock(-60, "14:00", "17:30"));
    s.push_str(":END:\n");
    s.push_str("** TODO Fix the login crash\n");
    s.push_str(&format!(
        "** DONE Ship the beta\nCLOSED: {} SCHEDULED: {}\n",
        inactive(-1, "17:05"),
        active(-1, "")
    ));
    s.push_str("* Admin\n");
    s.push_str(&format!(
        "** TODO Write the Q3 review :work:\nSCHEDULED: {}\n",
        active(0, "")
    ));
    s.push_str(&format!(
        "** TODO Call the plumber\nDEADLINE: {}\n",
        active(0, "")
    ));
    s.push_str(&format!(
        "** TODO Pay the invoice :money:\nDEADLINE: {}\n",
        active(-3, "")
    ));
    s.push_str(&format!(
        "** TODO Renew passport\nSCHEDULED: {}\n",
        active(1, "")
    ));
    s.push_str(&format!(
        "** DONE Review the vendor contract\nCLOSED: {}\n",
        inactive(-9, "11:20")
    ));
    s
}

fn calendar() -> String {
    format!(
        "#+title: Calendar\n\n\
         * TODO Standup\nSCHEDULED: {}\n\
         * Dentist {}\n\
         * Team offsite {}\n\
         * Book club {}\n",
        active(0, " 09:30 +1d"),
        active(0, " 15:00"),
        active(10, ""),
        active(-4, " 19:00"),
    )
}

fn habits() -> String {
    let mut s = format!(
        "#+title: Habits\n\n\
         * TODO Exercise\nSCHEDULED: {}\n:PROPERTIES:\n:STYLE: habit\n:END:\n:LOGBOOK:\n",
        active(0, " .+1d")
    );
    for back in [1, 2, 4, 5, 7, 8, 9, 12, 14] {
        s.push_str(&format!(
            "- State \"DONE\"       from \"TODO\"       {}\n",
            inactive(-back, "07:10")
        ));
    }
    s.push_str(":END:\n");
    s
}

fn daily(offset: i64) -> String {
    let (day, name) = date(offset);
    let topic = [
        "the landing page",
        "interview loops",
        "the login crash",
        "budget numbers",
        "the vendor call",
    ][offset.unsigned_abs() as usize % 5];
    format!("#+title: {day} {name}\n\n* Journal\nSpent the morning on {topic}.\n")
}

/// A roam note: its ID, title, aliases, tags and body.
struct Note {
    id: &'static str,
    title: &'static str,
    aliases: &'static str,
    tags: &'static str,
    body: &'static str,
}

/// The notes the notes task must read, each with a phrase only its body has.
pub const MIGRATION_NOTES: &[(&str, &str)] = &[
    ("note-db-plan", "pgloader handles the bulk copy"),
    ("note-db-rollback", "read-only for 48 hours"),
];

const NOTES: &[Note] = &[
    Note {
        id: "note-db-plan",
        title: "Database migration plan",
        aliases: "",
        tags: ":database:",
        body: "Move from MySQL 5.7 to Postgres 16 in three phases. Freeze schema \
               changes two weeks before the cutover; pgloader handles the bulk copy. \
               See the [[id:note-db-rollback][rollback notes]].",
    },
    Note {
        id: "note-db-rollback",
        title: "Migration rollback notes",
        aliases: "\"Database migration rollback\"",
        tags: ":database:",
        body: "Keep the MySQL primary read-only for 48 hours after the cutover so a \
               rollback loses nothing. Practise the switch back on staging first.",
    },
    Note {
        id: "note-db-backups",
        title: "Database backups",
        aliases: "",
        tags: ":database:",
        body: "Nightly pg_dump to the object store, kept for 30 days.",
    },
    Note {
        id: "note-birds",
        title: "Bird migration in spring",
        aliases: "",
        tags: ":nature:",
        body: "Swifts arrive in late April; the marsh is best at dawn.",
    },
    Note {
        id: "note-sourdough",
        title: "Sourdough starter",
        aliases: "",
        tags: ":food:",
        body: "Feed 1:1:1 twice a day; it peaks after six hours.",
    },
];

fn note(n: &Note) -> String {
    let mut s = format!(":PROPERTIES:\n:ID: {}\n", n.id);
    if !n.aliases.is_empty() {
        s.push_str(&format!(":ROAM_ALIASES: {}\n", n.aliases));
    }
    s.push_str(&format!(":END:\n#+title: {}\n", n.title));
    if !n.tags.is_empty() {
        s.push_str(&format!("#+filetags: {}\n", n.tags));
    }
    s.push_str(&format!("\n{}\n", n.body));
    s
}

/// A corpus on disk.
pub struct Corpus {
    pub root: PathBuf,
}

impl Corpus {
    /// Write the corpus under `root`.
    pub fn write(root: &Path) -> std::io::Result<Self> {
        let org = root.join("org");
        let dailies = org.join("daily");
        let roam = root.join("roam");
        std::fs::create_dir_all(&dailies)?;
        std::fs::create_dir_all(&roam)?;
        std::fs::write(org.join("projects.org"), projects())?;
        std::fs::write(org.join("calendar.org"), calendar())?;
        std::fs::write(org.join("habits.org"), habits())?;
        for back in 1..=90 {
            let (day, _) = date(-back);
            std::fs::write(dailies.join(format!("{day}.org")), daily(-back))?;
        }
        for n in NOTES {
            let slug = n.title.to_lowercase().replace(' ', "_");
            std::fs::write(roam.join(format!("{slug}.org")), note(n))?;
        }
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    pub fn agenda_dir(&self) -> PathBuf {
        self.root.join("org")
    }

    pub fn roam_dir(&self) -> PathBuf {
        self.root.join("roam")
    }

    pub fn projects(&self) -> PathBuf {
        self.agenda_dir().join("projects.org")
    }

    /// Every file under the corpus, path and text, sorted by path.
    pub fn files(&self) -> Vec<(PathBuf, String)> {
        let mut out = Vec::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Ok(text) = std::fs::read_to_string(&p) {
                    out.push((p, text));
                }
            }
        }
        out.sort();
        out
    }
}
