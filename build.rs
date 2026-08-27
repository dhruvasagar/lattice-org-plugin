//! Fetches and builds org's grammar to wasm.
//!
//! The grammar is deliberately not vendored — `parser.c` is 2.2 MB of
//! generated C, and design §1's whole argument is that a grammar maintained
//! outside crates.io is the PLUGIN's build artefact, not the editor's. So it
//! is cloned here, on this plugin's own build, which is also what the plugin
//! manager does when a user `require`s this plugin from git.
//!
//! Offline, this writes empty bytes rather than failing. The host rejects an
//! empty grammar with a named reason, so the failure stays legible instead of
//! becoming a build error in something that was only ever a reference.

use std::path::PathBuf;

const REPO: &str = "https://github.com/nvim-orgmode/tree-sitter-org";

fn main() {
    // WT.2: the lattice API package is GENERATED, not vendored.
    //
    // `wit_bindgen::generate!` resolves its `path` when the macro expands, so
    // the files have to be on disk beside this crate — but that is a
    // build-time need, and the build is what should meet it. Copying by hand is
    // how this repo's `wit/` silently drifted behind three ABI changes in one
    // day and left the plugin unloadable with nothing said anywhere.
    //
    // `wit/` is gitignored: which ABI this plugin targets is now the pinned
    // `lattice-wit` dependency rather than the state of a folder.
    lattice_wit::write_to("wit").expect("write the lattice WIT API package");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=queries");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let dest = out.join("grammar.wasm");
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("grammar-src");

    if !src.join("src/parser.c").is_file() {
        let _ = std::fs::remove_dir_all(&src);
        let ok = std::process::Command::new("git")
            .args(["clone", "--depth", "1", "--quiet", REPO])
            .arg(&src)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            println!("cargo:warning=lattice-org-plugin: could not fetch the grammar (offline?)");
            std::fs::write(&dest, b"").expect("write placeholder");
            return;
        }
    }

    // The repo's own builder: clang + rustup, no emscripten or docker.
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/build-wasm-grammar.sh");
    let ok = std::process::Command::new("bash")
        .arg(&script)
        .arg("org")
        .arg(src.join("src"))
        .arg(out.join("wasm-grammars"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let built = out.join("wasm-grammars/tree-sitter-org.wasm");
    if ok && built.is_file() {
        std::fs::copy(&built, &dest).expect("copy grammar");
    } else {
        println!("cargo:warning=lattice-org-plugin: grammar build failed");
        std::fs::write(&dest, b"").expect("write placeholder");
    }
}
