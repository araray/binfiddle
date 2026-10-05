/// src/main.rs
use binfiddle::nn::{capabilities_envelope, capabilities_text, NnError};
use binfiddle::utils::parsing::{parse_search_pattern, validate_search_pattern};
use binfiddle::utils::progress::{Progress, ProgressReader};
use binfiddle::{BinaryData, BinarySource, BinfiddleError, Result, SearchConfig};
use clap::{Parser, Subcommand};
use std::io::{self, Read, Write};

#[derive(Parser)]
#[command(name = "binfiddle")]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Input file (use '-' for stdin)
    #[arg(short, long, group = "source")]
    input: Option<String>,

    /// Read from current process memory via /proc/self/mem (Linux only)
    #[arg(long, group = "source")]
    process_self: bool,

    /// Read from a process's memory via /proc/<pid>/mem (Linux only)
    #[arg(long, group = "source")]
    pid: Option<u32>,

    /// List memory regions of the target process instead of running a command
    #[arg(long)]
    list_regions: bool,

    /// Allow writing back to process memory (current process only)
    #[arg(long)]
    allow_write: bool,

    /// Temporarily make read-only process-memory pages writable before writing
    #[arg(long, requires = "allow_write")]
    force_writable: bool,

    /// Replace inaccessible process-memory pages with zeros instead of failing
    #[arg(long, conflicts_with = "skip_inaccessible")]
    zero_fill_inaccessible: bool,

    /// Skip inaccessible process-memory pages instead of failing (read only)
    #[arg(long, conflicts_with = "zero_fill_inaccessible")]
    skip_inaccessible: bool,

    /// Base address to read from when using --process-self or --pid (hex or decimal)
    #[arg(long)]
    address: Option<String>,

    /// Number of bytes to read when using --process-self or --pid (hex or decimal)
    #[arg(long)]
    size: Option<String>,

    /// Modify file directly (requires input file)
    #[arg(long, requires = "input", conflicts_with = "output")]
    in_file: bool,

    /// Output file (use '-' for stdout)
    #[arg(short, long)]
    output: Option<String>,

    /// Input format (hex, dec, oct, bin) for write/edit
    #[arg(long, default_value = "hex")]
    input_format: String,

    /// Output format (hex, dec, oct, bin, ascii, raw)
    #[arg(short, long, default_value = "hex")]
    format: String,

    /// Suppress diff output
    #[arg(long)]
    silent: bool,

    /// Show progress bars for long-running commands
    #[arg(long)]
    progress: bool,

    /// Chunk size in bits (default: 8)
    #[arg(short, long, default_value = "8")]
    chunk_size: usize,

    /// Number of chunks per line (default: 16)
    #[arg(long, default_value = "16")]
    width: usize,

    /// Show hex address offset prefix on each output line
    #[arg(long)]
    show_offset: bool,

    /// Show ASCII sidebar alongside hex output (implies --show-offset)
    #[arg(long)]
    show_ascii: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Read bytes from the binary data
    Read {
        /// Range in format 'start..end' or 'index'
        range: String,
    },

    /// Write bytes to the binary data
    Write {
        /// Position to write at
        position: usize,

        /// Value to write
        value: String,
    },

    /// Edit the binary data (insert, remove, replace)
    Edit {
        /// Operation: insert, remove, replace
        #[arg(value_parser = ["insert", "remove", "replace"])]
        operation: String,

        /// Position or range (for remove/replace)
        range: String,

        /// Data for insert/replace
        #[arg(
            required_if_eq("operation", "insert"),
            required_if_eq("operation", "replace")
        )]
        data: Option<String>,
    },

    /// Compute a hash digest of the binary data
    Hash {
        /// Hash algorithm: md5, sha1, sha256, blake3, crc32, xxhash64
        algorithm: String,

        /// Output format: hex, base64
        #[arg(long, default_value = "hex", value_parser = ["hex", "base64"])]
        output_format: String,

        /// Block size for block-based hashing (0 = whole file)
        #[arg(long, default_value = "0")]
        block_size: usize,

        /// Stream the input instead of memory-mapping (for huge files)
        #[arg(long)]
        stream: bool,

        /// Read chunk size when streaming (supports K/M/G suffixes, default 1M)
        #[arg(long, default_value = "1M")]
        read_block_size: String,

        /// Verify checksums from a file (md5sum/sha256sum format)
        #[arg(long, value_name = "CHECKSUM_FILE")]
        check: Option<String>,
    },

    /// Search for patterns in binary data
    Search {
        /// Pattern to search for (interpreted per --input-format)
        pattern: String,

        /// Input format for pattern: hex, ascii, dec, oct, bin, regex, hex-regex, mask
        #[arg(long, default_value = "hex", value_parser = ["hex", "ascii", "dec", "oct", "bin", "regex", "hex-regex", "hexregex", "mask"])]
        input_format: String,

        /// Find all matches (default: first match only)
        #[arg(long)]
        all: bool,

        /// Only output the count of matches
        #[arg(long)]
        count: bool,

        /// Only output match offsets (hex)
        #[arg(long)]
        offsets_only: bool,

        /// Show N bytes of context before and after each match
        #[arg(long, default_value = "0")]
        context: usize,

        /// Prevent overlapping matches
        #[arg(long)]
        no_overlap: bool,

        /// Colorize output (always, auto, never)
        #[arg(long, default_value = "auto", value_parser = ["always", "auto", "never"])]
        color: String,

        /// Stream input in blocks of this size (e.g., 64M, 1G, 256K)
        #[arg(long)]
        block_size: Option<String>,
    },

    /// Analyze binary data (entropy, histogram, index of coincidence)
    Analyze {
        /// Analysis type: entropy, histogram, ic
        #[arg(value_parser = ["entropy", "histogram", "hist", "ic", "ioc"])]
        analysis_type: String,

        /// Block size for block-based analysis (0 = entire file, supports K/M/G suffixes)
        #[arg(long, default_value = "256")]
        block_size: String,

        /// Output format: human, csv, json
        #[arg(long, default_value = "human", value_parser = ["human", "csv", "json"])]
        output_format: String,

        /// Range to analyze (format: 'start..end')
        #[arg(long)]
        range: Option<String>,
    },

    /// Compare two binary files and show differences
    Diff {
        /// First file to compare
        file1: String,

        /// Second file to compare
        file2: String,

        /// Output format: simple, unified, side-by-side, patch, summary, auto
        #[arg(long, default_value = "auto", value_parser = ["simple", "unified", "side-by-side", "sidebyside", "patch", "summary", "auto"])]
        diff_format: String,

        /// Number of context bytes around differences (for unified format)
        #[arg(long, default_value = "3")]
        context: usize,

        /// Colorize output (always, auto, never)
        #[arg(long, default_value = "auto", value_parser = ["always", "auto", "never"])]
        color: String,

        /// Ranges to ignore during comparison (e.g., "0x0..0x10,0x100..0x200")
        #[arg(long, default_value = "")]
        ignore_offsets: String,

        /// Bytes per line in output
        #[arg(long, default_value = "16")]
        diff_width: usize,

        /// Print summary of differences
        #[arg(long)]
        summary: bool,
    },

    /// Convert text encoding and line endings
    Convert {
        /// Source encoding (utf-8, utf-16le, utf-16be, latin-1, windows-1252)
        #[arg(long, default_value = "utf-8")]
        from: String,

        /// Target encoding (utf-8, utf-16le, utf-16be, latin-1, windows-1252)
        #[arg(long, default_value = "utf-8")]
        to: String,

        /// Line ending conversion (unix, windows, mac, keep)
        #[arg(long, default_value = "keep")]
        newlines: String,

        /// BOM handling (add, remove, keep)
        #[arg(long, default_value = "keep")]
        bom: String,

        /// Error handling (strict, replace, ignore)
        #[arg(long, default_value = "replace")]
        on_error: String,
    },

    /// Apply a binary patch file to a target file
    Patch {
        /// Target file to patch
        target: String,

        /// Patch file (use '-' for stdin)
        patch_file: String,

        /// Create backup with this suffix before patching (e.g., ".bak")
        #[arg(long)]
        backup: Option<String>,

        /// Show what would be done without making changes
        #[arg(long)]
        dry_run: bool,

        /// Apply patch in reverse (undo)
        #[arg(long)]
        revert: bool,
    },

    /// Parse binary data using a structural template
    Struct {
        /// Path to the YAML template file
        template: String,

        /// List all fields in the template without parsing data
        #[arg(long)]
        list_fields: bool,

        /// Get specific field value(s) - can be repeated
        #[arg(long, value_name = "FIELD")]
        get: Vec<String>,

        /// Output format: human, json, yaml
        #[arg(long, default_value = "human", value_parser = ["human", "json", "yaml"])]
        output_format: String,
    },

    /// Execute multiple binfiddle commands in sequence
    Chain {
        /// Command step to execute (can be repeated)
        #[arg(long, required = true)]
        step: Vec<String>,
    },

    /// Neural-network artifact workbench (early access)
    Nn {
        #[command(subcommand)]
        command: NnCommand,
    },
}

#[derive(Subcommand)]
enum NnCommand {
    /// Report implemented and unavailable NN workbench capabilities
    Capabilities {
        /// Report format: text, json
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        report_format: String,
    },

