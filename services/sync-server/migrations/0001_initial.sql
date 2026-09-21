CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE SEQUENCE IF NOT EXISTS sync_version_seq;

CREATE TABLE users (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    email text NOT NULL,
    display_name text,
    avatar_url text,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE auth_identities (
    issuer text NOT NULL,
    subject text NOT NULL,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    email text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (issuer, subject)
);

CREATE TABLE auth_flows (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    oauth_state_hash bytea NOT NULL UNIQUE,
    desktop_state text NOT NULL,
    redirect_uri text NOT NULL,
    code_challenge text NOT NULL,
    device_name text NOT NULL,
    expires_at timestamptz NOT NULL,
    completed_at timestamptz
);

CREATE TABLE one_time_codes (
    code_hash bytea PRIMARY KEY,
    flow_id uuid NOT NULL REFERENCES auth_flows(id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz
);

CREATE TABLE devices (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz,
    UNIQUE (user_id, id)
);

CREATE TABLE sessions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id uuid NOT NULL,
    access_hash bytea NOT NULL UNIQUE,
    access_expires_at timestamptz NOT NULL,
    refresh_hash bytea NOT NULL UNIQUE,
    refresh_expires_at timestamptz NOT NULL,
    replaced_by uuid REFERENCES sessions(id),
    revoked_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (user_id, device_id) REFERENCES devices(user_id, id) ON DELETE CASCADE
);

CREATE TABLE entitlements (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    feature text NOT NULL CHECK (feature IN ('sync', 'hosted_ai')),
    source text NOT NULL CHECK (source IN ('beta', 'subscription')),
    expires_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, feature)
);

CREATE TABLE sync_records (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    entity_type text NOT NULL,
    entity_id text NOT NULL,
    version bigint NOT NULL,
    payload jsonb,
    deleted boolean NOT NULL DEFAULT false,
    updated_by uuid NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, entity_type, entity_id),
    FOREIGN KEY (user_id, updated_by) REFERENCES devices(user_id, id)
);

CREATE TABLE record_revisions (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    entity_type text NOT NULL,
    entity_id text NOT NULL,
    version bigint NOT NULL,
    changed_fields text[] NOT NULL,
    payload jsonb,
    deleted boolean NOT NULL,
    updated_by uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, entity_type, entity_id, version)
);

CREATE TABLE sync_changes (
    sequence bigint PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    entity_type text NOT NULL,
    entity_id text NOT NULL,
    version bigint NOT NULL,
    payload jsonb,
    deleted boolean NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sync_changes_user_sequence ON sync_changes(user_id, sequence);

CREATE TABLE sync_conflicts (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    entity_type text NOT NULL,
    entity_id text NOT NULL,
    current_version bigint NOT NULL,
    overlapping_fields text[] NOT NULL,
    cloud_payload jsonb,
    cloud_deleted boolean NOT NULL,
    device_patch jsonb,
    device_deleted boolean NOT NULL,
    device_id uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    resolved_at timestamptz
);
CREATE INDEX sync_conflicts_open ON sync_conflicts(user_id, created_at) WHERE resolved_at IS NULL;

CREATE TABLE processed_operations (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    operation_id text NOT NULL,
    acknowledgement jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, operation_id)
);

-- Application queries still include user_id explicitly. RLS is an additional
-- guard for deployments that set threestrands.user_id per transaction.
ALTER TABLE sync_records ENABLE ROW LEVEL SECURITY;
ALTER TABLE sync_records FORCE ROW LEVEL SECURITY;
ALTER TABLE record_revisions ENABLE ROW LEVEL SECURITY;
ALTER TABLE record_revisions FORCE ROW LEVEL SECURITY;
ALTER TABLE sync_changes ENABLE ROW LEVEL SECURITY;
ALTER TABLE sync_changes FORCE ROW LEVEL SECURITY;
ALTER TABLE sync_conflicts ENABLE ROW LEVEL SECURITY;
ALTER TABLE sync_conflicts FORCE ROW LEVEL SECURITY;
ALTER TABLE processed_operations ENABLE ROW LEVEL SECURITY;
ALTER TABLE processed_operations FORCE ROW LEVEL SECURITY;
CREATE POLICY sync_records_owner ON sync_records USING (user_id::text = current_setting('threestrands.user_id', true));
CREATE POLICY record_revisions_owner ON record_revisions USING (user_id::text = current_setting('threestrands.user_id', true));
CREATE POLICY sync_changes_owner ON sync_changes USING (user_id::text = current_setting('threestrands.user_id', true));
CREATE POLICY sync_conflicts_owner ON sync_conflicts USING (user_id::text = current_setting('threestrands.user_id', true));
CREATE POLICY processed_operations_owner ON processed_operations USING (user_id::text = current_setting('threestrands.user_id', true));
