//! Per-process sampling through libproc and sysctl.

use std::mem::{MaybeUninit, size_of};

use super::ProcInfo;

/// Lists all pids, growing the buffer until the kernel's answer fits with room to spare.
fn list_pids() -> Vec<i32> {
    let mut capacity = 1024usize;
    loop {
        let mut pids = vec![0i32; capacity];
        let bytes = (capacity * size_of::<i32>()) as libc::c_int;
        // SAFETY: `pids` is a writable buffer of exactly `bytes` bytes.
        let rc = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        if rc <= 0 {
            return Vec::new();
        }
        let count = rc as usize / size_of::<i32>();
        if count < capacity {
            pids.truncate(count);
            return pids;
        }
        capacity *= 2;
    }
}

fn pid_path(pid: i32) -> Option<String> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `buf` is writable and its length is passed as the buffer size.
    let rc = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if rc <= 0 {
        return None;
    }
    buf.truncate(rc as usize);
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn pid_footprint(pid: i32) -> Option<u64> {
    let mut info = MaybeUninit::<libc::rusage_info_v2>::zeroed();
    // SAFETY: `info` is a writable rusage_info_v2 buffer, which is what flavor
    // RUSAGE_INFO_V2 fills in (the C API takes it as an opaque pointer).
    let rc = unsafe {
        libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, info.as_mut_ptr().cast())
    };
    if rc != 0 {
        return None;
    }
    // SAFETY: proc_pid_rusage succeeded, so the structure has been filled in.
    Some(unsafe { info.assume_init() }.ri_phys_footprint)
}

/// Extracts argv[0] from a KERN_PROCARGS2 buffer: i32 argc, exec path, NUL padding, argv[0].
fn parse_argv0(buf: &[u8]) -> String {
    let Some(rest) = buf.get(size_of::<i32>()..) else {
        return String::new();
    };
    let after_path = match rest.iter().position(|&b| b == 0) {
        Some(i) => &rest[i..],
        None => return String::new(),
    };
    let args = match after_path.iter().position(|&b| b != 0) {
        Some(i) => &after_path[i..],
        None => return String::new(),
    };
    let end = args.iter().position(|&b| b == 0).unwrap_or(args.len());
    String::from_utf8_lossy(&args[..end]).into_owned()
}

fn pid_argv0(pid: i32) -> String {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut len = 0usize;
    // SAFETY: `mib` has 3 elements; a null output buffer asks only for the required size.
    let rc = unsafe {
        libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut len, std::ptr::null_mut(), 0)
    };
    if rc != 0 || len == 0 {
        return String::new();
    }
    let mut buf = vec![0u8; len];
    // SAFETY: `buf` is writable for `len` bytes and `len` points to a valid usize.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return String::new();
    }
    buf.truncate(len.min(buf.len()));
    parse_argv0(&buf)
}

/// Samples a single process; `None` if it is gone or inaccessible.
pub fn sample_pid(pid: i32) -> Option<ProcInfo> {
    if pid <= 0 {
        return None;
    }
    let path = pid_path(pid)?;
    let footprint_bytes = pid_footprint(pid)?;
    Some(ProcInfo { pid, path, argv0: pid_argv0(pid), footprint_bytes })
}

/// Samples every process we are allowed to inspect.
pub fn sample() -> Vec<ProcInfo> {
    list_pids().into_iter().filter_map(sample_pid).collect()
}

#[cfg(test)]
mod tests {
    use super::parse_argv0;

    #[test]
    fn argv0_parses_normal_and_garbage() {
        let mut b = 2i32.to_ne_bytes().to_vec();
        b.extend_from_slice(b"/usr/bin/x\0\0\0claude\0--flag\0");
        assert_eq!(parse_argv0(&b), "claude");
        assert_eq!(parse_argv0(&[]), "");
        assert_eq!(parse_argv0(&[1, 2]), "");
        assert_eq!(parse_argv0(&[0, 0, 0, 0, b'a']), "");
    }
}
