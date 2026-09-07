-- Per-conversation routing mode (architecture.md Section 6.1 / 8.2).
--
-- Adds the nullable `routing_mode` JSON TEXT column storing an
-- `Option<RoutingMode>` (auto / preferLocal / preferQuality / manual). It is
-- additive and forward-compatible: existing rows read NULL, which the domain
-- model treats as `None` == Auto (no automatic hint bias). No default is set so
-- an unset mode stays NULL rather than being pinned to a concrete value.
ALTER TABLE conversations ADD COLUMN routing_mode TEXT; -- JSON: Option<RoutingMode>