    /// Inventory supported model artifacts (descriptor-only, no payload reads)
    Discover {
        /// Report format: text, json
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        report_format: String,

        /// Hash full file contents, upgrading source identity strength
        #[arg(long)]
        verify_content: bool,

        /// Fail (exit 8) when coverage is incomplete
        #[arg(long)]
        require_complete: bool,

        /// Save the resulting catalog to this file
        #[arg(long)]
        out_catalog: Option<String>,
    },

    /// List tensors or sources from a catalog (or discover one on the fly)
    Ls {
        /// Saved catalog file (use the root -i option to discover instead)
        #[arg(long)]
        catalog: Option<String>,

        /// View: tensors, sources, architecture (needs --pack)
        #[arg(long, default_value = "tensors", value_parser = ["tensors", "sources", "architecture"])]
        view: String,

        /// Model pack file or directory (pack.yaml)
        #[arg(long)]
        pack: Option<String>,

        /// Exact encoding filter (e.g. safetensors.F32, ggml.q4_0)
        #[arg(long)]
        encoding: Option<String>,

        /// Source scope: unique source id prefix or exact path
        #[arg(long)]
        source: Option<String>,

        /// Bounded regular expression filter over tensor names
        #[arg(long)]
        name_regex: Option<String>,

        /// Sort order: name, bytes
        #[arg(long, default_value = "name", value_parser = ["name", "bytes"])]
        sort: String,

        /// Maximum entries per page (1-10000)
        #[arg(long)]
        limit: Option<usize>,

        /// Entries to skip before the page
        #[arg(long, default_value = "0")]
        offset: usize,

        /// Report format: text, json
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        report_format: String,
    },

    /// Show one tensor's full record, with optional evidence explanation
    Show {
        /// Saved catalog file (use the root -i option to discover instead)
        #[arg(long)]
        catalog: Option<String>,

        /// Model pack file or directory (enables --component)
        #[arg(long)]
        pack: Option<String>,

        /// Component path from a pack (e.g. decoder.layers[3].attention)
        #[arg(long, requires = "pack")]
        component: Option<String>,

        /// Exact original tensor name
        #[arg(long, conflicts_with = "id")]
        tensor: Option<String>,

        /// Tensor identifier (full or unique digest prefix)
        #[arg(long, conflicts_with = "tensor")]
        id: Option<String>,

        /// Scope for --tensor: unique source id prefix or exact path
        #[arg(long, requires = "tensor")]
        source: Option<String>,

        /// Include the evidence explanation
        #[arg(long)]
        explain: bool,

        /// Report format: text, json
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        report_format: String,
    },

    /// Resolve a tensor selection and optionally save it
    Select {
        /// Saved catalog file (use the root -i option to discover instead)
        #[arg(long)]
        catalog: Option<String>,

        /// Exact original tensor name
        #[arg(long, conflicts_with_all = ["id", "select"])]
        tensor: Option<String>,

        /// Tensor identifier (full or unique digest prefix)
        #[arg(long, conflicts_with_all = ["tensor", "select"])]
        id: Option<String>,

        /// Component selector expression (requires a model pack to resolve)
        #[arg(long = "select", conflicts_with_all = ["tensor", "id"])]
        select_expr: Option<String>,

        /// Scope for --tensor: unique source id prefix or exact path
        #[arg(long, requires = "tensor")]
        source: Option<String>,

        /// Allow an empty selection instead of rejecting it
        #[arg(long)]
        allow_empty: bool,

        /// Save the resolved selection to this file
        #[arg(long)]
        out_selection: Option<String>,

        /// Report format: text, json
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        report_format: String,
    },

    /// Map a tensor (or one element) to its file-qualified byte/bit location
    Where {
        /// Saved catalog file (use the root -i option to discover instead)
        #[arg(long)]
        catalog: Option<String>,

        /// Exact original tensor name
        #[arg(long, conflicts_with = "id")]
        tensor: Option<String>,

        /// Tensor identifier (full or unique digest prefix)
        #[arg(long, conflicts_with = "tensor")]
        id: Option<String>,

        /// Scope for --tensor: unique source id prefix or exact path
        #[arg(long, requires = "tensor")]
        source: Option<String>,

        /// Element coordinate (comma-separated decimal, e.g. 123,456)
        #[arg(long)]
        index: Option<String>,

        /// Address space (only file addresses are supported)
        #[arg(long, default_value = "file", value_parser = ["file"])]
        space: String,

        /// Report format: text, json
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        report_format: String,
    },

    /// Reverse lookup: which tensors own a file offset
    Locate {
        /// Saved catalog file (use the root -i option to discover instead)
        #[arg(long)]
        catalog: Option<String>,

        /// File offset (decimal or 0x-prefixed hex)
        #[arg(long)]
        offset: String,

        /// Report format: text, json
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        report_format: String,
    },

    /// Extract selected tensors into a bundle (weights kind)
    Slice {
        /// Saved catalog file (use the root -i option to discover instead)
        #[arg(long)]
        catalog: Option<String>,

        /// Saved selection file (plan generation mode)
        #[arg(long, conflicts_with_all = ["plan"])]
        selection: Option<String>,

        /// Saved plan file (apply mode)
        #[arg(long, conflicts_with_all = ["selection", "storage", "quant", "save_plan", "dry_run"])]
        plan: Option<String>,

        /// Slice kind (weights; component/executable need model packs)
        #[arg(long, default_value = "weights", value_parser = ["weights"])]
        kind: String,

        /// Storage policy: reference, materialized
        #[arg(long, default_value = "materialized", value_parser = ["reference", "materialized"])]
        storage: String,

        /// Quantization policy: preserve_encoding, cover_blocks, decode
        #[arg(
            long,
            default_value = "preserve_encoding",
            value_parser = ["preserve_encoding", "cover_blocks", "decode"]
        )]
        quant: String,

        /// Resolve and print the plan without writing a bundle
        #[arg(long)]
        dry_run: bool,

        /// Save the resolved plan to this file
        #[arg(long)]
        save_plan: Option<String>,

        /// Bundle output directory (apply)
        #[arg(long)]
        out_dir: Option<String>,

        /// Report format: text, json
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        report_format: String,
    },

    /// Reconstruct tensor content from a materialized slice bundle
    Assemble {
        /// Materialized bundle directory (contains slice.json)
        #[arg(long)]
        bundle: String,

        /// Output directory for reconstructed payloads
        #[arg(long)]
        out_dir: String,

        /// Report format: text, json
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        report_format: String,
    },

    /// Numerically inspect one catalog tensor
    Analyze {
        /// Saved catalog file (use the root -i option to discover instead)
        #[arg(long)]
        catalog: Option<String>,

        /// Exact original tensor name
        #[arg(long, conflicts_with = "analyze_id")]
        tensor: Option<String>,

        /// Tensor identifier (full or unique digest prefix)
        #[arg(long = "id", id = "analyze_id", conflicts_with = "tensor")]
        target_id: Option<String>,

        /// Scope for --tensor: unique source id prefix or exact path
        #[arg(long, requires = "tensor")]
        source: Option<String>,

        /// Access mode: metadata (no payload reads), sample, full
        #[arg(long, default_value = "full", value_parser = ["metadata", "sample", "full"])]
        mode: String,

        /// Seed for deterministic sampling
        #[arg(long, default_value = "17")]
        seed: u64,

        /// Sample size in elements (sample mode)
        #[arg(long, default_value = "10000")]
        sample_size: u64,

        /// Number of histogram bins (0 = no histogram)
        #[arg(long, default_value = "0")]
        histogram_bins: usize,

        /// Number of leading quantization blocks to display (0 = none)
        #[arg(long, default_value = "0")]
        blocks: u64,

        /// Reference values file for error metrics (raw little-endian f32/f64)
        #[arg(long)]
        reference: Option<String>,

        /// Reference element width: 4 (f32) or 8 (f64)
        #[arg(long, default_value = "4", value_parser = ["4", "8"])]
        reference_width: String,

        /// Report format: text, json
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        report_format: String,
    },

    /// Model pack operations
    Pack {
        #[command(subcommand)]
        command: PackCommand,
    },

    /// Transactional fixed-size edits over catalog tensors
    Edit {
        #[command(subcommand)]
        command: EditCommand,
    },
}

#[derive(Subcommand)]
enum PackCommand {
    /// Verify a declarative model pack
    Verify {
        /// Pack file or directory (pack.yaml)
        #[arg(long)]
        pack: String,

        /// Report format: text, json
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        report_format: String,
    },
}

