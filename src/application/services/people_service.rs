//! People (faces) use cases: identity clustering + the read/mutation methods
//! the HTTP layer calls.
//!
//! Clustering is a full re-cluster over the user's faces: a union-find groups
//! faces whose embeddings are within a cosine threshold (connected
//! components), and groups of at least `min_faces` become a "person". This is
//! O(n²) in the user's face count — fine for moderate libraries; an ANN index
//! (pgvector/VectorChord) is the documented scale-up.
//!
//! Strictly user-scoped (the repository filters by user), so — like
//! `RecentService` / `PlacesService` — no `AuthorizationEngine` check is
//! needed: the `caller_id` parameter is the access scope.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use uuid::Uuid;

use crate::application::dtos::people_dto::{FaceBoxDto, PersonDto};
use crate::application::ports::face_ports::FaceRepository;
use crate::common::errors::DomainError;
use crate::domain::entities::face::Person;
use crate::infrastructure::repositories::pg::FacePgRepository;

/// Squared L2 norm, accumulated in the same order `cosine` used to, so
/// the precomputed-norm path is bit-identical to the old per-pair one.
fn norm_sq(v: &[f32]) -> f32 {
    let mut n = 0.0f32;
    for &x in v {
        n += x * x;
    }
    n
}

/// Cosine similarity of two equal-length vectors given their precomputed
/// squared norms. Embeddings are produced L2-normalized, so this is ~a dot
/// product; we normalize anyway for safety. The O(N²) recluster pair loop
/// used to re-accumulate BOTH norms on every pair — precomputing them once
/// per face keeps only the dot product in the hot loop while the final
/// `dot / (√na · √nb)` expression (and the zero guards) stay exactly as
/// before, so results are bit-identical (benches/ROUND11.md §17).
fn cosine_with_norms(a: &[f32], b: &[f32], na: f32, nb: f32) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    for (&x, &y) in a.iter().zip(b.iter()) {
        dot += x * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Disjoint-set with path-halving + union by rank.
struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        match self.rank[ra].cmp(&self.rank[rb]) {
            std::cmp::Ordering::Less => self.parent[ra] = rb,
            std::cmp::Ordering::Greater => self.parent[rb] = ra,
            std::cmp::Ordering::Equal => {
                self.parent[rb] = ra;
                self.rank[ra] += 1;
            }
        }
    }
}

pub struct PeopleService {
    repo: Arc<FacePgRepository>,
    /// Min cosine similarity to link two faces into the same identity.
    cluster_threshold: f32,
    /// Min faces in a cluster before it becomes a named-able "person".
    min_faces: usize,
}

impl PeopleService {
    pub fn new(repo: Arc<FacePgRepository>) -> Self {
        Self {
            repo,
            cluster_threshold: 0.5,
            min_faces: 3,
        }
    }

    /// Re-cluster a user's faces. Returns the number of new persons created.
    pub async fn recluster(&self, user_id: Uuid) -> Result<usize, DomainError> {
        let faces = self.repo.faces_for_user(user_id).await?;
        let n = faces.len();
        if n == 0 {
            return Ok(0);
        }

        let norms: Vec<f32> = faces.iter().map(|f| norm_sq(&f.embedding)).collect();
        let mut uf = UnionFind::new(n);
        for i in 0..n {
            for j in (i + 1)..n {
                if cosine_with_norms(&faces[i].embedding, &faces[j].embedding, norms[i], norms[j])
                    >= self.cluster_threshold
                {
                    uf.union(i, j);
                }
            }
        }

        let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..n {
            let root = uf.find(i);
            groups.entry(root).or_default().push(i);
        }

        // Accumulate every (face, person) change and apply them in ONE
        // UNNEST batch at the end — the old per-face `assign_person` loop
        // issued up to F sequential UPDATE round-trips per recluster
        // (benches/ROUND11.md §Q5; the ROUND10 `save_faces` pattern). The
        // final column state is identical.
        let mut assignments: Vec<(Uuid, Option<Uuid>)> = Vec::new();
        let mut created = 0usize;
        for idxs in groups.into_values() {
            if idxs.len() < self.min_faces {
                // Too small to be a person — leave/reset these faces unassigned.
                for &i in &idxs {
                    if faces[i].person_id.is_some() {
                        assignments.push((faces[i].id, None));
                    }
                }
                continue;
            }

            // Reuse an existing person on this cluster (preserves a user's name)
            // or mint a new one.
            let existing = idxs.iter().find_map(|&i| faces[i].person_id);
            let person_id = match existing {
                Some(pid) => pid,
                None => {
                    let pid = Uuid::new_v4();
                    let person = Person {
                        id: pid,
                        user_id,
                        display_name: None,
                        cover_face_id: Some(faces[idxs[0]].id),
                        is_hidden: false,
                        created_at: Utc::now(),
                    };
                    self.repo.create_person(&person).await?;
                    created += 1;
                    pid
                }
            };
            for &i in &idxs {
                if faces[i].person_id != Some(person_id) {
                    assignments.push((faces[i].id, Some(person_id)));
                }
            }
            let _ = self
                .repo
                .set_person_cover(person_id, faces[idxs[0]].id)
                .await;
        }
        self.repo.assign_person_batch(&assignments).await?;

        Ok(created)
    }

