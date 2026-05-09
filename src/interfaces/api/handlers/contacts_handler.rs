use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::error;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::application::dtos::address_book_dto::{CreateAddressBookDto, UpdateAddressBookDto};
use crate::application::dtos::contact_dto::{
    AddressDto, ContactDto, ContactGroupDto, CreateContactDto, CreateContactGroupDto, EmailDto,
    GroupMembershipDto, PhoneDto, UpdateContactDto, UpdateContactGroupDto,
};
use crate::application::dtos::user_dto::UserDto;
use crate::application::ports::carddav_ports::{AddressBookUseCase, ContactUseCase};
use crate::application::services::auth_application_service::AuthApplicationService;
use crate::domain::errors::ErrorKind;
use crate::infrastructure::adapters::contact_storage_adapter::ContactStorageAdapter;
use crate::interfaces::middleware::auth::AuthUser;

const SYSTEM_BOOK_ID: &str = "system";

/// Combined state for the contacts REST API.
#[derive(Clone)]
pub struct ContactsApiState {
    pub contact_service: Arc<ContactStorageAdapter>,
    pub auth_service: Option<Arc<AuthApplicationService>>,
}

/// Address book entry with `is_readonly` and `is_system` flags.
#[derive(Debug, Serialize, ToSchema)]
pub struct AddressBookResponse {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub description: Option<String>,
    pub color: Option<String>,
    pub is_public: bool,
    pub is_readonly: bool,
    pub is_system: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for creating an address book.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateAddressBookRequest {
    pub name: String,
    pub description: Option<String>,
    pub color: Option<String>,
    pub is_public: Option<bool>,
}

/// Request body for updating an address book.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateAddressBookRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub color: Option<String>,
    pub is_public: Option<bool>,
}

/// Request body for creating a contact.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateContactRequest {
    pub full_name: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub nickname: Option<String>,
    #[serde(default)]
    pub email: Vec<EmailDto>,
    #[serde(default)]
    pub phone: Vec<PhoneDto>,
    #[serde(default)]
    pub address: Vec<AddressDto>,
    pub organization: Option<String>,
    pub title: Option<String>,
    pub notes: Option<String>,
    pub photo_url: Option<String>,
    pub birthday: Option<NaiveDate>,
    pub anniversary: Option<NaiveDate>,
}

/// Request body for updating a contact (all fields optional).
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateContactRequest {
    pub full_name: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub nickname: Option<String>,
    pub email: Option<Vec<EmailDto>>,
    pub phone: Option<Vec<PhoneDto>>,
    pub address: Option<Vec<AddressDto>>,
    pub organization: Option<String>,
    pub title: Option<String>,
    pub notes: Option<String>,
    pub photo_url: Option<String>,
    pub birthday: Option<NaiveDate>,
    pub anniversary: Option<NaiveDate>,
}

/// Request body for creating or renaming a group.
#[derive(Debug, Deserialize, ToSchema)]
pub struct GroupNameRequest {
    pub name: String,
}

/// Request body for adding a contact to a group.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AddMemberRequest {
    pub contact_id: String,
}

/// Query parameters for paginated listing.
#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_limit() -> i64 {
    100
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn domain_err_to_response(err: crate::domain::errors::DomainError) -> Response {
    let status = match err.kind {
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::AccessDenied => StatusCode::FORBIDDEN,
        ErrorKind::InvalidInput => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (
        status,
        Json(serde_json::json!({ "error": err.to_string() })),
    )
        .into_response()
}

fn system_book_unavailable() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "System address book not available" })),
    )
        .into_response()
}

fn system_book_readonly() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(serde_json::json!({ "error": "System address book is read-only" })),
    )
        .into_response()
}

/// Attach a quoted `ETag` header to an existing response.
fn with_etag(mut response: Response, etag: &str) -> Response {
    if let Ok(val) = HeaderValue::from_str(&format!("\"{}\"", etag)) {
        response.headers_mut().insert(header::ETAG, val);
    }
    response
}

