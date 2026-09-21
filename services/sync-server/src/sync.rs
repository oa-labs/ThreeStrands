use std::{collections::BTreeSet, str::FromStr, sync::Arc};

use axum::{extract::{Path, State}, Json};
use chrono::Utc;
use serde_json::Value;
use sqlx::{FromRow, Postgres, Transaction};
use threestrands_sync_protocol::{
    merge_patch, EntityType, OperationAck, OperationStatus, ResolveConflictRequest, SyncConflict,
    SyncOperation, SyncRecord, SyncRequest, SyncResponse, MAX_OPERATIONS_PER_REQUEST,
};
use uuid::Uuid;

use crate::{ApiError, AppState, AuthContext};

const PAGE_SIZE: i64 = 500;

pub async fn synchronize(
    State(state): State<Arc<AppState>>,
    auth: AuthContext,
    Json(request): Json<SyncRequest>,
) -> Result<Json<SyncResponse>, ApiError> {
    if request.cursor < 0 || request.operations.len() > MAX_OPERATIONS_PER_REQUEST {
        return Err(ApiError::bad_request("Invalid sync request"));
    }
    require_sync(&state, auth.user_id).await?;
    for operation in &request.operations {
        operation.validate().map_err(ApiError::bad_request)?;
        if operation.device_id != auth.device_id.to_string() {
            return Err(ApiError::forbidden("A device cannot submit another device's operations"));
        }
    }

    let mut tx = state.pool.begin().await?;
    set_tenant(&mut tx, auth.user_id).await?;
    let mut acknowledgements = Vec::with_capacity(request.operations.len());
    for operation in &request.operations {
        acknowledgements.push(apply_operation(&mut tx, &auth, operation).await?);
    }
    let rows = sqlx::query_as::<_, ChangeRow>(
        "SELECT sequence,entity_type,entity_id,version,payload,deleted
         FROM sync_changes WHERE user_id=$1 AND sequence>$2 ORDER BY sequence LIMIT $3",
    )
    .bind(auth.user_id).bind(request.cursor).bind(PAGE_SIZE + 1)
    .fetch_all(&mut *tx).await?;
    let has_more = rows.len() as i64 > PAGE_SIZE;
    let rows = rows.into_iter().take(PAGE_SIZE as usize).collect::<Vec<_>>();
    let next_cursor = rows.last().map_or(request.cursor, |row| row.sequence);
    let conflicts = fetch_conflicts(&mut tx, auth.user_id).await?;
    tx.commit().await?;
    Ok(Json(SyncResponse {
        acknowledgements,
        changes: rows.into_iter().map(ChangeRow::try_into).collect::<Result<_, _>>()?,
        conflicts,
        next_cursor,
        has_more,
    }))
}

