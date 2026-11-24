//! Pure Rust spatial geometry contracts (`GeoJSON` subset, bounding-box prefilters, polygon containment,
//! spherical Haversine distance, and convex hull DTOs) for `AxGraph`, `xBlueView`, and `AxSim` (`06-STEP28` §28.3).

use geo::{Area as _, Contains as _, ConvexHull as _, CoordsIter as _, Intersects as _};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppError;
use crate::enums::GeoPredicateOp;
use crate::graph::GeoPoint;

/// Mean spherical Earth radius in meters (IUGG WGS-84 mean radius) for `GEO_DWITHIN_METERS`.
pub const EARTH_MEAN_RADIUS_M: f64 = 6_371_008.8;

const CLOSURE_EPS: f64 = 1e-9;
const GEOM_EPS: f64 = 1e-12;

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

/// Validates a [`GeoPoint`] against finite WGS-84 bounds (`lng ∈ [-180, 180]`, `lat ∈ [-90, 90]`).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_GEOMETRY`) if `lng` or `lat` is non-finite or out of range.
pub fn validate_geo_point(pt: GeoPoint) -> Result<(), AppError> {
    validate_coord(pt.lng, pt.lat)
}

/// Validates WGS-84 coordinate bounds, ring closure, non-degeneracy, and simple non-self-intersection on a [`GeoShapeDto`].
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_GEOMETRY`) if any coordinate is non-finite, out of
/// WGS-84 bounds, degenerate, unclosed, or self-intersecting.
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
            let first = coords.first().copied().unwrap_or([0.0, 0.0]);
            let has_distinct = coords
                .iter()
                .skip(1)
                .any(|&c| coord_dist_sq(first, c) > GEOM_EPS * GEOM_EPS);
            if !has_distinct {
                return Err(AppError::bad(
                    "INVALID_GEOMETRY",
                    "LineString cannot collapse to a single repeated coordinate",
                ));
            }
            let ls: geo::LineString<f64> =
                coords.iter().map(|&[x, y]| geo::Coord { x, y }).collect();
            if ls.coords_count() < 2 {
                return Err(AppError::bad("INVALID_GEOMETRY", "degenerate LineString"));
            }
            Ok(())
        }
        GeoShapeDto::Polygon(rings) => validate_polygon_rings(rings),
    }
}

fn validate_polygon_rings(rings: &[Vec<[f64; 2]>]) -> Result<(), AppError> {
    let Some(exterior) = rings.first() else {
        return Err(AppError::bad(
            "INVALID_GEOMETRY",
            "Polygon requires at least an exterior ring",
        ));
    };
    let ext_poly = validate_ring_and_build_poly(exterior)?;
    let mut holes_area_sum = 0.0_f64;
    for hole in rings.iter().skip(1) {
        let hole_poly = validate_ring_and_build_poly(hole)?;
        let inside = hole_poly
            .exterior()
            .coords()
            .all(|c| ext_poly.intersects(&geo::Point::from(*c)));
        if !inside {
            return Err(AppError::bad(
                "INVALID_GEOMETRY",
                "Polygon interior hole must lie within the exterior ring",
            ));
        }
        holes_area_sum += hole_poly.unsigned_area();
    }
    let full_poly = build_geo_polygon(rings)?;
    if ext_poly.unsigned_area() - holes_area_sum <= GEOM_EPS
        || full_poly.unsigned_area() <= GEOM_EPS
    {
        return Err(AppError::bad(
            "INVALID_GEOMETRY",
            "Polygon net area must be > 0",
        ));
    }
    Ok(())
}

/// Parses a URL-decoded `polygon` query parameter (`GeoJSON` `Polygon` object or ring array) and validates it.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_GEOMETRY`) if JSON parsing fails, geometry type is not
/// `Polygon`, or ring validation fails.
pub fn parse_polygon_param(raw: &str) -> Result<GeoShapeDto, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::bad(
            "INVALID_GEOMETRY",
            "polygon parameter cannot be empty",
        ));
    }
    let val: serde_json::Value = serde_json::from_str(trimmed).map_err(|err| {
        AppError::bad(
            "INVALID_GEOMETRY",
            format!("malformed polygon JSON parameter: {err}"),
        )
    })?;

    let shape = if val.is_object() {
        let dto: GeoShapeDto = serde_json::from_value(val).map_err(|err| {
            AppError::bad("INVALID_GEOMETRY", format!("invalid GeoJSON shape: {err}"))
        })?;
        if !matches!(dto, GeoShapeDto::Polygon(_)) {
            return Err(AppError::bad(
                "INVALID_GEOMETRY",
                "GEO_WITHIN_POLYGON requires a Polygon geometry",
            ));
        }
        dto
    } else if val.is_array() {
        if let Ok(rings) = serde_json::from_value::<Vec<Vec<[f64; 2]>>>(val.clone()) {
            GeoShapeDto::Polygon(rings)
        } else if let Ok(single_ring) = serde_json::from_value::<Vec<[f64; 2]>>(val) {
            GeoShapeDto::Polygon(vec![single_ring])
        } else {
            return Err(AppError::bad(
                "INVALID_GEOMETRY",
                "polygon coordinate array must be [[[lng, lat], ...]]",
            ));
        }
    } else {
        return Err(AppError::bad(
            "INVALID_GEOMETRY",
            "polygon must be a GeoJSON Polygon object or coordinate ring array",
        ));
    };

    validate_geo_shape(&shape)?;
    Ok(shape)
}