/// Return `false` when the stored etag does not satisfy the `If-Match` value.
/// Handles the `*` wildcard and quoted strings per RFC 7232.
fn if_match_passes(if_match: Option<&str>, stored_etag: &str) -> bool {
    match if_match {
        None | Some("*") => true,
        Some(value) => value.trim_matches('"') == stored_etag,
    }
}

/// Map a `UserDto` to a `ContactDto` so OxiCloud users appear as contacts
/// inside the virtual system address book.
fn user_to_contact(user: UserDto) -> ContactDto {
    ContactDto {
        id: user.id.clone(),
        address_book_id: SYSTEM_BOOK_ID.to_string(),
        uid: format!("{}@oxicloud", user.id),
        full_name: Some(user.username.clone()),
        first_name: None,
        last_name: None,
        nickname: None,
        email: vec![EmailDto {
            email: user.email,
            r#type: "work".to_string(),
            is_primary: true,
        }],
        phone: vec![],
        address: vec![],
        organization: Some("OxiCloud".to_string()),
        title: None,
        notes: None,
        photo_url: None,
        birthday: None,
        anniversary: None,
        created_at: user.created_at,
        updated_at: user.updated_at,
        etag: user.id,
    }
}

// ── Address books ─────────────────────────────────────────────────────────────

/// List all address books accessible to the current user (owned + shared),
/// plus the virtual read-only system book listing all OxiCloud users.
#[utoipa::path(
    get,
    path = "/api/address-books",
    responses(
        (status = 200, description = "List of address books"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "contacts"
)]
pub async fn list_address_books(
    State(state): State<ContactsApiState>,
    auth_user: AuthUser,
) -> impl IntoResponse {
    match state
        .contact_service
        .list_user_address_books(auth_user.id)
        .await
    {
        Ok(books) => {
            let user_id_str = auth_user.id.to_string();
            let mut response: Vec<AddressBookResponse> = books
                .into_iter()
                .map(|b| {
                    let is_readonly = b.owner_id != user_id_str;
                    AddressBookResponse {
                        id: b.id,
                        name: b.name,
                        owner_id: b.owner_id,
                        description: b.description,
                        color: b.color,
                        is_public: b.is_public,
                        is_readonly,
                        is_system: false,
                        created_at: b.created_at,
                        updated_at: b.updated_at,
                    }
                })
                .collect();

            if state.auth_service.is_some() {
                let now = Utc::now();
                response.push(AddressBookResponse {
                    id: SYSTEM_BOOK_ID.to_string(),
                    name: "OxiCloud Users".to_string(),
                    owner_id: "system".to_string(),
                    description: Some("All users registered on this OxiCloud instance".to_string()),
                    color: None,
                    is_public: false,
                    is_readonly: true,
                    is_system: true,
                    created_at: now,
                    updated_at: now,
                });
            }

            (StatusCode::OK, Json(response)).into_response()
        }
        Err(err) => {
            error!(
                "Error listing address books for user {}: {}",
                auth_user.id, err
            );
            domain_err_to_response(err)
        }
    }
}

