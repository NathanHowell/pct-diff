use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

use pct_diff::compare::{build_index, find_divergences};
use pct_diff::osm::fetch_relation_ways;
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

    eprintln!("Loading PCTA data from {}...", cli.pcta.display());
    let pcta_sections = load_pcta_gdb(&cli.pcta)?;

    eprintln!("Fetching OSM relation {}...", cli.relation);
    let osm_lines = fetch_relation_ways(cli.relation, &cli.cache_dir)?;

    eprintln!("Building spatial index ({} OSM segments)...", osm_lines.len());
    let index = build_index(&osm_lines);

    eprintln!("Comparing geometries...");
    let divergences = find_divergences(
        &pcta_sections,
        &index,
        cli.threshold,
        cli.min_length,
        cli.sample_interval,
    );

    eprintln!("Found {} divergent segments", divergences.len());
    for d in &divergences {
        eprintln!(
            "  {} - {:.0}m long, max {:.0}m, mean {:.0}m off",
            d.section_name, d.length_m, d.max_distance_m, d.mean_distance_m
        );
    }

    let geojson = to_geojson(&divergences);
    let json = serde_json::to_string_pretty(&geojson)?;
    std::fs::write(&cli.output, json)?;
    eprintln!("Wrote {}", cli.output.display());

    Ok(())
}
