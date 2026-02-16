use geo::{Coord, Haversine, Line, LineString, MultiLineString, Point};
use geo::{Distance, Length};
use indicatif::ProgressBar;
use rayon::prelude::*;
use rstar::{PointDistance, RTree, RTreeObject, AABB};

/// A single OSM line segment stored in the R-tree.
#[derive(Debug, Clone)]
pub struct IndexedSegment {
    pub line: Line<f64>,
}

impl IndexedSegment {
    pub fn new(start: Coord<f64>, end: Coord<f64>) -> Self {
        Self {
            line: Line::new(start, end),
        }
    }
}

impl RTreeObject for IndexedSegment {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        let min_x = self.line.start.x.min(self.line.end.x);
        let max_x = self.line.start.x.max(self.line.end.x);
        let min_y = self.line.start.y.min(self.line.end.y);
        let max_y = self.line.start.y.max(self.line.end.y);
        AABB::from_corners([min_x, min_y], [max_x, max_y])
    }
}

impl PointDistance for IndexedSegment {
    fn distance_2(&self, point: &[f64; 2]) -> f64 {
        let coord = Coord {
            x: point[0],
            y: point[1],
        };
        let dist = haversine_point_to_segment(coord, self.line.start, self.line.end);
        // rstar expects squared distance for ordering, but since we use haversine
        // (not Euclidean), we square the result to maintain correct ordering.
        dist * dist
    }
}

/// Compute haversine distance from a point to a line segment.
/// Projects the point onto the segment and returns distance to the closest point.
fn haversine_point_to_segment(point: Coord<f64>, seg_start: Coord<f64>, seg_end: Coord<f64>) -> f64 {
    let p = Point::from(point);
    let a = Point::from(seg_start);

    let ab_len = Line::new(seg_start, seg_end).length::<Haversine>();

    if ab_len < 1e-10 {
        return Haversine::distance(p, a);
    }

    // Project point onto the line using a linear approximation in lat/lon space.
    // This is sufficiently accurate for short segments (< ~1km).
    let dx = seg_end.x - seg_start.x;
    let dy = seg_end.y - seg_start.y;
    let t = ((point.x - seg_start.x) * dx + (point.y - seg_start.y) * dy) / (dx * dx + dy * dy);
    let t = t.clamp(0.0, 1.0);

    let closest = Coord {
        x: seg_start.x + t * dx,
        y: seg_start.y + t * dy,
    };

    Haversine::distance(p, Point::from(closest))
}

/// Build an R-tree index from OSM linestrings.
pub fn build_index(osm_lines: &[LineString<f64>], progress: Option<&ProgressBar>) -> RTree<IndexedSegment> {
    let segments: Vec<IndexedSegment> = osm_lines
        .par_iter()
        .flat_map_iter(|ls| {
            ls.lines()
                .map(|line| IndexedSegment::new(line.start, line.end))
        })
        .collect();
    if let Some(pb) = progress {
        pb.set_message(format!("Building R-tree from {} segments...", segments.len()));
    }
    RTree::bulk_load(segments)
}

/// A section of the PCTA trail with its name and geometry.
#[derive(Debug, Clone)]
pub struct PctaSection {
    pub section_name: String,
    pub geometry: MultiLineString<f64>,
}

/// A detected divergence between the PCTA and OSM data.
#[derive(Debug, Clone)]
pub struct Divergence {
    pub pcta_segment: LineString<f64>,
    pub section_name: String,
    pub max_distance_m: f64,
    pub mean_distance_m: f64,
    pub length_m: f64,
}

/// Sample points along a linestring at regular intervals using haversine interpolation.
fn sample_along(ls: &LineString<f64>, interval_m: f64) -> Vec<Coord<f64>> {
    if ls.0.len() < 2 {
        return ls.0.clone();
    }

    let mut samples = vec![ls.0[0]];
    let mut remaining = interval_m;

    for line in ls.lines() {
        let seg_len = Haversine::distance(Point::from(line.start), Point::from(line.end));
        if seg_len < 1e-10 {
            continue;
        }

        let mut offset = remaining;
        while offset <= seg_len {
            let t = offset / seg_len;
            let coord = Coord {
                x: line.start.x + t * (line.end.x - line.start.x),
                y: line.start.y + t * (line.end.y - line.start.y),
            };
            samples.push(coord);
            offset += interval_m;
        }
        remaining = offset - seg_len;
    }

    // Always include the last point
    if let Some(&last) = ls.0.last() {
        if samples.last() != Some(&last) {
            samples.push(last);
        }
    }

    samples
}

