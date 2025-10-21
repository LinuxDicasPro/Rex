use std::error::Error;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path};
use tempfile::tempdir;
use std::mem::size_of;
use xz2::read::XzDecoder;
use std::process::Command;

#[cfg(debug_assertions)]
use std::io::Write;

const MAGIC_MARKER: [u8; 10] = *b"REX_BUNDLE";

#[repr(C)]
struct BundleMetadata {
    payload_size: u64,
    compression_type: u32,
    target_bin_name_len: u32,
}

const _: () = assert!(size_of::<BundleMetadata>() == 16);

struct PayloadInfo {
    metadata: BundleMetadata,
    payload_start_offset: u64,
    target_binary_name: String,
}

pub struct Runtime {
    payload_info: Option<PayloadInfo>,
    executed: bool,
}

fn print_help() {
    println!(r#"Rex Runtime - Self-contained binary runner

Usage:
  ./program [options]

Options:
  --rex-help
      Show this help message

  --rex-extract
      Extract the embedded bundle to the current directory"#
    );
}

impl Runtime {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let payload_info = Self::find_payload_info()?;
        Ok(Self { payload_info, executed: false })
    }

    pub fn is_bundled(&self) -> bool {
        self.payload_info.is_some()
    }

    pub fn run(&mut self) -> Result<(), Box<dyn Error>> {
        let args: Vec<String> = std::env::args().collect();

        if args.len() > 1 {
            match args[1].as_str() {
                "--rex-help" => {
                    print_help();
                    return Ok(());
                }
                "--rex-extract" => {
                    return if let Some(info) = &self.payload_info {
                        let current_dir = std::env::current_dir()?;
                        println!("[rex] Extracting bundle to {}", current_dir.display());
                        Self::extract_payload(info, &current_dir)?;
                        println!("[rex] Extraction completed successfully!");
                        Ok(())
                    } else {
                        Err("No payload found in the executable.".into())
                    }
                }
                _ => {}
            }
        }
        if let Some(info) = self.payload_info.take() {
            self.run_bundled_binary(&info)
        } else {
            Err("No bundled payload found.".into())
        }
    }

    fn find_payload_info() -> Result<Option<PayloadInfo>, Box<dyn Error>> {
        let current_exe_path = std::env::current_exe()?;
        let mut file = File::open(&current_exe_path)?;
        let file_size = file.metadata()?.len();

        const FIXED_METADATA_SIZE: u64 = size_of::<BundleMetadata>() as u64 + MAGIC_MARKER.len() as u64;
        const MAX_NAME_LEN: u64 = 256;

        let search_start_offset = file_size.saturating_sub(FIXED_METADATA_SIZE + MAX_NAME_LEN);
        file.seek(SeekFrom::Start(search_start_offset))?;

        let mut buffer = vec![0; (file_size - search_start_offset) as usize];
        file.read_exact(&mut buffer)?;

        let marker_index = buffer.windows(MAGIC_MARKER.len()).rposition(|w| w == MAGIC_MARKER);

        let marker_start_in_file = match marker_index {
            Some(index) => search_start_offset + index as u64,
            None => return Ok(None),
        };

        let metadata_start = marker_start_in_file - size_of::<BundleMetadata>() as u64;
        file.seek(SeekFrom::Start(metadata_start))?;

        let mut metadata_bytes = [0u8; size_of::<BundleMetadata>()];
        file.read_exact(&mut metadata_bytes)?;

        let metadata = BundleMetadata {
            payload_size: u64::from_le_bytes(metadata_bytes[0..8].try_into().unwrap()),
            compression_type: u32::from_le_bytes(metadata_bytes[8..12].try_into().unwrap()),
            target_bin_name_len: u32::from_le_bytes(metadata_bytes[12..16].try_into().unwrap()),
        };

        let name_len = metadata.target_bin_name_len as usize;
        if name_len == 0 {
            return Err("Structure mismatch: Target binary name length is zero.".into());
        }

        let name_start = metadata_start.checked_sub(name_len as u64).unwrap();
        file.seek(SeekFrom::Start(name_start))?;

        let mut name_bytes = vec![0u8; name_len];
        file.read_exact(&mut name_bytes)?;

        let target_binary_name = String::from_utf8(name_bytes)?;
        let payload_start_offset = file_size
            .checked_sub(FIXED_METADATA_SIZE + name_len as u64 + metadata.payload_size)
            .ok_or("Invalid payload offset")?;

        Ok(Some(PayloadInfo { metadata, payload_start_offset, target_binary_name }))
    }

    fn extract_payload(info: &PayloadInfo, dest_path: &Path) -> Result<(), Box<dyn Error>> {
        let current_exe_path = std::env::current_exe()?;
        let mut file = File::open(&current_exe_path)?;
        file.seek(SeekFrom::Start(info.payload_start_offset))?;

        let payload_reader = file.take(info.metadata.payload_size);

        let decoder: Box<dyn Read> = match info.metadata.compression_type {
            0 => Box::new(zstd::Decoder::new(payload_reader)?),
            1 => Box::new(XzDecoder::new(payload_reader)),
            id => return Err(format!("Unknown compression ID: {id}").into()),
        };

        let mut archive = tar::Archive::new(decoder);
        archive.unpack(dest_path)?;
        Ok(())
    }

    fn run_bundled_binary(&mut self, info: &PayloadInfo) -> Result<(), Box<dyn Error>> {
        let temp_dir_handle = tempdir()?;
        let extraction_root = temp_dir_handle.path();

        Self::extract_payload(info, extraction_root)?;

        let bundle_dir = extraction_root.join(format!("{}_bundle", info.target_binary_name));
        let bin_dir = bundle_dir.join("bins");
        let libs_dir = bundle_dir.join("libs");
        let target_bin_path = bundle_dir.join(&info.target_binary_name);

        if !target_bin_path.exists() {
            return Err(format!("Target binary not found in bundle: {}", target_bin_path.display()).into());
        }

        #[cfg(target_os = "linux")]
        let loader_path = {
            let glibc_loader = libs_dir.join("ld-linux-x86-64.so.2");
            let musl_loader = libs_dir.join("ld-musl-x86_64.so.1");

            if glibc_loader.exists() {
                glibc_loader
            } else if musl_loader.exists() {
                musl_loader
            } else {
                return Err("No compatible loader found (ld-linux or ld-musl)".into());
            }
        };

        if cfg!(target_os = "linux") {
            let existing_path = std::env::var("PATH").unwrap_or_default();
            let new_path = format!("{}:{}", bin_dir.display(), existing_path);
            unsafe { std::env::set_var("PATH", new_path); }
        }

        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut cmd_args = vec![
            "--library-path".to_string(),
            libs_dir.to_string_lossy().to_string(),
            target_bin_path.to_string_lossy().to_string(),
        ];
        cmd_args.extend(args);

        let result = Command::new(loader_path)
            .args(&cmd_args)
            .current_dir(bin_dir)
            .status();

        self.executed = true;

        match result {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(format!("Bundled binary exited with code: {}", status).into()),
            Err(e) => Err(format!("Failed to run bundled binary: {}", e).into()),
        }
    }

    pub fn has_run(&self) -> bool {
        self.executed
    }

    #[cfg(debug_assertions)]
    fn _pause() {
        println!("Pressione ENTER para continuar...");
        let _ = std::io::stdout().flush();
        let mut buffer = String::new();
        let _ =  std::io::stdin().read_line(&mut buffer);
    }
}