#[derive(Subcommand)]
enum EditCommand {
    /// Plan one typed or raw-bit value change (records the preimage)
    Set {
        /// Saved catalog file (content-verified discovery required)
        #[arg(long)]
        catalog: Option<String>,

        /// Exact original tensor name
        #[arg(long, conflicts_with = "edit_id")]
        tensor: Option<String>,

        /// Tensor identifier (full or unique digest prefix)
        #[arg(long = "id", id = "edit_id", conflicts_with = "tensor")]
        target_id: Option<String>,

        /// Scope for --tensor: unique source id prefix or exact path
        #[arg(long, requires = "tensor")]
        source: Option<String>,

        /// Element coordinate (comma-separated decimal)
        #[arg(long)]
        index: String,

        /// Requested numeric value
        #[arg(
            long,
            conflicts_with = "raw_bits",
            required = true,
            group = "edit_value"
        )]
        value: Option<String>,

        /// Requested raw bits (hex; nibble for sub-byte units)
        #[arg(long = "raw-bits", conflicts_with = "value")]
        raw_bits: Option<String>,

        /// Value policy: exact_only, nearest, fixed_parameters
        #[arg(
            long,
            default_value = "auto",
            value_parser = ["auto", "exact_only", "nearest", "fixed_parameters"]
        )]
        policy: String,

        /// Allow saturating out-of-range quantized codes
        #[arg(long)]
        clamp: bool,

        /// Save the plan to this file
        #[arg(long)]
        save_plan: Option<String>,

        /// Report format: text, json
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        report_format: String,
    },

    /// Apply a saved edit plan to a fresh output file
    Apply {
        /// Saved catalog file (must match the plan)
        #[arg(long)]
        catalog: String,

        /// Saved edit plan file
        #[arg(long)]
        plan: String,

        /// Output file (must not exist; the original is never modified)
        #[arg(long)]
        out_model: String,

        /// Directory for the undo bundle
        #[arg(long)]
        undo_bundle: Option<String>,

        /// Report format: text, json
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        report_format: String,
    },

    /// Reverse an applied edit against its exact edited revision
    Undo {
        /// Undo bundle directory (from edit apply)
        #[arg(long)]
        bundle: String,

        /// The edited file to reverse
        #[arg(long)]
        target: String,

        /// Output file (must not exist)
        #[arg(long)]
        out_model: String,

        /// Report format: text, json
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        report_format: String,
    },
}

