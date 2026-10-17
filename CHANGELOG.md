# Changelog

## 0.1.1 — 2026-10-17

Builds against `lattice:plugin-host@0.3.x` (lattice-wit 0.3.0).

### Fixed
- **A relative file link opens the file beside the note.** `<CR>` on
  `[[file:./About/screenshot.png]]` looked for the file relative to the
  directory Lattice was started in, so it only opened when you launched the
  editor from the note's own folder. It now resolves against the folder of the
  note the link is written in, as org does — the same place the inline image
  was already drawn from. Absolute and `~/` paths are unchanged.