/// Process a single linestring to find divergent segments.
fn process_linestring(
    ls: &LineString<f64>,
    section_name: &str,
    osm_index: &RTree<IndexedSegment>,
    threshold_m: f64,
    min_length_m: f64,
    sample_interval_m: f64,
) -> Vec<Divergence> {
    let samples = sample_along(ls, sample_interval_m);
    if samples.is_empty() {
        return Vec::new();
    }

    // Compute distances for each sample in parallel
    let distances: Vec<(Coord<f64>, f64)> = samples
        .par_iter()
        .map(|&coord| {
            let point = [coord.x, coord.y];
            let dist = osm_index
                .nearest_neighbor(&point)
                .map(|seg| haversine_point_to_segment(coord, seg.line.start, seg.line.end))
                .unwrap_or(f64::MAX);
            (coord, dist)
        })
        .collect();

    // State machine to detect contiguous divergent runs (sequential — order-dependent)
    let mut divergences = Vec::new();
    let mut run_start: Option<usize> = None;

    for i in 0..=distances.len() {
        let divergent = i < distances.len() && distances[i].1 > threshold_m;

        if divergent && run_start.is_none() {
            run_start = Some(i);
        } else if !divergent && run_start.is_some() {
            let start = run_start.unwrap();
            let run = &distances[start..i];
            emit_divergence(run, section_name, min_length_m, &mut divergences);
            run_start = None;
        }
    }

    divergences
}

/// Find divergent segments between PCTA sections and the OSM index.
pub fn find_divergences(
    pcta_sections: &[PctaSection],
    osm_index: &RTree<IndexedSegment>,
    threshold_m: f64,
    min_length_m: f64,
    sample_interval_m: f64,
    progress: Option<&ProgressBar>,
) -> Vec<Divergence> {
    pcta_sections
        .par_iter()
        .flat_map_iter(|section| {
            let divs: Vec<Divergence> = section
                .geometry
                .0
                .iter()
                .flat_map(|ls| {
                    process_linestring(
                        ls,
                        &section.section_name,
                        osm_index,
                        threshold_m,
                        min_length_m,
                        sample_interval_m,
                    )
                })
                .collect();
            if let Some(pb) = progress {
                pb.inc(1);
            }
            divs
        })
        .collect()
}

