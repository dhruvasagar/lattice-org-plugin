# Changelog

## Unreleased

Needs **lattice 0.9.10** or later; an older editor refuses the plugin by name.

### Changed
- **Org is one file.** Its manifest is embedded in the component, so a
  release is the `.wasm` alone and there is no `plugin.toml` to copy or
  edit.
- **Where org may write follows your configuration.** Archive, refile and
  capture write only under `org.directory` and `org.roam-directory`. Set
  those to where your notes live — that setting is the permission, and org
  reloads itself when it changes; `:plugins` shows what is in force. The
  shipped manifest used to grant a fixed path (the author's notes folder),
  which you had to edit by hand and which every update put back.
- **`org.directory` defaults to `~/org` and `org.roam-directory` to
  `~/org-roam`**, emacs's and org-roam's own defaults. Both used to be
  unset. A directory that does not exist changes nothing: roam stays off
  until `org.roam-directory` exists, and capture drafts fall back as they
  did when `org.directory` was unset.

### Upgrading
- If your notes are not in `~/org`, set `org.directory` (in `lattice.toml`,
  `init.rs`, or `:set`). Without it, archive, refile and capture into files
  outside `~/org` and `~/org-roam` are refused, with a message naming the
  option.

## 0.1.1 — 2026-10-17

Builds against `lattice:plugin-host@0.3.x` (lattice-wit 0.3.0).

### Fixed
- **A relative file link opens the file beside the note.** `<CR>` on
  `[[file:./About/screenshot.png]]` looked for the file relative to the
  directory Lattice was started in, so it only opened when you launched the
  editor from the note's own folder. It now resolves against the folder of the
  note the link is written in, as org does — the same place the inline image
  was already drawn from. Absolute and `~/` paths are unchanged.