async fn apply_operation(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    operation: &SyncOperation,
) -> Result<OperationAck, ApiError> {
    if let Some(value) = sqlx::query_scalar::<_, Value>(
        "SELECT acknowledgement FROM processed_operations WHERE user_id=$1 AND operation_id=$2",
    ).bind(auth.user_id).bind(&operation.operation_id).fetch_optional(&mut **tx).await? {
        let mut ack: OperationAck = serde_json::from_value(value)?;
        ack.status = OperationStatus::Duplicate;
        return Ok(ack);
    }

    let entity_type = operation.entity_type.as_str();
    let current = sqlx::query_as::<_, RecordRow>(
        "SELECT version,payload,deleted FROM sync_records
         WHERE user_id=$1 AND entity_type=$2 AND entity_id=$3 FOR UPDATE",
    ).bind(auth.user_id).bind(entity_type).bind(&operation.entity_id)
    .fetch_optional(&mut **tx).await?;

    let ack = match current {
        None if operation.base_version != 0 => create_conflict(tx, auth, operation, None, true, 0, BTreeSet::from(["*".into()])).await?,
        None => {
            if operation.deleted {
                create_version(tx, auth, operation, None, true).await?
            } else {
                let payload = operation.patch.clone().ok_or_else(|| ApiError::bad_request("Missing patch"))?;
                operation.entity_type.validate_payload(&payload).map_err(ApiError::bad_request)?;
                create_version(tx, auth, operation, Some(payload), false).await?
            }
        }
        Some(current) if current.version == operation.base_version => {
            apply_to_current(tx, auth, operation, current).await?
        }
        Some(current) if operation.base_version > current.version => {
            create_conflict(tx, auth, operation, current.payload, current.deleted, current.version, BTreeSet::from(["*".into()])).await?
        }
        Some(current) => {
            let changed_since = sqlx::query_scalar::<_, Vec<String>>(
                "SELECT COALESCE(array_agg(DISTINCT field), ARRAY[]::text[])
                 FROM record_revisions, unnest(changed_fields) field
                 WHERE user_id=$1 AND entity_type=$2 AND entity_id=$3 AND version>$4",
            ).bind(auth.user_id).bind(entity_type).bind(&operation.entity_id).bind(operation.base_version)
            .fetch_one(&mut **tx).await?;
            let changed_since: BTreeSet<String> = changed_since.into_iter().collect();
            let overlap = overlap(&changed_since, &operation.changed_fields);
            if overlap.is_empty() && !current.deleted && !operation.deleted {
                apply_to_current(tx, auth, operation, current).await?
            } else {
                create_conflict(tx, auth, operation, current.payload, current.deleted, current.version, overlap).await?
            }
        }
    };
    sqlx::query(
        "INSERT INTO processed_operations(user_id,operation_id,acknowledgement) VALUES($1,$2,$3)",
    ).bind(auth.user_id).bind(&operation.operation_id).bind(serde_json::to_value(&ack)?)
    .execute(&mut **tx).await?;
    Ok(ack)
}

async fn apply_to_current(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    operation: &SyncOperation,
    current: RecordRow,
) -> Result<OperationAck, ApiError> {
    if operation.deleted {
        return create_version(tx, auth, operation, None, true).await;
    }
    if current.deleted {
        return create_conflict(tx, auth, operation, None, true, current.version, BTreeSet::from(["*".into()])).await;
    }
    let mut payload = current.payload.ok_or_else(|| ApiError::conflict("Stored record has no payload"))?;
    merge_patch(&mut payload, operation.patch.as_ref().ok_or_else(|| ApiError::bad_request("Missing patch"))?)
        .map_err(ApiError::bad_request)?;
    operation.entity_type.validate_payload(&payload).map_err(ApiError::bad_request)?;
    create_version(tx, auth, operation, Some(payload), false).await
}

async fn create_version(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    operation: &SyncOperation,
    payload: Option<Value>,
    deleted: bool,
) -> Result<OperationAck, ApiError> {
    let version = sqlx::query_scalar::<_, i64>("SELECT nextval('sync_version_seq')")
        .fetch_one(&mut **tx).await?;
    sqlx::query(
        "INSERT INTO sync_records(user_id,entity_type,entity_id,version,payload,deleted,updated_by)
         VALUES($1,$2,$3,$4,$5,$6,$7)
         ON CONFLICT(user_id,entity_type,entity_id) DO UPDATE SET
           version=excluded.version,payload=excluded.payload,deleted=excluded.deleted,
           updated_by=excluded.updated_by,updated_at=now()",
    ).bind(auth.user_id).bind(operation.entity_type.as_str()).bind(&operation.entity_id)
    .bind(version).bind(&payload).bind(deleted).bind(auth.device_id).execute(&mut **tx).await?;
    let fields: Vec<String> = operation.changed_fields.iter().cloned().collect();
    sqlx::query(
        "INSERT INTO record_revisions(user_id,entity_type,entity_id,version,changed_fields,payload,deleted,updated_by)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8)",
    ).bind(auth.user_id).bind(operation.entity_type.as_str()).bind(&operation.entity_id)
    .bind(version).bind(fields).bind(&payload).bind(deleted).bind(auth.device_id).execute(&mut **tx).await?;
    sqlx::query(
        "INSERT INTO sync_changes(sequence,user_id,entity_type,entity_id,version,payload,deleted)
         VALUES($1,$2,$3,$4,$5,$6,$7)",
    ).bind(version).bind(auth.user_id).bind(operation.entity_type.as_str()).bind(&operation.entity_id)
    .bind(version).bind(payload).bind(deleted).execute(&mut **tx).await?;
    Ok(OperationAck { operation_id: operation.operation_id.clone(), status: OperationStatus::Applied, version: Some(version), conflict_id: None })
}

