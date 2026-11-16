//! `[INV-ARCH-96]`: a launched client must not inherit the compositor's
//! dedicated-CPU pin.
//!
//! A child inherits the **forking thread's** affinity mask. The compositor's
//! frame thread owns one CPU, so a client spawned from it would be born
//! single-CPU and confined to the core the compositor reserved for
//! presentation — inverting the purpose of the pin. Every `pre_exec` seam
//! widens the mask back to the plan's full available set before `exec`.
//!
//! This drives the real `launch()` path. The probe is a compiled binary rather
//! than a shell one-liner on purpose: `launch` passes `exec` through
//! `expand_exec`, which re-tokenizes and re-quotes, so a nested
//! `sh -c '... > file'` is destroyed by the quoting pass and the probe silently
//! never runs. A single argument-free binary also happens to be the shape a
//! real desktop entry has.

use std::path::{Path, PathBuf};

use tessera_launch_services::launcher::{LaunchOpts, LaunchSource, launch};
use tessera_sched::SchedPlan;

struct Probe {
    exec: String,
}

impl LaunchSource for Probe {
    fn id(&self) -> &str {
        "affinity-probe"
    }
    fn name(&self) -> &str {
        "affinity probe"
    }
    fn exec(&self) -> Option<&str> {
        Some(&self.exec)
    }
    fn icon(&self) -> Option<&str> {
        None
    }
    fn terminal(&self) -> bool {
        false
    }
    fn working_dir(&self) -> Option<&Path> {
        None
    }
}

const PROBE_SOURCE: &str = r#"
use std::io::Write;
fn main() {
    let mut status = String::new();
    if std::fs::File::open("/proc/self/status")
        .and_then(|mut f| { use std::io::Read; f.read_to_string(&mut status) })
        .is_err()
    {
        std::process::exit(1);
    }
    let mask = status
        .lines()
        .find_map(|line| line.strip_prefix("Cpus_allowed_list:"))
        .map(str::trim)
        .unwrap_or("");
    let out = std::env::args().nth(1).expect("probe needs an output path");
    let mut file = std::fs::File::create(out).expect("probe cannot create its output");
    file.write_all(mask.as_bytes()).expect("probe cannot write");
}
"#;

/// Compile the probe once per test invocation and return its path.
fn build_probe(dir: &Path) -> PathBuf {
    let source = dir.join("probe.rs");
    std::fs::write(&source, PROBE_SOURCE).expect("write probe source");
    let binary = dir.join("affinity-probe");
    let status = std::process::Command::new("rustc")
        .arg("-O")
        .arg("-o")
        .arg(&binary)
        .arg(&source)
        .status()
        .expect("rustc must be available to build the probe");
    assert!(status.success(), "probe compilation failed");
    binary
}

fn read_mask(path: &Path) -> String {
    // The launch is detached (`setsid --fork`), so poll for the write instead
    // of assuming the file already exists.
    for _ in 0..300 {
        if let Ok(mask) = std::fs::read_to_string(path)
            && !mask.trim().is_empty()
        {
            return mask.trim().to_owned();
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!(
        "probe never reported its affinity mask at {}",
        path.display()
    );
}

/// Parse a kernel CPU list (`"0-15"`, `"0,2-3"`, `"7"`) into its members, so
/// the assertions describe the set rather than its formatting.
fn parse_cpu_list(list: &str) -> Vec<usize> {
    let mut cpus = Vec::new();
    for part in list.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.split_once('-') {
            Some((lo, hi)) => {
                let (Ok(lo), Ok(hi)) = (lo.trim().parse::<usize>(), hi.trim().parse::<usize>())
                else {
                    continue;
                };
                cpus.extend(lo..=hi);
            }
            None => {
                if let Ok(cpu) = part.parse::<usize>() {
                    cpus.push(cpu);
                }
            }
        }
    }
    cpus.sort_unstable();
    cpus.dedup();
    cpus
}

#[test]
fn test_inv_arch_96_launched_client_does_not_inherit_the_frame_pin() {
    let plan = SchedPlan::detect(true, 10, -10);
    let available = plan.available_cpus();
    if available.len() <= 1 {
        // A single-CPU environment cannot express the property.
        return;
    }
    // Install the plan the launcher's `pre_exec` hook reads. If another test in
    // this binary armed one first the set is a no-op, so re-read what is
    // actually installed rather than assuming ours won.
    tessera_sched::arm_global_plan(plan);

    let dir = tempfile::tempdir().expect("tempdir");
    let probe = build_probe(dir.path());
    let output = dir.path().join("mask.txt");
    let pinned_cpu = available[0];
    let size = std::mem::size_of::<libc::cpu_set_t>();

    // Pin the calling thread exactly like the frame loop, so a child would
    // inherit a one-CPU mask if the release hook were missing.
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    unsafe { libc::CPU_ZERO(&mut set) };
    unsafe { libc::CPU_SET(pinned_cpu, &mut set) };
    assert_eq!(
        unsafe { libc::sched_setaffinity(0, size, &set) },
        0,
        "pin the calling thread"
    );

    let source = Probe {
        exec: format!("{} {}", probe.display(), output.display()),
    };
    let result = launch(&source, &LaunchOpts::default());
    // Restore the caller before asserting, so a failure does not leave the
    // whole test binary on one CPU.
    let mut all: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    unsafe { libc::CPU_ZERO(&mut all) };
    for cpu in &available {
        if *cpu < 1024 {
            unsafe { libc::CPU_SET(*cpu, &mut all) };
        }
    }
    // SAFETY: `all` is a fully initialised cpu_set_t covering the plan's set.
    unsafe { libc::sched_setaffinity(0, size, &all) };
    result.expect("launch the probe");

    let observed = parse_cpu_list(&read_mask(&output));
    assert!(
        observed.len() > 1,
        "client inherited a single-CPU mask ({observed:?}); the pre_exec \
         affinity release did not run"
    );
    assert!(
        observed.contains(&pinned_cpu),
        "the released mask {observed:?} should still include the pinned CPU {pinned_cpu}"
    );
    // The release must widen to the plan's whole set, not merely add one CPU.
    for cpu in &available {
        assert!(
            observed.contains(cpu),
            "released mask {observed:?} is missing plan CPU {cpu}"
        );
    }
}
