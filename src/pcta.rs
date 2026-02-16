use anyhow::{Context, Result};
use gdal::vector::LayerAccess;
use gdal::Dataset;
use geo::{Coord, LineString, MultiLineString};
use std::path::Path;

use crate::compare::PctaSection;

/// Load PCTA sections from a GDB zip file using GDAL's /vsizip/ virtual filesystem.
pub fn load_pcta_gdb(path: &Path) -> Result<Vec<PctaSection>> {
    let vsi_path = format!("/vsizip/{}", path.canonicalize()?.display());
    let dataset = Dataset::open(&vsi_path).context("Failed to open PCTA GDB via GDAL")?;

    let mut layer = dataset
        .layer(0)
        .context("Failed to get first layer from GDB")?;

    let mut sections = Vec::new();

    for feature in layer.features() {
        // Try common field names for section identification
        let section_name = feature
            .field_index("Section")
            .or_else(|_| feature.field_index("SECTION"))
            .or_else(|_| feature.field_index("Name"))
            .or_else(|_| feature.field_index("NAME"))
            .ok()
            .and_then(|idx| feature.field_as_string(idx).ok().flatten())
            .unwrap_or_else(|| "Unknown".to_string());

        let Some(geom) = feature.geometry() else {
            continue;
        };

        let mls = gdal_geom_to_multilinestring(geom);
        if mls.0.is_empty() {
            continue;
        }

        sections.push(PctaSection {
            section_name,
            geometry: mls,
        });
    }

    Ok(sections)
}

fn gdal_geom_to_multilinestring(geom: &gdal::vector::Geometry) -> MultiLineString<f64> {
    let mut linestrings = Vec::new();
    collect_linestrings(geom, &mut linestrings);
    MultiLineString::new(linestrings)
}

fn collect_linestrings(geom: &gdal::vector::Geometry, out: &mut Vec<LineString<f64>>) {
    let name = geom.geometry_name();
    match name.as_str() {
        "LINESTRING" => {
            if let Some(ls) = extract_linestring(geom) {
                out.push(ls);
            }
        }
        "MULTILINESTRING" | "GEOMETRYCOLLECTION" => {
            for i in 0..geom.geometry_count() {
                let child = geom.get_geometry(i);
                collect_linestrings(&child, out);
            }
        }
        _ => {}
    }
}

fn extract_linestring(geom: &gdal::vector::Geometry) -> Option<LineString<f64>> {
    let n = geom.point_count();
    if n < 2 {
        return None;
    }

    let coords: Vec<Coord<f64>> = (0..n as i32)
        .map(|i| {
            let (x, y, _z) = geom.get_point(i);
            Coord { x, y }
        })
        .collect();

    Some(LineString::from(coords))
}
