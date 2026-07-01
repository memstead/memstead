use std::env;
use std::fs;
use std::path::Path;

const BASE_UDL: &str = "src/memstead.udl";
const TEST_SUPPORT_UDL: &str = "src/memstead-test-support.udl";

fn main() {
    println!("cargo:rerun-if-changed={BASE_UDL}");
    println!("cargo:rerun-if-changed={TEST_SUPPORT_UDL}");

    // When built with `--features test-support`, append the test-support
    // UDL fragment so the scaffolding exposes the seeding entry point. The
    // featureless build generates scaffolding from the base UDL alone, so
    // the seeding symbols never enter the shipping framework. The fragment
    // keeps the base file's single `namespace memstead { … }` intact (it
    // only adds whole `interface` blocks), so a verbatim concatenation is a
    // valid UDL with the same `memstead` namespace — and therefore the same
    // `memstead.uniffi.rs` output `include_scaffolding!("memstead")` expects.
    let udl_path = if env::var_os("CARGO_FEATURE_TEST_SUPPORT").is_some() {
        // The synthesized UDL must satisfy two uniffi constraints:
        //   1. its grand-parent dir holds `Cargo.toml` (crate-root guess), so
        //      it lives one level deep — a sibling `.synth/` dir of `src/`;
        //   2. its file stem is `memstead`, since the emitted scaffolding is
        //      named `<stem>.uniffi.rs` and `include_scaffolding!("memstead")`
        //      reads `memstead.uniffi.rs`. Hence `.synth/memstead.udl`, not a
        //      `memstead.synth.udl` in `src/` (which would collide with the
        //      base UDL or emit the wrong filename). Git-ignored.
        let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by cargo");
        let synth_dir = Path::new(&manifest).join(".synth");
        fs::create_dir_all(&synth_dir).expect("create .synth dir");
        let synthesized = synth_dir.join("memstead.udl");
        let base = fs::read_to_string(BASE_UDL).expect("read base UDL");
        let fragment = fs::read_to_string(TEST_SUPPORT_UDL).expect("read test-support UDL");
        fs::write(&synthesized, format!("{base}\n{fragment}")).expect("write synthesized UDL");
        synthesized.to_string_lossy().into_owned()
    } else {
        BASE_UDL.to_string()
    };

    uniffi::generate_scaffolding(&udl_path).expect("UniFFI scaffolding generation failed");
}
