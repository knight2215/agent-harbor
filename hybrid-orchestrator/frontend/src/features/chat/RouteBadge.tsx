// RouteBadge (architecture.md Section 8.1 chat surface).
//
// Renders a message's RouteMetadata: which provider/model answered, the route
// source, and the routing rationale (surfaced on hover via the native title
// tooltip). Renders nothing when a message has no recorded route.

import type { RouteMetadata, RouteSource } from "../../types";

/** Human-readable label for a route source. */
function sourceLabel(source: RouteSource): string {
  switch (source) {
    case "manual":
      return "manual override";
    case "conversationPin":
      return "pinned";
    case "automatic":
      return "auto";
  }
}

export interface RouteBadgeProps {
  route: RouteMetadata;
}

export function RouteBadge({ route }: RouteBadgeProps) {
  return (
    <span className="route-badge" title={route.rationale} data-source={route.source}>
      <span className="route-badge__model">
        {route.providerId} / {route.model}
      </span>
      <span className="route-badge__source">{sourceLabel(route.source)}</span>
    </span>
  );
}
