//! UniFFI binding generator, shipped as its own binary.
//!
//! Bindings are derived from the compiled dylib rather than from the source,
//! so what Swift sees is always what the library actually exports.

// Generates foreign-language bindings from the built dylib:
//   cargo run -p codescribe-ffi --bin uniffi-bindgen -- \
//     generate --library target/debug/libcodescribe_ffi.dylib --language swift --out-dir <dir>
/// Hand straight over to UniFFI's own CLI entry point.
fn main() {
    uniffi::uniffi_bindgen_main()
}