/// Computes the tight axis-aligned WGS-84 bounding box `[min_lng, min_lat, max_lng, max_lat]` of a [`GeoShapeDto`].
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_GEOMETRY`) if `shape` fails geometry validation.
pub fn shape_bbox(shape: &GeoShapeDto) -> Result<[f64; 4], AppError> {
    validate_geo_shape(shape)?;
    let coords: Vec<[f64; 2]> = shape_vertices(shape);
    coords_bbox(&coords).ok_or_else(|| AppError::bad("INVALID_GEOMETRY", "empty geometry"))
}

/// Computes the Stage-1 PostgreSQL `(geo_lng, geo_lat)` B-Tree prefilter bounding box `[min_lng, min_lat, max_lng, max_lat]`
/// for a [`SpatialFilterSpec`] (`06-STEP28` §28.3).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_GEOMETRY`) if required parameters for `filter.op` are missing or invalid.
pub fn spatial_filter_bbox(filter: &SpatialFilterSpec) -> Result<[f64; 4], AppError> {
    match filter.op {
        GeoPredicateOp::Bbox => {
            let Some([min_lng, min_lat, max_lng, max_lat]) = filter.bbox else {
                return Err(AppError::bad(
                    "INVALID_GEOMETRY",
                    "bbox is required when geo_op is BBOX",
                ));
            };
            validate_coord(min_lng, min_lat)?;
            validate_coord(max_lng, max_lat)?;
            if min_lng > max_lng || min_lat > max_lat {
                return Err(AppError::bad(
                    "INVALID_GEOMETRY",
                    "bbox min coordinates must be <= max coordinates",
                ));
            }
            Ok([min_lng, min_lat, max_lng, max_lat])
        }
        GeoPredicateOp::GeoWithinPolygon => {
            let Some(ref poly) = filter.polygon else {
                return Err(AppError::bad(
                    "INVALID_GEOMETRY",
                    "polygon is required when geo_op is GEO_WITHIN_POLYGON",
                ));
            };
            if !matches!(poly, GeoShapeDto::Polygon(_)) {
                return Err(AppError::bad(
                    "INVALID_GEOMETRY",
                    "GEO_WITHIN_POLYGON requires a Polygon geometry",
                ));
            }
            shape_bbox(poly)
        }
        GeoPredicateOp::GeoDwithinMeters => {
            let Some(center) = filter.center else {
                return Err(AppError::bad(
                    "INVALID_GEOMETRY",
                    "center is required when geo_op is GEO_DWITHIN_METERS",
                ));
            };
            let Some(radius_m) = filter.radius_meters else {
                return Err(AppError::bad(
                    "INVALID_GEOMETRY",
                    "radius_meters is required when geo_op is GEO_DWITHIN_METERS",
                ));
            };
            validate_geo_point(center)?;
            if !radius_m.is_finite() || radius_m <= 0.0 {
                return Err(AppError::bad(
                    "INVALID_GEOMETRY",
                    "radius_meters must be finite and > 0",
                ));
            }
            Ok(dwithin_degree_bbox(center, radius_m))
        }
    }
}