/// Run one NN workbench command. `input` is the root `--input` value, when
/// given. Stdout carries the primary report; errors map to the NN exit-code
/// categories.
fn run_nn(command: &NnCommand, input: Option<&str>) -> std::result::Result<(), NnError> {
    use binfiddle::nn::{Budget, CancellationToken, DiscoverOptions, SignalGuard};
    use std::io::Write;
    use std::path::Path;

    let cancel = CancellationToken::new();
    let guard = SignalGuard::install()?;
    let budget = Budget::unrestricted();
    budget.checkpoint()?;
    guard.propagate(&cancel);

    match command {
        NnCommand::Capabilities { report_format } => {
            let stdout = io::stdout();
            let mut out = stdout.lock();
            if report_format == "json" {
                let envelope = capabilities_envelope()?;
                envelope.write_json(&mut out)?;
            } else {
                out.write_all(capabilities_text().as_bytes())?;
                out.write_all(b"\n")?;
            }
            out.flush()?;
        }
        NnCommand::Discover {
            report_format,
            verify_content,
            require_complete,
            out_catalog,
        } => {
            let path = input.ok_or_else(|| NnError::InvalidRequest {
                message: "nn discover requires --input <file-or-directory>".to_string(),
            })?;
            if path == "-" {
                return Err(NnError::InvalidRequest {
                    message:
                        "nn discover requires a seekable file or directory; stdin is not supported"
                            .to_string(),
                });
            }
            let options = DiscoverOptions {
                verify_content: *verify_content,
            };
            let report = binfiddle::nn::discover(Path::new(path), &options, &budget)?;
            if let Some(catalog_path) = out_catalog {
                binfiddle::nn::Catalog::from_discovery(&report)?.save(Path::new(catalog_path))?;
            }
            guard.propagate(&cancel);
            let stdout = io::stdout();
            let mut out = stdout.lock();
            if report_format == "json" {
                report.envelope()?.write_json(&mut out)?;
            } else {
                out.write_all(report.text().as_bytes())?;
            }
            out.flush()?;
            if *require_complete && !report.complete() {
                let (considered, parsed, _) = report.coverage();
                return Err(NnError::IncompleteRejected {
                    detail: format!(
                        "discovery coverage incomplete: {} of {} sources fully inventoried",
                        parsed.min(considered),
                        considered
                    ),
                });
            }
        }
        NnCommand::Ls {
            catalog,
            view,
            pack,
            encoding,
            source,
            name_regex,
            sort,
            limit,
            offset,
            report_format,
        } => {
            use binfiddle::nn::queries::{self, clamp_pagination, ListFilters, SortField};
            let catalog_path = nn_path_arg(catalog.as_deref(), "ls")?;
            let input_path = nn_path_arg(input, "ls")?;
            let loaded = binfiddle::nn::Catalog::from_route(
                catalog_path,
                input_path,
                &DiscoverOptions::default(),
                &budget,
            )?;
            guard.propagate(&cancel);
            let stdout = io::stdout();
            let mut out = stdout.lock();
            if view == "architecture" {
                use binfiddle::nn::packs;
                let pack_path = pack.as_deref().ok_or_else(|| NnError::InvalidRequest {
                    message: "architecture view requires --pack <file-or-dir>".to_string(),
                })?;
                let loaded_pack = packs::Pack::load(Path::new(pack_path))?;
                let recognition = packs::Recognition::recognize(&loaded_pack, &loaded)?;
                if report_format == "json" {
                    packs::architecture_envelope(&recognition, &loaded_pack, &loaded)?
                        .write_json(&mut out)?;
                } else {
                    out.write_all(packs::architecture_text(&recognition, &loaded_pack).as_bytes())?;
                }
                out.flush()?;
                return Ok(());
            }
            if view == "sources" {
                let envelope = queries::sources_envelope(&loaded)?;
                if report_format == "json" {
                    envelope.write_json(&mut out)?;
                } else {
                    out.write_all(queries::sources_text(&loaded).as_bytes())?;
                }
            } else {
                let (limit, offset) = clamp_pagination(*limit, *offset)?;
                let sort_field = if sort == "bytes" {
                    SortField::Bytes
                } else {
                    SortField::Name
                };
                let filters = ListFilters {
                    encoding: encoding.clone(),
                    source: source.clone(),
                    name_regex: name_regex.clone(),
                };
                let page = queries::list_tensors(&loaded, &filters, sort_field, limit, offset)?;
                if report_format == "json" {
                    queries::tensors_envelope(&loaded, &page, &filters, sort_field)?
                        .write_json(&mut out)?;
                } else {
                    out.write_all(queries::tensors_text(&loaded, &page).as_bytes())?;
                }
            }
            out.flush()?;
        }
        NnCommand::Show {
            catalog,
            tensor,
            id,
            pack,
            component,
            source,
            explain,
            report_format,
        } => {
            use binfiddle::nn::show::{self, ShowTarget};
            let catalog_path = nn_path_arg(catalog.as_deref(), "show")?;
            let input_path = nn_path_arg(input, "show")?;
            let loaded = binfiddle::nn::Catalog::from_route(
                catalog_path,
                input_path,
                &DiscoverOptions::default(),
                &budget,
            )?;
            if let Some(component_path) = component.as_deref() {
                use binfiddle::nn::packs;
                let pack_path = pack
                    .as_deref()
                    .expect("clap requires --pack with --component");
                let loaded_pack = packs::Pack::load(Path::new(pack_path))?;
                let recognition = packs::Recognition::recognize(&loaded_pack, &loaded)?;
                let detail = packs::component_detail(&recognition, &loaded_pack, component_path)?;
                guard.propagate(&cancel);
                let stdout = io::stdout();
                let mut out = stdout.lock();
                if report_format == "json" {
                    packs::component_envelope(&detail, &recognition)?.write_json(&mut out)?;
                } else {
                    out.write_all(packs::component_text(&detail).as_bytes())?;
                }
                out.flush()?;
                return Ok(());
            }
            let target = match (tensor, id) {
                (Some(name), None) => ShowTarget::Name {
                    name,
                    source: source.as_deref(),
                },
                (None, Some(id)) => ShowTarget::Id { id },
                _ => {
                    return Err(NnError::InvalidRequest {
                        message: "nn show requires exactly one of --tensor or --id".to_string(),
                    })
                }
            };
            let tensor = show::resolve_show_target(&loaded, &target)?;
            guard.propagate(&cancel);
            let stdout = io::stdout();
            let mut out = stdout.lock();
            if report_format == "json" {
                show::show_envelope(&loaded, tensor, *explain)?.write_json(&mut out)?;
            } else {
                out.write_all(show::show_text(&loaded, tensor, *explain).as_bytes())?;
            }
            out.flush()?;
        }
        NnCommand::Select {
            catalog,
            tensor,
            id,
            select_expr,
            source,
            allow_empty,
            out_selection,
            report_format,
        } => {
            use binfiddle::nn::selection::{EmptyPolicy, Selection, SelectionRequest};
            let catalog_path = nn_path_arg(catalog.as_deref(), "select")?;
            let input_path = nn_path_arg(input, "select")?;
            let loaded = binfiddle::nn::Catalog::from_route(
                catalog_path,
                input_path,
                &DiscoverOptions::default(),
                &budget,
            )?;
            let request = match (tensor, id, select_expr) {
                (Some(name), None, None) => SelectionRequest::TensorName {
                    name: name.clone(),
                    source: source.clone(),
                },
                (None, Some(id), None) => SelectionRequest::TensorId { id: id.clone() },
                (None, None, Some(expression)) => SelectionRequest::ComponentExpression {
                    expression: expression.clone(),
                },
                _ => {
                    return Err(NnError::InvalidRequest {
                        message: "nn select requires exactly one of --tensor, --id, or --select"
                            .to_string(),
                    })
                }
            };
            let policy = if *allow_empty {
                EmptyPolicy::Allow
            } else {
                EmptyPolicy::Reject
            };
            let selection = Selection::resolve(&loaded, request, policy)?;
            if let Some(path) = out_selection {
                selection.save(Path::new(path))?;
            }
            guard.propagate(&cancel);
            let stdout = io::stdout();
            let mut out = stdout.lock();
            if report_format == "json" {
                selection.envelope()?.write_json(&mut out)?;
            } else {
                out.write_all(selection.text().as_bytes())?;
            }
            out.flush()?;
        }
        NnCommand::Where {
            catalog,
            tensor,
            id,
            source,
            index,
            space: _,
            report_format,
        } => {
            use binfiddle::nn::show::{self, ShowTarget};
            use binfiddle::nn::where_cmd;
            let catalog_path = nn_path_arg(catalog.as_deref(), "where")?;
            let input_path = nn_path_arg(input, "where")?;
            let loaded = binfiddle::nn::Catalog::from_route(
                catalog_path,
                input_path,
                &DiscoverOptions::default(),
                &budget,
            )?;
            let target = match (tensor, id) {
                (Some(name), None) => ShowTarget::Name {
                    name,
                    source: source.as_deref(),
                },
                (None, Some(id)) => ShowTarget::Id { id },
                _ => {
                    return Err(NnError::InvalidRequest {
                        message: "nn where requires exactly one of --tensor or --id".to_string(),
                    })
                }
            };
            let tensor = show::resolve_show_target(&loaded, &target)?;
            let coordinate = match index {
                Some(text) => Some(where_cmd::parse_coordinate(text)?),
                None => None,
            };
            guard.propagate(&cancel);
            let stdout = io::stdout();
            let mut out = stdout.lock();
            if report_format == "json" {
                where_cmd::where_envelope(&loaded, tensor, coordinate.as_deref())?
                    .write_json(&mut out)?;
            } else {
                out.write_all(where_cmd::where_text(tensor, coordinate.as_deref())?.as_bytes())?;
            }
            out.flush()?;
        }
        NnCommand::Locate {
            catalog,
            offset,
            report_format,
        } => {
            use binfiddle::nn::where_cmd;
            let catalog_path = nn_path_arg(catalog.as_deref(), "locate")?;
            let input_path = nn_path_arg(input, "locate")?;
            let loaded = binfiddle::nn::Catalog::from_route(
                catalog_path,
                input_path,
                &DiscoverOptions::default(),
                &budget,
            )?;
            let offset_value = parse_nn_offset(offset)?;
            guard.propagate(&cancel);
            let stdout = io::stdout();
            let mut out = stdout.lock();
            if report_format == "json" {
                where_cmd::locate_envelope(&loaded, offset_value)?.write_json(&mut out)?;
            } else {
                out.write_all(where_cmd::locate_text(&loaded, offset_value)?.as_bytes())?;
            }
            out.flush()?;
        }
        NnCommand::Slice {
            catalog,
            selection,
            plan,
            kind: _,
            storage,
            quant,
            dry_run,
            save_plan,
            out_dir,
            report_format,
        } => {
            use binfiddle::nn::selection::Selection;
            use binfiddle::nn::slice::{
                apply_plan, receipt_envelope, receipt_text, QuantPolicy, SlicePlan, StoragePolicy,
            };
            let catalog_path = nn_path_arg(catalog.as_deref(), "slice")?;
            let input_path = nn_path_arg(input, "slice")?;
            let loaded = binfiddle::nn::Catalog::from_route(
                catalog_path,
                input_path,
                &DiscoverOptions::default(),
                &budget,
            )?;

            let resolved_plan = match (plan.as_deref(), selection.as_deref()) {
                (Some(plan_path), None) => SlicePlan::load(Path::new(plan_path))?,
                (None, Some(selection_path)) => {
                    let selection = Selection::load(Path::new(selection_path))?;
                    let storage = StoragePolicy::parse(storage)?;
                    let quant = QuantPolicy::parse(quant)?;
                    let built = SlicePlan::build(&loaded, &selection, storage, quant)?;
                    if let Some(save_path) = save_plan.as_deref() {
                        built.save(Path::new(save_path))?;
                    }
                    built
                }
                _ => {
                    return Err(NnError::InvalidRequest {
                        message: "nn slice requires exactly one of --selection or --plan"
                            .to_string(),
                    })
                }
            };

            // Plan mode without --out-dir previews the plan (any --save-plan
            // file was already written); apply mode requires --out-dir.
            let apply_now = out_dir.is_some();
            if plan.is_some() && !apply_now {
                return Err(NnError::InvalidRequest {
                    message: "nn slice with --plan requires --out-dir to apply".to_string(),
                });
            }
            if *dry_run || !apply_now {
                guard.propagate(&cancel);
                let stdout = io::stdout();
                let mut out = stdout.lock();
                if report_format == "json" {
                    resolved_plan.envelope()?.write_json(&mut out)?;
                } else {
                    out.write_all(resolved_plan.text().as_bytes())?;
                }
                out.flush()?;
            } else {
                let out_dir = out_dir.as_deref().expect("checked above");
                let receipt = apply_plan(&resolved_plan, &loaded, Path::new(out_dir), &budget)?;
                guard.propagate(&cancel);
                let stdout = io::stdout();
                let mut out = stdout.lock();
                if report_format == "json" {
                    receipt_envelope(&receipt)?.write_json(&mut out)?;
                } else {
                    out.write_all(receipt_text(&receipt).as_bytes())?;
                }
                out.flush()?;
            }
        }
        NnCommand::Assemble {
            bundle,
            out_dir,
            report_format,
        } => {
            use binfiddle::nn::slice::{assemble_bundle, assemble_text};
            let envelope = assemble_bundle(Path::new(bundle), Path::new(out_dir), &budget)?;
            guard.propagate(&cancel);
            let stdout = io::stdout();
            let mut out = stdout.lock();
            if report_format == "json" {
                envelope.write_json(&mut out)?;
            } else {
                out.write_all(assemble_text(&envelope)?.as_bytes())?;
            }
            out.flush()?;
        }
        NnCommand::Analyze {
            catalog,
            tensor,
            target_id,
            source,
            mode,
            seed,
            sample_size,
            histogram_bins,
            blocks,
            reference,
            reference_width,
            report_format,
        } => {
            use binfiddle::nn::analyze::{self, ScanMode};
            use binfiddle::nn::show::{self, ShowTarget};
            let catalog_path = nn_path_arg(catalog.as_deref(), "analyze")?;
            let input_path = nn_path_arg(input, "analyze")?;
            let loaded = binfiddle::nn::Catalog::from_route(
                catalog_path,
                input_path,
                &DiscoverOptions::default(),
                &budget,
            )?;
            let target = match (tensor, target_id) {
                (Some(name), None) => ShowTarget::Name {
                    name,
                    source: source.as_deref(),
                },
                (None, Some(id)) => ShowTarget::Id { id },
                _ => {
                    return Err(NnError::InvalidRequest {
                        message: "nn analyze requires exactly one of --tensor or --id".to_string(),
                    })
                }
            };
            let tensor = show::resolve_show_target(&loaded, &target)?;
            let mode = match mode.as_str() {
                "metadata" => ScanMode::Metadata,
                "sample" => ScanMode::Sample,
                _ => ScanMode::Full,
            };
            // Metadata mode refuses to read payloads for metrics it cannot
            // honestly compute from descriptors alone.
            if mode == ScanMode::Metadata
                && (*histogram_bins > 0 || reference.is_some() || *blocks > 0)
            {
                return Err(NnError::InvalidRequest {
                    message: "metadata mode reads no payload; histograms, reference metrics, and block views need sample or full mode"
                        .to_string(),
                });
            }
            let reference_path = match reference.as_deref() {
                Some(path) => Some(nn_path_arg(Some(path), "analyze")?.ok_or_else(|| {
                    NnError::InvalidRequest {
                        message: "invalid reference path".to_string(),
                    }
                })?),
                None => None,
            };
            let width: u32 = reference_width.parse().unwrap_or(4);
            let result = analyze::analyze_tensor(
                &loaded,
                tensor,
                mode,
                *seed,
                *sample_size,
                *histogram_bins,
                *blocks,
                reference_path,
                width,
                &budget,
            )?;
            guard.propagate(&cancel);
            let stdout = io::stdout();
            let mut out = stdout.lock();
            if report_format == "json" {
                analyze::analysis_envelope(&loaded, &result)?.write_json(&mut out)?;
            } else {
                out.write_all(analyze::analysis_text(&result).as_bytes())?;
            }
            out.flush()?;
        }
        NnCommand::Pack { command } => match command {
            PackCommand::Verify {
                pack,
                report_format,
            } => {
                use binfiddle::nn::packs;
                let loaded_pack = packs::Pack::load(Path::new(pack))?;
                // Verify: every binding pattern compiles, every expression
                // evaluates against the config, and the schedule resolves.
                for binding in &loaded_pack.bindings {
                    for axis in &binding.shape {
                        loaded_pack.eval(axis)?;
                    }
                }
                let _ = loaded_pack.full_attention_layers().ok();
                guard.propagate(&cancel);
                let stdout = io::stdout();
                let mut out = stdout.lock();
                if report_format == "json" {
                    let semantic = binfiddle::nn::Json::object(vec![
                        ("pack_id", binfiddle::nn::Json::Str(loaded_pack.pack_id()?)),
                        ("id", binfiddle::nn::Json::Str(loaded_pack.id_name.clone())),
                        (
                            "version",
                            binfiddle::nn::Json::Str(loaded_pack.version.clone()),
                        ),
                        (
                            "bindings",
                            binfiddle::nn::Json::Str(loaded_pack.bindings.len().to_string()),
                        ),
                    ])?;
                    binfiddle::nn::ResultEnvelope::new("pack verify")
                        .with_semantic(semantic)
                        .write_json(&mut out)?;
                } else {
                    out.write_all(
                        format!(
                            "pack verified: {} v{} ({} bindings, id {})\n",
                            loaded_pack.id_name,
                            loaded_pack.version,
                            loaded_pack.bindings.len(),
                            loaded_pack.pack_id()?,
                        )
                        .as_bytes(),
                    )?;
                }
                out.flush()?;
            }
        },
        NnCommand::Edit { command } => {
            use binfiddle::nn::edit;
            use binfiddle::nn::show::{self, ShowTarget};
            match command {
                EditCommand::Set {
                    catalog,
                    tensor,
                    target_id,
                    source,
                    index,
                    value,
                    raw_bits,
                    policy,
                    clamp,
                    save_plan,
                    report_format,
                } => {
                    let catalog_path = nn_path_arg(catalog.as_deref(), "edit set")?;
                    let input_path = nn_path_arg(input, "edit set")?;
                    let loaded = binfiddle::nn::Catalog::from_route(
                        catalog_path,
                        input_path,
                        &DiscoverOptions::default(),
                        &budget,
                    )?;
                    let target = match (tensor, target_id) {
                        (Some(name), None) => ShowTarget::Name {
                            name,
                            source: source.as_deref(),
                        },
                        (None, Some(id)) => ShowTarget::Id { id },
                        _ => {
                            return Err(NnError::InvalidRequest {
                                message: "nn edit set requires exactly one of --tensor or --id"
                                    .to_string(),
                            })
                        }
                    };
                    let tensor = show::resolve_show_target(&loaded, &target)?;
                    let coordinate = binfiddle::nn::where_cmd::parse_coordinate(index)?;
                    let requested = match (value.as_deref(), raw_bits.as_deref()) {
                        (Some(v), None) => edit::RequestedValue::Typed(v.to_string()),
                        (None, Some(h)) => edit::RequestedValue::RawBits(h.to_string()),
                        _ => {
                            return Err(NnError::InvalidRequest {
                                message: "exactly one of --value or --raw-bits is required"
                                    .to_string(),
                            })
                        }
                    };
                    let layout = binfiddle::nn::codec::layout_for_encoding(&tensor.encoding);
                    let policy = if policy == "auto" {
                        edit::EditPolicy::default_for(layout)
                    } else {
                        edit::EditPolicy::parse(policy)?
                    };
                    let plan = edit::EditPlan::build(
                        &loaded,
                        tensor,
                        &coordinate,
                        &requested,
                        policy,
                        *clamp,
                        &budget,
                    )?;
                    if let Some(path) = save_plan.as_deref() {
                        plan.save(Path::new(path))?;
                    }
                    guard.propagate(&cancel);
                    let stdout = io::stdout();
                    let mut out = stdout.lock();
                    if report_format == "json" {
                        plan.envelope()?.write_json(&mut out)?;
                    } else {
                        out.write_all(plan.text().as_bytes())?;
                    }
                    out.flush()?;
                }
                EditCommand::Apply {
                    catalog,
                    plan,
                    out_model,
                    undo_bundle,
                    report_format,
                } => {
                    let catalog_path = nn_path_arg(Some(catalog.as_str()), "edit apply")?;
                    let loaded = binfiddle::nn::Catalog::from_route(
                        catalog_path,
                        None,
                        &DiscoverOptions::default(),
                        &budget,
                    )?;
                    let loaded_plan = edit::EditPlan::load(Path::new(plan))?;
                    let receipt = edit::apply_edit_plan(
                        &loaded,
                        &loaded_plan,
                        Path::new(out_model),
                        undo_bundle.as_deref().map(Path::new),
                        &budget,
                    )?;
                    guard.propagate(&cancel);
                    let stdout = io::stdout();
                    let mut out = stdout.lock();
                    if report_format == "json" {
                        edit::receipt_envelope(&receipt)?.write_json(&mut out)?;
                    } else {
                        out.write_all(edit::receipt_text(&receipt).as_bytes())?;
                    }
                    out.flush()?;
                }
                EditCommand::Undo {
                    bundle,
                    target,
                    out_model,
                    report_format,
                } => {
                    let target_path =
                        nn_path_arg(Some(target.as_str()), "edit undo")?.ok_or_else(|| {
                            NnError::InvalidRequest {
                                message: "invalid target path".to_string(),
                            }
                        })?;
                    let receipt = edit::undo_edit(
                        Path::new(bundle),
                        target_path,
                        Path::new(out_model),
                        &budget,
                    )?;
                    guard.propagate(&cancel);
                    let stdout = io::stdout();
                    let mut out = stdout.lock();
                    if report_format == "json" {
                        edit::receipt_envelope(&receipt)?.write_json(&mut out)?;
                    } else {
                        out.write_all(edit::receipt_text(&receipt).as_bytes())?;
                    }
                    out.flush()?;
                }
            }
        }
    }

    // One final checkpoint so cancellation during output is still reported.
    guard.propagate(&cancel);
    budget.checkpoint()?;
    Ok(())
}

