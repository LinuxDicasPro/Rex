use std::env;
use std::path::PathBuf;
use std::error::Error;
use crate::runtime::Runtime;

mod generator;
mod runtime;

const VERSION: &str = env!("CARGO_PKG_VERSION");

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
        let mut cli = Cli {
            target_binary: None,
            compression_level: 5,
            extra_libs: Vec::new(),
            extra_bins: Vec::new(),
            additional_files: Vec::new(),
        };

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-t" | "--target-binary" => {
                    if let Some(val) = args.next() {
                        cli.target_binary = Some(PathBuf::from(val));
                    } else {
                        return Err("missing value for --target-binary".into());
                    }
                }
                "-L" | "--compression-level" => {
                    if let Some(val) = args.next() {
                        cli.compression_level = val.parse::<i32>()?;
                    } else {
                        return Err("missing value for --compression-level".into());
                    }
                }
                "-l" | "--extra-libs" => {
                    if let Some(val) = args.next() {
                        cli.extra_libs.push(PathBuf::from(val));
                    } else {
                        return Err("missing value for --extra-libs".into());
                    }
                }
                "-b" | "--extra-bins" => {
                    if let Some(val) = args.next() {
                        cli.extra_bins.push(PathBuf::from(val));
                    } else {
                        return Err("missing value for --extra-bins".into());
                    }
                }
                "-a" | "--additional-files" => {
                    if let Some(val) = args.next() {
                        cli.additional_files.push(val);
                    } else {
                        return Err("missing value for --additional-files".into());
                    }
                }
                "-h" | "--help" => {
                    Cli::print_help();
                    std::process::exit(0);
                }
                "-v" | "--version" => {
                    println!("Rex version {}", VERSION);
                    std::process::exit(0);
                }
                other => {
                    eprintln!("Unknown option: {}", other);
                    Cli::print_usage();
                    std::process::exit(1);
                }
            }
        }

        Ok(cli)
    }

    fn print_usage() {
        println!("Usage: rex [OPTIONS]");
        println!("Try 'rex --help' for more information.");
    }

    fn print_help() {
        println!("Rex - Static Rust Executable Generator and Runtime");
        println!("Version: {}", VERSION);
        println!();
        println!("Options:");
        println!("  -t, --target-binary <FILE>       Path to the main target binary to bundle");
        println!("  -L, --compression-level <NUM>    Compression level (1-22, default 5)");
        println!("  -l, --extra-libs <FILE>          Additional libraries to include");
        println!("  -b, --extra-bins <FILE>          Additional binaries to include");
        println!("  -a, --additional-files <PATH>    Extra files or directories to include");
        println!("  -v, --version                    Show version information");
        println!("  -h, --help                       Show this help message");
    }
}

fn rex_main(runtime: &mut Runtime) -> Result<(), Box<dyn Error>> {
    let args_vec: Vec<String> = env::args().collect();
    let is_runtime = runtime.is_bundled();

    if is_runtime {
        return runtime.run();
    }

    if args_vec.len() == 1 {
        Cli::print_help();
        return Ok(());
    }

    let cli = Cli::parse()?;
    let args = generator::BundleArgs {
        target_binary: cli.target_binary.ok_or("missing --target-binary")?,
        compression_level: cli.compression_level,
        extra_libs: cli.extra_libs,
        extra_bins: cli.extra_bins,
        additional_files: cli.additional_files,
    };
    generator::generate_bundle(args).map_err(|e| e.into())
}

fn main() {
    let mut runtime = match Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Error create runtime: {}", e);
            std::process::exit(1);
        }
    };

    let exit_code: i32 = match rex_main(&mut runtime) {
        Ok(()) => 0,
        Err(e) => {
            if !runtime.has_run() {
                eprintln!("Error: {}", e);
            }
            1
        }
    };
    std::process::exit(exit_code);
}