/// Computes spherical Haversine distance in meters between two WGS-84 coordinates.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_GEOMETRY`) if either coordinate is invalid.
pub fn haversine_distance_meters(a: GeoPoint, b: GeoPoint) -> Result<f64, AppError> {
    validate_geo_point(a)?;
    validate_geo_point(b)?;
    let d_lat = (b.lat - a.lat).to_radians();
    let d_lng = (b.lng - a.lng).to_radians();
    let lat1 = a.lat.to_radians();
    let lat2 = b.lat.to_radians();
    let half_lat = (d_lat * 0.5).sin();
    let half_lng = (d_lng * 0.5).sin();
    let h = (half_lat * half_lat + lat1.cos() * lat2.cos() * half_lng * half_lng).clamp(0.0, 1.0);
    Ok(2.0 * EARTH_MEAN_RADIUS_M * h.sqrt().asin())
}

/// Evaluates exact Stage-2 spatial containment / spherical buffer predicate on a node's `(point, shape)` (`06-STEP28` §28.3).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_GEOMETRY`) if `filter`, `point`, or `shape` contains invalid geometry.
pub fn matches_spatial_filter(
    point: Option<GeoPoint>,
    shape: Option<&GeoShapeDto>,
    filter: &SpatialFilterSpec,
) -> Result<bool, AppError> {
    let f_bbox = spatial_filter_bbox(filter)?;
    if let Some(pt) = point {
        validate_geo_point(pt)?;
    }
    if let Some(s) = shape {
        validate_geo_shape(s)?;
    }
    if point.is_none() && shape.is_none() {
        return Ok(false);
    }

    match filter.op {
        GeoPredicateOp::Bbox => {
            if let Some(pt) = point {
                return Ok(point_in_bbox(pt, f_bbox));
            }
            if let Some(s) = shape {
                let sb = shape_bbox(s)?;
                return Ok(sb[0] >= f_bbox[0]
                    && sb[2] <= f_bbox[2]
                    && sb[1] >= f_bbox[1]
                    && sb[3] <= f_bbox[3]);
            }
            Ok(false)
        }
        GeoPredicateOp::GeoWithinPolygon => {
            let Some(GeoShapeDto::Polygon(ref rings)) = filter.polygon else {
                return Err(AppError::bad(
                    "INVALID_GEOMETRY",
                    "GEO_WITHIN_POLYGON requires a Polygon geometry",
                ));
            };
            let target_poly = build_geo_polygon(rings)?;
            if let Some(pt) = point {
                if !point_in_bbox(pt, f_bbox) {
                    return Ok(false);
                }
                let g_pt = geo::Point::new(pt.lng, pt.lat);
                return Ok(target_poly.contains(&g_pt) || target_poly.intersects(&g_pt));
            }
            if let Some(s) = shape {
                return Ok(shape_within_polygon(s, &target_poly));
            }
            Ok(false)
        }
        GeoPredicateOp::GeoDwithinMeters => {
            let center = filter.center.ok_or_else(|| {
                AppError::bad("INVALID_GEOMETRY", "center is required for DWITHIN")
            })?;
            let radius_m = filter.radius_meters.ok_or_else(|| {
                AppError::bad("INVALID_GEOMETRY", "radius_meters is required for DWITHIN")
            })?;
            if !radius_m.is_finite() || radius_m <= 0.0 {
                return Err(AppError::bad(
                    "INVALID_GEOMETRY",
                    "radius_meters must be finite and > 0",
                ));
            }
            if let Some(pt) = point {
                if !point_in_bbox(pt, f_bbox) {
                    return Ok(false);
                }
                return Ok(haversine_distance_meters(center, pt)? <= radius_m);
            }
            if let Some(s) = shape {
                return shape_within_radius(s, center, radius_m);
            }
            Ok(false)
        }
    }
}

