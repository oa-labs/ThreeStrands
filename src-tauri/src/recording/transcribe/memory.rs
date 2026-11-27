//! How much memory the Mac has to spare. A transcription worker holds a
//! whisper model (hundreds of megabytes) while it runs, so the scheduler
//! starts one only when memory pressure is normal and enough memory is
//! available, and stops one when pressure rises.
//!
//! Behind a trait so tests can set both, as [`super::power`] does for the
//! power source.

/// The system's memory pressure, as macOS reports it to apps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemoryPressure {
    Normal,
    /// The system is compressing memory; work that can wait should stop.
    Warning,
    /// The system is about to swap or terminate processes.
    Critical,
}

/// The memory the system has, behind a trait so tests can set it.
pub trait MemoryProbe: Send + Sync {
    fn pressure(&self) -> MemoryPressure;
    /// Bytes a new process could use without making the system page out or
    /// compress others: free, inactive, purgeable, and speculative pages.
    /// `None` if the system can't say.
    fn available_bytes(&self) -> Option<u64>;
}

/// The real probe.
pub struct SystemMemory;

/// The pressure for a `kern.memorystatus_vm_pressure_level` value: the
/// kernel's `kVMPressureNormal` (1), `kVMPressureWarning` (2), and
/// `kVMPressureCritical` (4), the levels a `DISPATCH_SOURCE_TYPE_MEMORYPRESSURE`
/// source reports. Anything unreadable or unknown counts as normal, so a Mac
/// whose level can't be read still transcribes; the memory budget still
/// stops a runaway worker.
pub fn pressure_from_level(level: Option<i32>) -> MemoryPressure {
    match level {
        Some(2) => MemoryPressure::Warning,
        Some(4) => MemoryPressure::Critical,
        _ => MemoryPressure::Normal,
    }
}

/// Available bytes from page counts and the page size.
pub fn available_from_pages(
    free: u64,
    inactive: u64,
    purgeable: u64,
    speculative: u64,
    page_size: u64,
) -> u64 {
    free.saturating_add(inactive)
        .saturating_add(purgeable)
        .saturating_add(speculative)
        .saturating_mul(page_size)
}

#[cfg(target_os = "macos")]
mod mach {
    use std::ffi::c_int;
    use std::sync::OnceLock;

    // libSystem's Mach calls; libc marks its own `mach_host_self` deprecated
    // in favour of another crate.
    extern "C" {
        fn mach_host_self() -> libc::mach_port_t;
        fn host_page_size(host: libc::mach_port_t, size: *mut libc::vm_size_t) -> c_int;
    }

    /// The host port, taken once: every `mach_host_self` call adds a
    /// reference to the same send right.
    fn host() -> libc::mach_port_t {
        static HOST: OnceLock<libc::mach_port_t> = OnceLock::new();
        // SAFETY: plain call returning this task's host port name.
        *HOST.get_or_init(|| unsafe { mach_host_self() })
    }

    /// `kern.memorystatus_vm_pressure_level`, readable by any process.
    pub fn pressure_level() -> Option<i32> {
        let mut level: c_int = 0;
        let mut size = std::mem::size_of::<c_int>();
        // SAFETY: the name is NUL-terminated, `level` and `size` are valid
        // for writes of the sizes given, and no new value is set.
        let status = unsafe {
            libc::sysctlbyname(
                c"kern.memorystatus_vm_pressure_level".as_ptr(),
                (&mut level as *mut c_int).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        (status == 0 && size == std::mem::size_of::<c_int>()).then_some(level)
    }

    /// Free, inactive, purgeable, and speculative pages from
    /// `host_statistics64`, as `vm_stat` reports them, in bytes.
    pub fn available_bytes() -> Option<u64> {
        let mut stats = std::mem::MaybeUninit::<libc::vm_statistics64>::zeroed();
        let mut count = libc::HOST_VM_INFO64_COUNT;
        // SAFETY: `stats` is a zeroed vm_statistics64 and `count` its size
        // in integers; the kernel fills at most that many.
        let status = unsafe {
            libc::host_statistics64(
                host(),
                libc::HOST_VM_INFO64,
                stats.as_mut_ptr().cast(),
                &mut count,
            )
        };
        if status != libc::KERN_SUCCESS {
            return None;
        }
        // SAFETY: zero-initialized and partly filled by the call above.
        let stats = unsafe { stats.assume_init() };
        let mut page_size: libc::vm_size_t = 0;
        // SAFETY: `page_size` is valid for one write.
        if unsafe { host_page_size(host(), &mut page_size) } != libc::KERN_SUCCESS || page_size == 0
        {
            return None;
        }
        Some(super::available_from_pages(
            stats.free_count as u64,
            stats.inactive_count as u64,
            stats.purgeable_count as u64,
            stats.speculative_count as u64,
            page_size as u64,
        ))
    }
}

impl MemoryProbe for SystemMemory {
    #[cfg(target_os = "macos")]
    fn pressure(&self) -> MemoryPressure {
        pressure_from_level(mach::pressure_level())
    }

    #[cfg(not(target_os = "macos"))]
    fn pressure(&self) -> MemoryPressure {
        MemoryPressure::Normal
    }

    #[cfg(target_os = "macos")]
    fn available_bytes(&self) -> Option<u64> {
        mach::available_bytes()
    }

    #[cfg(not(target_os = "macos"))]
    fn available_bytes(&self) -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_levels_map_to_pressure() {
        assert_eq!(pressure_from_level(Some(1)), MemoryPressure::Normal);
        assert_eq!(pressure_from_level(Some(2)), MemoryPressure::Warning);
        assert_eq!(pressure_from_level(Some(4)), MemoryPressure::Critical);
        assert_eq!(pressure_from_level(Some(0)), MemoryPressure::Normal);
        assert_eq!(pressure_from_level(Some(3)), MemoryPressure::Normal);
        assert_eq!(pressure_from_level(None), MemoryPressure::Normal);
        assert!(MemoryPressure::Normal < MemoryPressure::Warning);
        assert!(MemoryPressure::Warning < MemoryPressure::Critical);
    }

    #[test]
    fn available_memory_adds_the_reusable_pages() {
        assert_eq!(available_from_pages(1, 2, 3, 4, 16_384), 10 * 16_384);
        assert_eq!(available_from_pages(u64::MAX, 1, 0, 0, 2), u64::MAX);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_reports_a_level_and_available_memory() {
        let level = mach::pressure_level();
        assert!(matches!(level, Some(1 | 2 | 4)), "{level:?}");
        let available = SystemMemory.available_bytes().unwrap();
        let mut total: u64 = 0;
        let mut size = std::mem::size_of::<u64>();
        // SAFETY: as in `pressure_level`, for a u64.
        let status = unsafe {
            libc::sysctlbyname(
                c"hw.memsize".as_ptr(),
                (&mut total as *mut u64).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        assert_eq!(status, 0);
        assert!(available > 0 && available < total, "{available} of {total}");
        let _ = SystemMemory.pressure();
    }
}