/// Parse a file offset for `nn locate`: decimal or 0x-prefixed hex.
fn parse_nn_offset(text: &str) -> std::result::Result<u64, NnError> {
    let trimmed = text.trim();
    let (radix, digits) = match trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        Some(hex) => (16, hex),
        None => (10, trimmed),
    };
    u64::from_str_radix(digits, radix).map_err(|_| NnError::InvalidRequest {
        message: format!("invalid offset {} (use decimal or 0x hex)", text),
    })
}

/// Convert an NN command path argument, rejecting stdin (NN commands need
/// seekable files or directories).
fn nn_path_arg<'a>(
    value: Option<&'a str>,
    command: &str,
) -> std::result::Result<Option<&'a std::path::Path>, NnError> {
    match value {
        None => Ok(None),
        Some("-") => Err(NnError::InvalidRequest {
            message: format!("nn {command} requires seekable inputs; stdin is not supported"),
        }),
        Some(path) => Ok(Some(std::path::Path::new(path))),
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Resolve the effective process-memory target pid, if any.
    let target_pid = if cli.process_self { Some(0) } else { cli.pid };
    let source_is_process_memory = target_pid.is_some();

    // Handle chain command early, before normal input loading.
    if let Some(Commands::Chain { step }) = &cli.command {
        if source_is_process_memory {
            return Err(BinfiddleError::InvalidInput(
                "--process-self and --pid cannot be used with chain".to_string(),
            ));
        }
        return binfiddle::ChainExecutor::execute(
            step,
            cli.input.as_deref(),
            cli.output.as_deref(),
            cli.silent,
        );
    }

    // Handle nn commands early: they own their own input handling and do not
    // use the binary-data loading path below.
    if let Some(Commands::Nn { command }) = &cli.command {
        if source_is_process_memory {
            eprintln!("error: invalid request: --process-self/--pid cannot be used with nn");
            std::process::exit(2);
        }
        if let Err(err) = run_nn(command, cli.input.as_deref()) {
            eprintln!("error: {err}");
            std::process::exit(err.exit_code());
        }
        return Ok(());
    }

    // Handle --list-regions before loading binary data.
    if cli.list_regions {
        let pid = target_pid.unwrap_or(0);
        let regions = binfiddle::process_memory::parse_maps(pid)?;
        print!("{}", binfiddle::process_memory::format_regions(&regions));
        return Ok(());
    }

    let command = cli.command.as_ref().ok_or_else(|| {
        BinfiddleError::InvalidInput("A subcommand is required (or use --list-regions)".to_string())
    })?;

    // Validate process-memory args.
    if source_is_process_memory {
        if cli.address.is_none() {
            return Err(BinfiddleError::InvalidInput(
                "--address is required when using --process-self or --pid".to_string(),
            ));
        }
        if cli.size.is_none() {
            return Err(BinfiddleError::InvalidInput(
                "--size is required when using --process-self or --pid".to_string(),
            ));
        }
    }

    let fill_mode = if cli.zero_fill_inaccessible {
        if !source_is_process_memory {
            return Err(BinfiddleError::InvalidInput(
                "--zero-fill-inaccessible can only be used with --process-self or --pid"
                    .to_string(),
            ));
        }
        if !matches!(command, Commands::Read { .. } | Commands::Search { .. }) {
            return Err(BinfiddleError::InvalidInput(
                "--zero-fill-inaccessible is only supported with read and search commands"
                    .to_string(),
            ));
        }
        binfiddle::process_memory::FillMode::ZeroFill
    } else if cli.skip_inaccessible {
        if !source_is_process_memory {
            return Err(BinfiddleError::InvalidInput(
                "--skip-inaccessible can only be used with --process-self or --pid".to_string(),
            ));
        }
        if !matches!(command, Commands::Read { .. }) {
            return Err(BinfiddleError::InvalidInput(
                "--skip-inaccessible is only supported with the read command".to_string(),
            ));
        }
        binfiddle::process_memory::FillMode::Skip
    } else {
        binfiddle::process_memory::FillMode::Error
    };

    // Handle streaming search before loading binary data, so huge inputs are
    // not memory-mapped or copied in full.
    if let Commands::Search {
        pattern,
        input_format,
        all,
        count,
        offsets_only,
        context,
        no_overlap,
        color,
        block_size: Some(block_size_str),
    } = command
    {
        if source_is_process_memory {
            return Err(BinfiddleError::InvalidInput(
                "--block-size cannot be used with --process-self or --pid".to_string(),
            ));
        }
        if *context > 0 {
            return Err(BinfiddleError::InvalidInput(
                "--context is not supported with --block-size streaming search".to_string(),
            ));
        }

        let block_size = parse_byte_size(block_size_str)?;

        if !cli.silent {
            let warnings = validate_search_pattern(pattern, input_format);
            for warning in warnings {
                eprintln!("{}\n", warning);
            }
        }

        let search_pattern = parse_search_pattern(pattern, input_format)?;
        let color_mode = match color.as_str() {
            "always" => binfiddle::ColorMode::Always,
            "never" => binfiddle::ColorMode::Never,
            _ => binfiddle::ColorMode::Auto,
        };

        let config = SearchConfig {
            pattern: search_pattern,
            format: cli.format.clone(),
            chunk_size: cli.chunk_size,
            find_all: *all,
            count_only: *count,
            offsets_only: *offsets_only,
            context: *context,
            no_overlap: *no_overlap,
            color: color_mode,
        };
        let search_cmd = binfiddle::SearchCommand::new(config);

        let (input, total): (Box<dyn Read>, Option<u64>) = match cli.input.as_deref() {
            Some("-") | None => (Box::new(io::stdin()), None),
            Some(path) => {
                let file = std::fs::File::open(path)?;
                let total = file.metadata().ok().map(|m| m.len());
                (Box::new(file), total)
            }
        };

        let progress = Progress::new(total, "Searching", cli.progress && !cli.silent);
        let input = ProgressReader::new(input, progress);

        let matches = search_cmd.search_stream(input, block_size)?;

        if matches.is_empty() {
            if !cli.silent {
                eprintln!("No matches found");
            }
        } else {
            let output = search_cmd.format_results(&[], &matches)?;
            if !output.is_empty() {
                println!("{}", output);
            }
        }

        return Ok(());
    }

    // Handle streaming analyze before loading binary data.
    if let Commands::Analyze {
        analysis_type,
        block_size,
        output_format,
        range,
    } = command
    {
        let block_size = parse_byte_size(block_size)?;
        if block_size > 0 {
            if source_is_process_memory {
                return Err(BinfiddleError::InvalidInput(
                    "--block-size streaming analyze cannot be used with --process-self or --pid"
                        .to_string(),
                ));
            }
            if range.is_some() {
                return Err(BinfiddleError::InvalidInput(
                    "--range is not supported with --block-size streaming analyze".to_string(),
                ));
            }

            let analysis = analysis_type.parse::<binfiddle::AnalysisType>()?;
            let format = output_format.parse::<binfiddle::AnalyzeOutputFormat>()?;
            let config = binfiddle::AnalyzeConfig {
                analysis_type: analysis,
                block_size,
                format,
                range: None,
            };
            let analyze_cmd = binfiddle::AnalyzeCommand::new(config);

            let (input, total): (Box<dyn Read>, Option<u64>) = match cli.input.as_deref() {
                Some("-") | None => (Box::new(io::stdin()), None),
                Some(path) => {
                    let file = std::fs::File::open(path)?;
                    let total = file.metadata().ok().map(|m| m.len());
                    (Box::new(file), total)
                }
            };

            let progress = Progress::new(total, "Analyzing", cli.progress && !cli.silent);
            let input = ProgressReader::new(input, progress);

            let output = analyze_cmd.analyze_stream(input)?;
            println!("{}", output);
            return Ok(());
        }
    }

    // Handle streaming hash before loading binary data.
    if let Commands::Hash {
        algorithm,
        output_format,
        block_size,
        stream,
        read_block_size,
        check: _,
    } = command
    {
        if *stream {
            if source_is_process_memory {
                return Err(BinfiddleError::InvalidInput(
                    "--stream hashing cannot be used with --process-self or --pid".to_string(),
                ));
            }

            let algorithm = algorithm.parse::<binfiddle::HashAlgorithm>()?;
            let output_format = output_format.parse::<binfiddle::HashOutputFormat>()?;
            let config = binfiddle::HashConfig {
                algorithm,
                output_format,
                block_size: *block_size,
            };
            let hash_cmd = binfiddle::HashCommand::new(config);

            let read_chunk_size = parse_byte_size(read_block_size)?;
            let (input, total): (Box<dyn Read>, Option<u64>) = match cli.input.as_deref() {
                Some("-") | None => (Box::new(io::stdin()), None),
                Some(path) => {
                    let file = std::fs::File::open(path)?;
                    let total = file.metadata().ok().map(|m| m.len());
                    (Box::new(file), total)
                }
            };

            let progress = Progress::new(total, "Hashing", cli.progress && !cli.silent);
            let input = ProgressReader::new(input, progress);

            let output = hash_cmd.compute_stream(input, read_chunk_size)?;
            println!("{}", output);
            return Ok(());
        }
    }

    // Handle checksum verification before loading binary data.
    if let Commands::Hash {
        algorithm,
        output_format,
        block_size,
        check: Some(check_file),
        read_block_size,
        ..
    } = command
    {
        if *block_size != 0 {
            return Err(BinfiddleError::InvalidInput(
                "--check requires --block-size 0 (whole file)".to_string(),
            ));
        }

        let algorithm = algorithm.parse::<binfiddle::HashAlgorithm>()?;
        let output_format = output_format.parse::<binfiddle::HashOutputFormat>()?;
        let config = binfiddle::HashConfig {
            algorithm,
            output_format,
            block_size: 0,
        };
        let hash_cmd = binfiddle::HashCommand::new(config);

        let read_chunk_size = parse_byte_size(read_block_size)?;
        let (report, ok) = hash_cmd.check(check_file.as_ref(), read_chunk_size)?;
        println!("{}", report);
        if !ok {
            return Err(BinfiddleError::ChecksumVerificationFailed);
        }
        return Ok(());
    }

    // Check if this command needs binary_data loaded
    let needs_input = matches!(
        command,
        Commands::Read { .. }
            | Commands::Write { .. }
            | Commands::Edit { .. }
            | Commands::Hash { check: None, .. }
            | Commands::Search { .. }
            | Commands::Convert { .. }
            | Commands::Analyze { .. }
    );

    // Load data only for commands that need it
    let mut binary_data = if needs_input {
        if let Some(pid) = target_pid {
            let address = parse_address_or_size(cli.address.as_deref().unwrap())?;
            let size = parse_address_or_size(cli.size.as_deref().unwrap())?;
            let source = if pid == 0 {
                BinarySource::ProcessSelf {
                    address,
                    size,
                    fill_mode,
                }
            } else {
                BinarySource::Process {
                    pid,
                    address,
                    size,
                    fill_mode,
                }
            };
            BinaryData::new(source, cli.chunk_size, cli.width)?
        } else {
            match cli.input.as_deref() {
                Some("-") | None => {
                    let mut data = Vec::new();
                    io::stdin().read_to_end(&mut data)?;
                    BinaryData::new(BinarySource::RawData(data), cli.chunk_size, cli.width)?
                }
                Some(path) => {
                    let writable_in_place =
                        matches!(command, Commands::Write { .. }) && cli.in_file;
                    let source = if writable_in_place {
                        BinarySource::WritableFile(path.into())
                    } else {
                        BinarySource::File(path.into())
                    };
                    BinaryData::new(source, cli.chunk_size, cli.width)?
                }
            }
        }
    } else {
        // Create a dummy BinaryData for commands that don't need it
        BinaryData::new(BinarySource::RawData(Vec::new()), cli.chunk_size, cli.width)?
    };

    // Execute command
    let changes_made = match command {
        Commands::Read { range } => {
            let (start, end) = binfiddle::utils::parsing::parse_range(range, binary_data.len())?;
            let chunk = binary_data.read_range(start, end)?;

            if cli.format == "raw" {
                // Raw binary output — write bytes directly to stdout
                io::stdout().write_all(chunk.get_bytes())?;
            } else if cli.show_offset || cli.show_ascii {
                // Offset-prefixed output (xxd-style)
                let output = binfiddle::utils::display::display_bytes_with_offset(
                    chunk.get_bytes(),
                    &cli.format,
                    binary_data.get_chunk_size(),
                    cli.width,
                    start, // base_offset: show actual file offset
                    cli.show_ascii,
                )?;
                println!("{}", output);
            } else {
                let output = binfiddle::utils::display::display_bytes(
                    chunk.get_bytes(),
                    &cli.format,
                    binary_data.get_chunk_size(),
                    cli.width,
                )?;
                println!("{}", output);
            }
            false
        }
        Commands::Write { position, value } => {
            let bytes = binfiddle::utils::parsing::parse_input(value, &cli.input_format)?;
            let original = binary_data.read_range(*position, Some(position + bytes.len()))?;
            binary_data.write_range(*position, &bytes)?;
            if !cli.silent {
                println!("Previous: {}", hex::encode(original.get_bytes()));
                println!("New:     {}", hex::encode(bytes));
            }
            true
        }
        Commands::Edit {
            operation,
            range,
            data,
        } => {
            let (start, end) = binfiddle::utils::parsing::parse_range(range, binary_data.len())?;
            let end = end.unwrap_or(start + 1);

            match operation.as_str() {
                "insert" => {
                    let bytes = binfiddle::utils::parsing::parse_input(
                        data.as_ref().expect("Data required for insert"),
                        &cli.input_format,
                    )?;
                    if !cli.silent {
                        println!("Inserting {} bytes at position {}", bytes.len(), start);
                    }
                    binary_data.insert_data(start, &bytes)?;
                }
                "remove" => {
                    if !cli.silent {
                        let original = binary_data.read_range(start, Some(end))?;
                        println!(
                            "Removing {} bytes from position {}:",
                            original.get_bytes().len(),
                            start
                        );
                        println!("Data removed: {}", hex::encode(original.get_bytes()));
                    }
                    binary_data.remove_range(start, end)?;
                }
                "replace" => {
                    let bytes = binfiddle::utils::parsing::parse_input(
                        data.as_ref().expect("Data required for replace"),
                        &cli.input_format,
                    )?;
                    if !cli.silent {
                        let original = binary_data.read_range(start, Some(end))?;
                        println!(
                            "Replacing {} bytes at position {}:",
                            original.get_bytes().len(),
                            start
                        );
                        println!("Previous: {}", hex::encode(original.get_bytes()));
                        println!("New:     {}", hex::encode(&bytes));
                    }
                    binary_data.remove_range(start, end)?;
                    binary_data.insert_data(start, &bytes)?;
                }
                _ => {
                    return Err(binfiddle::error::BinfiddleError::UnsupportedOperation(
                        format!("Unknown edit operation: {}", operation),
                    ))
                }
            }
            true
        }
        Commands::Search {
            pattern,
            input_format,
            all,
            count,
            offsets_only,
            context,
            no_overlap,
            color,
            block_size: _,
        } => {
            // Validate pattern and show warnings if format might be incorrect
            let warnings = validate_search_pattern(pattern, input_format);
            if !warnings.is_empty() && !cli.silent {
                for warning in warnings {
                    eprintln!("{}\n", warning);
                }
            }

            // Parse the search pattern based on input format
            let search_pattern = parse_search_pattern(pattern, input_format)?;

            // Determine color mode
            let color_mode = match color.as_str() {
                "always" => binfiddle::ColorMode::Always,
                "never" => binfiddle::ColorMode::Never,
                _ => binfiddle::ColorMode::Auto,
            };

            // Build search configuration
            let config = SearchConfig {
                pattern: search_pattern,
                format: cli.format.clone(),
                chunk_size: cli.chunk_size,
                find_all: *all,
                count_only: *count,
                offsets_only: *offsets_only,
                context: *context,
                no_overlap: *no_overlap,
                color: color_mode,
            };

            // Create and execute search command
            let search_cmd = binfiddle::SearchCommand::new(config);

            // Search directly against the backing bytes without copying the whole file.
            let bytes = binary_data.as_bytes();

            // Show a spinner for full scans, which can be slow on large files.
            let progress = if *all {
                Some(Progress::new(
                    None,
                    "Searching",
                    cli.progress && !cli.silent,
                ))
            } else {
                None
            };

            // Perform search
            let matches = search_cmd.search(bytes)?;
            if let Some(progress) = progress {
                progress.finish();
            }

            // Report results
            if matches.is_empty() {
                if !cli.silent {
                    eprintln!("No matches found");
                }
            } else {
                let output = search_cmd.format_results(bytes, &matches)?;
                println!("{}", output);
            }

            false // Search doesn't modify data
        }
        Commands::Analyze {
            analysis_type,
            block_size,
            output_format,
            range,
        } => {
            // Parse analysis type
            let analysis = analysis_type.parse::<binfiddle::AnalysisType>()?;

            // Parse output format
            let format = output_format.parse::<binfiddle::AnalyzeOutputFormat>()?;

            // Parse optional range
            let range_bounds = if let Some(range_str) = range {
                let (start, end) =
                    binfiddle::utils::parsing::parse_range(range_str, binary_data.len())?;
                Some((start, end.unwrap_or(binary_data.len())))
            } else {
                None
            };

            // Build analyze configuration
            let config = binfiddle::AnalyzeConfig {
                analysis_type: analysis,
                block_size: parse_byte_size(block_size)?,
                format,
                range: range_bounds,
            };

            // Create and execute analyze command
            let analyze_cmd = binfiddle::AnalyzeCommand::new(config.clone());

            // Analyze directly against the backing bytes without copying the whole file.
            let bytes = binary_data.as_bytes();

            // Show progress for block-based analysis; use a spinner otherwise.
            let output = if config.block_size > 0 && config.range.is_none() {
                let total = Some(bytes.len() as u64);
                let progress = Progress::new(total, "Analyzing", cli.progress && !cli.silent);
                let cursor = std::io::Cursor::new(bytes);
                let reader = ProgressReader::new(cursor, progress);
                analyze_cmd.analyze_stream(reader)?
            } else {
                let progress = Progress::new(None, "Analyzing", cli.progress && !cli.silent);
                let result = analyze_cmd.analyze(bytes)?;
                progress.finish();
                result
            };

            // Perform analysis and print results
            println!("{}", output);

            false // Analyze doesn't modify data
        }
        Commands::Hash {
            algorithm,
            output_format,
            block_size,
            stream: _,
            read_block_size: _,
            check: _,
        } => {
            let algorithm = algorithm.parse::<binfiddle::HashAlgorithm>()?;
            let output_format = output_format.parse::<binfiddle::HashOutputFormat>()?;

            let config = binfiddle::HashConfig {
                algorithm,
                output_format,
                block_size: *block_size,
            };
            let hash_cmd = binfiddle::HashCommand::new(config);

            // Hash directly against the backing bytes without copying the whole file,
            // showing a progress bar for large inputs.
            let bytes = binary_data.as_bytes();
            let total = Some(bytes.len() as u64);
            let progress = Progress::new(total, "Hashing", cli.progress && !cli.silent);
            let cursor = std::io::Cursor::new(bytes);
            let reader = ProgressReader::new(cursor, progress);
            let output = hash_cmd.compute_stream(reader, 1024 * 1024)?;
            println!("{}", output);

            false // Hash doesn't modify data
        }
        Commands::Diff {
            file1,
            file2,
            diff_format,
            context,
            color,
            ignore_offsets,
            diff_width,
            summary,
        } => {
            // Load both files
            let data1 = std::fs::read(file1)?;
            let data2 = std::fs::read(file2)?;

            // Determine color mode
            let color_mode = match color.as_str() {
                "always" => binfiddle::ColorMode::Always,
                "never" => binfiddle::ColorMode::Never,
                _ => binfiddle::ColorMode::Auto,
            };

            // Parse ignore ranges
            let ignore_ranges = binfiddle::parse_ignore_ranges(ignore_offsets)?;

            // Create diff command for comparison (with placeholder format)
            let temp_config = binfiddle::DiffConfig {
                format: binfiddle::DiffFormat::Simple,
                context: *context,
                color: color_mode,
                ignore_ranges,
                width: *diff_width,
            };
            let diff_cmd = binfiddle::DiffCommand::new(temp_config);

            // Compare files FIRST to enable auto-selection
            let differences = diff_cmd.compare(&data1, &data2);

            // Auto-select format if requested
            let format = if diff_format == "auto" {
                let max_size = data1.len().max(data2.len());
                binfiddle::DiffFormat::auto_select(differences.len(), max_size)
            } else {
                diff_format.parse::<binfiddle::DiffFormat>()?
            };

            // Warn about large diffs BEFORE outputting
            if differences.len() > 10000 && !cli.silent {
                eprintln!();
                eprintln!(
                    "⚠️  Large diff detected: {} differences ({:.1}% of file)",
                    differences.len(),
                    (differences.len() as f64 / data1.len().max(data2.len()) as f64) * 100.0
                );

                // Suggest better format if they chose simple for a large diff
                if matches!(format, binfiddle::DiffFormat::Simple) {
                    eprintln!("   Output will be very large. Consider:");
                    eprintln!("   - Use --format summary for overview");
                    eprintln!("   - Use --format unified for grouped view");
                    eprintln!();
                } else if matches!(format, binfiddle::DiffFormat::Summary) {
                    eprintln!("   Showing summary. Use --format unified for details.");
                    eprintln!();
                }
            }

            // Rebuild config with correct format
            let config = binfiddle::DiffConfig {
                format,
                context: *context,
                color: color_mode,
                ignore_ranges: binfiddle::parse_ignore_ranges(ignore_offsets)?,
                width: *diff_width,
            };
            let diff_cmd = binfiddle::DiffCommand::new(config);

            // Report results
            if differences.is_empty() {
                if !cli.silent {
                    eprintln!("Files are identical");
                }
            } else {
                let output = diff_cmd.format_diff(&data1, &data2, &differences, file1, file2)?;
                println!("{}", output);

                if *summary {
                    println!();
                    println!(
                        "{}",
                        diff_cmd.summary(&differences, data1.len(), data2.len())
                    );
                }
            }

            false // Diff doesn't modify data
        }
        Commands::Convert {
            from,
            to,
            newlines,
            bom,
            on_error,
        } => {
            // Parse configuration options
            let from_encoding = binfiddle::parse_encoding(from)?;
            let to_encoding = binfiddle::parse_encoding(to)?;
            let newline_mode = newlines.parse::<binfiddle::NewlineMode>()?;
            let bom_mode = bom.parse::<binfiddle::BomMode>()?;
            let error_mode = on_error.parse::<binfiddle::ErrorMode>()?;

            // Build configuration
            let config = binfiddle::ConvertConfig {
                from_encoding,
                to_encoding,
                newlines: newline_mode,
                bom: bom_mode,
                on_error: error_mode,
            };

            // Create and execute convert command
            let convert_cmd = binfiddle::ConvertCommand::new(config);

            // Convert directly against the backing bytes without copying the whole file.
            let bytes = binary_data.as_bytes();

            // Show a spinner because conversion can take a while on large files.
            let progress = Progress::new(None, "Converting", cli.progress && !cli.silent);

            // Perform conversion
            let converted = convert_cmd.convert(bytes)?;
            progress.finish();

            // Output the converted data
            // Convert always produces output (doesn't modify in-place via BinaryData)
            if let Some(output_path) = &cli.output {
                if output_path == "-" {
                    io::stdout().write_all(&converted)?;
                } else {
                    std::fs::write(output_path, &converted)?;
                }
            } else if cli.in_file {
                if let Some(input_path) = &cli.input {
                    std::fs::write(input_path, &converted)?;
                }
            } else {
                // Default: write to stdout
                io::stdout().write_all(&converted)?;
            }

            if !cli.silent && cli.output.is_none() && !cli.in_file {
                // If writing to stdout without explicit --output, add a note to stderr
                // (only if not silent)
            }

            false // Convert handles its own output, don't use standard mechanism
        }
        Commands::Patch {
            target,
            patch_file,
            backup,
            dry_run,
            revert,
        } => {
            // Load target file
            let target_data = std::fs::read(target)?;

            // Load patch file content
            let patch_content = if patch_file == "-" {
                let mut buf = String::new();
                io::stdin().read_to_string(&mut buf)?;
                buf
            } else {
                std::fs::read_to_string(patch_file)?
            };

            // Build configuration
            let config = binfiddle::PatchConfig {
                backup_suffix: backup.clone(),
                dry_run: *dry_run,
                revert: *revert,
            };

            // Create patch command and parse patch file
            let patch_cmd = binfiddle::PatchCommand::new(config);
            let entries = patch_cmd.parse_patch_file(&patch_content)?;

            if entries.is_empty() {
                if !cli.silent {
                    eprintln!("No patch entries found in patch file");
                }
                return Ok(());
            }

            // Create backup if requested
            if let Some(suffix) = backup {
                if !*dry_run {
                    let backup_path = binfiddle::PatchCommand::create_backup(target, suffix)?;
                    if !cli.silent {
                        eprintln!("Created backup: {}", backup_path);
                    }
                }
            }

            // Apply patches
            let (result_data, results) = patch_cmd.apply(&target_data, &entries)?;

            // Print results
            if !cli.silent {
                println!("{}", patch_cmd.format_results(&results));
            }

            // Check if all patches succeeded
            let all_success = results.iter().all(|r| r.success);

            if !*dry_run && all_success {
                // Write output
                if let Some(output_path) = &cli.output {
                    if output_path == "-" {
                        io::stdout().write_all(&result_data)?;
                    } else {
                        std::fs::write(output_path, &result_data)?;
                        if !cli.silent {
                            eprintln!("Wrote patched file to: {}", output_path);
                        }
                    }
                } else if cli.in_file {
                    std::fs::write(target, &result_data)?;
                    if !cli.silent {
                        eprintln!("Modified file in-place: {}", target);
                    }
                } else {
                    // Default: write to stdout
                    io::stdout().write_all(&result_data)?;
                }
            } else if !all_success && !*dry_run {
                eprintln!("Some patches failed - no changes written");
                std::process::exit(1);
            }

            false // Patch handles its own output
        }
        Commands::Struct {
            template,
            list_fields,
            get,
            output_format,
        } => {
            // Load template
            let struct_template = binfiddle::StructTemplate::from_file(template)?;

            // Build configuration
            let config = binfiddle::StructConfig {
                format: output_format.parse::<binfiddle::StructOutputFormat>()?,
                get_fields: get.clone(),
                list_fields: *list_fields,
            };

            let cmd = binfiddle::StructCommand::new(config);

            if *list_fields {
                // Just list fields, don't need data
                println!("{}", cmd.list_fields(&struct_template));
            } else {
                // Need to load data
                let data = match cli.input.as_deref() {
                    Some("-") | None => {
                        let mut buf = Vec::new();
                        io::stdin().read_to_end(&mut buf)?;
                        buf
                    }
                    Some(path) => std::fs::read(path)?,
                };

                // Parse structure
                let parsed = cmd.parse(&data, &struct_template)?;

                // Output based on format
                if get.len() == 1 {
                    // Single field requested - output just the value
                    if let Some(value) = cmd.get_field_value(&parsed, &get[0]) {
                        println!("{}", value);
                    } else {
                        eprintln!("Field '{}' not found in template", get[0]);
                        std::process::exit(1);
                    }
                } else {
                    // Full output
                    let output = cmd.format_output(&parsed)?;
                    println!("{}", output);
                }

                // Report assertion failures
                if !parsed.all_assertions_passed && !cli.silent {
                    eprintln!("Warning: Some field assertions failed");
                    std::process::exit(1);
                }
            }

            false // Struct handles its own output
        }
        Commands::Chain { .. } => {
            // Chain is handled before this match.
            unreachable!()
        }
        Commands::Nn { .. } => {
            // NN commands are handled before binary-data loading.
            unreachable!()
        }
    };

    // Handle output
    if changes_made {
        if source_is_process_memory {
            if !cli.allow_write {
                return Err(BinfiddleError::ProcessMemoryError(
                    "Writing to process memory requires --allow-write".to_string(),
                ));
            }

            let (pid, address, original_size) = match binary_data.source() {
                BinarySource::ProcessSelf { address, size, .. } => (0, *address, *size as usize),
                BinarySource::Process {
                    pid, address, size, ..
                } => (*pid, *address, *size as usize),
                _ => unreachable!(),
            };

            if binary_data.len() != original_size {
                return Err(BinfiddleError::ProcessMemoryError(
                    "Process memory write would change region size; insert/remove are not supported"
                        .to_string(),
                ));
            }

            binfiddle::process_memory::write_process_memory(
                pid,
                address,
                binary_data.as_bytes(),
                cli.force_writable,
            )?;
        } else if cli.in_file {
            if let Some(path) = &cli.input {
                // WritableFile has already flushed changes directly to disk.
                if !matches!(binary_data.source(), BinarySource::WritableFile(_)) {
                    std::fs::write(path, binary_data.as_bytes())?;
                }
            }
        } else if let Some(output) = &cli.output {
            if output == "-" {
                io::stdout().write_all(binary_data.as_bytes())?;
            } else {
                std::fs::write(output, binary_data.as_bytes())?;
            }
        } else if !cli.silent {
            eprintln!("Warning: Changes were made but no output specified");
            eprintln!("Use --in-file to modify input file or --output to specify output");
        }
    }

    Ok(())
}

