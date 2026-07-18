# Search

OxiCloud provides authenticated file and folder search with simple query parameters, advanced JSON criteria, pagination, recursive traversal, and in-memory result caching.

## Endpoints

| Method | Endpoint | Description |
| --- | --- | --- |
| `GET` | `/api/search/` | Simple search using query parameters |
| `POST` | `/api/search/advanced` | Advanced search with a JSON body |
| `GET` | `/api/search/suggest` | Lightweight autocomplete suggestions |
| `DELETE` | `/api/admin/search/cache` | Flush the shared search results cache (admin only) |

All search endpoints require authentication. The cache flush is
additionally restricted to administrators — see [Result Caching](#result-caching).

## Simple Search Parameters

| Parameter | Description |
| --- | --- |
| `query` | Text to search in file and folder names |
| `type` | Comma-separated file extensions |
| `created_after` / `created_before` | Filter by creation time |
| `modified_after` / `modified_before` | Filter by modification time |
| `min_size` / `max_size` | Filter by file size in bytes |
| `folder_id` | Restrict search scope to one folder |
| `recursive` | Search subfolders, defaults to `true` |
| `limit` | Maximum results, defaults to `100` |
| `offset` | Pagination offset |
| `sort_by` | `relevance`, `name`, `name_desc`, `date`, `date_desc`, `size`, or `size_desc` |

### Example

```bash
curl -H "Authorization: Bearer $TOKEN" \
  "https://oxicloud.example.com/api/search/?query=report&type=pdf,docx&recursive=true&limit=20"
```

## Advanced Search

```json
{
  "name_contains": "report",
  "file_types": ["pdf", "docx"],
  "min_size": 1024,
  "folder_id": "folder-uuid",
  "recursive": true,
  "limit": 50,
  "offset": 0
}
```

## Suggestions

Use `/api/search/suggest?query=rep&limit=10` for quick autocomplete-style results. Suggestions can also be scoped to a folder with `folder_id`.

## Result Caching

Search results are cached in memory using the search criteria and user ID as the cache key.

- Cache TTL: 5 minutes
- Max entries: 1000
- Manual invalidation: `DELETE /api/admin/search/cache` — admin-only.
  The endpoint calls `invalidate_all()` on the shared moka cache, so
  one call cold-starts every subsequent search for every tenant; it's
  an operator debug lever, not a per-user affordance.

## Feature Flag

Search can be disabled with `OXICLOUD_ENABLE_SEARCH=false`.
