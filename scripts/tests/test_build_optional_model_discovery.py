"""Execute the real build-script body with discovery tripwires.

Only the three optional resolvers and the HF lookup receive a panic tripwire in
an isolated source copy. Their callers, embedding policies and generated files
remain the production code. No user cache, model or network is accessed.
"""

import argparse
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
RESOLVERS = ("resolve_whisper_embed_model_path", "resolve_tts_embed_model_path",
             "resolve_embedder_model_path", "find_hf_snapshot")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-ref", help="frozen Git source for the before witness")
    args = parser.parse_args()
    source = (subprocess.check_output(["git", "show", f"{args.source_ref}:core/build.rs"],
                                     cwd=ROOT, text=True) if args.source_ref
              else (ROOT / "core/build.rs").read_text())
    for module in ("whisper_weights.rs", "licensing/key_contract.rs"):
        source = source.replace(f'#[path = "{module}"]',
                                f'#[path = "{ROOT / "core" / module}"]')
    source += "\nstatic DISCOVERY_TRIPWIRE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);\n"
    for name in RESOLVERS:
        pattern = rf"(fn {name}\([^{{]*\{{)"
        source, count = re.subn(pattern, lambda match: match[1] +
            f'\nif DISCOVERY_TRIPWIRE.load(std::sync::atomic::Ordering::SeqCst) {{ panic!("discovery:{name}"); }}\n',
            source)
        if count != 1:
            raise RuntimeError(f"discovery instrument expected one definition of {name}, got {count}")
    source += r'''
#[cfg(test)]
mod optional_model_discovery {
    fn run(whisper: bool, tts: bool, embedder: bool, off: bool) {
        let output = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("PROFILE", "debug");
            std::env::set_var("OUT_DIR", output.path());
            std::env::set_var("CARGO_MANIFEST_DIR", std::env::var("DISCOVERY_REPO_CORE").unwrap());
            for (key, value) in [("CODESCRIBE_EMBED_WHISPER", whisper),
                                 ("CODESCRIBE_EMBED_TTS", tts),
                                 ("CODESCRIBE_EMBED_EMBEDDER", embedder)] {
                std::env::set_var(key, if value { "1" } else { "0" });
            }
            if off { std::env::set_var("CODESCRIBE_NO_EMBED", "1"); }
            else { std::env::remove_var("CODESCRIBE_NO_EMBED"); }
        }
        super::main();
        assert!(output.path().join("embedded_vad_data.rs").exists());
    }
    #[test]
    fn slim_does_not_touch_optional_models() { run(false, false, false, false); }
    #[test]
    fn off_overrides_all_optional_requests() { run(true, true, true, true); }
    #[test]
    #[should_panic(expected = "discovery:resolve_whisper_embed_model_path")]
    fn whisper_opt_in_reaches_discovery() { run(true, false, false, false); }
    #[test]
    #[should_panic(expected = "discovery:resolve_tts_embed_model_path")]
    fn tts_opt_in_reaches_discovery() { run(false, true, false, false); }
    #[test]
    #[should_panic(expected = "discovery:resolve_embedder_model_path")]
    fn embedder_opt_in_reaches_discovery() { run(false, false, true, false); }
}
'''
    with tempfile.TemporaryDirectory(prefix="codescribe-build-discovery-") as temporary:
        probe = Path(temporary)
        (probe / "lib.rs").write_text(source)
        (probe / "Cargo.toml").write_text('''[package]
name = "codescribe-build-discovery-probe"
version = "0.0.0"
edition = "2024"
[lib]
path = "lib.rs"
[dependencies]
anyhow = "1"
dirs = "7"
serde_json = "1"
sha2 = "0.11"
hex = "0.4"
tokenizers = "0.23"
tempfile = "3"
''')
        import os
        environment = dict(os.environ, DISCOVERY_REPO_CORE=str(ROOT / "core"))
        result = subprocess.run(["cargo", "test", "--offline", "--lib", "--manifest-path",
                                 str(probe / "Cargo.toml"), "optional_model_discovery", "--",
                                 "--test-threads=1"], env=environment,
                                text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        print(result.stdout, end="")
        if not re.search(r"running 5 tests\b", result.stdout):
            raise SystemExit("discovery probe did not execute all five acceptance cases")
        return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
