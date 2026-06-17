/// src/main.rs
use binfiddle::utils::parsing::{parse_search_pattern, validate_search_pattern};
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
        /// Hash algorithm: md5, sha256, blake3, crc32
        algorithm: String,

        /// Output format: hex
        #[arg(long, default_value = "hex", value_parser = ["hex"])]
        output_format: String,

        /// Block size for block-based hashing (0 = whole file)
        #[arg(long, default_value = "0")]
        block_size: usize,
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

        let input: Box<dyn Read> = match cli.input.as_deref() {
            Some("-") | None => Box::new(io::stdin()),
            Some(path) => Box::new(std::fs::File::open(path)?),
        };

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

            let input: Box<dyn Read> = match cli.input.as_deref() {
                Some("-") | None => Box::new(io::stdin()),
                Some(path) => Box::new(std::fs::File::open(path)?),
            };

            let output = analyze_cmd.analyze_stream(input)?;
            println!("{}", output);
            return Ok(());
        }
    }

    // Check if this command needs binary_data loaded
    let needs_input = matches!(
        command,
        Commands::Read { .. }
            | Commands::Write { .. }
            | Commands::Edit { .. }
            | Commands::Hash { .. }
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

            // Perform search
            let matches = search_cmd.search(bytes)?;

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
            let analyze_cmd = binfiddle::AnalyzeCommand::new(config);

            // Analyze directly against the backing bytes without copying the whole file.
            let bytes = binary_data.as_bytes();

            // Perform analysis and print results
            let output = analyze_cmd.analyze(bytes)?;
            println!("{}", output);

            false // Analyze doesn't modify data
        }
        Commands::Hash {
            algorithm,
            output_format,
            block_size,
        } => {
            let algorithm = algorithm.parse::<binfiddle::HashAlgorithm>()?;
            let output_format = output_format.parse::<binfiddle::HashOutputFormat>()?;

            let config = binfiddle::HashConfig {
                algorithm,
                output_format,
                block_size: *block_size,
            };
            let hash_cmd = binfiddle::HashCommand::new(config);

            // Hash directly against the backing bytes without copying the whole file.
            let bytes = binary_data.as_bytes();
            let output = hash_cmd.compute(bytes)?;
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

            // Perform conversion
            let converted = convert_cmd.convert(bytes)?;

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
