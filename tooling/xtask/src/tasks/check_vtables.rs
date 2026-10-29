//! Enforce `[INV-ARCH-50] Metadata-Derived Vtable Admission` (ADR-0193).
//!
//! libwayland indexes a request vtable by opcode, so the number of slots an
//! `*_interface_impl` struct carries must cover every request the interface can
//! dispatch at the version this server advertises. ADR-0006 enforced that with
//! a hand-typed count per struct; those counts are the very defect class the
//! ADR documents, since a count transcribed from the struct cannot detect that
//! the struct is wrong for the bound version.
//!
//! The admission mechanism now lives in `crates/tessera-wayland/src/ffi.rs`:
//! `declare_vtables!` binds every impl struct to its interface and
//! `checked_resource_create` derives the required slots from libwayland's own
//! metadata at bind time. This gate keeps that mechanism airtight by failing
//! closed when either half is bypassed:
//!
//! 1. Every `*_interface_impl` struct must be declared in `declare_vtables!`
//!    (and every declaration must name a real struct) — no vtable may escape
//!    the admission audit.
//! 2. No call site may reach the raw `ffi::wl_resource_create`; every resource
//!    must be created through `checked_resource_create`, or the admission
//!    check is skipped for that object.

use anyhow::{Context, Result, bail};
use clap::Parser;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
pub struct CheckVtablesArgs {}

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

/// Struct names of the form `pub struct x_interface_impl`.
fn impl_structs(source: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for line in source.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix("pub struct ") else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.ends_with("_interface_impl") {
            found.insert(name);
        }
    }
    found
}

/// `interface => impl` pairs declared inside `declare_vtables! { … }`.
fn declared_vtables(source: &str) -> Result<BTreeSet<String>> {
    let start = source
        .find("declare_vtables! {")
        .context("declare_vtables! block not found in ffi.rs")?;
    let body = &source[start..];
    let end = body
        .find("\n}")
        .context("unterminated declare_vtables! block")?;
    let mut impls = BTreeSet::new();
    for line in body[..end].lines() {
        let Some((_, impl_name)) = line.split_once("=>") else {
            continue;
        };
        let impl_name = impl_name.trim().trim_end_matches(',').trim();
        if !impl_name.is_empty() {
            impls.insert(impl_name.to_owned());
        }
    }
    Ok(impls)
}

pub fn run_check_vtables(_args: CheckVtablesArgs) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let src = root.join("crates/tessera-wayland/src");
    let ffi_path = src.join("ffi.rs");
    let ffi = fs::read_to_string(&ffi_path)?;

    let declared = declared_vtables(&ffi)?;
    let structs = impl_structs(&ffi);

    let undeclared: Vec<_> = structs.difference(&declared).cloned().collect();
    let phantom: Vec<_> = declared.difference(&structs).cloned().collect();
    if !undeclared.is_empty() || !phantom.is_empty() {
        for name in &undeclared {
            eprintln!("INV-ARCH-50 violation: `{name}` is not declared in declare_vtables!");
        }
        for name in &phantom {
            eprintln!(
                "INV-ARCH-50 violation: declare_vtables! names `{name}`, which is not a struct"
            );
        }
        bail!(
            "{} undeclared, {} phantom vtable(s); every impl struct must be admitted",
            undeclared.len(),
            phantom.len()
        );
    }

    let mut files = Vec::new();
    rust_files(&src, &mut files)?;
    files.sort();

    let mut bypasses = Vec::new();
    for file in &files {
        if file.file_name().is_some_and(|n| n == "ffi.rs") {
            continue;
        }
        let relative = file
            .strip_prefix(&src)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        for (index, line) in fs::read_to_string(file)?.lines().enumerate() {
            if line.contains("ffi::wl_resource_create(") {
                bypasses.push(format!(
                    "crates/tessera-wayland/src/{relative}:{}",
                    index + 1
                ));
            }
        }
    }
    if !bypasses.is_empty() {
        for bypass in &bypasses {
            eprintln!(
                "INV-ARCH-50 violation: raw resource creation bypasses vtable admission: {bypass}"
            );
        }
        bail!(
            "{} raw ffi::wl_resource_create call(s); use ffi::checked_resource_create",
            bypasses.len()
        );
    }

    println!(
        "Vtable admission: OK ({} impl structs declared, no raw resource creation)",
        declared.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_impl_structs_and_declarations() {
        let source = "\
pub struct wl_compositor_interface_impl {
    pub create_surface: fn(),
}
declare_vtables! {
    wl_compositor_interface => wl_compositor_interface_impl,
}
";
        assert!(impl_structs(source).contains("wl_compositor_interface_impl"));
        let declared = declared_vtables(source).unwrap();
        assert!(declared.contains("wl_compositor_interface_impl"));
    }
}
