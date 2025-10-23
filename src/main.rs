use clap::{CommandFactory, Parser};
use std::path::PathBuf;
use std::error::Error;
use crate::runtime::Runtime;

mod generator;
mod runtime;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser, Debug)]
#[command(name = "Rex", version = VERSION, author = "LinuxDicasPro",
    about = "Rex - Static Rust Executable Generator and Runtime"
)]

struct Cli {
    #[arg(short = 't', long, value_name = "TARGET_BINARY_FILE",
        help = "Path to the main target binary to bundle")]
    target_binary: Option<PathBuf>,

    #[arg(short = 'L', long, value_name = "COMPRESSION_LEVEL", default_value_t = 5,
        help = "Compression level (1-22)")]
    compression_level: i32,

    #[arg(short = 'l', long, value_name = "EXTRA_LIBRARIES",
        help = "Additional libraries to include in libs")]
    extra_libs: Vec<PathBuf>,

    #[arg(short = 'b', long, value_name = "EXTRA_BINARIES",
        help = "Additional binaries to include in bins")]
    extra_bins: Vec<PathBuf>,

    #[arg(short = 'a', long, value_name = "ADDITIONAL_FILES",
        help = "Extra files or directories to include in the bundle")]
    additional_files: Vec<String>,
}

fn rex_main(runtime: &mut Runtime) -> Result<(), Box<dyn Error>> {
    let args_vec: Vec<String> = std::env::args().collect();
    let is_runtime = runtime.is_bundled();

    if is_runtime {
        return runtime.run()
    }

    if args_vec.len() == 1 {
        Cli::command().print_help()?;
        return Ok(());
    }

    let cli = Cli::parse();
    let args = generator::BundleArgs {
        target_binary: cli.target_binary.unwrap(),
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
