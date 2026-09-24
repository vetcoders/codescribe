//! Process memory hygiene helpers.
//!
//! After a recording's transcription completes, large transient buffers (audio,
//! mel spectrograms, candle/ORT scratch) are freed — but the system allocator
//! keeps the dirty pages cached for reuse instead of returning them to the OS.
//! Measured on a fresh run: ~700 MB of freed-but-retained `MALLOC_LARGE` after
//! only three recordings (37 MB actually live), inflating `phys_footprint`
//! toward the multi-GB figures users see in Activity Monitor. This module asks
//! the allocator to give that memory back at natural quiescent points.

/// This process's `phys_footprint` in bytes.
///
/// Same counter `footprint -p` prints. macOS reports it through `task_info`
/// flavor `TASK_VM_INFO` (22) on the current task. `None` off macOS, or when
/// the kernel returns a `task_vm_info` shorter than the `phys_footprint` field.
pub fn phys_footprint_bytes() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        macos_phys_footprint_bytes()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// `task_vm_info` prefix through `phys_footprint`.
///
/// xnu places `phys_footprint` at byte 144 of `task_vm_info`.
#[cfg(target_os = "macos")]
#[repr(C)]
struct TaskVmInfo {
    _before_phys_footprint: [u8; 144],
    phys_footprint: u64,
}

#[cfg(target_os = "macos")]
fn macos_phys_footprint_bytes() -> Option<u64> {
    // The libc wrapper around this symbol is deprecated in favor of mach2.
    // The Mach global itself is the current-task port.
    unsafe extern "C" {
        static mach_task_self_: libc::mach_port_t;
    }
    /// `TASK_VM_INFO` from `<mach/task_info.h>`. Not exported by libc 0.2.
    const TASK_VM_INFO: libc::task_flavor_t = 22;

    let mut info = std::mem::MaybeUninit::<TaskVmInfo>::zeroed();
    let mut count = (std::mem::size_of::<TaskVmInfo>() / std::mem::size_of::<libc::natural_t>())
        as libc::mach_msg_type_number_t;
    // SAFETY: `mach_task_self_` is this process's task port. `info` is a
    // zeroed `task_vm_info` prefix and `count` is its size in `natural_t`s.
    // `KERN_SUCCESS` means the kernel wrote `count` fields.
    let rc = unsafe {
        libc::task_info(
            mach_task_self_,
            TASK_VM_INFO,
            info.as_mut_ptr().cast(),
            &mut count,
        )
    };
    if rc != libc::KERN_SUCCESS {
        return None;
    }
    let needed = (std::mem::size_of::<TaskVmInfo>() / std::mem::size_of::<libc::natural_t>())
        as libc::mach_msg_type_number_t;
    if count < needed {
        return None;
    }
    Some(unsafe { info.assume_init() }.phys_footprint)
}

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

/// Force the Candle Metal free-buffer pool prune after model weights drop.
///
/// Dropped weight tensors do not free their `MTLBuffer`s: candle's
/// `MetalDevice` returns them to a free-buffer pool, and the only prune
/// (`drop_unused_buffers`, private in candle-core) runs when an encoder
/// request rotates a command buffer — i.e. during the NEXT inference, never
/// while idle. A TTL weight unload alone therefore frees ~14 MB of host
/// bookkeeping while multi-GB of pooled buffers stay resident.
///
/// This forces that prune deterministically through public API only: each
/// `blit_command_encoder()` call increments the per-command-buffer compute
/// counter, and once a counter passes `compute_per_buffer` the device prunes
/// every pool buffer with `strong_count == 1` — exactly the dropped weights.
/// With `pool_size` command buffers, `(compute_per_buffer + 1) * pool_size`
/// sequential empty encoders guarantee at least one counter passes the
/// threshold (pigeonhole), regardless of which buffer each call lands on.
/// The thresholds mirror candle's env-tunable defaults so the bound holds
/// under `CANDLE_METAL_*` overrides too. No-op on CPU/CUDA devices.
pub fn reclaim_metal_buffer_pool(device: &candle_core::Device) {
    let candle_core::Device::Metal(metal) = device else {
        return;
    };
    let compute_per_buffer = candle_env_usize("CANDLE_METAL_COMPUTE_PER_BUFFER", 50);
    let pool_size = candle_env_usize("CANDLE_METAL_COMMAND_POOL_SIZE", 5);
    let calls = compute_per_buffer
        .saturating_add(1)
        .saturating_mul(pool_size);
    for _ in 0..calls {
        match metal.blit_command_encoder() {
            // BlitCommandEncoder has no Drop impl: end_encoding() is mandatory
            // or the entry's semaphore stays in Encoding and the next call hangs.
            Ok(encoder) => encoder.end_encoding(),
            Err(e) => {
                tracing::warn!("reclaim_metal_buffer_pool: encoder request failed: {e}");
                return;
            }
        }
    }
    // Drain the empty command buffers committed by the forced rotations.
    if let Err(e) = metal.wait_until_completed() {
        tracing::warn!("reclaim_metal_buffer_pool: wait_until_completed failed: {e}");
    }
    tracing::info!("reclaim_metal_buffer_pool: forced Metal free-pool prune ({calls} rotations)");
}

/// Read a candle tuning env var with the same fallback candle itself uses
/// (`candle-metal-kernels` parses these at device creation).
fn candle_env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    /// `size_t malloc_zone_pressure_relief(malloc_zone_t *zone, size_t goal);`
    /// from `<malloc/malloc.h>`. Not exposed by the `libc` crate as a function,
    /// so we declare it directly.
    fn malloc_zone_pressure_relief(zone: *mut core::ffi::c_void, goal: usize) -> usize;
}

/// Heap release and Metal free-pool prune safety (no-op / terminate / callable).
#[cfg(test)]
mod tests {
    use super::*;

    /// CPU devices return immediately without attempting Metal encoder rotation.
    #[test]
    fn reclaim_metal_buffer_pool_is_noop_on_cpu() {
        reclaim_metal_buffer_pool(&candle_core::Device::Cpu);
    }

    /// On Metal, reclaim must finish a bounded rotation and stay idempotent.
    #[test]
    fn reclaim_metal_buffer_pool_terminates_on_metal() {
        // The deterministic claim is a bounded call count: the reclaim must
        // terminate (no semaphore wedge) even with pooled buffers present.
        let Ok(device) = candle_core::Device::new_metal(0) else {
            return; // no Metal on this host (CI without GPU)
        };
        let tensor = candle_core::Tensor::zeros((1024, 1024), candle_core::DType::F32, &device)
            .expect("tensor alloc");
        drop(tensor);
        reclaim_metal_buffer_pool(&device);
        reclaim_metal_buffer_pool(&device); // idempotent on an empty pool
    }

    /// `phys_footprint` is a live positive counter on macOS and absent elsewhere.
    #[test]
    fn phys_footprint_bytes_matches_host() {
        let bytes = phys_footprint_bytes();
        if cfg!(target_os = "macos") {
            let bytes = bytes.expect("task_info phys_footprint");
            assert!(bytes > 1_048_576, "phys_footprint {bytes} is below 1 MiB");
            assert!(
                bytes < 64 * 1024 * 1024 * 1024,
                "phys_footprint {bytes} exceeds 64 GiB"
            );
        } else {
            assert!(bytes.is_none());
        }
    }

    /// `release_freed_heap` is always safe to call (no-op off macOS).
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
