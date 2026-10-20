-- The `bot` channel role now means "a member whose key has a recorded agent
-- owner". The relay stores `bot` instead of `member` exactly when
-- users.agent_owner_pubkey is set, whatever the client asked for. This makes
-- the existing rows follow the same rule:
--   1. an owned agent's `member` rows become `bot`;
--   2. a `bot` row whose key has no recorded owner becomes `member`.
-- Only active rows change; a removed row's role is history. Communities that
-- are not active are skipped, because the community write fence refuses
-- writes to them. Bot and member have the same permissions, so no access
-- changes. The relay republishes the affected kind:39002 rosters at startup.
SET LOCAL lock_timeout = '5s';

UPDATE channel_members cm
SET role = 'bot'
FROM users u, communities c
WHERE u.community_id = cm.community_id
  AND u.pubkey = cm.pubkey
  AND u.agent_owner_pubkey IS NOT NULL
  AND c.id = cm.community_id
  AND c.deletion_state = 'active'
  AND cm.role = 'member'
  AND cm.removed_at IS NULL;

UPDATE channel_members cm
SET role = 'member'
FROM communities c
WHERE c.id = cm.community_id
  AND c.deletion_state = 'active'
  AND cm.role = 'bot'
  AND cm.removed_at IS NULL
  AND NOT EXISTS (
      SELECT 1
      FROM users u
      WHERE u.community_id = cm.community_id
        AND u.pubkey = cm.pubkey
        AND u.agent_owner_pubkey IS NOT NULL
  );
