-- Delta-upload protocol: chunk-level ownership lookups.
--
-- The negotiate/commit endpoints answer "which of these N chunk hashes may
-- this caller claim without uploading bytes?" — a chunk is claimable when a
-- manifest of one of the caller's (non-trashed) files contains it. That
-- containment test (`chunk_hashes @> ARRAY[hash]`) would be a sequential
-- scan over storage.chunk_manifests without an index; GIN makes each probe
-- an index lookup.
--
-- Plan B if this disappoints at scale: a normalized
-- storage.manifest_chunks(file_hash, chunk_hash) join table.
CREATE INDEX IF NOT EXISTS idx_chunk_manifests_chunk_hashes_gin
    ON storage.chunk_manifests USING GIN (chunk_hashes);