/// Computes the deterministic outer convex hull, planar degree-squared area, and bounding box of `points` (`06-STEP28` §28.3).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_GEOMETRY`) if any input [`GeoPoint`] has non-finite or out-of-bounds coordinates.
pub fn compute_convex_hull(points: &[GeoPoint]) -> Result<ConvexHullResponse, AppError> {
    for &pt in points {
        validate_geo_point(pt)?;
    }
    let point_count = points.len();
    if points.is_empty() {
        return Ok(ConvexHullResponse {
            hull: None,
            point_count: 0,
            area_sq_deg: 0.0,
            bbox: None,
        });
    }

    let raw_coords: Vec<[f64; 2]> = points.iter().map(|p| [p.lng, p.lat]).collect();
    let bbox = coords_bbox(&raw_coords);
    let mp = geo::MultiPoint::new(
        points
            .iter()
            .map(|p| geo::Point::new(p.lng, p.lat))
            .collect(),
    );
    let hull_poly = mp.convex_hull();
    let area_sq_deg = hull_poly.unsigned_area();

    if area_sq_deg > GEOM_EPS {
        let ring: Vec<[f64; 2]> = hull_poly.exterior().coords().map(|c| [c.x, c.y]).collect();
        return Ok(ConvexHullResponse {
            hull: Some(GeoShapeDto::Polygon(vec![ring])),
            point_count,
            area_sq_deg,
            bbox,
        });
    }

    let (min_pt, max_pt) = extreme_collinear_endpoints(&raw_coords);
    if coord_dist_sq(min_pt, max_pt) <= GEOM_EPS * GEOM_EPS {
        Ok(ConvexHullResponse {
            hull: Some(GeoShapeDto::Point(min_pt)),
            point_count,
            area_sq_deg: 0.0,
            bbox,
        })
    } else {
        Ok(ConvexHullResponse {
            hull: Some(GeoShapeDto::LineString(vec![min_pt, max_pt])),
            point_count,
            area_sq_deg: 0.0,
            bbox,
        })
    }
}

