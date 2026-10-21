use super::*;

async fn store() -> EntityDeletionStore {
    EntityDeletionStore {
        pool: PgPool::connect(&crate::test_support::database_url())
            .await
            .expect("connect isolated database"),
    }
}

async fn assert_plain_schema(pool: &PgPool) {
    let fences: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_trigger
         WHERE tgrelid IN ('entity_deletion_requests'::regclass,
                           'entity_deletion_stage_history'::regclass)",
    )
    .fetch_one(pool)
    .await
    .expect("fences");
    assert_eq!(fences, 0);
    let foreign_keys: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_constraint
         WHERE conrelid IN ('entity_deletion_requests'::regclass,
                            'entity_deletion_stage_history'::regclass) AND contype = 'f'",
    )
    .fetch_one(pool)
    .await
    .expect("foreign keys");
    assert_eq!(foreign_keys, 0);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn concurrent_submissions_converge_and_conflicts_preserve_request() {
    let store = store().await;
    let (first, second) = tokio::join!(
        store.submit("demo:alice", "operator-a", Some("manual test")),
        store.submit("demo:alice", "operator-a", Some("manual test")),
    );
    let first = first.expect("first submission");
    let second = second.expect("concurrent submission");
    assert_eq!(first.id, second.id);
    assert_eq!(first.status, "inventoried");
    assert_eq!(first.entity_type, "user");
    assert_eq!(first.created_at, second.created_at);
    assert_eq!(first.updated_at, second.updated_at);
    assert!(store
        .submit("demo:alice", "operator-b", Some("manual test"))
        .await
        .is_err());
    assert!(store
        .submit("demo:alice", "operator-a", None)
        .await
        .is_err());
    let saved = store.get(first.id).await.expect("saved request");
    assert_eq!(
        serde_json::to_value(saved).unwrap(),
        serde_json::to_value(first).unwrap()
    );
    assert_eq!(store.list(100).await.expect("list").len(), 1);
    store
        .submit("demo:bob", "operator-a", None)
        .await
        .expect("second target");
    let recent = store.list(1).await.expect("bounded newest first");
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].target, "demo:bob");
    assert!(matches!(
        store.get(Uuid::new_v4()).await,
        Err(DbError::NotFound(_))
    ));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn invalid_submissions_do_not_create_rows() {
    let store = store().await;
    for target in [
        "alice",
        "demo:",
        "demo:Alice",
        "demo:-alice",
        "demo:alice@example.com",
    ] {
        assert!(
            store.submit(target, "operator-a", None).await.is_err(),
            "{target}"
        );
    }
    assert!(store
        .submit(&format!("demo:{}", "a".repeat(65)), "operator-a", None)
        .await
        .is_err());
    for actor in ["", " operator-a", "operator-a "] {
        assert!(store.submit("demo:alice", actor, None).await.is_err());
    }
    assert!(store
        .submit("demo:alice", &"a".repeat(257), None)
        .await
        .is_err());
    assert!(store
        .submit("demo:alice", "operator-a", Some(&"a".repeat(4097)))
        .await
        .is_err());
    assert!(store.list(100).await.expect("empty list").is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn desired_schema_has_plain_intake_without_database_guards() {
    let store = store().await;
    assert_plain_schema(&store.pool).await;
    crate::Db::from_pool(store.pool.clone())
        .validate_deletion_catalog()
        .await
        .expect("community catalog");
    assert!(sqlx::query(
        "INSERT INTO entity_deletion_requests (target, requested_by)
         VALUES ('real-user', 'operator-a')",
    )
    .execute(&store.pool)
    .await
    .is_err());
    assert!(sqlx::query(
        "INSERT INTO entity_deletion_requests (target, requested_by, status)
         VALUES ('demo:alice', 'operator-a', 'running')",
    )
    .execute(&store.pool)
    .await
    .is_err());
    store
        .submit("demo:alice", "operator-a", None)
        .await
        .expect("valid submission");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn migration_schema_entity_intake_preserves_existing_data() {
    let store = store().await;
    crate::migration::run_migrations_through(&store.pool, 67)
        .await
        .expect("previous schema");
    let community = Uuid::new_v4();
    sqlx::query("INSERT INTO communities (id, host) VALUES ($1, 'intake-upgrade.invalid')")
        .bind(community)
        .execute(&store.pool)
        .await
        .expect("existing community");
    crate::migration::run_migrations(&store.pool)
        .await
        .expect("upgrade");
    assert_plain_schema(&store.pool).await;
    let registered: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM _operator_global_tables
         WHERE table_name = 'entity_deletion_requests')",
    )
    .fetch_one(&store.pool)
    .await
    .expect("registry");
    assert!(registered);
    let request = store
        .submit("demo:alice", "operator-a", None)
        .await
        .expect("submit");
    crate::migration::run_migrations(&store.pool)
        .await
        .expect("rerun");
    assert_eq!(
        store
            .get(request.id)
            .await
            .expect("preserved request")
            .target,
        "demo:alice"
    );
    let host: String = sqlx::query_scalar("SELECT host FROM communities WHERE id = $1")
        .bind(community)
        .fetch_one(&store.pool)
        .await
        .expect("preserved community");
    assert_eq!(host, "intake-upgrade.invalid");
    crate::Db::from_pool(store.pool.clone())
        .validate_deletion_catalog()
        .await
        .expect("community catalog");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn approval_is_atomic_idempotent_and_bound_to_inventory() {
    let store = store().await;
    let request = store
        .submit("demo:approval", "operator-a", None)
        .await
        .unwrap();
    let digest = request.inventory_digest.as_deref().unwrap();
    let before = store.inspect(request.id).await.unwrap();
    assert!(before.stage_history.is_empty());
    let inventory = before.request.inventory_manifest.as_ref().unwrap();
    assert_eq!(inventory["communities"][0]["relationship"], "participant");
    assert_eq!(
        inventory["communities"][1]["relationship"],
        "former_participant"
    );
    for (actor, hash) in [("operator-b", "wrong"), (" operator-b", digest)] {
        assert!(store.approve(request.id, actor, hash).await.is_err());
        assert_eq!(
            serde_json::to_value(store.inspect(request.id).await.unwrap()).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
    }
    let (a, b) = tokio::join!(
        store.approve(request.id, "operator-b", digest),
        store.approve(request.id, "operator-b", digest),
    );
    let approved = a.unwrap();
    assert_eq!(approved.status, "approved");
    assert_eq!(approved.approved_at, b.unwrap().approved_at);
    assert_eq!(approved.approved_inventory_digest.as_deref(), Some(digest));
    let after = store.inspect(request.id).await.unwrap();
    assert_eq!(after.stage_history.len(), 1);
    let stage = &after.stage_history[0];
    assert_eq!(stage.request_id, request.id);
    assert_eq!(stage.stage, "gate_user");
    assert_eq!(stage.status, "pending");
    assert!(stage.is_current);
    assert!(stage.target_community_id.is_none());
    assert!(stage.started_at.is_none());
    assert!(stage.finished_at.is_none());
    assert_eq!(stage.metadata_json, serde_json::json!({}));
    assert!(store
        .approve(request.id, "operator-c", digest)
        .await
        .is_err());
    store
        .submit("demo:approval", "operator-a", None)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(store.inspect(request.id).await.unwrap()).unwrap(),
        serde_json::to_value(&after).unwrap()
    );
    // The invariant is enforced by the live partial unique index.
    assert!(sqlx::query(
        "INSERT INTO entity_deletion_stage_history (request_id, stage) VALUES ($1, 'drain')"
    )
    .bind(request.id)
    .execute(&store.pool)
    .await
    .is_err());
    // A missing parent is rejected by application code, without a foreign key.
    assert!(matches!(
        store.approve(Uuid::new_v4(), "operator-b", digest).await,
        Err(DbError::NotFound(_))
    ));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn approval_rolls_back_when_starting_stage_cannot_be_saved() {
    let store = store().await;
    let request = store
        .submit("demo:rollback", "operator-a", None)
        .await
        .unwrap();
    // Fault injection: a current row makes the final INSERT fail after the
    // approval UPDATE. The production transaction must roll back that UPDATE.
    sqlx::query(
        "INSERT INTO entity_deletion_stage_history (request_id, stage) VALUES ($1, 'unexpected')",
    )
    .bind(request.id)
    .execute(&store.pool)
    .await
    .unwrap();
    let before = store.inspect(request.id).await.unwrap();
    assert!(store
        .approve(
            request.id,
            "operator-b",
            request.inventory_digest.as_deref().unwrap()
        )
        .await
        .is_err());
    assert_eq!(
        serde_json::to_value(store.inspect(request.id).await.unwrap()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn approval_refuses_changed_inventory_and_missing_resume_position() {
    let store = store().await;
    let request = store
        .submit("demo:changed", "operator-a", None)
        .await
        .unwrap();
    let digest = request.inventory_digest.as_deref().unwrap();
    let mut inventory = request.inventory_manifest.clone().unwrap();
    inventory["communities"].as_array_mut().unwrap().reverse();
    sqlx::query("UPDATE entity_deletion_requests SET inventory_manifest = $2 WHERE id = $1")
        .bind(request.id)
        .bind(&inventory)
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(store
        .approve(request.id, "operator-b", digest)
        .await
        .is_err());
    assert_eq!(store.get(request.id).await.unwrap().status, "inventoried");
    sqlx::query("UPDATE entity_deletion_requests SET inventory_manifest = $2 WHERE id = $1")
        .bind(request.id)
        .bind(&request.inventory_manifest)
        .execute(&store.pool)
        .await
        .unwrap();
    store
        .approve(request.id, "operator-b", digest)
        .await
        .unwrap();
    sqlx::query("DELETE FROM entity_deletion_stage_history WHERE request_id = $1")
        .bind(request.id)
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(store
        .approve(request.id, "operator-b", digest)
        .await
        .is_err());
    assert!(store
        .inspect(request.id)
        .await
        .unwrap()
        .stage_history
        .is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn migration_schema_approval_preserves_legacy_intake_and_resubmission_prepares_it() {
    let store = store().await;
    crate::migration::run_migrations_through(&store.pool, 68)
        .await
        .unwrap();
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO entity_deletion_requests (target, requested_by) VALUES ('demo:legacy', 'operator-a') RETURNING id")
        .fetch_one(&store.pool).await.unwrap();
    crate::migration::run_migrations(&store.pool).await.unwrap();
    let before = store.inspect(id).await.unwrap();
    assert_eq!(before.request.status, "submitted");
    assert!(before.request.inventory_manifest.is_none());
    assert!(before.stage_history.is_empty());
    assert!(store.approve(id, "operator-b", "unknown").await.is_err());
    let prepared = store
        .submit("demo:legacy", "operator-a", None)
        .await
        .unwrap();
    assert_eq!(prepared.id, id);
    assert_eq!(prepared.created_at, before.request.created_at);
    store
        .approve(
            id,
            "operator-b",
            prepared.inventory_digest.as_deref().unwrap(),
        )
        .await
        .unwrap();
    assert_plain_schema(&store.pool).await;
    let registered: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM _operator_global_tables WHERE table_name = 'entity_deletion_stage_history')")
        .fetch_one(&store.pool).await.unwrap();
    assert!(registered);
    // The previous CLI's INSERT remains valid after the upgrade.
    sqlx::query("INSERT INTO entity_deletion_requests (target, requested_by) VALUES ('demo:old-cli', 'operator-a')")
        .execute(&store.pool).await.unwrap();
}
