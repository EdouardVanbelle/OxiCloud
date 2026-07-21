//! Domain entities for the People (faces) feature.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Length of a face embedding vector (ArcFace-style).
pub const EMBEDDING_DIM: usize = 512;

/// A face bounding box in normalized image coordinates (each component 0..1).
#[derive(Debug, Clone, Copy)]
pub struct BoundingBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl BoundingBox {
    /// `[x, y, w, h]` — the storage representation (Postgres `REAL[]`).
    pub fn to_array(self) -> Vec<f32> {
        vec![self.x, self.y, self.w, self.h]
    }

    /// Build from a stored `[x, y, w, h]` array; missing components default to 0.
    pub fn from_slice(a: &[f32]) -> Self {
        Self {
            x: a.first().copied().unwrap_or(0.0),
            y: a.get(1).copied().unwrap_or(0.0),
            w: a.get(2).copied().unwrap_or(0.0),
            h: a.get(3).copied().unwrap_or(0.0),
        }
    }
}

/// A face box for the lightbox tagging overlay — the narrow projection of a
/// persisted [`Face`] that the People API's `faces_for_file` needs (`id`,
/// `person_id`, `bbox`). Fetching this instead of a full [`Face`] keeps the
/// 2 KiB `embedding` BYTEA (plus det_score/quality/blob_hash/created_at) off
/// the wire on every lightbox open of a face-tagged photo. See
/// benches/ROUND14.md §Q1.
#[derive(Debug, Clone)]
pub struct FaceBox {
    pub id: Uuid,
    pub person_id: Option<Uuid>,
    pub bbox: BoundingBox,
}

/// A face produced by the analyzer but not yet persisted: where it is, how
/// confident the detector was, an optional quality score, and a 512-d,
/// L2-normalized embedding.
#[derive(Debug, Clone)]
pub struct DetectedFace {
    pub bbox: BoundingBox,
    pub det_score: f32,
    pub quality: Option<f32>,
    pub embedding: Vec<f32>,
}

/// A persisted face detection.
#[derive(Debug, Clone)]
pub struct Face {
    pub id: Uuid,
    pub file_id: Uuid,
    pub user_id: Uuid,
    /// Identity cluster this face belongs to, if any.
    pub person_id: Option<Uuid>,
    pub bbox: BoundingBox,
    pub det_score: f32,
    pub quality: Option<f32>,
    pub embedding: Vec<f32>,
    pub blob_hash: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// An identity cluster ("person"). `display_name` is `None` until the user
/// names it.
#[derive(Debug, Clone)]
pub struct Person {
    pub id: Uuid,
    pub user_id: Uuid,
    pub display_name: Option<String>,
    pub cover_face_id: Option<Uuid>,
    pub is_hidden: bool,
    pub created_at: DateTime<Utc>,
}
