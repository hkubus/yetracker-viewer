//! Static catalog table, copied verbatim from `apps/api/src/catalogs.ts`.

pub const PRIMARY_CATALOG_ID: &str = "unreleased";

pub struct CatalogDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub gid: &'static str,
    pub description: &'static str,
    pub main_page_section: bool,
}

pub const CATALOGS: &[CatalogDefinition] = &[
    CatalogDefinition {
        id: PRIMARY_CATALOG_ID,
        name: "Unreleased",
        gid: "34972268",
        description: "The main Ye Tracker era catalog.",
        main_page_section: false,
    },
    CatalogDefinition {
        id: "released",
        name: "Released",
        gid: "762588265",
        description: "Released songs, features, and production credits.",
        main_page_section: false,
    },
    CatalogDefinition {
        id: "recent",
        name: "Recent",
        gid: "77894385",
        description: "Recently added songs and updates from the tracker.",
        main_page_section: false,
    },
    CatalogDefinition {
        id: "best-of",
        name: "Best Of",
        gid: "787540803",
        description: "Standout songs selected from the tracker.",
        main_page_section: false,
    },
    CatalogDefinition {
        id: "worst-of",
        name: "Worst Of",
        gid: "1371492190",
        description: "The tracker’s collection of infamous and lowlight songs.",
        main_page_section: false,
    },
    CatalogDefinition {
        id: "special",
        name: "Special",
        gid: "812818104",
        description: "Songs with unusual history, versions, or context.",
        main_page_section: false,
    },
    CatalogDefinition {
        id: "grails-wanted",
        name: "Grails / Wanted",
        gid: "1948929917",
        description: "Highly sought-after songs and recordings.",
        main_page_section: false,
    },
    CatalogDefinition {
        id: "stems",
        name: "Stems",
        gid: "495336364",
        description: "Available stems, instrumentals, and stem bounces.",
        main_page_section: false,
    },
    CatalogDefinition {
        id: "album-copies",
        name: "Album Copies",
        gid: "1297512832",
        description: "Complete album and demo-tape copies.",
        main_page_section: true,
    },
    CatalogDefinition {
        id: "ssc",
        name: "Sunday Service Choir",
        gid: "1333371598",
        description: "Sunday Service Choir recordings and performances.",
        main_page_section: false,
    },
    CatalogDefinition {
        id: "fakes",
        name: "Fakes",
        gid: "61838480",
        description: "Documented fake leaks, rumors, and misattributions.",
        main_page_section: false,
    },
];

pub fn get_catalog(id: &str) -> Option<&'static CatalogDefinition> {
    CATALOGS.iter().find(|catalog| catalog.id == id)
}

/// All catalogs except the primary one and main-page sections (`album-copies`).
pub fn get_category_catalogs() -> impl Iterator<Item = &'static CatalogDefinition> {
    CATALOGS
        .iter()
        .filter(|catalog| catalog.id != PRIMARY_CATALOG_ID && !catalog.main_page_section)
}

pub fn catalog_source_url(gid: &str) -> String {
    format!("https://yetracker.net/#gid={gid}")
}
