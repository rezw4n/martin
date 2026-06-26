//! Manual end-to-end check of PostGIS discovery + MVT serving against a running
//! cluster (defaults to the dev test cluster from M1). Run with:
//!   cargo run -p mts-tile-server --example pg_smoke

use martin_core::tiles::postgres::PostgresPool;
use mts_tile_server::pg::discover::{discover_tables, query_mvt};
use mts_tile_server::pg::PgConnection;

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(run());
}

async fn run() {
    let conn = PgConnection {
        id: "test".into(),
        label: "test".into(),
        host: "127.0.0.1".into(),
        port: 5433,
        dbname: "gis".into(),
        user: "postgres".into(),
        password: "postgres".into(),
        sslmode: "disable".into(),
        enabled: true,
        bundled: false,
    };
    let pool = PostgresPool::new(&conn.conn_string(), None, None, None, 2)
        .await
        .expect("connect");
    println!("supports_tile_margin: {}", pool.supports_tile_margin());

    let tables = discover_tables(&pool).await.expect("discover");
    println!("\n=== discovered {} table(s) ===", tables.len());
    for t in &tables {
        println!(
            "  {}.{}  geom={} srid={} type={} props={}",
            t.schema,
            t.table,
            t.geom,
            t.srid,
            t.geom_type,
            t.properties.len()
        );
    }

    for name in ["district", "mauza_diff"] {
        if let Some(t) = tables.iter().find(|t| t.table == name) {
            let sql = t.build_mvt_sql(pool.supports_tile_margin());
            let bytes = query_mvt(&pool, &sql, 7, 96, 55).await.expect("mvt");
            let bounds = t.compute_bounds(&pool).await;
            println!(
                "\n[{name}] z7/96/55 -> {} bytes | bounds(4326)={:?}",
                bytes.as_ref().map_or(0, std::vec::Vec::len),
                bounds
            );
        }
    }
}
