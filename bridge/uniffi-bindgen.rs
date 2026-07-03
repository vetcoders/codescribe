// Generates foreign-language bindings from the built dylib:
//   cargo run -p codescribe-ffi --bin uniffi-bindgen -- \
//     generate --library target/debug/libcodescribe_ffi.dylib --language swift --out-dir <dir>
fn main() {
    uniffi::uniffi_bindgen_main()
}
