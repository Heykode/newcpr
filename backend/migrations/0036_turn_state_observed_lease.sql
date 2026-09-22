-- Shorten legacy local leases without granting any new lifetime during upgrade.
-- Preserve slots, identity bindings, versions, diagnostics and original capture clocks.
-- A separate probe clock prevents busy business traffic from postponing backup collection.
ALTER TABLE provider_turn_states ADD COLUMN probe_refresh_at TIMESTAMPTZ;

UPDATE provider_turn_states
SET active_expires_at = CASE WHEN active_state IS NOT NULL THEN
        LEAST(active_expires_at, COALESCE(active_captured_at, active_issued_at) + INTERVAL '240 seconds')
        ELSE active_expires_at END,
    standby_expires_at = CASE WHEN standby_state IS NOT NULL THEN
        LEAST(standby_expires_at, COALESCE(standby_captured_at, standby_issued_at) + INTERVAL '240 seconds')
        ELSE standby_expires_at END;
