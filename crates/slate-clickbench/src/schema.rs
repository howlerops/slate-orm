//! The ClickBench `hits` table, as a record-layer schema.
//!
//! All 105 columns, because loading only the ones the queries read would
//! flatter a row store: the cost a row store pays and a column store does not
//! is exactly reading columns nobody asked for.

use slate_schema::{IndexDef, IndexId, Ordinal, TableDef, TableId};
use slate_tuple::ValueType;

/// The table's id in the catalog.
pub(crate) const HITS: TableId = TableId(1);

/// Every column, in the order the parquet file has them.
///
/// ClickBench's types are `int16`/`int32`/`int64`, `uint16` and `binary`. The
/// integers all become `I64`, which is what this value system offers and is
/// wide enough for every one of them; the binaries become `Str`.
pub(crate) const COLUMNS: &[(&str, ValueType)] = &[
    ("WatchID", ValueType::I64),
    ("JavaEnable", ValueType::I64),
    ("Title", ValueType::Str),
    ("GoodEvent", ValueType::I64),
    ("EventTime", ValueType::I64),
    ("EventDate", ValueType::I64),
    ("CounterID", ValueType::I64),
    ("ClientIP", ValueType::I64),
    ("RegionID", ValueType::I64),
    ("UserID", ValueType::I64),
    ("CounterClass", ValueType::I64),
    ("OS", ValueType::I64),
    ("UserAgent", ValueType::I64),
    ("URL", ValueType::Str),
    ("Referer", ValueType::Str),
    ("IsRefresh", ValueType::I64),
    ("RefererCategoryID", ValueType::I64),
    ("RefererRegionID", ValueType::I64),
    ("URLCategoryID", ValueType::I64),
    ("URLRegionID", ValueType::I64),
    ("ResolutionWidth", ValueType::I64),
    ("ResolutionHeight", ValueType::I64),
    ("ResolutionDepth", ValueType::I64),
    ("FlashMajor", ValueType::I64),
    ("FlashMinor", ValueType::I64),
    ("FlashMinor2", ValueType::Str),
    ("NetMajor", ValueType::I64),
    ("NetMinor", ValueType::I64),
    ("UserAgentMajor", ValueType::I64),
    ("UserAgentMinor", ValueType::Str),
    ("CookieEnable", ValueType::I64),
    ("JavascriptEnable", ValueType::I64),
    ("IsMobile", ValueType::I64),
    ("MobilePhone", ValueType::I64),
    ("MobilePhoneModel", ValueType::Str),
    ("Params", ValueType::Str),
    ("IPNetworkID", ValueType::I64),
    ("TraficSourceID", ValueType::I64),
    ("SearchEngineID", ValueType::I64),
    ("SearchPhrase", ValueType::Str),
    ("AdvEngineID", ValueType::I64),
    ("IsArtifical", ValueType::I64),
    ("WindowClientWidth", ValueType::I64),
    ("WindowClientHeight", ValueType::I64),
    ("ClientTimeZone", ValueType::I64),
    ("ClientEventTime", ValueType::I64),
    ("SilverlightVersion1", ValueType::I64),
    ("SilverlightVersion2", ValueType::I64),
    ("SilverlightVersion3", ValueType::I64),
    ("SilverlightVersion4", ValueType::I64),
    ("PageCharset", ValueType::Str),
    ("CodeVersion", ValueType::I64),
    ("IsLink", ValueType::I64),
    ("IsDownload", ValueType::I64),
    ("IsNotBounce", ValueType::I64),
    ("FUniqID", ValueType::I64),
    ("OriginalURL", ValueType::Str),
    ("HID", ValueType::I64),
    ("IsOldCounter", ValueType::I64),
    ("IsEvent", ValueType::I64),
    ("IsParameter", ValueType::I64),
    ("DontCountHits", ValueType::I64),
    ("WithHash", ValueType::I64),
    ("HitColor", ValueType::Str),
    ("LocalEventTime", ValueType::I64),
    ("Age", ValueType::I64),
    ("Sex", ValueType::I64),
    ("Income", ValueType::I64),
    ("Interests", ValueType::I64),
    ("Robotness", ValueType::I64),
    ("RemoteIP", ValueType::I64),
    ("WindowName", ValueType::I64),
    ("OpenerName", ValueType::I64),
    ("HistoryLength", ValueType::I64),
    ("BrowserLanguage", ValueType::Str),
    ("BrowserCountry", ValueType::Str),
    ("SocialNetwork", ValueType::Str),
    ("SocialAction", ValueType::Str),
    ("HTTPError", ValueType::I64),
    ("SendTiming", ValueType::I64),
    ("DNSTiming", ValueType::I64),
    ("ConnectTiming", ValueType::I64),
    ("ResponseStartTiming", ValueType::I64),
    ("ResponseEndTiming", ValueType::I64),
    ("FetchTiming", ValueType::I64),
    ("SocialSourceNetworkID", ValueType::I64),
    ("SocialSourcePage", ValueType::Str),
    ("ParamPrice", ValueType::I64),
    ("ParamOrderID", ValueType::Str),
    ("ParamCurrency", ValueType::Str),
    ("ParamCurrencyID", ValueType::I64),
    ("OpenstatServiceName", ValueType::Str),
    ("OpenstatCampaignID", ValueType::Str),
    ("OpenstatAdID", ValueType::Str),
    ("OpenstatSourceID", ValueType::Str),
    ("UTMSource", ValueType::Str),
    ("UTMMedium", ValueType::Str),
    ("UTMCampaign", ValueType::Str),
    ("UTMContent", ValueType::Str),
    ("UTMTerm", ValueType::Str),
    ("FromTag", ValueType::Str),
    ("HasGCLID", ValueType::I64),
    ("RefererHash", ValueType::I64),
    ("URLHash", ValueType::I64),
    ("CLID", ValueType::I64),
];

/// A synthetic column appended to make the sort key unique. See [`table`].
pub(crate) const ROW_ORDINAL: &str = "RowOrdinal";

/// The ordinal of a column by name.
///
/// # Panics
/// If the column does not exist, which would be a typo in a query below.
#[must_use]
pub(crate) fn col(name: &str) -> Ordinal {
    table().ordinal_of(name).expect("a hits column")
}

/// The `hits` table.
///
/// The primary key is ClickHouse's own `ORDER BY` for this dataset —
/// `(CounterID, EventDate, UserID, EventTime, WatchID)` — because that is the
/// fairest mapping onto a key-ordered store, and it is what makes the
/// `CounterID = 62 AND EventDate BETWEEN …` queries a range rather than a
/// scan. That key is not unique, so a row ordinal is appended: the order is
/// preserved and two identical hits do not collide.
///
/// Two secondary indexes, chosen the way a person would rather than to win:
/// the grouping columns several queries filter on.
#[must_use]
pub(crate) fn table() -> TableDef {
    let mut builder = TableDef::builder("hits", HITS);
    for (name, ty) in COLUMNS {
        builder = builder.column(*name, *ty);
    }
    builder
        .column(ROW_ORDINAL, ValueType::U64)
        .primary_key([
            "CounterID",
            "EventDate",
            "UserID",
            "EventTime",
            "WatchID",
            ROW_ORDINAL,
        ])
        .index(IndexDef::builder("by_advengine", IndexId(10)).column("AdvEngineID"))
        .index(IndexDef::builder("by_userid", IndexId(11)).column("UserID"))
        .build()
        .expect("valid schema")
}
