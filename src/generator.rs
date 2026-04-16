use recursive_copy::{CopyOptions, copy_recursive};
use rldd_minimal::{ElfType, rldd_minimal};
use std::env;
use std::error::Error;
use std::fs::{self, File, Permissions};
use std::io::{self, Write};
use std::mem::size_of;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use zstd::stream::write::Encoder;

const MAGIC_MARKER: [u8; 10] = *b"REX_BUNDLE";

#[repr(C, packed)]
struct BundleMetadata {
    payload_size: u64,
    target_bin_name_len: u32,
}

#[derive(Debug)]
pub struct BundleArgs {
    pub target_binary: PathBuf,
    pub compression_level: i32,
    pub extra_libs: Vec<PathBuf>,
    pub additional_files: Vec<String>,
    pub extra_bins: Vec<PathBuf>,
}

fn recreate_dir(path: &Path) -> io::Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    fs::create_dir_all(path)
}

fn collect_deps(path: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let deps = rldd_minimal(path)?;
    if matches!(deps.elf_type, ElfType::Invalid | ElfType::Static) {
        return Ok(vec![]);
    }
    Ok(deps
        .deps
        .iter()
        .map(|(_, p)| PathBuf::from(p))
        .filter(|p| p.exists())
        .collect())
}

fn create_payload(path: &Path, target: &str, level: i32) -> Result<PathBuf, Box<dyn Error>> {
    let tmp = env::temp_dir().join(format!("{target}_bundle_tmp"));
    recreate_dir(&tmp)?;

    let pay = tmp.join(format!("{target}.tar.zstd"));
    println!("[Packaging] Creating TAR+ZSTD (level {level})");

    let file = File::create(&pay)?;
    let mut enc = Encoder::new(file, level)?;
    enc.long_distance_matching(true)?;
    let mut encoder = enc.auto_finish();

    let mut builder = tar_minimal::Builder::new(&mut encoder);
    builder.append_dir_all(&format!("{target}_bundle"), path)?;
    Ok(pay)
}

fn copy_bin_and_deps(file: &Path, bin_dir: &Path, libs_dir: &Path) -> Result<(), Box<dyn Error>> {
    let dest = bin_dir.join(file.file_name().unwrap_or_default());
    fs::copy(file, &dest)?;
    println!("[Staging] Copied binary: {}", dest.display());

    let mut coptions = CopyOptions::default();
    coptions.content_only = true;
    coptions.follow_symlinks = true;
    for dep in collect_deps(file)? {
        copy_recursive(&dep, libs_dir, &coptions).ok();
    }
    Ok(())
}

pub fn generate_bundle(args: BundleArgs) -> Result<(), Box<dyn Error>> {
    let target = &args.target_binary;
    let deps = rldd_minimal(target)?;

    if matches!(deps.elf_type, ElfType::Invalid | ElfType::Static) {
        return Err("Not Shared ELF binary".into());
    }

    let target_name = target.file_name().unwrap().to_str().ok_or("Invalid UTF-8")?;
    let staging_dir = env::temp_dir().join(format!("{target_name}_bundle"));

    recreate_dir(&staging_dir)?;
    let bin_dir = staging_dir.join("bins");
    let libs_dir = staging_dir.join("libs");
    fs::create_dir_all(&bin_dir)?;
    fs::create_dir_all(&libs_dir)?;

    let cwd = env::current_dir()?;
    let mut coptions = CopyOptions::default();

    let libs: Vec<PathBuf> = deps
        .deps
        .iter()
        .map(|(_, p)| PathBuf::from(p))
        .filter(|p| p.exists())
        .collect();

    println!("[Staging] Copying target binary: {}", target.display());
    fs::copy(target, staging_dir.join(target_name))?;

    if !args.extra_bins.is_empty() {
        println!(
            "[Staging] Processing {} extra binaries...",
            args.extra_bins.len()
        );
        for entry in &args.extra_bins {
            if entry.is_dir() {
                for f in fs::read_dir(entry)? {
                    let path = f?.path();
                    if path.is_file() {
                        copy_bin_and_deps(&path, &bin_dir, &libs_dir)?;
                    }
                }
            } else {
                copy_bin_and_deps(entry, &bin_dir, &libs_dir)?;
            }
        }
    }

    println!("[Staging] Copying {} shared libs...", libs.len());
    for lib in &libs {
        coptions.content_only = true;
        coptions.follow_symlinks = true;
        copy_recursive(lib, &libs_dir, &coptions).ok();
    }

    if !args.extra_libs.is_empty() {
        println!("[Staging] Copying {} extra libs...", args.extra_libs.len());
        for entry in &args.extra_libs {
            coptions.follow_symlinks = false;
            if entry.is_dir() {
                for f in fs::read_dir(entry)? {
                    let p = f?.path();
                    if p.is_file() {
                        copy_recursive(&p, &libs_dir, &coptions).ok();
                    }
                }
            } else {
                copy_recursive(entry, &libs_dir, &coptions).ok();
            }
        }
    }

    for extra in &args.additional_files {
        coptions.content_only = false;
        let path = cwd.join(extra);
        if path.is_dir() {
            let parent_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            let dest = staging_dir.join(parent_name);
            recreate_dir(&dest)?;
            println!("[Staging] Copying directory: {}", path.display());
            copy_recursive(&path, &dest, &coptions).ok();
        } else {
            println!("[Staging] Copying file: {}", path.display());
            copy_recursive(&path, &staging_dir, &coptions).ok();
        }
    }

    let payload = create_payload(&staging_dir, target_name, args.compression_level)?;
    let payload_size = payload.metadata()?.len();
    let output = format!("{target_name}.Rex",);

    println!("[Output] Creating bundle: {output}");
    fs::copy(env::current_exe()?, &output)?;
    fs::set_permissions(&output, Permissions::from_mode(0o755))?;

    let mut final_file = fs::OpenOptions::new().append(true).open(&output)?;
    io::copy(&mut File::open(&payload)?, &mut final_file)?;

    let metadata = BundleMetadata {
        payload_size,
        target_bin_name_len: target_name.len() as u32,
    };
    final_file.write_all(target_name.as_bytes())?;
    let metadata_bytes = unsafe {
        std::slice::from_raw_parts(
            &metadata as *const _ as *const u8,
            size_of::<BundleMetadata>(),
        )
    };
    final_file.write_all(metadata_bytes)?;
    final_file.write_all(&MAGIC_MARKER)?;

    fs::remove_file(&payload).ok();
    fs::remove_dir_all(&staging_dir).ok();

    println!(
        "\n[Generator Success]\n  Payload Size: {payload_size} bytes\n  Metadata Size: {} bytes",
        size_of::<BundleMetadata>() + target_name.len() + MAGIC_MARKER.len()
    );
    Ok(())
}
