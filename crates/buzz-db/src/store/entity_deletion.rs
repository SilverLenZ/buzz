//! Operator-global intake for synthetic user-deletion requests.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{Db, DbError, Result};

/// A demonstration request; approval authorizes only the synthetic workflow.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct EntityRequest {
    /// Stable request identifier.
    pub id: Uuid,
    /// Currently always `user`.
    pub entity_type: String,
    /// Synthetic `demo:` label, not a resolved account identity.
    pub target: String,
    /// `submitted` (legacy intake), `inventoried`, or `approved`.
    pub status: String,
    /// Operator identity supplied at submission.
    pub requested_by: String,
    /// Optional explanation supplied at submission.
    pub reason: Option<String>,
    /// Time of the original submission.
    pub created_at: DateTime<Utc>,
    /// Time of the last change; identical resubmissions do not change it.
    pub updated_at: DateTime<Utc>,
    /// Saved synthetic communities in processing order; absent on legacy intake.
    pub inventory_manifest: Option<serde_json::Value>,
    /// SHA-256 of the saved inventory.
    pub inventory_digest: Option<String>,
}

/// Writer-database access to synthetic deletion requests.
#[derive(Clone)]
pub struct EntityDeletionStore {
    pool: PgPool,
}

impl Db {
    /// Use the writer pool for operator-global request intake and inspection.
    pub fn entity_deletion_store(&self) -> EntityDeletionStore {
        EntityDeletionStore {
            pool: self.pool().clone(),
        }
    }
}

impl EntityDeletionStore {
    /// Save a request or return an identical previous submission.
    ///
    /// A conflicting operator or reason returns an error without changing the
    /// existing request. Concurrent identical submissions converge on one row.
    /// Legacy submitted rows acquire their demo inventory on matching resubmission.
    pub async fn submit(
        &self,
        target: &str,
        requested_by: &str,
        reason: Option<&str>,
    ) -> Result<EntityRequest> {
        validate_submission(target, requested_by, reason)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO entity_deletion_requests (target, requested_by, reason)
             VALUES ($1, $2, $3) ON CONFLICT (entity_type, target) DO NOTHING",
        )
        .bind(target)
        .bind(requested_by)
        .bind(reason)
        .execute(&mut *tx)
        .await?;
        let mut request: EntityRequest = sqlx::query_as(
            "SELECT id, entity_type, target, status, requested_by, reason, created_at, updated_at,
                    inventory_manifest, inventory_digest
             FROM entity_deletion_requests WHERE entity_type = 'user' AND target = $1 FOR UPDATE",
        )
        .bind(target)
        .fetch_one(&mut *tx)
        .await?;
        if request.requested_by != requested_by || request.reason.as_deref() != reason {
            return Err(DbError::DeletionSafety(format!(
                "target already has request {}; inspect it before resubmission",
                request.id
            )));
        }
        if request.status == "submitted" {
            let inventory = demo_inventory(target);
            let digest = inventory_digest(&inventory)?;
            request = sqlx::query_as(
                "UPDATE entity_deletion_requests
                 SET status = 'inventoried', inventory_manifest = $2, inventory_digest = $3,
                     updated_at = now()
                 WHERE id = $1
                 RETURNING id, entity_type, target, status, requested_by, reason, created_at,
                           updated_at, inventory_manifest, inventory_digest",
            )
            .bind(request.id)
            .bind(inventory)
            .bind(digest)
            .fetch_one(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(request)
    }

    /// Load a request from the authoritative writer database.
    pub async fn get(&self, id: Uuid) -> Result<EntityRequest> {
        sqlx::query_as(
            "SELECT id, entity_type, target, status, requested_by, reason, created_at, updated_at,
                    inventory_manifest, inventory_digest
             FROM entity_deletion_requests WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("entity deletion request {id}")))
    }

    /// List up to 1,000 requests, newest first.
    pub async fn list(&self, limit: u16) -> Result<Vec<EntityRequest>> {
        Ok(sqlx::query_as(
            "SELECT id, entity_type, target, status, requested_by, reason, created_at, updated_at,
                    inventory_manifest, inventory_digest
             FROM entity_deletion_requests ORDER BY created_at DESC, id LIMIT $1",
        )
        .bind(i64::from(limit.clamp(1, 1000)))
        .fetch_all(&self.pool)
        .await?)
    }
}

fn validate_submission(target: &str, requested_by: &str, reason: Option<&str>) -> Result<()> {
    let slug = target.strip_prefix("demo:").unwrap_or_default();
    if slug.is_empty()
        || slug.len() > 64
        || !slug.as_bytes()[0].is_ascii_alphanumeric()
        || !slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err(DbError::DeletionSafety(
            "target must be demo:<1..64 lowercase letters, digits, underscores or hyphens>".into(),
        ));
    }
    validate_operator(requested_by)?;
    if reason.is_some_and(|r| r.len() > 4096) {
        return Err(DbError::DeletionSafety("reason exceeds 4096 bytes".into()));
    }
    Ok(())
}

fn validate_operator(operator: &str) -> Result<()> {
    if operator.is_empty() || operator.len() > 256 || operator.trim() != operator {
        return Err(DbError::DeletionSafety(
            "operator identity must be 1..256 bytes without surrounding whitespace".into(),
        ));
    }
    Ok(())
}

fn demo_inventory(target: &str) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "target": target,
        "communities": [
            {
                "community_id": "00000000-0000-0000-0000-00000000000b",
                "label": "demo:community-b",
                "relationship": "participant",
                "action": "delete_subject_data"
            },
            {
                "community_id": "00000000-0000-0000-0000-00000000000c",
                "label": "demo:community-c",
                "relationship": "former_participant",
                "action": "delete_subject_data"
            }
        ]
    })
}

fn inventory_digest(inventory: &serde_json::Value) -> Result<String> {
    // JSONB does not preserve object-key order. Sorting preserves the digest
    // across a database round trip while retaining the community array order.
    let mut canonical = inventory.clone();
    canonical.sort_all_objects();
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&canonical)?)))
}

mod approval;
pub use approval::{EntityApproval, EntityInspection, EntityStage};

#[cfg(test)]
#[path = "entity_deletion_postgres_tests.rs"]
mod postgres_tests;
