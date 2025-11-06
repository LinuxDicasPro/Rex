use fs_extra::dir::CopyOptions;
use rldd_rex::{ElfType, rldd_rex};
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Write};
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

fn create_temp_dir(base_name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let path = std::env::temp_dir().join(format!("{}_bundle", base_name));
    if path.exists() {
        fs::remove_dir_all(&path)?;
    }
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn create_payload(path: &Path, target: &str, l: i32) -> Result<PathBuf, Box<dyn Error>> {
    let tmp_dir = std::env::temp_dir().join(format!("{}_bundle_tmp", target));
    if tmp_dir.exists() {
        fs::remove_dir_all(&tmp_dir)?;
    }
    fs::create_dir_all(&tmp_dir)?;

    let payload = tmp_dir.join(format!("{target}.tar.zstd"));
    let file_writer = File::create(&payload)?;

    println!("\n[Packaging] Starting TAR archiving and ZSTD compression (level {l}) ...",);

    let mut enc = Encoder::new(file_writer, l)?;
    enc.long_distance_matching(true)?;
    let encoder: Box<dyn Write> = Box::new(enc.auto_finish());

    let bundle_dir_name = format!("{}_bundle", target);
    let mut builder = tar::Builder::new(encoder);
    builder.append_dir_all(&bundle_dir_name, path)?;

    Ok(payload)
}

fn copy_bin_and_deps(
    file_path: &Path,
    bin_dir: &Path,
    libs_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    let dest_path = bin_dir.join(file_path.file_name().unwrap());
    fs::copy(file_path, &dest_path)?;
    println!("[Staging] Copied binary: {}", dest_path.display());

    let deps = rldd_rex(file_path)?;
    let mut unique_deps = Vec::new();

    if deps.elf_type == ElfType::Static {
        println!("binario estatico tambem");
        return Ok(());
    } else {
        let paths: Vec<PathBuf> = deps
            .deps
            .iter()
            .map(|(_, path)| PathBuf::from(path))
            .filter(|p| p.exists())
            .collect();
        unique_deps.extend(paths);
    }

    println!(
        "[Staging] Copying {} dependencies for {}",
        unique_deps.len(),
        file_path.display()
    );
    let dep_refs: Vec<&Path> = unique_deps.iter().map(|p| p.as_path()).collect();
    fs_extra::copy_items(&dep_refs, &libs_dir, &CopyOptions::new())?;

    Ok(())
}

pub fn generate_bundle(args: BundleArgs) -> Result<(), Box<dyn Error>> {
    let target_binary = &args.target_binary;
    let target_name = target_binary.file_name().unwrap().to_str().unwrap();
    let cwd = std::env::current_dir()?;
    let deps = rldd_rex(target_binary)?;
    let mut final_libs = Vec::new();

    if deps.elf_type == ElfType::Invalid {
        return Err("nao é binario elf".into());
    }

    if deps.elf_type == ElfType::Static {
        println!("binario estatico");
        return Ok(());
    } else {
        let paths: Vec<PathBuf> = deps
            .deps
            .iter()
            .map(|(_, path)| PathBuf::from(path))
            .filter(|p| p.exists())
            .collect();
        final_libs.extend(paths);
    }

    for lib in &args.extra_libs {
        if lib.exists() && !final_libs.contains(lib) {
            final_libs.push(lib.clone());
        }
    }

    let staging_dir = create_temp_dir(&target_name)?;
    let root_path = staging_dir.as_path();
    let bin_dir = root_path.join("bins");
    let libs_dir = root_path.join("libs");

    fs::create_dir_all(&bin_dir)?;
    fs::create_dir_all(&libs_dir)?;

    println!("[Staging] Copying target binary...");
    fs::copy(target_binary, root_path.join(target_name))?;

    if !args.extra_bins.is_empty() {
        println!(
            "[Staging] Processing {} extra binaries/folders...",
            args.extra_bins.len()
        );
        for entry in &args.extra_bins {
            if entry.is_dir() {
                for file in fs::read_dir(entry)? {
                    let file_path = file?.path();
                    if file_path.is_file() {
                        copy_bin_and_deps(&file_path, &bin_dir, &libs_dir)?;
                    }
                }
            } else if entry.is_file() {
                copy_bin_and_deps(entry, &bin_dir, &libs_dir)?;
            }
        }
    }

    println!(
        "[Staging] Copying {} dynamic libraries...",
        final_libs.len()
    );
    if !final_libs.is_empty() {
        let options = CopyOptions::new();
        let lib_paths: Vec<&Path> = final_libs.iter().map(|p| p.as_path()).collect();
        fs_extra::copy_items(&lib_paths, &libs_dir, &options)?;
    }

    let extra_paths: Vec<_> = args
        .additional_files
        .iter()
        .filter_map(|e| {
            println!("[Staging] Copying additional files...");
            let p = cwd.join(e);
            if p.exists() { Some(p) } else { None }
        })
        .collect();

    if !extra_paths.is_empty() {
        let options = CopyOptions::new();
        let entry_refs: Vec<&Path> = extra_paths.iter().map(|p| p.as_path()).collect();
        fs_extra::copy_items(&entry_refs, root_path, &options)?;
    }

    let compress_payload_path = create_payload(root_path, &target_name, args.compression_level)?;
    let payload_size = compress_payload_path.metadata()?.len();
    let out = format!("{}.Rex", args.target_binary.file_name().unwrap().display());

    println!("\n[Output] Creating final bundle file: {}", out);

    let exec = std::env::current_exe()?;
    fs::copy(&exec, &out)?;

    use std::fs::Permissions;

    let perms = Permissions::from_mode(0o755);
    fs::set_permissions(&out, perms)?;

    let mut final_file = fs::OpenOptions::new().append(true).open(&out)?;
    let mut payload_file = File::open(&compress_payload_path)?;

    io::copy(&mut payload_file, &mut final_file)?;

    let metadata = BundleMetadata {
        payload_size,
        target_bin_name_len: target_name.len() as u32,
    };

    final_file.write_all(target_name.as_bytes())?;

    let metadata_bytes = unsafe {
        std::slice::from_raw_parts(
            &metadata as *const BundleMetadata as *const u8,
            size_of::<BundleMetadata>(),
        )
    };
    final_file.write_all(metadata_bytes)?;
    final_file.write_all(&MAGIC_MARKER)?;

    fs::remove_file(&compress_payload_path).ok();
    fs::remove_dir_all(&staging_dir).ok();

    println!(
        "\n[Generator Success]\n  Payload Size: {payload_size} bytes\
    \n  Metadata Size: {} bytes\n  Compressed Bundle created at: {}",
        size_of::<BundleMetadata>() + target_name.len() + MAGIC_MARKER.len(),
        out
    );
    Ok(())
}
