//! Session lifecycle.
//!
//! The `Conversation`, `Message`, and related domain types now live in
//! [`crate::models`] (architecture.md Section 7.1). The session manager
//! (create / append / persona assign / per-conversation lock) lands in a later
//! Phase 1 feature and builds on those models.
