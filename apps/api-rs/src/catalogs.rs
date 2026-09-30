//! The catalog the importer reads: the `unreleased` tab of the tracker sheet.

pub const PRIMARY_CATALOG_ID: &str = "unreleased";

pub struct CatalogDefinition {
    /// Sheet tab id (`gid`).
    pub gid: &'static str,
    /// Google Sheets document that yetracker.net mirrors (the "Sheet Link" in
    /// the name column's header). Its own `htmlview` is never cached, unlike
    /// the mirror, so the artwork URLs in it can still be downloaded.
    pub google_doc_id: &'static str,
}

pub const PRIMARY_CATALOG: CatalogDefinition = CatalogDefinition {
    gid: "34972268",
    google_doc_id: "1shKl9S-r5d1vgzYGSEyWflyyn0LS_AKJ7Ydjsczbb0Y",
};

/// Human-facing link to the catalog, used in logs.
pub fn catalog_source_url(gid: &str) -> String {
    format!("https://yetracker.net/#gid={gid}")
}

/// The sheet HTML the importer fetches (yetracker.net's mirror of the Google
/// Sheets `htmlview`, cached by its CDN for up to an hour).
pub fn catalog_sheet_url(catalog: &CatalogDefinition) -> String {
    format!(
        "https://yetracker.net/htmlview/sheet?headers=true&gid={}",
        catalog.gid
    )
}

/// The same sheet straight from Google Sheets, rendered fresh on every request
/// (a few seconds per fetch). Artwork URLs (`docs.google.com/sheets-images-rt/…`)
/// only download shortly after the page was rendered, so cover downloads need
/// this page rather than the cached mirror.
pub fn google_sheet_url(catalog: &CatalogDefinition) -> String {
    format!(
        "https://docs.google.com/spreadsheets/d/{}/htmlview/sheet?headers=true&gid={}",
        catalog.google_doc_id, catalog.gid
    )
}
