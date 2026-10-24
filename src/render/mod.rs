//! Output policy: markdown tables and field/value rows, pagers, raw JSON.
//!
//! List/view commands render tables for humans and raw JSON under `--json`;
//! long output pages through `GLABRS_PAGER` > `PAGER` > config `pager` >
//! `less`.

/// Pager and stdin/file input helpers.
pub mod pager;
/// Table and JSON rendering.
pub mod table;
