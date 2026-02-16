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

    let mut sections = Vec::new();

    for layer_idx in 0..dataset.layer_count() {
        let mut layer = dataset
            .layer(layer_idx)
            .context("Failed to get layer from GDB")?;

        for feature in layer.features() {
            let section_name = ["Section_Name", "Section", "SECTION", "Name", "NAME"]
                .iter()
                .find_map(|&name| feature.field_index(name).ok())
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
    if name.starts_with("LINESTRING") {
        if let Some(ls) = extract_linestring(geom) {
            out.push(ls);
        }
    } else if name.starts_with("MULTILINESTRING")
        || name.starts_with("GEOMETRYCOLLECTION")
        || name.starts_with("MULTICURVE")
        || name.starts_with("COMPOUNDCURVE")
    {
        for i in 0..geom.geometry_count() {
            let child = geom.get_geometry(i);
            collect_linestrings(&child, out);
        }
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
