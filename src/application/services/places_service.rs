use std::sync::Arc;

use uuid::Uuid;

use crate::application::dtos::geo_dto::{GeoBounds, GeoCluster};
use crate::common::errors::DomainError;
use crate::domain::services::authorization::Subject;
use crate::infrastructure::repositories::pg::FileBlobReadRepository;
use crate::infrastructure::services::pg_acl_engine::PgAclEngine;

/// "Places" use case: the caller's geotagged photos aggregated into map
/// clusters.
///
/// Post-§15 the surface follows the Photos scope: default personal drive
/// + drives where `policies.include_in_photo_index = true` AND caller
/// has Read. The repository query joins `role_grants` on the drive
/// resource type; group-mediated grants are honoured via the caller
/// expansion done here.
pub struct PlacesService {
    file_read: Arc<FileBlobReadRepository>,
    authorization: Arc<PgAclEngine>,
}

impl PlacesService {
    pub fn new(file_read: Arc<FileBlobReadRepository>, authorization: Arc<PgAclEngine>) -> Self {
        Self {
            file_read,
            authorization,
        }
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
        let (subject_types, subject_ids) = self
            .authorization
            .expand_subject_for_listing(Subject::User(caller_id))
            .await?;
        self.file_read
            .list_geo_clusters(&subject_types, &subject_ids, bounds, cell)
            .await
    }
}
