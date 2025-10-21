use fs_extra::dir::CopyOptions;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Write};
use std::mem::forget;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::{tempdir, NamedTempFile};
use xz2::write::XzEncoder;
use zstd::stream::write::Encoder;

const MAGIC_MARKER: [u8; 10] = *b"REX_BUNDLE";

#[repr(C, packed)]
struct BundleMetadata {
    payload_size: u64,
    compression_type: u32,
    target_bin_name_len: u32,
}

#[derive(Debug)]
pub struct BundleArgs {
    pub target_binary: PathBuf,
    pub output: PathBuf,
    pub compression: String,
    pub compression_level: i32,
    pub extra_libs: Vec<PathBuf>,
    pub additional_files: Vec<String>,
    pub extra_bins: Vec<PathBuf>,
}

fn find_system_loader() -> Option<PathBuf> {
    let possible_paths = [
        "/lib64/ld-linux-x86-64.so.2",
        "/usr/lib64/ld-linux-x86-64.so.2",
        "/lib/ld-linux-x86-64.so.2",
        "/usr/lib/ld-linux-x86-64.so.2",
        "/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2",
        "/lib/ld-musl-x86_64.so.1",
        "/usr/lib/ld-musl-x86_64.so.1"
    ];

    for path in possible_paths.iter() {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn create_compressed_payload(staging_root: &Path, target_name: &str, compression: &str, level: i32) -> Result<PathBuf, Box<dyn Error>> {
    println!("\n[Packaging] Starting TAR archiving and {} compression (level {})...", compression.to_uppercase(), level);

    let payload_file = NamedTempFile::new()?;
    let payload_path = payload_file.path().to_path_buf();
    let file_writer = File::create(&payload_path)?;
    forget(payload_file);

    let encoder: Box<dyn Write> = match compression.to_lowercase().as_str() {
        "zstd" => {
            let mut enc = Encoder::new(file_writer, level)?;
            enc.long_distance_matching(true)?;
            Box::new(enc.auto_finish())
        }
        "xz" => {
            let xz_enc = XzEncoder::new(file_writer, level as u32);
            Box::new(xz_enc)
        }
        other => return Err(format!("Unknown compression format: {}", other).into()),
    };

    let bundle_dir_name = format!("{}_bundle", target_name);
    let mut tar_builder = tar::Builder::new(encoder);
    tar_builder.append_dir_all(&bundle_dir_name, staging_root)?;
    drop(tar_builder);

    Ok(payload_path)
}

fn get_dynamic_dependencies(binary_path: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    println!("[Generator] Attempting to detect dynamic dependencies (ldd)...");

    if !binary_path.exists() {
        return Err(format!("Binary not found at: {}", binary_path.display()).into());
    }

    let output = Command::new("ldd").arg(binary_path).output();

    let output = match output {
        Ok(out) => out,
        Err(e) => {
            eprintln!("[Generator Warning] Failed to run 'ldd': {}", e);
            return Ok(Vec::new());
        }
    };

    if !output.status.success() {
        eprintln!("[Generator Warning] 'ldd' exited with code {}", output.status);
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut dependencies = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 && parts.contains(&"=>") {
            if let Some(path_str) = parts.iter().skip_while(|&p| *p != "=>").nth(1) {
                let path = PathBuf::from(path_str.trim());
                if path.exists() && !path_str.contains("ld-linux") && !path_str.contains("ld-musl") {
                    dependencies.push(path);
                }
            }
        }
    }

    println!("[Generator] Successfully detected: {} dependencies", dependencies.len());
    Ok(dependencies)
}

fn copy_bin_and_deps(file_path: &Path, bin_dir: &Path, libs_dir: &Path) -> Result<(), Box<dyn Error>> {
    let dest_path = bin_dir.join(file_path.file_name().unwrap());
    fs::copy(file_path, &dest_path)?;
    println!("[Staging] Copied binary: {}", dest_path.display());

    if let Ok(deps) = get_dynamic_dependencies(file_path) {
        let mut unique_deps = Vec::new();
        for dep in deps {
            let dest_lib = libs_dir.join(dep.file_name().unwrap());
            if !dest_lib.exists() {
                unique_deps.push(dep);
            }
        }

        if !unique_deps.is_empty() {
            println!("[Staging] Copying {} dependencies for {}", unique_deps.len(), file_path.display());
            let options = CopyOptions::new();
            let dep_refs: Vec<&Path> = unique_deps.iter().map(|p| p.as_path()).collect();
            fs_extra::copy_items(&dep_refs, &libs_dir, &options)?;
        }
    }

    Ok(())
}

pub fn generate_bundle(args: BundleArgs) ->  Result<(), Box<dyn Error>> {
    let target_binary = &args.target_binary;
    let cwd = std::env::current_dir()?;

    let mut final_libs = Vec::new();
    match get_dynamic_dependencies(target_binary) {
        Ok(detected_libs) => final_libs.extend(detected_libs),
        Err(e) => eprintln!("[Generator Warning] Automatic detection failed: {}", e),
    }

    for lib in &args.extra_libs {
        if lib.exists() && !final_libs.contains(lib) {
            final_libs.push(lib.clone());
        }
    }

    println!("\n[Staging] Creating temporary staging directory...");

    let staging_dir = tempdir()?;
    let root_path = staging_dir.path();
    let bin_dir = root_path.join("bins");
    let libs_dir = root_path.join("libs");

    fs::create_dir_all(&bin_dir)?;
    fs::create_dir_all(&libs_dir)?;

    if let Some(loader_path) = find_system_loader() {
        let dest = libs_dir.join(loader_path.file_name().unwrap());
        fs::copy(&loader_path, &dest)?;
        println!("[Staging] Copied loader: {}", dest.display());
    } else {
        eprintln!("[Warning] No system loader found! Bundle may fail.");
    }

    println!("[Staging] Copying target binary...");
    let target_file_name = target_binary.file_name().unwrap();
    fs::copy(target_binary, root_path.join(target_file_name))?;

    if !args.extra_bins.is_empty() {
        println!("[Staging] Processing {} extra binaries/folders...", args.extra_bins.len());
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

    println!("[Staging] Copying {} dynamic libraries...", final_libs.len());
    if !final_libs.is_empty() {
        let options = CopyOptions::new();
        let lib_paths: Vec<&Path> = final_libs.iter().map(|p| p.as_path()).collect();
        fs_extra::copy_items(&lib_paths, &libs_dir, &options)?;
    }

    println!("[Staging] Copying additional files...");
    let mut extra_paths = Vec::new();
    for entry in &args.additional_files {
        let entry_path = cwd.join(entry);
        if entry_path.exists() {
            extra_paths.push(entry_path);
        } else {
            eprintln!("[Warning] Additional path not found: {}", entry);
        }
    }

    if !extra_paths.is_empty() {
        let options = CopyOptions::new();
        let entry_refs: Vec<&Path> = extra_paths.iter().map(|p| p.as_path()).collect();
        fs_extra::copy_items(&entry_refs, root_path, &options)?;
    }

    let target_name = target_file_name.to_string_lossy().to_string();
    let compressed_payload_path = create_compressed_payload(root_path, &target_name, &args.compression, args.compression_level)?;
    let payload_size = compressed_payload_path.metadata()?.len();

    println!("\n[Output] Creating final bundle file: {}", args.output.display());

    let current_exe = std::env::current_exe()?;
    
    fs::copy(&current_exe, &args.output)?;

    #[cfg(unix)]
    {
        use std::fs::Permissions;
        let perms = Permissions::from_mode(0o755);
        fs::set_permissions(&args.output, perms)?;
    }

    let mut final_file = fs::OpenOptions::new().append(true).open(&args.output)?;
    let mut payload_file = File::open(&compressed_payload_path)?;
    
    io::copy(&mut payload_file, &mut final_file)?;

    let compression_id = match args.compression.to_lowercase().as_str() {
        "zstd" => 0,
        "xz" => 1,
        _ => 99,
    };

    let target_bin_name = target_file_name.to_string_lossy();

    let metadata = BundleMetadata {
        payload_size,
        compression_type: compression_id,
        target_bin_name_len: target_bin_name.len() as u32,
    };

    final_file.write_all(target_bin_name.as_bytes())?;

    let metadata_bytes = unsafe {
        std::slice::from_raw_parts(&metadata as *const BundleMetadata as *const u8,
                                   size_of::<BundleMetadata>())
    };
    final_file.write_all(metadata_bytes)?;
    final_file.write_all(&MAGIC_MARKER)?;

    fs::remove_file(&compressed_payload_path).ok();

    println!("\n[Generator Success]");
    println!("  Payload Size: {} bytes", payload_size);
    println!("  Metadata Size: {} bytes", size_of::<BundleMetadata>() + target_bin_name.len() + MAGIC_MARKER.len());
    println!("  Compressed Bundle created at: {}", args.output.display());
    Ok(())
}
