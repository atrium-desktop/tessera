//! Enforce `[INV-ARCH-49] Zero-Math Invariant in Chrome` (ADR-0174).
//!
//! `tessera-shell` owns *policy*; Optics `transit` owns motion *mechanism*
//! (ADR-0139, ADR-0172). All continuous scalar travel must therefore funnel
//! through the single authorized seam, `crates/tessera-shell/src/widgets/motion.rs`,
//! which re-exports the `transit` primitives (`Spring`, `approach`, `decay`,
//! `blend`, easing curves, …).
//!
//! Nothing else in `tessera-shell` may hand-roll the exponential follow
//! (`f32::exp`) or another local integrator: doing so re-introduces the exact
//! divergence and tuning-drift bugs `transit` exists to prevent, and silently
//! bypasses the reduced-motion and `dt`-clamping guarantees the library proves.
//!
//! This check fails closed: any `.exp(` outside the seam is a violation.

use anyhow::Result;
use clap::Parser;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
pub struct CheckMotionArgs {}

/// The one module allowed to touch motion mechanism directly — the re-export
/// seam over `transit`.
const SEAM: &str = "widgets/motion.rs";

/// A source line that hand-rolls motion math instead of delegating to `transit`.
fn is_violation(line: &str) -> bool {
    // `1.0 - (-rate * dt).exp()` and friends: the exponential follow, inlined.
    line.contains(".exp(") || line.contains(".expf(")
}

/// Scan one file's source for violation line numbers (1-based).
fn violations_in(source: &str) -> Vec<usize> {
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| is_violation(line))
        .map(|(index, _)| index + 1)
        .collect()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            rust_files(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

pub fn run_check_motion(_args: CheckMotionArgs) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let src = root.join("crates/tessera-shell/src");
    let mut files = Vec::new();
    rust_files(&src, &mut files)?;
    files.sort();

    let mut violations = Vec::new();
    for file in &files {
        let relative = file
            .strip_prefix(&src)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        if relative == SEAM {
            continue;
        }
        for line in violations_in(&fs::read_to_string(file)?) {
            violations.push(format!("crates/tessera-shell/src/{relative}:{line}"));
        }
    }

    if !violations.is_empty() {
        for violation in &violations {
            eprintln!("INV-ARCH-49 violation (hand-rolled motion math): {violation}");
        }
        anyhow::bail!(
            "{} hand-rolled `f32::exp` site(s) in tessera-shell; delegate to \
             `crate::widgets::motion` (`approach`, `blend`, `decay`, `Spring`) instead",
            violations.len()
        );
    }

    println!(
        "Chrome zero-math invariant: OK ({} source files, only {SEAM} touches motion mechanism)",
        files.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_the_exponential_follow_idiom() {
        assert_eq!(violations_in("let b = 1.0 - (-r * dt).exp();"), vec![1]);
        assert_eq!(violations_in("self.x += 0.0;"), Vec::<usize>::new());
        assert_eq!(
            violations_in("ok\nbad = 1.0 - (-r * dt).exp();\n"),
            vec![2]
        );
    }

    #[test]
    fn delegated_primitive_calls_are_not_violations() {
        assert_eq!(
            violations_in("self.v = crate::widgets::motion::approach(self.v, t, r, dt);"),
            Vec::<usize>::new()
        );
    }
}
