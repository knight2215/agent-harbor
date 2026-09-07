// AutoRationaleTooltip (architecture.md Section 8.2 "why this model").
//
// Explains why Automatic routing picked a model. It lazily fetches the
// display-safe route preview via `get_route_explanation`; a `fallbackRationale`
// (typically the last assistant message's `RouteMetadata.rationale`) is shown
// until the fetch resolves or when no conversation is active.

import { useEffect, useState } from "react";
import { getRouteExplanation } from "../../ipc/commands";
import type { RouteExplanation } from "../../types";

export interface AutoRationaleTooltipProps {
  /** The conversation to explain, or null when none is active. */
  conversationId: string | null;
  /** A rationale to show before/without a fetched explanation (e.g. last route). */
  fallbackRationale?: string | null;
}

/** A small tooltip surfacing the routing rationale for the active conversation. */
export function AutoRationaleTooltip({
  conversationId,
  fallbackRationale = null,
}: AutoRationaleTooltipProps) {
  const [explanation, setExplanation] = useState<RouteExplanation | null>(null);

  useEffect(() => {
    if (conversationId === null) {
      setExplanation(null);
      return;
    }
    let active = true;
    getRouteExplanation(conversationId)
      .then((result) => {
        if (active) setExplanation(result);
      })
      .catch(() => {
        if (active) setExplanation(null);
      });
    return () => {
      active = false;
    };
  }, [conversationId]);

  const rationale = explanation?.rationale ?? fallbackRationale;
  if (rationale === null || rationale === undefined) return null;

  return (
    <span className="auto-rationale" role="note" title={rationale}>
      <span className="auto-rationale__label">Why this model?</span>
      <span className="auto-rationale__text">{rationale}</span>
    </span>
  );
}
