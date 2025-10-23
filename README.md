<p align="center">
  <img src="logo.png" width="256">
</p>

<h1 align="center">Rex - Static Rust Executable Generator and Runtime</h1> 

**Rex** is a small Rust project that packs a target binary together with its
dynamic libraries, extra binaries and files into a single executable.
The runtime extracts the embedded payload at execution time and runs the
target program using a suitable loader (glibc or musl) when available.

## Summary

* Language: **Rust**
* License: **MIT**
* Packaging output: a single executable file containing the runtime + compressed
`.tar` payload appended to it
* Bundle internal directory name convention: `<target_binary_name>_bundle/`

## Project layout (important files)

* `main.rs` — CLI entrypoint. Detects whether the executable is a runtime
(contains a payload) or is being run as the builder. Uses `clap` for the CLI.
* `generator.rs` — The packer/generator that creates the staging directory,
copies binaries/libs/files, creates a tar and compresses it (zstd),
and appends the payload and metadata to the runtime executable.
* `runtime.rs` — The runtime that scans the running executable for the metadata
marker extracts the payload to a temporary directory, adjusts environment
variables and executes the embedded binary.

## Building

### Prerequisites

* Rust toolchain (stable). Tested with modern Rust toolchains.
* Unix-like environment (Linux) for `ldd` dependency detection and loader handling.
* `ldd` should be available for best automatic dependency detection.

### Build

```bash
cargo build --release
# binary: target/release/rex
```

## CLI (how to use)

Run `rex` (the built binary) as a generator to create a bundle:

```bash
# minimal: specify target binary
./rex --target-binary ./myapp

# typical full command
./rex \
  --target-binary ./myapp \
  --compression-level 19 \
  --extra-libs /usr/lib/x86_64-linux-gnu/libexample.so \
  --extra-bins ./helpers ./tools \
  --additional-files config.json README.md
```

The generator will produce `myapp.Rex` (target file base name + `.Rex`).

### Runtime flags (embedded runtime inside the produced bundle)

When the generated bundle runs and contains payload, the runtime inspects
arguments. The runtime supports:

* `--rex-help` — prints a short help message.
* `--rex-extract` — extracts the embedded bundle into the **current working directory**.

If no runtime flag is given, the runtime will extract to a temporary
directory and execute the embedded binary automatically.

## Bundle internal structure

Inside the compressed `.tar` payload the generator creates a directory named
`<target>_bundle` with this layout:

```
<target>_bundle/
├─ bins/         # helper/extras copied here
├─ libs/         # shared libraries, and loader (ld-linux or ld-musl)
├─ <target>      # the target binary copied at the bundle root
└─ [other files] # additional files or folders copied to the bundle root
```

The runtime expects exactly this layout and looks up the active bundle
using the `target` name provided in the appended metadata.

## Loader handling and execution (Linux)

* The generator attempts to copy a system loader into `libs/` (ld-linux or ld-musl)
if found on the build machine.
* The runtime chooses the loader from the extracted `libs/` (`ld-linux-x86-64.so.2`
or `ld-musl-x86_64.so.1`) and invokes it with `--library-path <libs> <target>`
so the target binary runs using the bundled libraries.
* The `PATH` environment variable is temporarily prefixed with the extracted
`bins/` directory so helper tools in `bins` can be resolved.

## Notes and behaviors

* The generator uses `ldd` output to detect dynamic dependencies automatically;
if `ldd` is unavailable or fails, it warns and proceeds (you can pass `--extra-libs`
to include libraries manually).
* The generator copies the system loader into `libs/` (if found) to improve
portability of the bundle.
* Temporary files used during packaging are cleaned up after the bundle is produced.
* The runtime extracts to a temporary directory by default, but `--rex-extract`
writes to the current working directory and prints progress messages.

## Example: Create and run a bundle

```bash
# 1) Build the tool
cargo build --release

# 2) Package your app
./target/release/rex --target-binary ./target/release/myapp --compression-level 19

# 3) Run the bundle
./myapp.Rex
```

Or extract files manually to inspect contents:

```bash
./myapp.Rex --rex-extract
# this extracts into the current directory into 'myapp_bundle/' (or similar) depending on the target name
```

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for more details.
