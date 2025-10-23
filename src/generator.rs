use fs_extra::dir::CopyOptions;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
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

fn find_system_loader() -> Option<PathBuf> {
    let possible_paths = [
        "/lib64/ld-linux-x86-64.so.2",
        "/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2",
        "/lib/ld-musl-x86_64.so.1"
    ];

    for path in possible_paths.iter() {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn create_temp_dir(base_name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let path = std::env::temp_dir().join(format!("{}_bundle", base_name));
    if path.exists() { fs::remove_dir_all(&path)?; }
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn create_compressed_payload(path: &Path, target: &str, l: i32) -> Result<PathBuf, Box<dyn Error>> {
    let tmp_dir = std::env::temp_dir().join(format!("{}_bundle_tmp", target));
    if tmp_dir.exists() {
        fs::remove_dir_all(&tmp_dir)?;
    }
    fs::create_dir_all(&tmp_dir)?;

    let payload = tmp_dir.join(format!("{target}.tar.zstd"));
    let file_writer = File::create(&payload)?;

    println!("\n[Packaging] Starting TAR archiving and ZSTD compression (level {l}) to:\n{}", payload.display());

    let mut enc = Encoder::new(file_writer, l)?;
    enc.long_distance_matching(true)?;
    let encoder: Box<dyn Write> =Box::new(enc.auto_finish());

    let bundle_dir_name = format!("{}_bundle", target);
    let mut builder = tar::Builder::new(encoder);
    builder.append_dir_all(&bundle_dir_name, path)?;

    Ok(payload)
}

fn get_dynamic_dependencies(binary: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    println!("[Generator] Attempting to detect dynamic dependencies (ldd)...");

    if !binary.exists() {
        return Err(format!("Binary not found at: {}", binary.display()).into());
    }

    let output = Command::new("ldd").arg(binary).output().ok();

    if output.as_ref().map_or(true, |out| !out.status.success()) {
        eprintln!("[Generator Warning] Failed to run or 'ldd' exited with error");
        return Err("ldd failed".into());
    }

    let output = output.unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut deps = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 && parts.contains(&"=>") {
            if let Some(path_str) = parts.iter().skip_while(|&p| *p != "=>").nth(1) {
                let path = PathBuf::from(path_str.trim());
                if path.exists() && !path_str.contains("ld-linux") && !path_str.contains("ld-musl") {
                    deps.push(path);
                }
            }
        }
    }

    println!("[Generator] Successfully detected: {} dependencies", deps.len());
    Ok(deps)
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
    let target_name = target_binary.file_name().unwrap().to_str().unwrap();
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

    let staging_dir = create_temp_dir(&target_name)?;
    let root_path = staging_dir.as_path();
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

    fs::copy(target_binary, root_path.join(target_name))?;

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
    let extra_paths: Vec<_> = args.additional_files.iter().filter_map(|e| {
            let p = cwd.join(e);
            if p.exists() { Some(p) } else { None }
        }).collect();

    if !extra_paths.is_empty() {
        let options = CopyOptions::new();
        let entry_refs: Vec<&Path> = extra_paths.iter().map(|p| p.as_path()).collect();
        fs_extra::copy_items(&entry_refs, root_path, &options)?;
    }

    let compressed_payload_path = create_compressed_payload(root_path, &target_name, args.compression_level)?;
    let payload_size = compressed_payload_path.metadata()?.len();
    let out = format!("{}.Rex", args.target_binary.file_name().unwrap().display());

    println!("\n[Output] Creating final bundle file: {}", out);

    let exec = std::env::current_exe()?;
    fs::copy(&exec, &out)?;

    use std::fs::Permissions;

    let perms = Permissions::from_mode(0o755);
    fs::set_permissions(&out, perms)?;

    let mut final_file = fs::OpenOptions::new().append(true).open(&out)?;
    let mut payload_file = File::open(&compressed_payload_path)?;
    
    io::copy(&mut payload_file, &mut final_file)?;

    let metadata = BundleMetadata {
        payload_size,
        target_bin_name_len: target_name.len() as u32,
    };

    final_file.write_all(target_name.as_bytes())?;

    let metadata_bytes = unsafe {
        std::slice::from_raw_parts(&metadata as *const BundleMetadata as *const u8,
                                   size_of::<BundleMetadata>())
    };
    final_file.write_all(metadata_bytes)?;
    final_file.write_all(&MAGIC_MARKER)?;

    fs::remove_file(&compressed_payload_path).ok();
    fs::remove_dir_all(&staging_dir).ok();

    println!("\n[Generator Success]");
    println!("  Payload Size: {} bytes", payload_size);
    println!("  Metadata Size: {} bytes", size_of::<BundleMetadata>() + target_name.len() + MAGIC_MARKER.len());
    println!("  Compressed Bundle created at: {}", out);
    Ok(())
}
