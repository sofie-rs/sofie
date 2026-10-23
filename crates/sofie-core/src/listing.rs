//! The keys a page of a list is answered and asked under, from the database to the browser client.

pub const ITEMS: &str = "items";
pub const PAGE: &str = "page";
pub const SIZE: &str = "size";
pub const DEFAULT_SIZE: u64 = 20;
pub const HAS_NEXT: &str = "has_next";
pub const SORT: &str = "sort";
pub const FILTER: &str = "filter";
pub const QUERY: &str = "query";
pub const CONTROLS: &str = "controls";
pub const ERRORS: &str = "errors";
pub const REFUSAL_KEY: &str = "key";
pub const REFUSAL_ARGUMENTS: &str = "arguments";
pub const REFUSAL_MESSAGE: &str = "message";

pub const PAGE_FIELDS: [&str; 3] = [PAGE, SIZE, HAS_NEXT];
pub const LISTED_FIELDS: [&str; 8] = [PAGE, SIZE, HAS_NEXT, SORT, FILTER, QUERY, CONTROLS, ERRORS];
