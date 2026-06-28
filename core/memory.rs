//! Process memory hygiene helpers.
//!
//! After a recording's transcription completes, large transient buffers (audio,
//! mel spectrograms, candle/ORT scratch) are freed — but the system allocator
//! keeps the dirty pages cached for reuse instead of returning them to the OS.
//! Measured on a fresh run: ~700 MB of freed-but-retained `MALLOC_LARGE` after
//! only three recordings (37 MB actually live), inflating `phys_footprint`
//! toward the multi-GB figures users see in Activity Monitor. This module asks
//! the allocator to give that memory back at natural quiescent points.

/// Ask the system allocator to return freed-but-retained pages to the OS.
///
/// On macOS this calls `malloc_zone_pressure_relief(NULL, 0)`, which madvises
/// free pages in every malloc zone back to the kernel. It is safe to call at
/// any time; the cost is a scan of free regions, so call it at natural
/// quiescent points (e.g. once after each recording finishes), never in a hot
/// loop. No-op on non-macOS targets.
pub fn release_freed_heap() {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: FFI to a stable libmalloc entry point. A NULL zone means
        // "all zones" and a goal of 0 means "release as much as possible".
        // Returns the number of bytes handed back to the OS.
        let released = unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
        tracing::debug!("release_freed_heap: allocator returned {released} bytes to the OS");
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    /// `size_t malloc_zone_pressure_relief(malloc_zone_t *zone, size_t goal);`
    /// from `<malloc/malloc.h>`. Not exposed by the `libc` crate as a function,
    /// so we declare it directly.
    fn malloc_zone_pressure_relief(zone: *mut core::ffi::c_void, goal: usize) -> usize;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_freed_heap_is_callable_and_safe() {
        // Allocate and drop a large buffer so there is something to reclaim,
        // then ensure the call does not panic (and is a no-op off macOS).
        let big = vec![0u8; 32 * 1024 * 1024];
        let len = big.len();
        drop(big);
        assert_eq!(len, 32 * 1024 * 1024);
        release_freed_heap();
    }
}
