/// src/main.rs
use binfiddle::utils::parsing::parse_search_pattern;
use binfiddle::{BinaryData, BinarySource, Result, SearchConfig};
use clap::{Parser, Subcommand};
use std::io::{self, Read, Write};

#[derive(Parser)]
#[command(name = "binfiddle")]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Input file (use '-' for stdin)
    #[arg(short, long)]
    input: Option<String>,

    /// Modify file directly (requires input file)
    #[arg(long, requires = "input", conflicts_with = "output")]
    in_file: bool,

    /// Output file (use '-' for stdout)
    #[arg(short, long)]
    output: Option<String>,

    /// Input format (hex, dec, oct, bin) for write/edit
    #[arg(long, default_value = "hex")]
    input_format: String,

    /// Output format (hex, dec, oct, bin, ascii)
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

    /// Search for patterns in binary data
    Search {
        /// Pattern to search for (interpreted per --input-format)
        pattern: String,

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
    },

    /// Analyze binary data (entropy, histogram, index of coincidence)
    Analyze {
        /// Analysis type: entropy, histogram, ic
        #[arg(value_parser = ["entropy", "histogram", "hist", "ic", "ioc"])]
        analysis_type: String,

        /// Block size for block-based analysis (0 = entire file)
        #[arg(long, default_value = "256")]
        block_size: usize,

        /// Output format: human, csv, json
        #[arg(long, default_value = "human", value_parser = ["human", "csv", "json"])]
        output_format: String,

        /// Range to analyze (format: 'start..end')
        #[arg(long)]
        range: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load data
    let mut binary_data = match cli.input.as_deref() {
        Some("-") | None => {
            let mut data = Vec::new();
            io::stdin().read_to_end(&mut data)?;
            BinaryData::new(BinarySource::RawData(data), cli.chunk_size, cli.width)?
            // Wrapped in BinarySource
        }
        Some(path) => BinaryData::new(BinarySource::File(path.into()), cli.chunk_size, cli.width)?,
    };

    // Execute command
    let changes_made = match &cli.command {
        Commands::Read { range } => {
            let (start, end) = binfiddle::utils::parsing::parse_range(range, binary_data.len())?;
            let chunk = binary_data.read_range(start, end)?;
            let output = binfiddle::utils::display::display_bytes(
                chunk.get_bytes(),
                &cli.format,
                binary_data.get_chunk_size(),
                cli.width,
            )?;
            println!("{}", output);
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
            all,
            count,
            offsets_only,
            context,
            no_overlap,
            color,
        } => {
            // Parse the search pattern based on input format
            let search_pattern = parse_search_pattern(pattern, &cli.input_format)?;

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

            // Read all data for searching
            let chunk = binary_data.read_range(0, None)?;
            let bytes = chunk.get_bytes();

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
            let analysis = binfiddle::AnalysisType::from_str(analysis_type)?;

            // Parse output format
            let format = binfiddle::AnalyzeOutputFormat::from_str(output_format)?;

            // Parse optional range
            let range_bounds = if let Some(range_str) = range {
                let (start, end) = binfiddle::utils::parsing::parse_range(range_str, binary_data.len())?;
                Some((start, end.unwrap_or(binary_data.len())))
            } else {
                None
            };

            // Build analyze configuration
            let config = binfiddle::AnalyzeConfig {
                analysis_type: analysis,
                block_size: *block_size,
                format,
                range: range_bounds,
            };

            // Create and execute analyze command
            let analyze_cmd = binfiddle::AnalyzeCommand::new(config);

            // Read all data for analysis
            let chunk = binary_data.read_range(0, None)?;
            let bytes = chunk.get_bytes();

            // Perform analysis and print results
            let output = analyze_cmd.analyze(bytes)?;
            println!("{}", output);

            false // Analyze doesn't modify data
        }
    };

    // Handle output
    if changes_made {
        if cli.in_file {
            if let Some(path) = &cli.input {
                std::fs::write(path, binary_data.read_range(0, None)?.get_bytes())?;
            }
        } else if let Some(output) = &cli.output {
            if output == "-" {
                io::stdout().write_all(binary_data.read_range(0, None)?.get_bytes())?;
            } else {
                std::fs::write(output, binary_data.read_range(0, None)?.get_bytes())?;
            }
        } else if !cli.silent {
            eprintln!("Warning: Changes were made but no output specified");
            eprintln!("Use --in-file to modify input file or --output to specify output");
        }
    }

    Ok(())
}
