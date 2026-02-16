use crate::compare::Divergence;
use geojson::{Feature, FeatureCollection, GeoJson, Geometry, Value};
use serde_json::json;

/// Convert divergences to a GeoJSON FeatureCollection.
pub fn to_geojson(divergences: &[Divergence]) -> GeoJson {
    let features: Vec<Feature> = divergences.iter().map(divergence_to_feature).collect();

    GeoJson::FeatureCollection(FeatureCollection {
        bbox: None,
        features,
        foreign_members: None,
    })
}

fn divergence_to_feature(div: &Divergence) -> Feature {
    let coords: Vec<Vec<f64>> = div
        .pcta_segment
        .0
        .iter()
        .map(|c| vec![c.x, c.y])
        .collect();

    Feature {
        bbox: None,
        geometry: Some(Geometry::new(Value::LineString(coords))),
        id: None,
        properties: Some(
            json!({
                "section_name": div.section_name,
                "max_distance_m": (div.max_distance_m * 10.0).round() / 10.0,
                "mean_distance_m": (div.mean_distance_m * 10.0).round() / 10.0,
                "length_m": (div.length_m * 10.0).round() / 10.0,
            })
            .as_object()
            .unwrap()
            .clone(),
        ),
        foreign_members: None,
    }
}
