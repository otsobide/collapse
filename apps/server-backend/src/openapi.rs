//! The OpenAPI description of this server, and the interactive page that
//! renders it.
//!
//! Both are embedded in the binary: the docs work with no network access and
//! no external assets, which is the point of shipping them at all (a CDN-backed
//! Swagger UI, the FastAPI default, would break on an offline host).

/// The OpenAPI 3.1 document, hand-written and kept next to the code.
///
/// `__VERSION__` is substituted at runtime so the documented version can never
/// drift from the crate's.
const DOCUMENT: &str = include_str!("../assets/openapi.json");

/// The self-contained documentation page served at `/docs`. It fetches
/// `openapi.json` from this same server and renders the operations, so adding
/// an endpoint to the document is enough to document it.
pub const DOCS_HTML: &str = include_str!("../assets/docs.html");

/// The OpenAPI document, with the crate version filled in.
pub fn spec() -> serde_json::Value {
    let document = DOCUMENT.replace("__VERSION__", env!("CARGO_PKG_VERSION"));
    serde_json::from_str(&document).expect("the embedded OpenAPI document must be valid JSON")
}
