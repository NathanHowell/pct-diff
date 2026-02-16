use anyhow::Result;
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;
use std::time::Duration;

use pct_diff::compare::{build_index, find_divergences};
use pct_diff::osm::{fetch_relation_ways, FetchProgress};
use pct_diff::output::to_geojson;
use pct_diff::pcta::load_pcta_gdb;

#[derive(Parser)]
#[command(about = "Find PCTA reroutes not yet in OpenStreetMap")]
struct Cli {
    /// Path to Full_PCT.gdb.zip
    #[arg(long, default_value = "Full_PCT.gdb.zip")]
    pcta: PathBuf,

    /// OSM relation ID for the PCT
    #[arg(long, default_value_t = 1225378)]
    relation: u64,

    /// Minimum distance (meters) to count as divergence
    #[arg(long, default_value_t = 10.0)]
    threshold: f64,

    /// Minimum divergent segment length (meters)
    #[arg(long, default_value_t = 500.0)]
    min_length: f64,

    /// Distance between sample points (meters)
    #[arg(long, default_value_t = 25.0)]
    sample_interval: f64,

    /// Output GeoJSON path
    #[arg(long, default_value = "divergences.geojson")]
    output: PathBuf,

    /// Cache directory for OSM data
    #[arg(long, default_value = ".cache")]
    cache_dir: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let spinner_style = ProgressStyle::with_template("{spinner:.cyan} {msg}").unwrap();
    let bar_style = ProgressStyle::with_template("{spinner:.cyan} {msg} [{bar:40}] {pos}/{len}")
        .unwrap()
        .progress_chars("=> ");

    // Load PCTA data
    let pb = ProgressBar::new_spinner();
    pb.set_style(spinner_style.clone());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_message(format!("Loading PCTA data from {}...", cli.pcta.display()));
    let pcta_sections = load_pcta_gdb(&cli.pcta)?;
    pb.finish_with_message(format!("Loaded {} PCTA sections", pcta_sections.len()));

    // Fetch OSM data
    let pb = ProgressBar::new(0);
    pb.set_style(bar_style.clone());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_message(format!("Fetching OSM relation {}...", cli.relation));
    let osm_lines = fetch_relation_ways(cli.relation, &cli.cache_dir, Some(&|event| match event {
        FetchProgress::SubRelationsFound(count) => pb.set_length(count as u64),
        FetchProgress::SubRelationFetched(_) => pb.inc(1),
    }))?;
    pb.finish_with_message(format!("Fetched {} OSM ways", osm_lines.len()));

    // Build spatial index
    let pb = ProgressBar::new_spinner();
    pb.set_style(spinner_style);
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_message("Building spatial index...");
    let index = build_index(&osm_lines, Some(&pb));
    pb.finish_with_message("Spatial index built");

    // Find divergences
    let pb = ProgressBar::new(pcta_sections.len() as u64);
    pb.set_style(bar_style);
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_message("Comparing geometries...");
    let divergences = find_divergences(
        &pcta_sections,
        &index,
        cli.threshold,
        cli.min_length,
        cli.sample_interval,
        Some(&pb),
    );
    pb.finish_with_message(format!("Found {} divergent segments", divergences.len()));

    // Results summary
    for d in &divergences {
        println!(
            "  {} - {:.0}m long, max {:.0}m, mean {:.0}m off",
            d.section_name, d.length_m, d.max_distance_m, d.mean_distance_m
        );
    }

    let geojson = to_geojson(&divergences);
    let json = serde_json::to_string_pretty(&geojson)?;
    std::fs::write(&cli.output, json)?;
    println!("Wrote {}", cli.output.display());

    Ok(())
}
