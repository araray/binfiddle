//! Benchmarks for binfiddle search operations.
//!
//! Run with: `cargo bench`

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use binfiddle::commands::SearchCommand;
use binfiddle::utils::parsing::SearchPattern;
use binfiddle::{ColorMode, SearchConfig};

/// Generate test data of a specific size with embedded patterns.
fn generate_test_data(size: usize, pattern_interval: usize) -> Vec<u8> {
    let mut data = vec![0u8; size];
    // Fill with pseudo-random data
    for (i, byte) in data.iter_mut().enumerate() {
        *byte = ((i * 17 + 31) % 256) as u8;
    }
    // Insert pattern at regular intervals
    let pattern = [0xDE, 0xAD, 0xBE, 0xEF];
    for offset in (0..size).step_by(pattern_interval) {
        if offset + pattern.len() <= size {
            data[offset..offset + pattern.len()].copy_from_slice(&pattern);
        }
    }
    data
}

fn make_search_config(pattern: SearchPattern, find_all: bool) -> SearchConfig {
    SearchConfig {
        pattern,
        format: "hex".to_string(),
        chunk_size: 8,
        find_all,
        count_only: false,
        offsets_only: true, // Faster output
        context: 0,
        no_overlap: true,
        color: ColorMode::Never,
    }
}

fn bench_exact_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("exact_search");

    // Test different data sizes
    for size in [1024, 10 * 1024, 100 * 1024, 1024 * 1024, 10 * 1024 * 1024].iter() {
        let data = generate_test_data(*size, 1024);

        group.throughput(Throughput::Bytes(*size as u64));
        group.bench_with_input(BenchmarkId::new("sequential", size), &data, |b, data| {
            let config = make_search_config(
                SearchPattern::Exact(vec![0xDE, 0xAD, 0xBE, 0xEF]),
                true,
            );
            let cmd = SearchCommand::new(config);
            b.iter(|| {
                let matches = cmd.search(black_box(data)).unwrap();
                black_box(matches.len())
            });
        });
    }
    group.finish();
}

fn bench_mask_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("mask_search");

    // Test different data sizes
    for size in [1024, 10 * 1024, 100 * 1024, 1024 * 1024].iter() {
        let data = generate_test_data(*size, 1024);

        group.throughput(Throughput::Bytes(*size as u64));
        group.bench_with_input(BenchmarkId::new("sequential", size), &data, |b, data| {
            // Pattern: DE ?? BE EF (with wildcard)
            let config = make_search_config(
                SearchPattern::Mask(vec![Some(0xDE), None, Some(0xBE), Some(0xEF)]),
                true,
            );
            let cmd = SearchCommand::new(config);
            b.iter(|| {
                let matches = cmd.search(black_box(data)).unwrap();
                black_box(matches.len())
            });
        });
    }
    group.finish();
}

fn bench_regex_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("regex_search");

    // Test different data sizes (smaller for regex due to overhead)
    for size in [1024, 10 * 1024, 100 * 1024].iter() {
        let data = generate_test_data(*size, 1024);

        group.throughput(Throughput::Bytes(*size as u64));
        group.bench_with_input(BenchmarkId::new("sequential", size), &data, |b, data| {
            // Pattern: Match DEADBEEF as regex
            let config = make_search_config(
                SearchPattern::Regex(r"\xDE\xAD\xBE\xEF".to_string()),
                true,
            );
            let cmd = SearchCommand::new(config);
            b.iter(|| {
                let matches = cmd.search(black_box(data)).unwrap();
                black_box(matches.len())
            });
        });
    }
    group.finish();
}

fn bench_parallel_vs_sequential(c: &mut Criterion) {
    let mut group = c.benchmark_group("parallel_comparison");

    // Only test larger sizes where parallelism matters
    for size in [1024 * 1024, 10 * 1024 * 1024].iter() {
        let data = generate_test_data(*size, 1024);

        group.throughput(Throughput::Bytes(*size as u64));

        // Sequential exact search
        group.bench_with_input(
            BenchmarkId::new("exact_sequential", size),
            &data,
            |b, data| {
                let config = make_search_config(
                    SearchPattern::Exact(vec![0xDE, 0xAD, 0xBE, 0xEF]),
                    true,
                );
                let cmd = SearchCommand::new(config);
                b.iter(|| {
                    let matches = cmd.search(black_box(data)).unwrap();
                    black_box(matches.len())
                });
            },
        );

        // Parallel exact search
        group.bench_with_input(
            BenchmarkId::new("exact_parallel", size),
            &data,
            |b, data| {
                let config = make_search_config(
                    SearchPattern::Exact(vec![0xDE, 0xAD, 0xBE, 0xEF]),
                    true,
                );
                let cmd = SearchCommand::new(config);
                b.iter(|| {
                    let matches = cmd.search_parallel(black_box(data)).unwrap();
                    black_box(matches.len())
                });
            },
        );

        // Sequential mask search
        group.bench_with_input(
            BenchmarkId::new("mask_sequential", size),
            &data,
            |b, data| {
                let config = make_search_config(
                    SearchPattern::Mask(vec![Some(0xDE), None, Some(0xBE), Some(0xEF)]),
                    true,
                );
                let cmd = SearchCommand::new(config);
                b.iter(|| {
                    let matches = cmd.search(black_box(data)).unwrap();
                    black_box(matches.len())
                });
            },
        );

        // Parallel mask search
        group.bench_with_input(
            BenchmarkId::new("mask_parallel", size),
            &data,
            |b, data| {
                let config = make_search_config(
                    SearchPattern::Mask(vec![Some(0xDE), None, Some(0xBE), Some(0xEF)]),
                    true,
                );
                let cmd = SearchCommand::new(config);
                b.iter(|| {
                    let matches = cmd.search_parallel(black_box(data)).unwrap();
                    black_box(matches.len())
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_exact_search,
    bench_mask_search,
    bench_regex_search,
    bench_parallel_vs_sequential,
);
criterion_main!(benches);