async fn create_conflict(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    operation: &SyncOperation,
    cloud_payload: Option<Value>,
    cloud_deleted: bool,
    current_version: i64,
    mut overlapping_fields: BTreeSet<String>,
) -> Result<OperationAck, ApiError> {
    if overlapping_fields.is_empty() { overlapping_fields.insert("*".into()); }
    let id = Uuid::new_v4();
    let fields: Vec<String> = overlapping_fields.into_iter().collect();
    sqlx::query(
        "INSERT INTO sync_conflicts(id,user_id,entity_type,entity_id,current_version,overlapping_fields,cloud_payload,cloud_deleted,device_patch,device_deleted,device_id)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    ).bind(id).bind(auth.user_id).bind(operation.entity_type.as_str()).bind(&operation.entity_id)
    .bind(current_version).bind(fields).bind(cloud_payload).bind(cloud_deleted).bind(&operation.patch)
    .bind(operation.deleted).bind(auth.device_id).execute(&mut **tx).await?;
    Ok(OperationAck { operation_id: operation.operation_id.clone(), status: OperationStatus::Conflict, version: Some(current_version), conflict_id: Some(id.to_string()) })
}

fn overlap(left: &BTreeSet<String>, right: &BTreeSet<String>) -> BTreeSet<String> {
    if left.contains("*") || right.contains("*") {
        return BTreeSet::from(["*".into()]);
    }
    left.intersection(right).cloned().collect()
}

#[derive(FromRow)]
struct RecordRow { version: i64, payload: Option<Value>, deleted: bool }

#[derive(FromRow)]
struct ChangeRow {
    sequence: i64,
    entity_type: String,
    entity_id: String,
    version: i64,
    payload: Option<Value>,
    deleted: bool,
}

impl TryFrom<ChangeRow> for SyncRecord {
    type Error = ApiError;
    fn try_from(value: ChangeRow) -> Result<Self, Self::Error> {
        Ok(Self { entity_type: EntityType::from_str(&value.entity_type).map_err(ApiError::bad_request)?, entity_id: value.entity_id, version: value.version, payload: value.payload, deleted: value.deleted })
    }
}

#[derive(FromRow)]
struct ConflictRow {
    id: Uuid,
    entity_type: String,
    entity_id: String,
    current_version: i64,
    overlapping_fields: Vec<String>,
    cloud_payload: Option<Value>,
    cloud_deleted: bool,
    device_patch: Option<Value>,
    device_deleted: bool,
    created_at: chrono::DateTime<Utc>,
}

impl TryFrom<ConflictRow> for SyncConflict {
    type Error = ApiError;
    fn try_from(value: ConflictRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id.to_string(), entity_type: EntityType::from_str(&value.entity_type).map_err(ApiError::bad_request)?,
            entity_id: value.entity_id, current_version: value.current_version,
            overlapping_fields: value.overlapping_fields.into_iter().collect(), cloud_payload: value.cloud_payload, cloud_deleted: value.cloud_deleted,
            device_patch: value.device_patch, device_deleted: value.device_deleted, created_at: value.created_at.to_rfc3339(),
        })
    }
}

async fn fetch_conflicts(tx: &mut Transaction<'_, Postgres>, user_id: Uuid) -> Result<Vec<SyncConflict>, ApiError> {
    let rows = sqlx::query_as::<_, ConflictRow>(
        "SELECT id,entity_type,entity_id,current_version,overlapping_fields,cloud_payload,cloud_deleted,device_patch,device_deleted,created_at
         FROM sync_conflicts WHERE user_id=$1 AND resolved_at IS NULL ORDER BY created_at",
    ).bind(user_id).fetch_all(&mut **tx).await?;
    rows.into_iter().map(TryInto::try_into).collect()
}