/// Parse an address or size string that may be decimal or hex (with optional `0x` prefix).
fn parse_address_or_size(value: &str) -> Result<u64> {
    let value = value.trim();
    if value.is_empty() {
        return Err(BinfiddleError::InvalidInput(
            "Address/size cannot be empty".to_string(),
        ));
    }
    let (radix, stripped) = if let Some(stripped) = value.strip_prefix("0x") {
        (16, stripped)
    } else {
        (10, value)
    };
    u64::from_str_radix(stripped, radix).map_err(|e| {
        BinfiddleError::InvalidInput(format!("Invalid address/size '{}': {}", value, e))
    })
}

/// Parse a human-readable byte size such as `64K`, `128M`, or `2G`.
fn parse_byte_size(value: &str) -> Result<usize> {
    let value = value.trim();
    if value.is_empty() {
        return Err(BinfiddleError::InvalidInput(
            "Block size cannot be empty".to_string(),
        ));
    }

    let last = value.chars().last().unwrap();
    let multiplier = match last.to_ascii_uppercase() {
        'B' => 1usize,
        'K' => 1024usize,
        'M' => 1024 * 1024,
        'G' => 1024 * 1024 * 1024,
        _ => {
            return value.parse::<usize>().map_err(|e| {
                BinfiddleError::InvalidInput(format!("Invalid block size '{}': {}", value, e))
            });
        }
    };

    let number_part = &value[..value.len() - 1];
    if number_part.is_empty() {
        return Err(BinfiddleError::InvalidInput(format!(
            "Invalid block size '{}': missing number",
            value
        )));
    }

    let number = number_part.parse::<usize>().map_err(|e| {
        BinfiddleError::InvalidInput(format!("Invalid block size '{}': {}", value, e))
    })?;

    number
        .checked_mul(multiplier)
        .ok_or_else(|| BinfiddleError::InvalidInput(format!("Block size '{}' is too large", value)))
}
