//! Table discovery, MVT SQL generation, and the per-tile query.
//!
//! The MVT query mirrors Martin's proven approach: `ST_AsMVT` over
//! `ST_AsMVTGeom(ST_Transform(geom, 3857), …)`. Because every geometry is
//! transformed to Web Mercator at query time, a table in **any** SRID (e.g. a
//! local Cassini grid) renders correctly aligned — no client-side reprojection.

use martin_core::tiles::postgres::PostgresPool;
use martin_tile_utils::EARTH_CIRCUMFERENCE_DEGREES;
use tokio_postgres::types::ToSql;

const EXTENT: u32 = 4096;
const BUFFER: u32 = 64;

/// A discovered spatial table and the metadata needed to serve + describe it.
#[derive(Clone, Debug)]
pub struct PgTable {
    pub schema: String,
    pub table: String,
    pub geom: String,
    pub srid: i32,
    /// PostGIS geometry type string, e.g. `MULTIPOLYGON`.
    pub geom_type: String,
    /// Non-geometry columns as `(name, pg_type)` — emitted as MVT feature props.
    pub properties: Vec<(String, String)>,
}

/// Quote a SQL identifier (double-quote, double any embedded quote).
fn ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Quote a SQL string literal (single-quote, double any embedded quote).
fn lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// One round-trip discovery of all PostGIS vector tables (geometry + geography),
/// excluding system schemas, with their non-geometry columns as properties.
pub async fn discover_tables(pool: &PostgresPool) -> Result<Vec<PgTable>, String> {
    const SQL: &str = r"
WITH cols AS (
    SELECT n.nspname AS schema, c.relname AS tbl, a.attname AS col,
           trim(LEADING '_' FROM t.typname) AS typ
    FROM pg_attribute a
    JOIN pg_class c ON a.attrelid = c.oid
    JOIN pg_namespace n ON c.relnamespace = n.oid
    JOIN pg_type t ON a.atttypid = t.oid
    WHERE NOT a.attisdropped AND a.attnum > 0
)
SELECT gc.schema, gc.name, gc.geom, gc.srid, gc.type,
    (coalesce(
        jsonb_object_agg(cols.col, cols.typ) FILTER (
            WHERE cols.col IS NOT NULL
              AND cols.typ NOT IN ('geometry', 'geography')
        ), '{}'::jsonb))::text AS properties
FROM (
    SELECT f_table_schema AS schema, f_table_name AS name,
           f_geometry_column AS geom, srid, type
    FROM geometry_columns
    UNION ALL
    SELECT f_table_schema, f_table_name, f_geography_column, srid, type
    FROM geography_columns
) gc
LEFT JOIN cols
    ON cols.schema = gc.schema AND cols.tbl = gc.name AND cols.col <> gc.geom
WHERE gc.schema NOT IN ('information_schema', 'pg_catalog', 'topology', 'tiger', 'tiger_data')
GROUP BY gc.schema, gc.name, gc.geom, gc.srid, gc.type
ORDER BY gc.schema, gc.name;
";
    let client = pool.get().await.map_err(|e| format!("connect: {e}"))?;
    let rows = client.query(SQL, &[]).await.map_err(|e| format!("discover: {e}"))?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let props_json: String = r.get("properties");
        let properties = parse_props(&props_json);
        out.push(PgTable {
            schema: r.get("schema"),
            table: r.get("name"),
            geom: r.get("geom"),
            srid: r.get("srid"),
            geom_type: r.get("type"),
            properties,
        });
    }
    Ok(out)
}

