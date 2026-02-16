use anyhow::{Context, Result};
use geo::{Coord, LineString};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

const OSM_API_BASE: &str = "https://api.openstreetmap.org/api/0.6";

pub enum FetchProgress {
    SubRelationsFound(usize),
    SubRelationFetched(u64),
}

#[derive(Debug, Deserialize)]
struct RelationResponse {
    elements: Vec<Element>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum Element {
    #[serde(rename = "node")]
    Node {
        id: u64,
        lat: Option<f64>,
        lon: Option<f64>,
    },
    #[serde(rename = "way")]
    Way {
        #[allow(dead_code)]
        id: u64,
        nodes: Option<Vec<u64>>,
        #[allow(dead_code)]
        tags: Option<serde_json::Value>,
    },
    #[serde(rename = "relation")]
    Relation {
        #[allow(dead_code)]
        id: u64,
        members: Option<Vec<RelationMember>>,
        #[allow(dead_code)]
        tags: Option<serde_json::Value>,
    },
}

#[derive(Debug, Deserialize)]
struct RelationMember {
    #[serde(rename = "type")]
    member_type: String,
    #[serde(rename = "ref")]
    member_ref: u64,
    #[allow(dead_code)]
    role: Option<String>,
}

/// Fetch all OSM linestrings for a relation, using cached responses when available.
pub fn fetch_relation_ways(
    relation_id: u64,
    cache_dir: &Path,
    on_progress: Option<&dyn Fn(FetchProgress)>,
) -> Result<Vec<LineString<f64>>> {
    std::fs::create_dir_all(cache_dir)?;
    let client = reqwest::blocking::Client::builder()
        .user_agent("pct-diff/0.1 (PCT reroute detection tool)")
        .build()?;

    // Step 1: Get the top-level relation to find sub-relation IDs
    let top_json = fetch_cached(
        &client,
        &format!("{}/relation/{}.json", OSM_API_BASE, relation_id),
        &cache_dir.join(format!("relation_{}.json", relation_id)),
    )?;

    let top: RelationResponse =
        serde_json::from_str(&top_json).context("Failed to parse top-level relation JSON")?;

    let sub_relation_ids: Vec<u64> = top
        .elements
        .iter()
        .filter_map(|e| match e {
            Element::Relation { members, .. } => members.as_ref(),
            _ => None,
        })
        .flat_map(|members| members.iter())
        .filter(|m| m.member_type == "relation")
        .map(|m| m.member_ref)
        .collect();

    if let Some(cb) = &on_progress {
        cb(FetchProgress::SubRelationsFound(sub_relation_ids.len()));
    }

    // Step 2: Fetch each sub-relation's full data
    let mut all_lines = Vec::new();

    for &sub_id in &sub_relation_ids {
        let full_json = fetch_cached(
            &client,
            &format!("{}/relation/{}/full.json", OSM_API_BASE, sub_id),
            &cache_dir.join(format!("relation_{}_full.json", sub_id)),
        )?;

        let lines = parse_full_response(&full_json)
            .with_context(|| format!("Failed to parse full response for relation {}", sub_id))?;

        all_lines.extend(lines);

        if let Some(cb) = &on_progress {
            cb(FetchProgress::SubRelationFetched(sub_id));
        }
    }

    Ok(all_lines)
}

/// Fetch a URL, using a cached file if it exists.
fn fetch_cached(
    client: &reqwest::blocking::Client,
    url: &str,
    cache_path: &Path,
) -> Result<String> {
    if cache_path.exists() {
        return std::fs::read_to_string(cache_path).context("Failed to read cache file");
    }

    let response = client.get(url).send()?.error_for_status()?;
    let body = response.text()?;

    std::fs::write(cache_path, &body).context("Failed to write cache file")?;
    Ok(body)
}

/// Parse a `/relation/{id}/full.json` response into linestrings.
pub fn parse_full_response(json: &str) -> Result<Vec<LineString<f64>>> {
    let resp: RelationResponse =
        serde_json::from_str(json).context("Failed to parse relation full JSON")?;

    // Build node lookup: id -> (lon, lat)
    let mut nodes: HashMap<u64, Coord<f64>> = HashMap::new();
    for element in &resp.elements {
        if let Element::Node { id, lat, lon } = element {
            if let (Some(lat), Some(lon)) = (lat, lon) {
                nodes.insert(*id, Coord { x: *lon, y: *lat });
            }
        }
    }

    // Build linestrings from ways
    let mut lines = Vec::new();
    for element in &resp.elements {
        if let Element::Way {
            nodes: Some(node_refs),
            ..
        } = element
        {
            let coords: Vec<Coord<f64>> = node_refs.iter().filter_map(|id| nodes.get(id)).copied().collect();
            if coords.len() >= 2 {
                lines.push(LineString::from(coords));
            }
        }
    }

    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_response_basic() {
        let json = r#"{
            "version": "0.6",
            "elements": [
                {"type": "node", "id": 1, "lat": 34.0, "lon": -118.0},
                {"type": "node", "id": 2, "lat": 34.001, "lon": -117.999},
                {"type": "node", "id": 3, "lat": 34.002, "lon": -117.998},
                {"type": "node", "id": 4, "lat": 34.003, "lon": -117.997},
                {"type": "way", "id": 100, "nodes": [1, 2, 3], "tags": {}},
                {"type": "way", "id": 101, "nodes": [3, 4], "tags": {}},
                {"type": "relation", "id": 200, "members": [
                    {"type": "way", "ref": 100, "role": ""},
                    {"type": "way", "ref": 101, "role": ""}
                ], "tags": {}}
            ]
        }"#;

        let lines = parse_full_response(json).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].0.len(), 3);
        assert_eq!(lines[1].0.len(), 2);

        // Verify coordinates
        assert_eq!(lines[0].0[0].x, -118.0);
        assert_eq!(lines[0].0[0].y, 34.0);
        assert_eq!(lines[1].0[1].x, -117.997);
        assert_eq!(lines[1].0[1].y, 34.003);
    }

    #[test]
    fn parse_full_response_missing_nodes() {
        let json = r#"{
            "version": "0.6",
            "elements": [
                {"type": "node", "id": 1, "lat": 34.0, "lon": -118.0},
                {"type": "way", "id": 100, "nodes": [1, 999], "tags": {}}
            ]
        }"#;

        let lines = parse_full_response(json).unwrap();
        // Way with only one resolvable node should be dropped (< 2 coords)
        assert!(lines.is_empty());
    }

    #[test]
    fn parse_full_response_empty() {
        let json = r#"{"version": "0.6", "elements": []}"#;
        let lines = parse_full_response(json).unwrap();
        assert!(lines.is_empty());
    }
}