    /// People (non-empty clusters), most-photographed first.
    ///
    /// Counts come from a grouped-COUNT query and cover photos from one
    /// batched lookup of just the cover face ids — the previous
    /// `faces_for_user` shipped every face row (2 KiB embedding included)
    /// only to count them: ~20 MB of BYTEA per request on a 10k-face
    /// library (benches/PEOPLE-LIST.md).
    pub async fn list_people(&self, caller_id: Uuid) -> Result<Vec<PersonDto>, DomainError> {
        let persons = self.repo.persons_for_user(caller_id).await?;
        let count: HashMap<Uuid, i64> = self
            .repo
            .person_face_stats(caller_id)
            .await?
            .into_iter()
            .collect();
        let cover_ids: Vec<Uuid> = persons.iter().filter_map(|p| p.cover_face_id).collect();
        let face_file: HashMap<Uuid, Uuid> =
            self.repo.file_ids_for_faces(caller_id, &cover_ids).await?;

        let mut out: Vec<PersonDto> = persons
            .into_iter()
            .filter_map(|p| {
                let c = count.get(&p.id).copied().unwrap_or(0);
                if c == 0 {
                    return None; // hide empty clusters (e.g. after a merge)
                }
                let cover_file_id = p
                    .cover_face_id
                    .and_then(|fid| face_file.get(&fid).copied())
                    .map(|u| u.to_string());
                Some(PersonDto {
                    id: p.id.to_string(),
                    name: p.display_name,
                    cover_file_id,
                    face_count: c,
                    is_hidden: p.is_hidden,
                })
            })
            .collect();
        out.sort_by_key(|p| std::cmp::Reverse(p.face_count));
        Ok(out)
    }

    /// File ids of a person's photos (most recent first).
    pub async fn person_photos(
        &self,
        caller_id: Uuid,
        person_id: Uuid,
    ) -> Result<Vec<String>, DomainError> {
        let files = self.repo.files_for_person(caller_id, person_id).await?;
        Ok(files.into_iter().map(|u| u.to_string()).collect())
    }

    /// Face boxes within a photo (for lightbox tagging), caller-scoped.
    pub async fn faces_for_file(
        &self,
        caller_id: Uuid,
        file_id: Uuid,
    ) -> Result<Vec<FaceBoxDto>, DomainError> {
        // The narrow projection scopes to the caller in SQL (WHERE user_id),
        // so no post-filter is needed here. See benches/ROUND14.md §Q1.
        let boxes = self.repo.face_boxes_for_file(file_id, caller_id).await?;
        Ok(boxes
            .into_iter()
            .map(|f| FaceBoxDto {
                id: f.id.to_string(),
                person_id: f.person_id.map(|u| u.to_string()),
                x: f.bbox.x,
                y: f.bbox.y,
                w: f.bbox.w,
                h: f.bbox.h,
            })
            .collect())
    }

    pub async fn rename_person(
        &self,
        caller_id: Uuid,
        person_id: Uuid,
        name: Option<String>,
    ) -> Result<(), DomainError> {
        self.repo.rename_person(caller_id, person_id, name).await
    }

    /// Merge `from` into `into` by reassigning all of `from`'s faces. The
    /// now-empty `from` person is hidden by `list_people`.
    ///
    /// One set-based UPDATE — the previous shape loaded every face row
    /// (embeddings included) and issued one UPDATE per matching face.
    pub async fn merge(&self, caller_id: Uuid, into: Uuid, from: Uuid) -> Result<(), DomainError> {
        self.repo
            .reassign_person_faces(caller_id, from, into)
            .await?;
        Ok(())
    }

    /// Erase all of the caller's face data (right to erasure / opt-out).
    pub async fn delete_all(&self, caller_id: Uuid) -> Result<(), DomainError> {
        self.repo.delete_all_for_user(caller_id).await
    }
}
