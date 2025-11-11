use std::error::Error;
use std::fs;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::mem::size_of;
use std::path::Path;
use std::process::Command;

const MAGIC_MARKER: [u8; 10] = *b"REX_BUNDLE";

#[repr(C, packed)]
struct BundleMetadata {
    payload_size: u64,
    target_bin_name_len: u32,
}

const _: () = assert!(size_of::<BundleMetadata>() == 12);

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

Extra Options:
  --rex-help     Show this help message
  --rex-extract  Extract the embedded bundle to the current directory"#
    );
}

impl Runtime {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let payload_info = Self::find_payload_info()?;
        Ok(Self {
            payload_info,
            executed: false,
        })
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
                    if let Some(info) = &self.payload_info {
                        let current_dir = std::env::current_dir()?;
                        println!("[rex] Extracting bundle to {}", current_dir.display());
                        Self::extract_payload(info, &current_dir)?;
                        println!("[rex] Extraction completed successfully!");
                    }
                    return Ok(());
                }
                _ => {}
            }
        }
        self.payload_info.take()
            .map_or(Ok(()), |info| self.run_bundled_binary(&info))
    }

    fn find_payload_info() -> Result<Option<PayloadInfo>, Box<dyn Error>> {
        let exec = std::env::current_exe()?;
        let mut file = File::open(&exec)?;
        let file_size = file.metadata()?.len();

        const FIXED_METADATA_SIZE: u64 =
            size_of::<BundleMetadata>() as u64 + MAGIC_MARKER.len() as u64;
        const MAX_NAME_LEN: u64 = 256;

        let search_start = file_size.saturating_sub(FIXED_METADATA_SIZE + MAX_NAME_LEN);
        file.seek(SeekFrom::Start(search_start))?;

        let mut buffer = vec![0u8; (file_size - search_start) as usize];
        file.read_exact(&mut buffer)?;

        let marker_rel_index = buffer
            .windows(MAGIC_MARKER.len())
            .rposition(|w| w == MAGIC_MARKER);

        let marker_start_in_file = match marker_rel_index {
            Some(idx) => search_start + idx as u64,
            None => return Ok(None),
        };

        let metadata_start = marker_start_in_file
            .checked_sub(size_of::<BundleMetadata>() as u64)
            .ok_or("Invalid metadata position")?;

        file.seek(SeekFrom::Start(metadata_start))?;
        let mut metadata_bytes = [0u8; size_of::<BundleMetadata>()];
        file.read_exact(&mut metadata_bytes)?;

        let payload_size = u64::from_le_bytes(metadata_bytes[0..8].try_into().unwrap());
        let target_name_len = u32::from_le_bytes(metadata_bytes[8..12].try_into().unwrap()) as usize;

        let name_start = metadata_start
            .checked_sub(target_name_len as u64)
            .ok_or("Invalid target name position")?;
        file.seek(SeekFrom::Start(name_start))?;
        let mut name_bytes = vec![0u8; target_name_len];
        file.read_exact(&mut name_bytes)?;
        let target_binary_name = String::from_utf8(name_bytes)?;

        let payload_start_offset = file_size
            .checked_sub(FIXED_METADATA_SIZE + target_name_len as u64 + payload_size)
            .ok_or("Invalid payload offset")?;

        Ok(Some(PayloadInfo {
            metadata: BundleMetadata {
                payload_size,
                target_bin_name_len: target_name_len as u32,
            },
            payload_start_offset,
            target_binary_name,
        }))
    }

    fn extract_payload(info: &PayloadInfo, dest_path: &Path) -> Result<(), Box<dyn Error>> {
        let exec = std::env::current_exe()?;
        let mut file = File::open(&exec)?;
        file.seek(SeekFrom::Start(info.payload_start_offset))?;

        let payload_reader = file.take(info.metadata.payload_size);
        let decoder = zstd::Decoder::new(payload_reader)?;
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(dest_path)?;
        Ok(())
    }

    fn run_bundled_binary(&mut self, info: &PayloadInfo) -> Result<(), Box<dyn Error>> {
        let extraction_root = std::env::temp_dir();

        Self::extract_payload(info, extraction_root.as_path())?;

        let bundle_dir = extraction_root.join(format!("{}_bundle", info.target_binary_name));
        let bin_dir = bundle_dir.join("bins");
        let libs_dir = bundle_dir.join("libs");
        let target_bin_path = bundle_dir.join(&info.target_binary_name);

        if !target_bin_path.exists() {
            fs::remove_dir_all(&bundle_dir).ok();
            return Err("Target binary not found".into());
        }

        let loader_path = ["ld-linux-x86-64.so.2", "ld-musl-x86_64.so.1"]
            .iter()
            .map(|f| libs_dir.join(f))
            .find(|p| p.exists())
            .ok_or("No compatible loader found (ld-linux or ld-musl)")?;

        let existing_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", bin_dir.display(), existing_path);
        unsafe {
            std::env::set_var("PATH", new_path);
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
        fs::remove_dir_all(&bundle_dir).ok();

        match result {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(format!("Bundled binary exited with code: {}", status).into()),
            Err(e) => Err(format!("Failed to run bundled binary: {}", e).into()),
        }
    }

    pub fn has_run(&self) -> bool {
        self.executed
    }
}