pub async fn list_conflicts(
    State(state): State<Arc<AppState>>, auth: AuthContext,
) -> Result<Json<Vec<SyncConflict>>, ApiError> {
    require_sync(&state, auth.user_id).await?;
    let mut tx = state.pool.begin().await?;
    set_tenant(&mut tx, auth.user_id).await?;
    let conflicts = fetch_conflicts(&mut tx, auth.user_id).await?;
    tx.commit().await?;
    Ok(Json(conflicts))
}

pub async fn resolve_conflict(
    State(state): State<Arc<AppState>>, auth: AuthContext, Path(id): Path<Uuid>,
    Json(request): Json<ResolveConflictRequest>,
) -> Result<Json<SyncRecord>, ApiError> {
    require_sync(&state, auth.user_id).await?;
    let mut tx = state.pool.begin().await?;
    set_tenant(&mut tx, auth.user_id).await?;
    let conflict = sqlx::query_as::<_, ConflictRow>(
        "SELECT id,entity_type,entity_id,current_version,overlapping_fields,cloud_payload,cloud_deleted,device_patch,device_deleted,created_at
         FROM sync_conflicts WHERE id=$1 AND user_id=$2 AND resolved_at IS NULL FOR UPDATE",
    ).bind(id).bind(auth.user_id).fetch_optional(&mut *tx).await?.ok_or_else(ApiError::not_found)?;
    if request.current_version != conflict.current_version {
        return Err(ApiError::conflict("The cloud record changed while resolving this conflict"));
    }
    let entity_type = EntityType::from_str(&conflict.entity_type).map_err(ApiError::bad_request)?;
    if !request.deleted {
        entity_type.validate_payload(request.resolved_payload.as_ref().ok_or_else(|| ApiError::bad_request("Resolved payload required"))?)
            .map_err(ApiError::bad_request)?;
    }
    let operation = SyncOperation {
        operation_id: format!("resolve-{id}"), device_id: auth.device_id.to_string(), entity_type,
        entity_id: conflict.entity_id.clone(), base_version: conflict.current_version,
        changed_fields: BTreeSet::from(["*".into()]), patch: request.resolved_payload.clone(),
        deleted: request.deleted, local_sequence: 0,
    };
    let ack = create_version(&mut tx, &auth, &operation, request.resolved_payload, request.deleted).await?;
    sqlx::query("UPDATE sync_conflicts SET resolved_at=now() WHERE id=$1 AND user_id=$2")
        .bind(id).bind(auth.user_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(SyncRecord { entity_type, entity_id: conflict.entity_id, version: ack.version.unwrap_or_default(), payload: operation.patch, deleted: request.deleted }))
}

async fn require_sync(state: &AppState, user_id: Uuid) -> Result<(), ApiError> {
    let enabled = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM entitlements WHERE user_id=$1 AND feature='sync' AND (expires_at IS NULL OR expires_at>now()))",
    ).bind(user_id).fetch_one(&state.pool).await?;
    if enabled { Ok(()) } else { Err(ApiError::forbidden("Cross-device sync is not enabled for this account")) }
}

async fn set_tenant(tx: &mut Transaction<'_, Postgres>, user_id: Uuid) -> Result<(), ApiError> {
    sqlx::query("SELECT set_config('threestrands.user_id',$1,true)")
        .bind(user_id.to_string()).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_intersection_treats_deletion_as_wildcard() {
        assert_eq!(overlap(&BTreeSet::from(["title".into()]), &BTreeSet::from(["notes".into()])), BTreeSet::new());
        assert_eq!(overlap(&BTreeSet::from(["*".into()]), &BTreeSet::from(["notes".into()])), BTreeSet::from(["*".into()]));
    }
}
