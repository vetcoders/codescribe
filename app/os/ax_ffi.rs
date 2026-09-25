//! The one Rust declaration of the Accessibility calls shared by several OS
//! modules. Separate `extern` blocks with different pointer types for the same
//! symbol trip `clashing_extern_declarations`, so callers import it from here.

/// Opaque `AXUIElementRef` / `CFTypeRef` handle, a raw pointer across the C FFI.
pub(crate) type AXId = *mut std::ffi::c_void;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    /// Copy one attribute off an AX element. Returns an `AXError` (0 = success)
    /// and writes a `+1` value the caller must `CFRelease`.
    pub(crate) fn AXUIElementCopyAttributeValue(
        element: AXId,
        attribute: AXId,
        value: *mut AXId,
    ) -> i32;
}
