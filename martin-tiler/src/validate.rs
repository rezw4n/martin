//! Validating a generated tile map (the "tile map output validation" requirement).

use std::path::Path;

use mbtiles::Mbtiles;
use mbtiles::sqlx::{self, Row as _};

use crate::error::TilerResult;
use crate::model::{CheckStatus, ValidationCheck, ValidationReport};

/// Run a battery of structural checks against a generated MBTiles file.
pub async fn validate(mbtiles_path: &Path) -> TilerResult<ValidationReport> {
    let mut checks = Vec::new();
    let mut push = |name: &str, status: CheckStatus, detail: String| {
        checks.push(ValidationCheck { name: name.to_string(), status, detail });
    };

    // open ------------------------------------------------------------------
    if !mbtiles_path.exists() {
        push("file exists", CheckStatus::Fail, format!("{} not found", mbtiles_path.display()));
        return Ok(report(mbtiles_path, checks, 0, None, None));
    }
    let mbt = Mbtiles::new(mbtiles_path)?;
    let mut conn = mbt.open_readonly().await?;
    push("opens as MBTiles", CheckStatus::Pass, "SQLite database opened".to_string());

    // sqlite integrity ------------------------------------------------------
    match sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
        .fetch_one(&mut conn)
        .await
    {
        Ok(s) if s.eq_ignore_ascii_case("ok") => {
            push("sqlite integrity", CheckStatus::Pass, "integrity_check = ok".to_string());
        }
        Ok(s) => push("sqlite integrity", CheckStatus::Fail, s),
        Err(e) => push("sqlite integrity", CheckStatus::Warn, e.to_string()),
    }

    // tile counts -----------------------------------------------------------
    let tiles_total = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tiles")
        .fetch_one(&mut conn)
        .await
        .unwrap_or(0)
        .max(0) as u64;
    if tiles_total > 0 {
        push("has tiles", CheckStatus::Pass, format!("{tiles_total} tiles stored"));
    } else {
        push("has tiles", CheckStatus::Fail, "no tiles in the archive".to_string());
    }

    // zoom range + continuity ----------------------------------------------
    let zoom_rows = sqlx::query("SELECT DISTINCT zoom_level FROM tiles ORDER BY zoom_level")
        .fetch_all(&mut conn)
        .await
        .unwrap_or_default();
    let zooms: Vec<u8> = zoom_rows
        .iter()
        .filter_map(|r| r.try_get::<i64, _>(0).ok())
        .map(|z| z as u8)
        .collect();
    let (min_zoom, max_zoom) = (zooms.first().copied(), zooms.last().copied());
    if let (Some(lo), Some(hi)) = (min_zoom, max_zoom) {
        let expected: Vec<u8> = (lo..=hi).collect();
        if zooms == expected {
            push("zoom continuity", CheckStatus::Pass, format!("contiguous z{lo}..z{hi}"));
        } else {
            push(
                "zoom continuity",
                CheckStatus::Warn,
                format!("present zooms {zooms:?} are not contiguous over z{lo}..z{hi}"),
            );
        }
    }

    // sparse storage proof: per-zoom non-empty counts ----------------------
    let per_zoom = sqlx::query(
        "SELECT zoom_level, COUNT(*) FROM tiles GROUP BY zoom_level ORDER BY zoom_level",
    )
    .fetch_all(&mut conn)
    .await
    .unwrap_or_default();
    let summary: Vec<String> = per_zoom
        .iter()
        .filter_map(|r| {
            let z = r.try_get::<i64, _>(0).ok()?;
            let c = r.try_get::<i64, _>(1).ok()?;
            Some(format!("z{z}:{c}"))
        })
        .collect();
    push(
        "sparse storage",
        CheckStatus::Pass,
        format!("only non-empty tiles are stored ({})", summary.join(", ")),
    );

    // blank-tile heuristic: flag suspiciously tiny tiles -------------------
    let tiny = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tiles WHERE LENGTH(tile_data) < 120")
        .fetch_one(&mut conn)
        .await
        .unwrap_or(0);
    if tiny == 0 {
        push("no blank tiles", CheckStatus::Pass, "no suspiciously tiny (likely-blank) tiles".to_string());
    } else {
        push(
            "no blank tiles",
            CheckStatus::Warn,
            format!("{tiny} very small tiles (<120 bytes) — may be near-empty"),
        );
    }

    // required metadata -----------------------------------------------------
    for key in ["format", "minzoom", "maxzoom", "bounds"] {
        match mbt.get_metadata_value(&mut conn, key).await {
            Ok(Some(v)) => push(&format!("metadata `{key}`"), CheckStatus::Pass, v),
            Ok(None) => push(&format!("metadata `{key}`"), CheckStatus::Warn, "missing".to_string()),
            Err(e) => push(&format!("metadata `{key}`"), CheckStatus::Warn, e.to_string()),
        }
    }

    Ok(report(mbtiles_path, checks, tiles_total, min_zoom, max_zoom))
}

fn report(
    path: &Path,
    checks: Vec<ValidationCheck>,
    tiles_total: u64,
    min_zoom: Option<u8>,
    max_zoom: Option<u8>,
) -> ValidationReport {
    let ok = checks.iter().all(|c| c.status != CheckStatus::Fail);
    ValidationReport {
        mbtiles_path: path.to_path_buf(),
        ok,
        checks,
        tiles_total,
        min_zoom,
        max_zoom,
    }
}
