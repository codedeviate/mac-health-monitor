//! Per-process memory sampling and grouping into "potential culprits".

mod macos;

use std::collections::HashMap;

pub use macos::{sample, sample_pid};

#[derive(Debug, Clone, PartialEq)]
pub struct ProcInfo {
    pub pid: i32,
    pub path: String,
    pub argv0: String,
    pub footprint_bytes: u64,
}

/// A group of processes shown as one line; `count > 1` only for non-app groups.
#[derive(Debug, Clone, PartialEq)]
pub struct Culprit {
    pub label: String,
    pub bytes: u64,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GroupKey {
    App(String),
    Vm,
    Process(String),
}

fn basename(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or("")
}

pub fn group_key(path: &str, argv0: &str) -> GroupKey {
    if path.contains("com.apple.Virtualization.VirtualMachine") {
        return GroupKey::Vm;
    }
    if let Some(app) = path
        .split('/')
        .find_map(|c| c.strip_suffix(".app").filter(|n| !n.is_empty()))
    {
        return GroupKey::App(app.to_string());
    }
    let from_argv = if argv0.contains('/') {
        // proc_pidpath is authoritative; argv0 may contain spaces in directory names.
        let from_path = basename(path);
        if from_path.is_empty() { basename(argv0) } else { from_path }
    } else {
        argv0.split_whitespace().next().unwrap_or("")
    };
    let from_argv = from_argv.trim_start_matches('-').trim_end_matches(':');
    let name = if !from_argv.is_empty() { from_argv } else { basename(path) };
    GroupKey::Process(if name.is_empty() { "unknown" } else { name }.to_string())
}

pub fn group(procs: &[ProcInfo], top_n: usize) -> Vec<Culprit> {
    let mut sums: HashMap<GroupKey, (u64, usize)> = HashMap::new();
    for p in procs {
        let e = sums.entry(group_key(&p.path, &p.argv0)).or_insert((0, 0));
        e.0 += p.footprint_bytes;
        e.1 += 1;
    }
    let docker = GroupKey::App("Docker".to_string());
    if let Some((vm_bytes, _)) = sums.remove(&GroupKey::Vm) {
        if let Some(d) = sums.get_mut(&docker) {
            d.0 += vm_bytes;
        } else {
            sums.insert(GroupKey::Vm, (vm_bytes, 1));
        }
    }
    let mut out: Vec<Culprit> = sums
        .into_iter()
        .map(|(key, (bytes, n))| match key {
            GroupKey::App(l) => Culprit { label: l, bytes, count: 1 },
            GroupKey::Vm => Culprit { label: "Virtual machine".into(), bytes, count: 1 },
            GroupKey::Process(l) => Culprit { label: l, bytes, count: n },
        })
        .collect();
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.label.cmp(&b.label)));
    out.truncate(top_n);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(path: &str, argv0: &str, mb: u64) -> ProcInfo {
        ProcInfo { pid: 1, path: path.into(), argv0: argv0.into(), footprint_bytes: mb * 1_000_000 }
    }

    #[test]
    fn key_outermost_app() {
        assert_eq!(group_key("/Applications/Arc.app/Contents/Frameworks/Arc Helper (Renderer).app/Contents/MacOS/Arc Helper (Renderer)", ""), GroupKey::App("Arc".into()));
        assert_eq!(group_key("/Applications/Docker.app/Contents/MacOS/Docker Desktop.app/Contents/MacOS/Docker Desktop", ""), GroupKey::App("Docker".into()));
    }

    #[test]
    fn key_vm() {
        assert_eq!(group_key("/System/Library/Frameworks/Virtualization.framework/Versions/A/XPCServices/com.apple.Virtualization.VirtualMachine.xpc/Contents/MacOS/com.apple.Virtualization.VirtualMachine", ""), GroupKey::Vm);
    }

    #[test]
    fn key_cli_path_with_space() {
        assert_eq!(
            group_key(
                "/Users/x/Library/Application Support/foo/bin/helper",
                "/Users/x/Library/Application Support/foo/bin/helper --flag"
            ),
            GroupKey::Process("helper".into())
        );
    }

    #[test]
    fn key_trims_login_dash_and_colon() {
        assert_eq!(group_key("/bin/zsh", "-zsh"), GroupKey::Process("zsh".into()));
        assert_eq!(
            group_key("/opt/homebrew/sbin/php-fpm", "php-fpm:"),
            GroupKey::Process("php-fpm".into())
        );
    }

    #[test]
    fn key_cli_uses_argv0() {
        assert_eq!(group_key("/Users/x/.local/share/claude/versions/2.1.319", "claude"), GroupKey::Process("claude".into()));
        assert_eq!(group_key("/usr/local/bin/node", "npm exec ccstatusline@latest"), GroupKey::Process("npm".into()));
        assert_eq!(group_key("/opt/homebrew/bin/foo", ""), GroupKey::Process("foo".into()));
        assert_eq!(group_key("", ""), GroupKey::Process("unknown".into()));
    }

    #[test]
    fn group_merges_vm_into_docker_and_sorts() {
        let procs = [
            p("/Applications/Docker.app/Contents/MacOS/com.docker.backend", "", 100),
            p("/System/Library/Frameworks/Virtualization.framework/X/com.apple.Virtualization.VirtualMachine", "", 800),
            p("/a/claude", "claude", 300),
            p("/a/claude", "claude", 200),
            p("/Applications/Arc.app/Contents/MacOS/Arc", "", 400),
        ];
        let g = group(&procs, 8);
        assert_eq!(g[0], Culprit { label: "Docker".into(), bytes: 900_000_000, count: 1 });
        assert_eq!(g[1], Culprit { label: "claude".into(), bytes: 500_000_000, count: 2 });
        assert_eq!(g[2].label, "Arc");
    }

    #[test]
    fn group_vm_without_docker() {
        let g = group(&[p("/S/com.apple.Virtualization.VirtualMachine", "", 5)], 8);
        assert_eq!(g[0].label, "Virtual machine");
    }

    #[test]
    fn group_truncates() {
        let procs: Vec<_> = (0..20).map(|i| p(&format!("/bin/p{i}"), "", i)).collect();
        assert_eq!(group(&procs, 8).len(), 8);
    }

    #[test]
    fn sample_pid_nonexistent_is_none() {
        assert_eq!(sample_pid(i32::MAX), None);
    }

    #[test]
    fn sample_includes_self() {
        let me = std::process::id() as i32;
        assert!(sample().iter().any(|p| p.pid == me && p.footprint_bytes > 0));
    }
}