/// Parse the `{"col":"type",…}` JSON object into a stable, sorted property list.
fn parse_props(json: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|val| val.as_object().cloned())
        .map(|obj| {
            obj.into_iter()
                .map(|(k, val)| (k, val.as_str().unwrap_or("").to_string()))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

impl PgTable {
    /// Build the cacheable `ST_AsMVT` query (`$1=z, $2=x, $3=y`) for this table.
    ///
    /// `supports_margin` reflects PostGIS ≥3.1 (so SRID-3857 tables can use the
    /// `ST_TileEnvelope` margin); 4326 uses `ST_Expand` in degrees; other SRIDs
    /// fall back to a plain transformed envelope.
    #[must_use]
    pub fn build_mvt_sql(&self, supports_margin: bool) -> String {
        let extent = EXTENT;
        let buffer = BUFFER;
        let srid = self.srid;
        let margin = f64::from(buffer) / f64::from(extent);
        let geom = ident(&self.geom);
        let schema = ident(&self.schema);
        let table = ident(&self.table);
        let layer = lit(&self.table);

        let bbox = if buffer == 0 {
            format!("ST_Transform(ST_TileEnvelope($1, $2, $3), {srid})")
        } else if supports_margin && srid == 3857 {
            format!(
                "ST_Transform(ST_TileEnvelope($1, $2, $3, margin => {margin}), {srid})"
            )
        } else if srid == 4326 {
            format!(
                "ST_Expand(ST_Transform(ST_TileEnvelope($1, $2, $3), {srid}), ({margin} * {EARTH_CIRCUMFERENCE_DEGREES}) / 2^$1)"
            )
        } else {
            format!("ST_Transform(ST_TileEnvelope($1, $2, $3), {srid})")
        };

        let props: String =
            self.properties.iter().map(|(name, _)| format!(", {}", ident(name))).collect();

        format!(
            "SELECT ST_AsMVT(tile, {layer}, {extent}, 'geom') FROM (\
             SELECT ST_AsMVTGeom(\
             ST_Transform(ST_CurveToLine({geom}::geometry), 3857), \
             ST_TileEnvelope($1, $2, $3), {extent}, {buffer}, true) AS geom{props} \
             FROM {schema}.{table} WHERE {geom} && {bbox}) AS tile"
        )
    }

    /// Best-effort bounds in WGS84 `[w, s, e, n]`: fast estimated extent first,
    /// then a time-boxed exact extent, else `None` (caller falls back to world).
    pub async fn compute_bounds(&self, pool: &PostgresPool) -> Option<[f64; 4]> {
        let client = pool.get().await.ok()?;

        // 1) ST_EstimatedExtent — instant, but null until the table is ANALYZEd.
        let est_sql = "SELECT ST_XMin(b), ST_YMin(b), ST_XMax(b), ST_YMax(b) FROM \
            (SELECT ST_Transform(ST_SetSRID(ST_EstimatedExtent($1,$2,$3)::geometry, $4), 4326) AS b) s \
            WHERE b IS NOT NULL";
        let params: [&(dyn ToSql + Sync); 4] = [&self.schema, &self.table, &self.geom, &self.srid];
        if let Ok(Some(row)) = client.query_opt(est_sql, &params).await {
            return Some([row.get(0), row.get(1), row.get(2), row.get(3)]);
        }

        // 2) Exact ST_Extent, time-boxed so a huge table can't stall discovery.
        // SET LOCAL inside a transaction auto-resets the timeout when the tx ends,
        // so it can never leak onto later tile queries that reuse this pooled
        // connection (deadpool's fast recycling does not reset session GUCs).
        let exact_sql = format!(
            "SELECT ST_XMin(b), ST_YMin(b), ST_XMax(b), ST_YMax(b) FROM \
             (SELECT ST_Transform(ST_SetSRID(ST_Extent({geom}::geometry), {srid}), 4326) AS b \
              FROM {schema}.{table}) s WHERE b IS NOT NULL",
            geom = ident(&self.geom),
            srid = self.srid,
            schema = ident(&self.schema),
            table = ident(&self.table),
        );
        let _ = client.batch_execute("BEGIN; SET LOCAL statement_timeout = '8s'").await;
        let result = client.query_opt(&exact_sql, &[]).await;
        let _ = client.batch_execute("ROLLBACK").await;
        match result {
            Ok(Some(row)) => Some([row.get(0), row.get(1), row.get(2), row.get(3)]),
            _ => None,
        }
    }
}

/// Run the MVT query for one tile, returning the raw protobuf (or `None` if empty).
pub async fn query_mvt(
    pool: &PostgresPool,
    sql: &str,
    z: i32,
    x: i32,
    y: i32,
) -> Result<Option<Vec<u8>>, String> {
    let client = pool.get().await.map_err(|e| format!("connect: {e}"))?;
    let stmt = client.prepare_cached(sql).await.map_err(|e| format!("prepare: {e}"))?;
    let params: [&(dyn ToSql + Sync); 3] = [&z, &x, &y];
    let row = client.query_opt(&stmt, &params).await.map_err(|e| format!("query: {e}"))?;
    Ok(row.and_then(|r| r.get::<_, Option<Vec<u8>>>(0)).filter(|b| !b.is_empty()))
}
