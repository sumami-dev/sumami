//! What this machine is.
//!
//! "How much traffic can this box take" is a question about **the box**, and
//! the answer is only reproducible if the box says what it is. So the
//! heartbeat reports it, and a load test — wherever it is run from — can
//! stamp its report with the machine that was measured, not with the machine
//! that measured it.
//!
//! Read with the standard library only: no dependency and no syscall (the
//! crate forbids `unsafe`). On Linux the facts come from `/proc`; elsewhere
//! the model and the memory come from `sysctl`, which is a subprocess and not
//! a syscall — the same trade the memory reader makes.
//!
//! The numbers are the ones a reader would write down by hand: the CPU's
//! model, how many CPUs the process may use, and the machine's memory.

use serde::Serialize;

/// What this machine is.
#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    /// The CPU's model name, when the platform can say it.
    pub cpu: Option<String>,
    /// How many CPUs this process may use.
    pub cores: Option<u32>,
    /// The machine's memory, in bytes.
    pub ram_bytes: Option<u64>,
    /// The operating system.
    pub os: Option<String>,
    /// The architecture.
    pub arch: Option<String>,
}

/// Read the machine's facts.
pub fn machine() -> Machine {
    Machine {
        cpu: cpu_model(),
        cores: std::thread::available_parallelism().ok().map(|n| n.get() as u32),
        ram_bytes: ram_bytes(),
        os: Some(std::env::consts::OS.to_string()),
        arch: Some(std::env::consts::ARCH.to_string()),
    }
}

/// The CPU's model name.
#[cfg(target_os = "linux")]
fn cpu_model() -> Option<String> {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    for line in cpuinfo.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        // x86 says "model name", ARM says "Model", and some say "Hardware".
        let key = key.trim();
        if key == "model name" || key == "Model" || key == "Hardware" {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// The CPU's model name.
#[cfg(not(target_os = "linux"))]
fn cpu_model() -> Option<String> {
    command_line("sysctl", &["-n", "machdep.cpu.brand_string"])
}

/// The machine's memory.
#[cfg(target_os = "linux")]
fn ram_bytes() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in meminfo.lines() {
        let Some(rest) = line.strip_prefix("MemTotal:") else {
            continue;
        };
        // The format is "16316532 kB".
        let kilobytes: u64 = rest.split_whitespace().next()?.parse().ok()?;
        return Some(kilobytes * 1024);
    }
    None
}

/// The machine's memory.
#[cfg(not(target_os = "linux"))]
fn ram_bytes() -> Option<u64> {
    command_line("sysctl", &["-n", "hw.memsize"])?.parse().ok()
}

/// The first line of a command's output, if the command worked.
///
/// Only the platforms without a `/proc` need it — on Linux the facts come
/// from the files themselves.
#[cfg(not(target_os = "linux"))]
fn command_line(program: &str, arguments: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(arguments)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?.trim().to_string();
    if line.is_empty() {
        None
    } else {
        Some(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machine_knows_itself() {
        let facts = machine();
        assert!(facts.os.is_some(), "the operating system is always known");
        assert!(facts.arch.is_some(), "the architecture is always known");
        assert!(facts.cores.is_some(), "the CPUs are always known");
        assert!(facts.cores.unwrap() >= 1);
    }

    #[test]
    fn the_memory_is_plausible() {
        if let Some(bytes) = machine().ram_bytes {
            assert!(bytes > 0, "a machine has memory");
            assert!(
                bytes < 1024 * 1024 * 1024 * 1024,
                "a terabyte of memory is not plausible"
            );
        }
    }
}
