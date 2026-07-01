---
title: "Registry HTTP"
---

# Registry HTTP

Every route registered on the axum router in `memstead-registry`. The route inventory is lifted by static scan of the router declaration (`memstead-registry/src/lib.rs`); each per-route section also surfaces the handler's live signature and the `ApiError` variants its body emits. A machine-readable OpenAPI 3.1.0 document covering the same paths is published at [`/openapi.json`](/openapi.json). Full request / response JSON schemas would require `utoipa` annotations on every handler; today the contract is the signature, the path inventory, and the per-route error variants.

**Routes:** 9

## Index

- [`GET` `/`](#get-)
- [`POST` `/api/admin/denylist`](#post-api-admin-denylist)
- [`GET` `/api/index`](#get-api-index)
- [`POST` `/api/publish`](#post-api-publish)
- [`DELETE` `/api/vault/{scope}/{name_ext}`](#delete-api-vault--scope---name-ext-)
- [`GET` `/api/vault/{scope}/{name_ext}`](#get-api-vault--scope---name-ext-)
- [`GET` `/api/vault/{scope}/{name}/{version_ext}`](#get-api-vault--scope---name---version-ext-)
- [`GET` `/healthz`](#get-healthz)
- [`GET` `/v/{scope}/{name}`](#get-v--scope---name-)

## `GET /` <span id="get-"></span>

**Handler:** `handlers::static_assets::get_root`

**Signature:**

```rust
pub async fn get_root() -> ApiResult<Response>
```

**Errors:** _(no `ApiError` variants found in handler body)_

## `POST /api/admin/denylist` <span id="post-api-admin-denylist"></span>

**Handler:** `handlers::admin::post_denylist`

**Signature:**

```rust
pub async fn post_denylist(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<DenylistRequest>,
) -> ApiResult<impl IntoResponse>
```

**`ApiError` variants emitted:** `Forbidden`, `Internal`, `InvalidContentHash`

## `GET /api/index` <span id="get-api-index"></span>

**Handler:** `handlers::index::get_index`

**Signature:**

```rust
pub async fn get_index(State(state): State<AppState>) -> ApiResult<Json<IndexResponse>>
```

**`ApiError` variants emitted:** `Internal`

## `POST /api/publish` <span id="post-api-publish"></span>

**Handler:** `handlers::publish::post_publish`

**Signature:**

```rust
pub async fn post_publish(
    State(state): State<AppState>,
    req: Request<Body>,
) -> ApiResult<impl IntoResponse>
```

**`ApiError` variants emitted:** `BodyEmpty`, `BodyTooLarge`, `ContentBlocked`, `Internal`, `RateLimited`, `TermsNotAccepted`, `Tombstoned`, `Validation`, `ValidatorTimeout`, `VersionExists`

## `DELETE /api/vault/{scope}/{name_ext}` <span id="delete-api-vault--scope---name-ext-"></span>

**Handler:** `handlers::unpublish::delete_unpublish`

**Signature:**

```rust
pub async fn delete_unpublish(
    State(state): State<AppState>,
    Path((scope, name_ext)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<impl IntoResponse>
```

**`ApiError` variants emitted:** `Forbidden`, `Internal`, `NotFound`

## `GET /api/vault/{scope}/{name_ext}` <span id="get-api-vault--scope---name-ext-"></span>

**Handler:** `handlers::artifacts::get_artifact`

**Signature:**

```rust
pub async fn get_artifact(
    State(state): State<AppState>,
    Path((scope, name_ext)): Path<(String, String)>,
) -> ApiResult<Response>
```

**`ApiError` variants emitted:** `NotFound`

## `GET /api/vault/{scope}/{name}/{version_ext}` <span id="get-api-vault--scope---name---version-ext-"></span>

**Handler:** `handlers::artifacts::get_artifact_versioned`

**Signature:**

```rust
pub async fn get_artifact_versioned(
    State(state): State<AppState>,
    Path((scope, name, version_ext)): Path<(String, String, String)>,
) -> ApiResult<Response>
```

**`ApiError` variants emitted:** `NotFound`

## `GET /healthz` <span id="get-healthz"></span>

**Handler:** `handlers::health::get_healthz`

**Signature:**

```rust
pub async fn get_healthz(State(state): State<AppState>) -> impl IntoResponse
```

**Errors:** _(no `ApiError` variants found in handler body)_

## `GET /v/{scope}/{name}` <span id="get-v--scope---name-"></span>

**Handler:** `handlers::vault_page::get_vault_page`

**Signature:**

```rust
pub async fn get_vault_page(
    State(state): State<AppState>,
    Path((scope, name)): Path<(String, String)>,
) -> ApiResult<Response>
```

**`ApiError` variants emitted:** `Internal`