/// Create a new personal address book.
#[utoipa::path(
    post,
    path = "/api/address-books",
    responses(
        (status = 201, description = "Address book created", body = AddressBookResponse),
        (status = 400, description = "Invalid input"),
    ),
    tag = "contacts"
)]
pub async fn create_address_book(
    State(state): State<ContactsApiState>,
    auth_user: AuthUser,
    Json(body): Json<CreateAddressBookRequest>,
) -> impl IntoResponse {
    let dto = CreateAddressBookDto {
        name: body.name,
        owner_id: auth_user.id.to_string(),
        description: body.description,
        color: body.color,
        is_public: body.is_public,
    };
    match state.contact_service.create_address_book(dto).await {
        Ok(book) => {
            let response = AddressBookResponse {
                id: book.id,
                name: book.name,
                owner_id: book.owner_id,
                description: book.description,
                color: book.color,
                is_public: book.is_public,
                is_readonly: false,
                is_system: false,
                created_at: book.created_at,
                updated_at: book.updated_at,
            };
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Err(err) => {
            error!("Error creating address book: {}", err);
            domain_err_to_response(err)
        }
    }
}

/// Update an address book's name, description, or color.
#[utoipa::path(
    put,
    path = "/api/address-books/{book_id}",
    params(("book_id" = String, Path, description = "Address book UUID")),
    responses(
        (status = 200, description = "Address book updated", body = AddressBookResponse),
        (status = 403, description = "Only the owner can update"),
        (status = 404, description = "Address book not found"),
    ),
    tag = "contacts"
)]
pub async fn update_address_book(
    State(state): State<ContactsApiState>,
    auth_user: AuthUser,
    Path(book_id): Path<String>,
    Json(body): Json<UpdateAddressBookRequest>,
) -> impl IntoResponse {
    if book_id == SYSTEM_BOOK_ID {
        return system_book_readonly();
    }
    let dto = UpdateAddressBookDto {
        name: body.name,
        description: body.description,
        color: body.color,
        is_public: body.is_public,
        user_id: auth_user.id.to_string(),
    };
    match state
        .contact_service
        .update_address_book(&book_id, dto)
        .await
    {
        Ok(book) => {
            let response = AddressBookResponse {
                id: book.id,
                name: book.name,
                owner_id: book.owner_id.clone(),
                description: book.description,
                color: book.color,
                is_public: book.is_public,
                is_readonly: book.owner_id != auth_user.id.to_string(),
                is_system: false,
                created_at: book.created_at,
                updated_at: book.updated_at,
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(err) => {
            error!("Error updating address book {}: {}", book_id, err);
            domain_err_to_response(err)
        }
    }
}

/// Delete an address book and all its contacts. Only the owner can do this.
#[utoipa::path(
    delete,
    path = "/api/address-books/{book_id}",
    params(("book_id" = String, Path, description = "Address book UUID")),
    responses(
        (status = 204, description = "Address book deleted"),
        (status = 403, description = "Only the owner can delete"),
        (status = 404, description = "Address book not found"),
    ),
    tag = "contacts"
)]
pub async fn delete_address_book(
    State(state): State<ContactsApiState>,
    auth_user: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    if book_id == SYSTEM_BOOK_ID {
        return system_book_readonly();
    }
    match state
        .contact_service
        .delete_address_book(&book_id, auth_user.id)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => {
            error!("Error deleting address book {}: {}", book_id, err);
            domain_err_to_response(err)
        }
    }
}

// ── Contacts ──────────────────────────────────────────────────────────────────

/// List contacts in an address book.
/// `book_id = "system"` returns all OxiCloud users (excluding the caller).
#[utoipa::path(
    get,
    path = "/api/address-books/{book_id}/contacts",
    params(
        ("book_id" = String, Path, description = "Address book UUID or \"system\""),
        ("limit"   = Option<i64>, Query, description = "Max results (default 100)"),
        ("offset"  = Option<i64>, Query, description = "Pagination offset (default 0)"),
    ),
    responses(
        (status = 200, description = "List of contacts"),
        (status = 403, description = "Access denied"),
        (status = 404, description = "Address book not found"),
    ),
    tag = "contacts"
)]
pub async fn list_contacts(
    State(state): State<ContactsApiState>,
    auth_user: AuthUser,
    Path(book_id): Path<String>,
    Query(params): Query<ListQuery>,
) -> impl IntoResponse {
    if book_id == SYSTEM_BOOK_ID {
        let Some(auth_service) = &state.auth_service else {
            return system_book_unavailable();
        };
        let caller_id = auth_user.id.to_string();
        match auth_service.list_users(params.limit, params.offset).await {
            Ok(users) => {
                let contacts: Vec<ContactDto> = users
                    .into_iter()
                    .filter(|u| u.id != caller_id)
                    .map(user_to_contact)
                    .collect();
                (StatusCode::OK, Json(contacts)).into_response()
            }
            Err(err) => {
                error!("Error listing OxiCloud users: {}", err);
                domain_err_to_response(err)
            }
        }
    } else {
        match state
            .contact_service
            .list_contacts(&book_id, auth_user.id)
            .await
        {
            Ok(contacts) => (StatusCode::OK, Json(contacts)).into_response(),
            Err(err) => {
                error!("Error listing contacts in book {}: {}", book_id, err);
                domain_err_to_response(err)
            }
        }
    }
}

