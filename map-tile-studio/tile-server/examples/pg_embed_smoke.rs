//! Exercises the bundled-cluster lifecycle (`PgEmbed`) end-to-end against the
//! staged portable binaries: initdb a fresh cluster, start it, create the db +
//! PostGIS, connect, then stop. Run with an explicit binaries path + data dir:
//!   cargo run -p mts-tile-server --example pg_embed_smoke -- <pgsql_dir> <data_dir>

use std::path::PathBuf;

use martin_core::tiles::postgres::PostgresPool;
use mts_tile_server::pg::discover::discover_tables;
use mts_tile_server::pg::{PgConnection, PgEmbed};

fn main() {
    let mut argv = std::env::args().skip(1);
    let root = PathBuf::from(argv.next().expect("usage: <pgsql_dir> <data_dir>"));
    let data = PathBuf::from(argv.next().expect("usage: <pgsql_dir> <data_dir>"));

    let embed = PgEmbed::new(root, data);
    println!("binaries_present: {}", embed.binaries_present());
    println!("is_initialized:   {}", embed.is_initialized());

    let mut conn = PgConnection::bundled_default();
    conn.port = 5434; // avoid the M1 dev cluster on 5433

    match embed.ensure_running(&conn) {
        Ok(()) => println!("ensure_running: OK on :{}", conn.port),
        Err(e) => {
            println!("ensure_running: FAILED: {e}");
            return;
        }
    }
    println!("is_running(5434): {}", embed.is_running(conn.port));

    // Connect through the same path the registry uses + run discovery.
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        match PostgresPool::new(&conn.conn_string(), None, None, None, 2).await {
            Ok(pool) => {
                println!("supports_tile_margin: {}", pool.supports_tile_margin());
                match discover_tables(&pool).await {
                    Ok(t) => println!("discovery OK — {} table(s) in fresh db", t.len()),
                    Err(e) => println!("discovery FAILED: {e}"),
                }
            }
            Err(e) => println!("pool connect FAILED: {e}"),
        }
    });

    embed.stop().ok();
    println!("stopped. is_running(5434): {}", embed.is_running(conn.port));
}