/// Extracts all WGS-84 `[lng, lat]` vertices from a [`GeoShapeDto`].
#[must_use]
pub fn shape_vertices(shape: &GeoShapeDto) -> Vec<[f64; 2]> {
    match shape {
        GeoShapeDto::Point(c) => vec![*c],
        GeoShapeDto::LineString(coords) => coords.clone(),
        GeoShapeDto::Polygon(rings) => rings.iter().flatten().copied().collect(),
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

fn validate_ring_and_build_poly(ring: &[[f64; 2]]) -> Result<geo::Polygon<f64>, AppError> {
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
    if (first[0] - last[0]).abs() > CLOSURE_EPS || (first[1] - last[1]).abs() > CLOSURE_EPS {
        return Err(AppError::bad(
            "INVALID_GEOMETRY",
            "Polygon ring must be closed (first coordinate == last coordinate)",
        ));
    }

    let edge_count = ring.len().saturating_sub(1);
    for idx in 0..edge_count {
        let a = ring.get(idx).copied().unwrap_or([0.0, 0.0]);
        let b = ring.get(idx + 1).copied().unwrap_or([0.0, 0.0]);
        if coord_dist_sq(a, b) <= GEOM_EPS * GEOM_EPS {
            return Err(AppError::bad(
                "INVALID_GEOMETRY",
                "Polygon ring contains consecutive duplicate coordinates",
            ));
        }
    }

    if ring_has_self_intersection(ring, edge_count) {
        return Err(AppError::bad(
            "INVALID_GEOMETRY",
            "Polygon ring is self-intersecting",
        ));
    }

    let ls: geo::LineString<f64> = ring.iter().map(|&[x, y]| geo::Coord { x, y }).collect();
    let poly = geo::Polygon::new(ls, Vec::new());
    if poly.unsigned_area() <= GEOM_EPS {
        return Err(AppError::bad(
            "INVALID_GEOMETRY",
            "Polygon ring is collinear or has zero area",
        ));
    }
    Ok(poly)
}

fn ring_has_self_intersection(ring: &[[f64; 2]], edge_count: usize) -> bool {
    for i in 0..edge_count {
        let a0 = ring.get(i).copied().unwrap_or([0.0, 0.0]);
        let a1 = ring.get(i + 1).copied().unwrap_or([0.0, 0.0]);
        for j in (i + 1)..edge_count {
            let b0 = ring.get(j).copied().unwrap_or([0.0, 0.0]);
            let b1 = ring.get(j + 1).copied().unwrap_or([0.0, 0.0]);
            let adjacent = (j == i + 1) || (i == 0 && j + 1 == edge_count);
            if adjacent {
                if adjacent_edges_fold_back(a0, a1, b0, b1, j == i + 1) {
                    return true;
                }
            } else if segments_intersect(a0, a1, b0, b1) {
                return true;
            }
        }
    }
    false
}

fn adjacent_edges_fold_back(
    a0: [f64; 2],
    a1: [f64; 2],
    b0: [f64; 2],
    b1: [f64; 2],
    share_a1_b0: bool,
) -> bool {
    let (pivot, u, v) = if share_a1_b0 {
        (a1, a0, b1)
    } else {
        (a0, a1, b0)
    };
    if orient_2d(pivot, u, v).abs() > GEOM_EPS {
        return false;
    }
    let ux = u[0] - pivot[0];
    let uy = u[1] - pivot[1];
    let vx = v[0] - pivot[0];
    let vy = v[1] - pivot[1];
    (ux * vx + uy * vy) > 0.0
}

fn segments_intersect(p1: [f64; 2], q1: [f64; 2], p2: [f64; 2], q2: [f64; 2]) -> bool {
    let o1 = orient_2d(p1, q1, p2);
    let o2 = orient_2d(p1, q1, q2);
    let o3 = orient_2d(p2, q2, p1);
    let o4 = orient_2d(p2, q2, q1);

    let straddle_1 = (o1 > GEOM_EPS && o2 < -GEOM_EPS) || (o1 < -GEOM_EPS && o2 > GEOM_EPS);
    let straddle_2 = (o3 > GEOM_EPS && o4 < -GEOM_EPS) || (o3 < -GEOM_EPS && o4 > GEOM_EPS);
    if straddle_1 && straddle_2 {
        return true;
    }

    (o1.abs() <= GEOM_EPS && point_on_bbox(p1, q1, p2))
        || (o2.abs() <= GEOM_EPS && point_on_bbox(p1, q1, q2))
        || (o3.abs() <= GEOM_EPS && point_on_bbox(p2, q2, p1))
        || (o4.abs() <= GEOM_EPS && point_on_bbox(p2, q2, q1))
}

fn orient_2d(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn point_on_bbox(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> bool {
    p[0] >= a[0].min(b[0]) - GEOM_EPS
        && p[0] <= a[0].max(b[0]) + GEOM_EPS
        && p[1] >= a[1].min(b[1]) - GEOM_EPS
        && p[1] <= a[1].max(b[1]) + GEOM_EPS
}

fn coord_dist_sq(a: [f64; 2], b: [f64; 2]) -> f64 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    dx * dx + dy * dy
}

fn coords_bbox(coords: &[[f64; 2]]) -> Option<[f64; 4]> {
    let first = coords.first().copied()?;
    let mut min_lng = first[0];
    let mut min_lat = first[1];
    let mut max_lng = first[0];
    let mut max_lat = first[1];
    for &[lng, lat] in coords.iter().skip(1) {
        min_lng = min_lng.min(lng);
        min_lat = min_lat.min(lat);
        max_lng = max_lng.max(lng);
        max_lat = max_lat.max(lat);
    }
    Some([min_lng, min_lat, max_lng, max_lat])
}

fn point_in_bbox(pt: GeoPoint, bbox: [f64; 4]) -> bool {
    pt.lng >= bbox[0] && pt.lng <= bbox[2] && pt.lat >= bbox[1] && pt.lat <= bbox[3]
}

fn dwithin_degree_bbox(center: GeoPoint, radius_m: f64) -> [f64; 4] {
    let ang_rad = radius_m / EARTH_MEAN_RADIUS_M;
    let delta_lat = ang_rad.to_degrees() + 1e-4;
    let min_lat = (center.lat - delta_lat).max(-90.0);
    let max_lat = (center.lat + delta_lat).min(90.0);

    let lat_rad = center.lat.to_radians().abs();
    if ang_rad >= std::f64::consts::FRAC_PI_2 - 1e-6
        || lat_rad + ang_rad >= std::f64::consts::FRAC_PI_2 - 1e-6
    {
        return [-180.0, min_lat, 180.0, max_lat];
    }

    let sin_ratio = (ang_rad.sin() / lat_rad.cos()).clamp(0.0, 1.0);
    let delta_lng = sin_ratio.asin().to_degrees() + 1e-4;
    let raw_min_lng = center.lng - delta_lng;
    let raw_max_lng = center.lng + delta_lng;
    if raw_min_lng < -180.0 || raw_max_lng > 180.0 {
        [-180.0, min_lat, 180.0, max_lat]
    } else {
        [raw_min_lng, min_lat, raw_max_lng, max_lat]
    }
}

fn build_geo_polygon(rings: &[Vec<[f64; 2]>]) -> Result<geo::Polygon<f64>, AppError> {
    let Some(ext_raw) = rings.first() else {
        return Err(AppError::bad(
            "INVALID_GEOMETRY",
            "Polygon requires an exterior ring",
        ));
    };
    let ext: geo::LineString<f64> = ext_raw.iter().map(|&[x, y]| geo::Coord { x, y }).collect();
    let holes: Vec<geo::LineString<f64>> = rings
        .iter()
        .skip(1)
        .map(|r| r.iter().map(|&[x, y]| geo::Coord { x, y }).collect())
        .collect();
    Ok(geo::Polygon::new(ext, holes))
}

fn shape_within_polygon(shape: &GeoShapeDto, target_poly: &geo::Polygon<f64>) -> bool {
    match shape {
        GeoShapeDto::Point([lng, lat]) => {
            let pt = geo::Point::new(*lng, *lat);
            target_poly.contains(&pt) || target_poly.intersects(&pt)
        }
        GeoShapeDto::LineString(coords) => {
            let ls: geo::LineString<f64> =
                coords.iter().map(|&[x, y]| geo::Coord { x, y }).collect();
            target_poly.contains(&ls)
                || coords
                    .iter()
                    .all(|&[x, y]| target_poly.intersects(&geo::Point::new(x, y)))
        }
        GeoShapeDto::Polygon(rings) => {
            let Ok(cand) = build_geo_polygon(rings) else {
                return false;
            };
            target_poly.contains(&cand)
                || (cand
                    .exterior()
                    .coords()
                    .all(|c| target_poly.intersects(&geo::Point::from(*c)))
                    && cand.unsigned_area() <= target_poly.unsigned_area() + GEOM_EPS)
        }
    }
}

fn shape_within_radius(
    shape: &GeoShapeDto,
    center: GeoPoint,
    radius_m: f64,
) -> Result<bool, AppError> {
    if let GeoShapeDto::Polygon(rings) = shape {
        let poly = build_geo_polygon(rings)?;
        let c_pt = geo::Point::new(center.lng, center.lat);
        if poly.contains(&c_pt) || poly.intersects(&c_pt) {
            return Ok(true);
        }
    }
    for [lng, lat] in shape_vertices(shape) {
        if haversine_distance_meters(center, GeoPoint { lng, lat })? <= radius_m {
            return Ok(true);
        }
    }
    Ok(false)
}

fn extreme_collinear_endpoints(coords: &[[f64; 2]]) -> ([f64; 2], [f64; 2]) {
    let first = coords.first().copied().unwrap_or([0.0, 0.0]);
    let mut min_pt = first;
    let mut max_pt = first;
    for &c in coords.iter().skip(1) {
        if c[0]
            .total_cmp(&min_pt[0])
            .then_with(|| c[1].total_cmp(&min_pt[1]))
            .is_lt()
        {
            min_pt = c;
        }
        if c[0]
            .total_cmp(&max_pt[0])
            .then_with(|| c[1].total_cmp(&max_pt[1]))
            .is_gt()
        {
            max_pt = c;
        }
    }
    (min_pt, max_pt)
}
