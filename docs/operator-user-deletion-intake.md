# Prepare and approve a synthetic user-deletion request

Submit saves a request and a fixed dummy inventory: community B (participant),
then community C (former participant). These are artificial UUIDs, not discovered
communities. Approve saves the approval and a pending `gate_user` stage. No worker
runs and no user or community data is changed. Only `demo:` labels are accepted.

## Prepare a local database

From the repository root, activate Hermit and start a separate PostgreSQL
container. These are disposable local credentials.

```bash
. ./bin/activate-hermit
docker run --detach --name buzz-user-deletion-intake \
  --publish 127.0.0.1:55433:5432 \
  --env POSTGRES_USER=buzz \
  --env POSTGRES_PASSWORD=buzz-local-demo \
  --env POSTGRES_DB=buzz postgres:17-alpine
export DATABASE_URL=postgres://buzz:buzz-local-demo@127.0.0.1:55433/buzz
cargo build -p buzz-admin
# Wait for "accepting connections" before migrating.
docker exec buzz-user-deletion-intake pg_isready -U buzz -d buzz
./target/debug/buzz-admin migrate
```

Use an unused port and container name if these are occupied. The commands below
need only PostgreSQL; a relay, Redis, and object storage are unnecessary.

## Submit and see the row

```bash
./target/debug/buzz-admin user-deletions submit \
  --target demo:alice --requested-by operator-a
```

The command prints the saved row as JSON. It contains:

| Field | Value |
| --- | --- |
| `id` | New UUID; copy this for inspection |
| `entity_type` | `user` |
| `target` | `demo:alice` |
| `status` | `inventoried` |
| `requested_by` | `operator-a` |
| `reason` | `null`, unless supplied with `--reason` |
| `created_at` | Submission time |
| `updated_at` | Same time; no execution has started |
| `inventory_manifest` | Ordered dummy communities and their actions |
| `inventory_digest` | SHA-256 identifying the saved inventory |
| `approved_by`, `approved_at`, `approved_inventory_digest` | `null` before approval |

List recent requests, then inspect the UUID returned above:

```bash
./target/debug/buzz-admin user-deletions list --limit 10
REQUEST_ID=<returned-uuid>
./target/debug/buzz-admin user-deletions inspect "$REQUEST_ID"
```

To see the PostgreSQL row directly:

```bash
docker exec buzz-user-deletion-intake psql -X -U buzz -d buzz -c \
  "SELECT id, entity_type, target, status, requested_by, created_at
   FROM entity_deletion_requests ORDER BY created_at DESC;"
```

Exactly one row is saved in `entity_deletion_requests`. Repeating the same
target, operator, and reason returns that row without changing its timestamps.
Reusing the target with a different operator or reason fails and preserves the
original row. Use a new `demo:` label for a new request.

## Approve and see the starting stage

Inspect the inventory before approving. Copy its `inventory_digest` into the
command so approval is bound to what you inspected:

```bash
INVENTORY_DIGEST=<digest-from-inspect>
./target/debug/buzz-admin user-deletions approve "$REQUEST_ID" \
  --approved-by operator-b --inventory-digest "$INVENTORY_DIGEST"
./target/debug/buzz-admin user-deletions inspect "$REQUEST_ID"
```

The parent now has `status: approved`, the approver, approval time, and approved
inventory digest. `stage_history` contains exactly one row:

```json
{
  "stage": "gate_user",
  "status": "pending",
  "is_current": true,
  "target_community_id": null,
  "started_at": null,
  "finished_at": null,
  "metadata_json": {}
}
```

The row also includes its UUID, parent request UUID, `last_error`, and update
time. It has not started executing. Inspect reads parent and history from one
consistent database snapshot.

```bash
docker exec buzz-user-deletion-intake psql -X -U buzz -d buzz -c \
  "SELECT id, status, approved_by, approved_at, approved_inventory_digest
   FROM entity_deletion_requests ORDER BY created_at DESC;"
docker exec buzz-user-deletion-intake psql -X -U buzz -d buzz -c \
  "SELECT request_id, stage, status, is_current, target_community_id,
          started_at, finished_at, metadata_json
   FROM entity_deletion_stage_history;"
```

Approval and creation of the first stage commit together. A failure rolls both
back. Repeating the same approval returns the existing evidence and stage;
a different approver or inventory digest fails without changing them. A partial
unique index prevents two current stages for one request. Retries in the later
executor will update the same stage row.

Operator names are self-asserted demo input. This is not authenticated approval
for real deletion. Owned communities, worker leases, stage transitions, and
actual deletion handlers belong to later changes.

## Upgrade and rollout

Apply after the intake schema from PR #8496. Migrations 0069 and 0070 add nullable
inventory/approval columns, widen the lifecycle constraint, and create the
operator-global history table. Constraint validation uses a separate transaction
so it does not retain the schema-change transaction's write-blocking lock. Both
migrations set a five-second lock timeout and a thirty-second statement timeout.
There are no new triggers, functions, foreign keys, or data backfills.

Existing `submitted` requests remain unchanged by migration. Resubmit with the
same target, operator, and reason to prepare their dummy inventory; the request
ID and original creation time stay unchanged. The previous CLI can still insert
and read requests against the expanded schema. The serving relay does not
execute these requests. History intentionally survives community deletion as
operator recovery/audit evidence.

Verify on a test environment with the submit, inspect, approve, and SQL commands
above using a new `demo:` target. Rollback has the usual sqlx limitation: an older
binary running auto-migration refuses unknown migrations (`VersionMissing`).
Do not roll back to an older auto-migrating relay without the deployment's
reviewed rollback procedure; the additive schema itself can remain in place.
