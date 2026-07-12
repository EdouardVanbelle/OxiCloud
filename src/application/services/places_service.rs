use std::sync::Arc;

use uuid::Uuid;

use crate::application::dtos::geo_dto::{GeoBounds, GeoCluster};
use crate::common::errors::DomainError;
use crate::infrastructure::repositories::pg::FileBlobReadRepository;

/// "Places" use case: the caller's geotagged photos aggregated into map
/// clusters.
///
/// Post-§15 the surface follows the Photos scope: drives where the
/// caller has Read AND `policies.include_in_photo_index = true`
/// (default personal drives materialise the flag at creation).
/// Group-membership expansion is handled inline by
/// `storage.caller_group_ids(caller)` inside the repo's SQL, so this
/// service is a thin coordinate-math wrapper — no engine dependency.
pub struct PlacesService {
    file_read: Arc<FileBlobReadRepository>,
}

impl PlacesService {
    pub fn new(file_read: Arc<FileBlobReadRepository>) -> Self {
        Self { file_read }
    }

    /// Aggregation cell side, in degrees, for a slippy-map zoom level. The
    /// world (360°) is split into `2^zoom` tiles; we use ~4 cells per tile so
    /// clusters refine as the user zooms in. Clamped to a sane range.
    fn cell_for_zoom(zoom: u8) -> f64 {
        let z = i32::from(zoom.min(20));
        360.0 / (2_f64.powi(z) * 4.0)
    }

    /// Clustered geotagged photos in the caller's Photos-scope drive set,
    /// within `bounds`.
    pub async fn clusters(
        &self,
        caller_id: Uuid,
        bounds: GeoBounds,
        zoom: u8,
    ) -> Result<Vec<GeoCluster>, DomainError> {
        let cell = Self::cell_for_zoom(zoom);
        self.file_read
            .list_geo_clusters(caller_id, bounds, cell)
            .await
    }
}