/// Create a new contact in an address book.
#[utoipa::path(
    post,
    path = "/api/address-books/{book_id}/contacts",
    params(("book_id" = String, Path, description = "Address book UUID")),
    responses(
        (status = 201, description = "Contact created"),
        (status = 400, description = "Invalid input"),
        (status = 403, description = "Access denied or read-only book"),
        (status = 404, description = "Address book not found"),
    ),
    tag = "contacts"
)]
pub async fn create_contact(
    State(state): State<ContactsApiState>,
    auth_user: AuthUser,
    Path(book_id): Path<String>,
    Json(body): Json<CreateContactRequest>,
) -> impl IntoResponse {
    if book_id == SYSTEM_BOOK_ID {
        return system_book_readonly();
    }
    let dto = CreateContactDto {
        address_book_id: book_id.clone(),
        full_name: body.full_name,
        first_name: body.first_name,
        last_name: body.last_name,
        nickname: body.nickname,
        email: body.email,
        phone: body.phone,
        address: body.address,
        organization: body.organization,
        title: body.title,
        notes: body.notes,
        photo_url: body.photo_url,
        birthday: body.birthday,
        anniversary: body.anniversary,
        user_id: auth_user.id.to_string(),
    };
    match state.contact_service.create_contact(dto).await {
        Ok(contact) => {
            let etag = contact.etag.clone();
            with_etag((StatusCode::CREATED, Json(contact)).into_response(), &etag)
        }
        Err(err) => {
            error!("Error creating contact in book {}: {}", book_id, err);
            domain_err_to_response(err)
        }
    }
}

/// Get a single contact. Returns an `ETag` header for optimistic concurrency.
/// `book_id = "system"` looks up an OxiCloud user by UUID.
#[utoipa::path(
    get,
    path = "/api/address-books/{book_id}/contacts/{contact_id}",
    params(
        ("book_id"    = String, Path, description = "Address book UUID or \"system\""),
        ("contact_id" = String, Path, description = "Contact UUID"),
    ),
    responses(
        (status = 200, description = "Contact details"),
        (status = 403, description = "Access denied"),
        (status = 404, description = "Contact not found"),
    ),
    tag = "contacts"
)]
pub async fn get_contact(
    State(state): State<ContactsApiState>,
    auth_user: AuthUser,
    Path((book_id, contact_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if book_id == SYSTEM_BOOK_ID {
        let Some(auth_service) = &state.auth_service else {
            return system_book_unavailable();
        };
        let Ok(uuid) = Uuid::parse_str(&contact_id) else {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Invalid user ID format" })),
            )
                .into_response();
        };
        match auth_service.get_user_by_id(uuid).await {
            Ok(user) => {
                let contact = user_to_contact(user);
                let etag = contact.etag.clone();
                with_etag((StatusCode::OK, Json(contact)).into_response(), &etag)
            }
            Err(err) => {
                error!("Error fetching OxiCloud user {}: {}", contact_id, err);
                domain_err_to_response(err)
            }
        }
    } else {
        match state
            .contact_service
            .get_contact(&contact_id, auth_user.id)
            .await
        {
            Ok(contact) => {
                let etag = contact.etag.clone();
                with_etag((StatusCode::OK, Json(contact)).into_response(), &etag)
            }
            Err(err) => {
                error!("Error fetching contact {}: {}", contact_id, err);
                domain_err_to_response(err)
            }
        }
    }
}

/// Update a contact. Honours `If-Match` for optimistic concurrency — returns
/// `412 Precondition Failed` when the stored ETag does not match.
#[utoipa::path(
    put,
    path = "/api/address-books/{book_id}/contacts/{contact_id}",
    params(
        ("book_id"    = String, Path, description = "Address book UUID"),
        ("contact_id" = String, Path, description = "Contact UUID"),
    ),
    responses(
        (status = 200, description = "Contact updated"),
        (status = 400, description = "Invalid input"),
        (status = 403, description = "Access denied or read-only book"),
        (status = 404, description = "Contact not found"),
        (status = 412, description = "ETag mismatch — contact was modified"),
    ),
    tag = "contacts"
)]
pub async fn update_contact(
    State(state): State<ContactsApiState>,
    auth_user: AuthUser,
    headers: HeaderMap,
    Path((book_id, contact_id)): Path<(String, String)>,
    Json(body): Json<UpdateContactRequest>,
) -> impl IntoResponse {
    if book_id == SYSTEM_BOOK_ID {
        return system_book_readonly();
    }

    // ETag check: fetch current contact first, compare against If-Match.
    let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok());

    if if_match.is_some() {
        match state
            .contact_service
            .get_contact(&contact_id, auth_user.id)
            .await
        {
            Ok(current) => {
                if !if_match_passes(if_match, &current.etag) {
                    return (
                        StatusCode::PRECONDITION_FAILED,
                        Json(serde_json::json!({
                            "error": "Contact was modified — fetch the latest version and retry"
                        })),
                    )
                        .into_response();
                }
            }
            Err(err) => return domain_err_to_response(err),
        }
    }

    let dto = UpdateContactDto {
        full_name: body.full_name,
        first_name: body.first_name,
        last_name: body.last_name,
        nickname: body.nickname,
        email: body.email,
        phone: body.phone,
        address: body.address,
        organization: body.organization,
        title: body.title,
        notes: body.notes,
        photo_url: body.photo_url,
        birthday: body.birthday,
        anniversary: body.anniversary,
        user_id: auth_user.id.to_string(),
    };

    match state.contact_service.update_contact(&contact_id, dto).await {
        Ok(contact) => {
            let etag = contact.etag.clone();
            with_etag((StatusCode::OK, Json(contact)).into_response(), &etag)
        }
        Err(err) => {
            error!("Error updating contact {}: {}", contact_id, err);
            domain_err_to_response(err)
        }
    }
}

