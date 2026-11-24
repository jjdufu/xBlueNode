//! Pure Rust spatial geometry contracts (`GeoJSON` subset, bounding-box prefilters, polygon containment,
//! spherical Haversine distance, and convex hull DTOs) for `AxGraph`, `xBlueView`, and `AxSim` (`06-STEP28` §28.3).

use geo::CoordsIter as _;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppError;
use crate::enums::GeoPredicateOp;
use crate::graph::GeoPoint;

/// Supported `GeoJSON` geometry subset stored in `xb_nodes.geo_shape` and `ax_sim_base_nodes.geo_shape` (`06-STEP28` §28.0).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "coordinates")]
pub enum GeoShapeDto {
    /// Single WGS-84 `[lng, lat]` point.
    Point([f64; 2]),
    /// Polyline sequence of `>= 2` WGS-84 `[lng, lat]` vertices.
    LineString(Vec<[f64; 2]>),
    /// Polygon rings `[exterior_ring, ...interior_holes]` where each ring is closed with `>= 4` `[lng, lat]` vertices.
    Polygon(Vec<Vec<[f64; 2]>>),
}

/// Declarative spatial predicate filter shared by `GET /api/xblue/nodes` and `SimGoalConstraint.spatial_filter` (`06-STEP28` §28.0 & §28.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpatialFilterSpec {
    /// Spatial predicate kind (`BBOX`, `GEO_WITHIN_POLYGON`, or `GEO_DWITHIN_METERS`).
    #[serde(default)]
    pub op: GeoPredicateOp,
    /// Bounding box `[min_lng, min_lat, max_lng, max_lat]` when `op == BBOX`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f64; 4]>,
    /// Target `Polygon` geometry when `op == GEO_WITHIN_POLYGON`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polygon: Option<GeoShapeDto>,
    /// Spherical center coordinate when `op == GEO_DWITHIN_METERS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<GeoPoint>,
    /// Positive radius in meters when `op == GEO_DWITHIN_METERS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius_meters: Option<f64>,
}

/// Request payload for `POST /api/xblue/geo/convex-hull` (`06-STEP28` §28.0 & §28.3).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConvexHullRequest {
    /// Optional subgraph slug (`"main"` default when querying persisted nodes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subgraph: Option<String>,
    /// Optional node `type_name` filter when querying persisted nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    /// Optional explicit persisted `xb_nodes.id` allowlist.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub node_ids: Vec<Uuid>,
    /// Optional inline coordinate list `[ { lng, lat }, ... ]` for pure deterministic hull computation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<GeoPoint>,
}

/// Response payload for `POST /api/xblue/geo/convex-hull` (`06-STEP28` §28.0 & §28.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConvexHullResponse {
    /// Computed outer convex hull (`Polygon`, `LineString`, or `Point`), or `None` when input is empty.
    pub hull: Option<GeoShapeDto>,
    /// Total valid input coordinate count included in hull computation.
    pub point_count: usize,
    /// Planar WGS-84 degree-squared area of the resulting convex hull (`> 0` for non-collinear `>= 3` points).
    pub area_sq_deg: f64,
    /// Outer bounding box `[min_lng, min_lat, max_lng, max_lat]` of the hull, if non-empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f64; 4]>,
}

/// Validates WGS-84 coordinate bounds and ring structural invariants on a [`GeoShapeDto`].
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_GEOMETRY`) if any coordinate is non-finite, out of
/// WGS-84 bounds, degenerate, or unclosed.
pub fn validate_geo_shape(shape: &GeoShapeDto) -> Result<(), AppError> {
    match shape {
        GeoShapeDto::Point([lng, lat]) => validate_coord(*lng, *lat),
        GeoShapeDto::LineString(coords) => {
            if coords.len() < 2 {
                return Err(AppError::bad(
                    "INVALID_GEOMETRY",
                    "LineString requires at least 2 coordinates",
                ));
            }
            for &[lng, lat] in coords {
                validate_coord(lng, lat)?;
            }
            let ls: geo::LineString<f64> =
                coords.iter().map(|&[x, y]| geo::Coord { x, y }).collect();
            if ls.coords_count() < 2 {
                return Err(AppError::bad("INVALID_GEOMETRY", "degenerate LineString"));
            }
            Ok(())
        }
        GeoShapeDto::Polygon(rings) => {
            let Some(exterior) = rings.first() else {
                return Err(AppError::bad(
                    "INVALID_GEOMETRY",
                    "Polygon requires at least an exterior ring",
                ));
            };
            validate_ring(exterior)?;
            for hole in rings.iter().skip(1) {
                validate_ring(hole)?;
            }
            Ok(())
        }
    }
}

fn validate_coord(lng: f64, lat: f64) -> Result<(), AppError> {
    if !lng.is_finite() || !(-180.0..=180.0).contains(&lng) {
        return Err(AppError::bad(
            "INVALID_GEOMETRY",
            format!("longitude out of [-180, 180]: {lng}"),
        ));
    }
    if !lat.is_finite() || !(-90.0..=90.0).contains(&lat) {
        return Err(AppError::bad(
            "INVALID_GEOMETRY",
            format!("latitude out of [-90, 90]: {lat}"),
        ));
    }
    Ok(())
}

fn validate_ring(ring: &[[f64; 2]]) -> Result<(), AppError> {
    if ring.len() < 4 {
        return Err(AppError::bad(
            "INVALID_GEOMETRY",
            "Polygon ring requires at least 4 coordinates (closed)",
        ));
    }
    for &[lng, lat] in ring {
        validate_coord(lng, lat)?;
    }
    let first = ring.first().copied().unwrap_or([0.0, 0.0]);
    let last = ring.last().copied().unwrap_or([0.0, 0.0]);
    if (first[0] - last[0]).abs() > 1e-9 || (first[1] - last[1]).abs() > 1e-9 {
        return Err(AppError::bad(
            "INVALID_GEOMETRY",
            "Polygon ring must be closed (first coordinate == last coordinate)",
        ));
    }
    Ok(())
}