fn emit_divergence(
    run: &[(Coord<f64>, f64)],
    section_name: &str,
    min_length_m: f64,
    divergences: &mut Vec<Divergence>,
) {
    let coords: Vec<Coord<f64>> = run.iter().map(|(c, _)| *c).collect();
    let ls = LineString::from(coords);
    let length = ls.length::<Haversine>();

    if length < min_length_m {
        return;
    }

    let max_distance_m = run.iter().map(|(_, d)| *d).fold(0.0_f64, f64::max);
    let mean_distance_m = run.iter().map(|(_, d)| *d).sum::<f64>() / run.len() as f64;

    divergences.push(Divergence {
        pcta_segment: ls,
        section_name: section_name.to_string(),
        max_distance_m,
        mean_distance_m,
        length_m: length,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{Coord, LineString, MultiLineString};

    /// Helper: create a straight horizontal linestring from (lon, lat) for `n` points
    /// spaced roughly `spacing_deg` apart along the x-axis at the given latitude.
    fn horizontal_line(start_lon: f64, lat: f64, n: usize, spacing_deg: f64) -> LineString<f64> {
        let coords: Vec<Coord<f64>> = (0..n)
            .map(|i| Coord {
                x: start_lon + i as f64 * spacing_deg,
                y: lat,
            })
            .collect();
        LineString::from(coords)
    }

    fn make_section(name: &str, ls: LineString<f64>) -> PctaSection {
        PctaSection {
            section_name: name.to_string(),
            geometry: MultiLineString::new(vec![ls]),
        }
    }

    #[test]
    fn identical_lines_no_divergences() {
        let line = horizontal_line(-118.0, 34.0, 100, 0.001);
        let osm_lines = vec![line.clone()];
        let index = build_index(&osm_lines, None);
        let sections = vec![make_section("Test", line)];

        let divs = find_divergences(&sections, &index, 10.0, 500.0, 25.0, None);
        assert!(divs.is_empty(), "Identical lines should produce no divergences");
    }

    #[test]
    fn parallel_lines_below_threshold() {
        // ~50m apart at 34°N latitude: 50m / 111320m per degree ≈ 0.000449 degrees
        let pcta_line = horizontal_line(-118.0, 34.0, 200, 0.001);
        let osm_line = horizontal_line(-118.0, 34.0 + 0.000449, 200, 0.001);
        let index = build_index(&vec![osm_line], None);
        let sections = vec![make_section("Test", pcta_line)];

        // threshold 100m, these are ~50m apart
        let divs = find_divergences(&sections, &index, 100.0, 500.0, 25.0, None);
        assert!(divs.is_empty(), "Lines ~50m apart should not diverge at 100m threshold");
    }

    #[test]
    fn parallel_lines_above_threshold() {
        // ~200m apart: 200m / 111320 ≈ 0.001797 degrees
        let pcta_line = horizontal_line(-118.0, 34.0, 500, 0.0005);
        let osm_line = horizontal_line(-118.0, 34.0 + 0.001797, 500, 0.0005);
        let index = build_index(&vec![osm_line], None);
        let sections = vec![make_section("Test", pcta_line)];

        let divs = find_divergences(&sections, &index, 100.0, 500.0, 25.0, None);
        assert!(!divs.is_empty(), "Lines ~200m apart should diverge at 100m threshold");
        assert_eq!(divs.len(), 1);
        assert!(divs[0].max_distance_m > 100.0);
    }

    #[test]
    fn diverge_and_reconverge() {
        // PCTA goes straight, OSM detours in the middle
        let mut pcta_coords: Vec<Coord<f64>> = Vec::new();
        let mut osm_coords: Vec<Coord<f64>> = Vec::new();

        for i in 0..300 {
            let lon = -118.0 + i as f64 * 0.0003;
            pcta_coords.push(Coord { x: lon, y: 34.0 });

            // OSM diverges in the middle segment (100..200)
            let lat_offset = if (100..200).contains(&i) {
                0.002 // ~222m
            } else {
                0.0
            };
            osm_coords.push(Coord {
                x: lon,
                y: 34.0 + lat_offset,
            });
        }

        let pcta_line = LineString::from(pcta_coords);
        let osm_line = LineString::from(osm_coords);
        let index = build_index(&vec![osm_line], None);
        let sections = vec![make_section("Test", pcta_line)];

        let divs = find_divergences(&sections, &index, 100.0, 500.0, 25.0, None);
        assert!(!divs.is_empty(), "Should detect the middle divergence");
    }

    #[test]
    fn short_divergence_filtered_out() {
        // Very short divergence (< 500m min_length)
        let mut pcta_coords: Vec<Coord<f64>> = Vec::new();
        let mut osm_coords: Vec<Coord<f64>> = Vec::new();

        for i in 0..100 {
            let lon = -118.0 + i as f64 * 0.0001;
            pcta_coords.push(Coord { x: lon, y: 34.0 });

            // OSM diverges for just 5 points (~50m)
            let lat_offset = if (45..50).contains(&i) {
                0.002
            } else {
                0.0
            };
            osm_coords.push(Coord {
                x: lon,
                y: 34.0 + lat_offset,
            });
        }

        let pcta_line = LineString::from(pcta_coords);
        let osm_line = LineString::from(osm_coords);
        let index = build_index(&vec![osm_line], None);
        let sections = vec![make_section("Test", pcta_line)];

        let divs = find_divergences(&sections, &index, 100.0, 500.0, 25.0, None);
        assert!(divs.is_empty(), "Short divergence should be filtered out");
    }

    #[test]
    fn multiple_divergences() {
        let mut pcta_coords: Vec<Coord<f64>> = Vec::new();
        let mut osm_coords: Vec<Coord<f64>> = Vec::new();

        for i in 0..600 {
            let lon = -118.0 + i as f64 * 0.0003;
            pcta_coords.push(Coord { x: lon, y: 34.0 });

            // Two divergent sections
            let lat_offset = if (50..150).contains(&i) || (300..400).contains(&i) {
                0.002
            } else {
                0.0
            };
            osm_coords.push(Coord {
                x: lon,
                y: 34.0 + lat_offset,
            });
        }

        let pcta_line = LineString::from(pcta_coords);
        let osm_line = LineString::from(osm_coords);
        let index = build_index(&vec![osm_line], None);
        let sections = vec![make_section("Test", pcta_line)];

        let divs = find_divergences(&sections, &index, 100.0, 500.0, 25.0, None);
        assert!(divs.len() >= 2, "Should detect two separate divergences, got {}", divs.len());
    }

    #[test]
    fn empty_inputs_no_panics() {
        let index = build_index(&[], None);
        let sections: Vec<PctaSection> = vec![];
        let divs = find_divergences(&sections, &index, 100.0, 500.0, 25.0, None);
        assert!(divs.is_empty());

        // Non-empty sections but empty index
        let line = horizontal_line(-118.0, 34.0, 10, 0.001);
        let sections = vec![make_section("Test", line)];
        let index = build_index(&[], None);
        let divs = find_divergences(&sections, &index, 100.0, 500.0, 25.0, None);
        // With empty index, every point has MAX distance, so everything is divergent
        // This should not panic
        let _ = divs;
    }

    #[test]
    fn sample_along_basic() {
        let line = horizontal_line(-118.0, 34.0, 50, 0.001);
        let samples = sample_along(&line, 25.0);
        assert!(samples.len() > 2, "Should produce multiple samples");
        assert_eq!(samples[0], line.0[0], "First sample should be first coord");
        assert_eq!(samples.last(), line.0.last(), "Last sample should be last coord");
    }
}
