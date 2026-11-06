//! macOS sampling through sysctl, mach and NSURL.

use std::ffi::CString;
use std::mem::{MaybeUninit, size_of};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSArray, NSNumber, NSString, NSURL, NSURLVolumeAvailableCapacityForImportantUsageKey,
    NSURLVolumeTotalCapacityKey,
};

use super::{Snapshot, VmPages, available_bytes};
use crate::status::PressureLevel;

/// Reads a fixed-size value via `sysctlbyname`; the kernel must fill exactly `size_of::<T>()` bytes.
fn sysctl_by_name<T: Copy>(name: &str) -> Result<T, String> {
    let cname = CString::new(name).map_err(|_| format!("sysctlbyname({name}): invalid name"))?;
    let mut value = MaybeUninit::<T>::zeroed();
    let mut len = size_of::<T>();
    // SAFETY: `cname` is NUL-terminated, `value` is a writable buffer of `len` bytes, and
    // `len` points to a valid usize; no new value is passed in.
    let rc = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            value.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(format!("sysctlbyname({name}) failed: {}", std::io::Error::last_os_error()));
    }
    if len != size_of::<T>() {
        return Err(format!("sysctlbyname({name}) returned {len} bytes, expected {}", size_of::<T>()));
    }
    // SAFETY: the kernel wrote all `size_of::<T>()` bytes (checked above) and the buffer was
    // zero-initialised; T is a plain-data type (Copy).
    Ok(unsafe { value.assume_init() })
}

fn vm_stats() -> Result<libc::vm_statistics64, String> {
    let mut stats = MaybeUninit::<libc::vm_statistics64>::zeroed();
    let mut count = libc::HOST_VM_INFO64_COUNT;
    #[allow(deprecated)]
    // SAFETY: `mach_host_self` has no preconditions; `stats` is a writable vm_statistics64
    // buffer and `count` holds its size in integer_t units, as host_statistics64 requires.
    let rc = unsafe {
        libc::host_statistics64(
            libc::mach_host_self(),
            libc::HOST_VM_INFO64,
            stats.as_mut_ptr().cast(),
            &mut count,
        )
    };
    if rc != libc::KERN_SUCCESS {
        return Err(format!("host_statistics64 failed: kern_return_t {rc}"));
    }
    // SAFETY: host_statistics64 succeeded, so the structure has been filled in.
    Ok(unsafe { stats.assume_init() })
}

fn number_for(dict: &objc2_foundation::NSDictionary<NSString, AnyObject>, key: &NSString) -> Option<u64> {
    let obj = dict.objectForKey(key)?;
    let n = obj.downcast_ref::<NSNumber>()?;
    Some(n.unsignedLongLongValue())
}

/// Disk (free, total) via NSURL resource values; free includes purgeable space.
fn disk_nsurl() -> Option<(u64, u64)> {
    let url: Retained<NSURL> = NSURL::fileURLWithPath(&NSString::from_str("/"));
    // SAFETY: both statics are valid NSURLResourceKey (NSString) constants provided by Foundation.
    let (free_key, total_key) = unsafe {
        (NSURLVolumeAvailableCapacityForImportantUsageKey, NSURLVolumeTotalCapacityKey)
    };
    let keys = NSArray::from_slice(&[free_key, total_key]);
    let values = url.resourceValuesForKeys_error(&keys).ok()?;
    let free = number_for(&values, free_key)?;
    let total = number_for(&values, total_key)?;
    Some((free, total))
}

/// Fallback disk reading from `statfs("/")`.
fn disk_statfs() -> Result<(u64, u64), String> {
    let mut st = MaybeUninit::<libc::statfs>::zeroed();
    // SAFETY: the path is a NUL-terminated literal and `st` is a writable statfs buffer.
    let rc = unsafe { libc::statfs(c"/".as_ptr(), st.as_mut_ptr()) };
    if rc != 0 {
        return Err(format!("statfs(/) failed: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: statfs succeeded, so the structure has been filled in.
    let st = unsafe { st.assume_init() };
    let bsize = u64::from(st.f_bsize);
    Ok((st.f_bavail * bsize, st.f_blocks * bsize))
}

/// Takes one reading of disk, memory, pressure and swap from the live system.
pub fn sample() -> Result<Snapshot, String> {
    let mem_total: u64 = sysctl_by_name("hw.memsize")?;
    let page_size: u64 = {
        // vm.pagesize is a 32-bit int on macOS; read as i32 and widen.
        let p: i32 = sysctl_by_name("vm.pagesize")?;
        u64::try_from(p).map_err(|_| format!("vm.pagesize invalid: {p}"))?
    };
    let vm = vm_stats()?;
    let pages = VmPages {
        free: u64::from(vm.free_count),
        inactive: u64::from(vm.inactive_count),
        purgeable: u64::from(vm.purgeable_count),
        speculative: u64::from(vm.speculative_count),
        compressor: u64::from(vm.compressor_page_count),
    };
    let raw_pressure: i32 = sysctl_by_name("kern.memorystatus_vm_pressure_level")?;
    let swap: libc::xsw_usage = sysctl_by_name("vm.swapusage")?;
    let (disk_free, disk_total) = match disk_nsurl() {
        Some(v) => v,
        None => disk_statfs()?,
    };

    Ok(Snapshot {
        disk_free_bytes: disk_free,
        disk_total_bytes: disk_total,
        mem_total_bytes: mem_total,
        mem_available_bytes: available_bytes(&pages, page_size).min(mem_total),
        mem_compressed_bytes: pages.compressor * page_size,
        pressure: PressureLevel::from_raw(raw_pressure),
        swap_used_bytes: swap.xsu_used,
        swap_total_bytes: swap.xsu_total,
    })
}