/// Delete a contact. Optionally honours `If-Match`.
#[utoipa::path(
    delete,
    path = "/api/address-books/{book_id}/contacts/{contact_id}",
    params(
        ("book_id"    = String, Path, description = "Address book UUID"),
        ("contact_id" = String, Path, description = "Contact UUID"),
    ),
    responses(
        (status = 204, description = "Contact deleted"),
        (status = 403, description = "Access denied or read-only book"),
        (status = 404, description = "Contact not found"),
        (status = 412, description = "ETag mismatch"),
    ),
    tag = "contacts"
)]
pub async fn delete_contact(
    State(state): State<ContactsApiState>,
    auth_user: AuthUser,
    headers: HeaderMap,
    Path((book_id, contact_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if book_id == SYSTEM_BOOK_ID {
        return system_book_readonly();
    }

    let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok());

    if if_match.is_some() {
        match state
            .contact_service
            .get_contact(&contact_id, auth_user.id)
            .await
        {
            Ok(current) => {
                if !if_match_passes(if_match, &current.etag) {
                    return (
                        StatusCode::PRECONDITION_FAILED,
                        Json(serde_json::json!({
                            "error": "Contact was modified — fetch the latest version and retry"
                        })),
                    )
                        .into_response();
                }
            }
            Err(err) => return domain_err_to_response(err),
        }
    }

    match state
        .contact_service
        .delete_contact(&contact_id, auth_user.id)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => {
            error!("Error deleting contact {}: {}", contact_id, err);
            domain_err_to_response(err)
        }
    }
}

// ── Groups ────────────────────────────────────────────────────────────────────

/// List contact groups in an address book. The system book has no groups.
#[utoipa::path(
    get,
    path = "/api/address-books/{book_id}/groups",
    params(("book_id" = String, Path, description = "Address book UUID or \"system\"")),
    responses(
        (status = 200, description = "List of contact groups"),
        (status = 403, description = "Access denied"),
        (status = 404, description = "Address book not found"),
    ),
    tag = "contacts"
)]
pub async fn list_groups(
    State(state): State<ContactsApiState>,
    auth_user: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    if book_id == SYSTEM_BOOK_ID {
        return (StatusCode::OK, Json(Vec::<ContactGroupDto>::new())).into_response();
    }
    match state
        .contact_service
        .list_groups(&book_id, auth_user.id)
        .await
    {
        Ok(groups) => (StatusCode::OK, Json(groups)).into_response(),
        Err(err) => {
            error!("Error listing groups in book {}: {}", book_id, err);
            domain_err_to_response(err)
        }
    }
}

