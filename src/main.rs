use crate::runtime::Runtime;
use std::env;
use std::error::Error;
use std::path::PathBuf;

mod generator;
mod runtime;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_COMPRESSION: i32 = 5;

struct Cli {
    target_binary: Option<PathBuf>,
    compression_level: i32,
    extra_libs: Vec<PathBuf>,
    extra_bins: Vec<PathBuf>,
    additional_files: Vec<String>,
}

impl Cli {
    fn parse() -> Result<Self, Box<dyn Error>> {
        let mut args = env::args().skip(1);

        let mut cli = Self {
            target_binary: None,
            compression_level: DEFAULT_COMPRESSION,
            extra_libs: vec![],
            extra_bins: vec![],
            additional_files: vec![],
        };

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-t" | "--target-binary" => {
                    cli.target_binary = Some(Self::expect_path(&mut args, "--target-binary")?)
                }
                "-L" | "--compression-level" => {
                    cli.compression_level =
                        Self::expect_value(&mut args, "--compression-level")?.parse()?
                }
                "-l" | "--extra-libs" => cli
                    .extra_libs
                    .push(Self::expect_path(&mut args, "--extra-libs")?),
                "-b" | "--extra-bins" => cli
                    .extra_bins
                    .push(Self::expect_path(&mut args, "--extra-bins")?),
                "-f" | "--extra-files" => cli
                    .additional_files
                    .push(Self::expect_value(&mut args, "--extra-files")?),
                "-h" | "--help" => {
                    Self::print_help();
                    return Err("help displayed".into());
                }
                other => return Err(format!("Unknown option: {other}").into()),
            }
        }

        Ok(cli)
    }

    fn expect_value<I: Iterator<Item = String>>(
        args: &mut I,
        name: &str,
    ) -> Result<String, Box<dyn Error>> {
        args.next()
            .ok_or_else(|| format!("missing value for {name}").into())
    }

    fn expect_path<I: Iterator<Item = String>>(
        args: &mut I,
        name: &str,
    ) -> Result<PathBuf, Box<dyn Error>> {
        Ok(PathBuf::from(Self::expect_value(args, name)?))
    }

    fn print_help() {
        println!(
            r#"Rex v{VERSION} - Static Rust Executable Generator and Runtime

Usage:
  rex [OPTIONS]

Options:
  -t, --target-binary <FILE>       Path to the main target binary to bundle
  -L, --compression-level <NUM>    Compression level (1–22, default {DEFAULT_COMPRESSION})
  -l, --extra-libs <FILE>          Additional libraries to include
  -b, --extra-bins <FILE>          Additional binaries to include
  -f, --extra-files <PATH>         Extra files or directories to include
  -h, --help                       Show this help message"#);
    }
}

fn rex_main(runtime: &mut Runtime) -> Result<(), Box<dyn Error>> {
    let args_vec: Vec<String> = env::args().collect();

    if runtime.is_bundled() {
        return runtime.run();
    }

    if args_vec.len() == 1 {
        Cli::print_help();
        return Ok(());
    }

    let cli = match Cli::parse() {
        Ok(c) => c,
        Err(e) if e.to_string().contains("displayed") => return Ok(()),
        Err(e) => return Err(e),
    };

    let args = generator::BundleArgs {
        target_binary: cli.target_binary.ok_or("missing --target-binary")?,
        compression_level: cli.compression_level,
        extra_libs: cli.extra_libs,
        extra_bins: cli.extra_bins,
        additional_files: cli.additional_files,
    };

    generator::generate_bundle(args)?;
    Ok(())
}

fn main() {
    match Runtime::new() {
        Ok(mut runtime) => {
            if let Err(e) = rex_main(&mut runtime) {
                if !runtime.has_run() {
                    eprintln!("Error: {e}");
                }
            }
        }
        Err(e) => eprintln!("Error creating runtime: {e}"),
    }
}
