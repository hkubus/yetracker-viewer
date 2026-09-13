//! Static catalog table. Only the primary `unreleased` sheet is imported; the
//! additional tracker sheets were dropped along with the categories feature.

pub const PRIMARY_CATALOG_ID: &str = "unreleased";

pub struct CatalogDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub gid: &'static str,
}

pub const CATALOGS: &[CatalogDefinition] = &[CatalogDefinition {
    id: PRIMARY_CATALOG_ID,
    name: "Unreleased",
    gid: "34972268",
}];

pub fn catalog_source_url(gid: &str) -> String {
    format!("https://yetracker.net/#gid={gid}")
}