/// Create a contact group in an address book.
#[utoipa::path(
    post,
    path = "/api/address-books/{book_id}/groups",
    params(("book_id" = String, Path, description = "Address book UUID")),
    responses(
        (status = 201, description = "Group created"),
        (status = 403, description = "Access denied or read-only book"),
        (status = 404, description = "Address book not found"),
    ),
    tag = "contacts"
)]
pub async fn create_group(
    State(state): State<ContactsApiState>,
    auth_user: AuthUser,
    Path(book_id): Path<String>,
    Json(body): Json<GroupNameRequest>,
) -> impl IntoResponse {
    if book_id == SYSTEM_BOOK_ID {
        return system_book_readonly();
    }
    let dto = CreateContactGroupDto {
        address_book_id: book_id.clone(),
        name: body.name,
        user_id: auth_user.id.to_string(),
    };
    match state.contact_service.create_group(dto).await {
        Ok(group) => (StatusCode::CREATED, Json(group)).into_response(),
        Err(err) => {
            error!("Error creating group in book {}: {}", book_id, err);
            domain_err_to_response(err)
        }
    }
}

/// Get a single contact group by ID.
#[utoipa::path(
    get,
    path = "/api/address-books/{book_id}/groups/{group_id}",
    params(
        ("book_id"  = String, Path, description = "Address book UUID"),
        ("group_id" = String, Path, description = "Group UUID"),
    ),
    responses(
        (status = 200, description = "Group details"),
        (status = 403, description = "Access denied"),
        (status = 404, description = "Group not found"),
    ),
    tag = "contacts"
)]
pub async fn get_group(
    State(state): State<ContactsApiState>,
    auth_user: AuthUser,
    Path((book_id, group_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if book_id == SYSTEM_BOOK_ID {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "System address book has no groups" })),
        )
            .into_response();
    }
    match state
        .contact_service
        .get_group(&group_id, auth_user.id)
        .await
    {
        Ok(group) => (StatusCode::OK, Json(group)).into_response(),
        Err(err) => {
            error!("Error fetching group {}: {}", group_id, err);
            domain_err_to_response(err)
        }
    }
}

/// Rename a contact group.
#[utoipa::path(
    put,
    path = "/api/address-books/{book_id}/groups/{group_id}",
    params(
        ("book_id"  = String, Path, description = "Address book UUID"),
        ("group_id" = String, Path, description = "Group UUID"),
    ),
    responses(
        (status = 200, description = "Group updated"),
        (status = 403, description = "Access denied or read-only book"),
        (status = 404, description = "Group not found"),
    ),
    tag = "contacts"
)]
pub async fn update_group(
    State(state): State<ContactsApiState>,
    auth_user: AuthUser,
    Path((book_id, group_id)): Path<(String, String)>,
    Json(body): Json<GroupNameRequest>,
) -> impl IntoResponse {
    if book_id == SYSTEM_BOOK_ID {
        return system_book_readonly();
    }
    let dto = UpdateContactGroupDto {
        name: body.name,
        user_id: auth_user.id.to_string(),
    };
    match state.contact_service.update_group(&group_id, dto).await {
        Ok(group) => (StatusCode::OK, Json(group)).into_response(),
        Err(err) => {
            error!("Error updating group {}: {}", group_id, err);
            domain_err_to_response(err)
        }
    }
}

