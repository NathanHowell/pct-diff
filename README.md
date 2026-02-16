# pct-diff

Find sections of the Pacific Crest Trail where the [PCTA](https://www.pcta.org/) published centerline diverges from [OpenStreetMap](https://www.openstreetmap.org/), indicating potential reroutes that haven't been mapped yet.

## How it works

1. Loads the PCTA trail geometry from a GDB zip file (via GDAL)
2. Fetches the OSM PCT relation and all sub-relations (cached locally)
3. Builds an R-tree spatial index of OSM trail segments
4. Samples points along each PCTA section and finds the nearest OSM segment using haversine distance
5. Detects contiguous runs where the distance exceeds a threshold
6. Outputs divergent segments as GeoJSON

## Requirements

- Rust 1.85+
- GDAL (install via `brew install gdal` on macOS)
- `Full_PCT.gdb.zip` from the [PCTA GIS data](https://www.pcta.org/discover-the-trail/maps/pct-data/)

## Usage

```
cargo run --release
```

### Options

| Flag | Default | Description |
|------|---------|-------------|
| `--pcta` | `Full_PCT.gdb.zip` | Path to the PCTA GDB zip file |
| `--relation` | `1225378` | OSM relation ID for the PCT |
| `--threshold` | `10.0` | Minimum distance (meters) to count as divergence |
| `--min-length` | `500.0` | Minimum divergent segment length (meters) |
| `--sample-interval` | `25.0` | Distance between sample points (meters) |
| `--output` | `divergences.geojson` | Output GeoJSON path |
| `--cache-dir` | `.cache` | Cache directory for OSM API responses |

## Output

The output GeoJSON contains one feature per divergent segment, with properties including section name, segment length, and max/mean distance from OSM.

## License

MIT
