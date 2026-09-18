# UniFFI smoke harness

`main.swift` drives the live `codescribe_ffi` engine end to end from Swift.
It has **no build wiring**: no Makefile target, no Xcode membership, no CI job,
and Cargo ignores this directory (no `main.rs`).

Run it by hand against a built `libcodescribe_ffi.dylib` plus generated bindings.
Archived here rather than deleted — it is the only end-to-end UniFFI proof.