/// Delete a contact group.
#[utoipa::path(
    delete,
    path = "/api/address-books/{book_id}/groups/{group_id}",
    params(
        ("book_id"  = String, Path, description = "Address book UUID"),
        ("group_id" = String, Path, description = "Group UUID"),
    ),
    responses(
        (status = 204, description = "Group deleted"),
        (status = 403, description = "Access denied or read-only book"),
        (status = 404, description = "Group not found"),
    ),
    tag = "contacts"
)]
pub async fn delete_group(
    State(state): State<ContactsApiState>,
    auth_user: AuthUser,
    Path((book_id, group_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if book_id == SYSTEM_BOOK_ID {
        return system_book_readonly();
    }
    match state
        .contact_service
        .delete_group(&group_id, auth_user.id)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => {
            error!("Error deleting group {}: {}", group_id, err);
            domain_err_to_response(err)
        }
    }
}

// ── Group membership ──────────────────────────────────────────────────────────

/// List contacts that belong to a group.
#[utoipa::path(
    get,
    path = "/api/address-books/{book_id}/groups/{group_id}/contacts",
    params(
        ("book_id"  = String, Path, description = "Address book UUID"),
        ("group_id" = String, Path, description = "Group UUID"),
    ),
    responses(
        (status = 200, description = "Contacts in the group"),
        (status = 403, description = "Access denied"),
        (status = 404, description = "Group not found"),
    ),
    tag = "contacts"
)]
pub async fn list_contacts_in_group(
    State(state): State<ContactsApiState>,
    auth_user: AuthUser,
    Path((book_id, group_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if book_id == SYSTEM_BOOK_ID {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "System address book has no groups" })),
        )
            .into_response();
    }
    match state
        .contact_service
        .list_contacts_in_group(&group_id, auth_user.id)
        .await
    {
        Ok(contacts) => (StatusCode::OK, Json(contacts)).into_response(),
        Err(err) => {
            error!("Error listing contacts in group {}: {}", group_id, err);
            domain_err_to_response(err)
        }
    }
}

/// Add a contact to a group.
#[utoipa::path(
    post,
    path = "/api/address-books/{book_id}/groups/{group_id}/contacts",
    params(
        ("book_id"  = String, Path, description = "Address book UUID"),
        ("group_id" = String, Path, description = "Group UUID"),
    ),
    responses(
        (status = 204, description = "Contact added to group"),
        (status = 400, description = "Invalid contact ID"),
        (status = 403, description = "Access denied or read-only book"),
        (status = 404, description = "Group or contact not found"),
    ),
    tag = "contacts"
)]
pub async fn add_contact_to_group(
    State(state): State<ContactsApiState>,
    auth_user: AuthUser,
    Path((book_id, group_id)): Path<(String, String)>,
    Json(body): Json<AddMemberRequest>,
) -> impl IntoResponse {
    if book_id == SYSTEM_BOOK_ID {
        return system_book_readonly();
    }
    let dto = GroupMembershipDto {
        group_id: group_id.clone(),
        contact_id: body.contact_id,
    };
    match state
        .contact_service
        .add_contact_to_group(dto, auth_user.id)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => {
            error!("Error adding contact to group {}: {}", group_id, err);
            domain_err_to_response(err)
        }
    }
}

/// Remove a contact from a group.
#[utoipa::path(
    delete,
    path = "/api/address-books/{book_id}/groups/{group_id}/contacts/{contact_id}",
    params(
        ("book_id"    = String, Path, description = "Address book UUID"),
        ("group_id"   = String, Path, description = "Group UUID"),
        ("contact_id" = String, Path, description = "Contact UUID"),
    ),
    responses(
        (status = 204, description = "Contact removed from group"),
        (status = 403, description = "Access denied or read-only book"),
        (status = 404, description = "Group or contact not found"),
    ),
    tag = "contacts"
)]
pub async fn remove_contact_from_group(
    State(state): State<ContactsApiState>,
    auth_user: AuthUser,
    Path((book_id, group_id, contact_id)): Path<(String, String, String)>,
) -> impl IntoResponse {
    if book_id == SYSTEM_BOOK_ID {
        return system_book_readonly();
    }
    let dto = GroupMembershipDto {
        group_id: group_id.clone(),
        contact_id,
    };
    match state
        .contact_service
        .remove_contact_from_group(dto, auth_user.id)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => {
            error!("Error removing contact from group {}: {}", group_id, err);
            domain_err_to_response(err)
        }
    }
}
