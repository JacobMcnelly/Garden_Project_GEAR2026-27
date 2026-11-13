use std::sync::{Arc, Mutex, PoisonError};

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use rusqlite::{Connection, named_params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tower_http::services::ServeDir;

const MIGRATIONS: &[(u32, &str)] = &[(1, include_str!("../../db/migrations/0001_init.sql"))];

fn open(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    // WAL can't be switched on inside a transaction, so pragmas live here, not in a migration.
    conn.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
    )?;
    Ok(conn)
}

fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let current: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for &(version, sql) in MIGRATIONS.iter().filter(|(v, _)| *v > current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

#[derive(Serialize)]
struct History {
    bucket: i64,
    t: Vec<i64>,
    sensors: Vec<SensorSeries>,
}

#[derive(Serialize)]
struct SensorSeries {
    name: String,
    unit: Option<String>,
    values: Vec<Option<f64>>,
}

#[derive(Deserialize)]
struct HistoryParams {
    from: i64,
    to: i64,
}

/// `max(60, ceil(span / 1000))`, so `t` never has more than 1001 entries. `None` if `to <= from`.
fn bucket_size(from: i64, to: i64) -> Option<i64> {
    let span = to.checked_sub(from).filter(|s| *s > 0)?;
    Some(((span - 1) / 1000 + 1).max(60))
}

fn history(conn: &Connection, from: i64, to: i64, bucket: i64) -> rusqlite::Result<History> {
    let start = from / bucket * bucket;
    let n = (to / bucket * bucket - start) / bucket + 1;
    let t: Vec<i64> = (0..n).map(|i| start + i * bucket).collect();

    let mut sensors: Vec<SensorSeries> = Vec::new();
    let mut stmt = conn.prepare_cached(include_str!("../../db/history.sql"))?;
    let mut rows = stmt.query(named_params! {":from": from, ":to": to, ":bucket": bucket})?;
    while let Some(row) = rows.next()? {
        let name: String = row.get("name")?;
        if sensors.last().is_none_or(|s| s.name != name) {
            sensors.push(SensorSeries {
                name,
                unit: row.get("unit")?,
                values: vec![None; t.len()],
            });
        }
        let i = (row.get::<_, i64>("t")? - start) / bucket;
        sensors.last_mut().unwrap().values[i as usize] = row.get("avg_value")?;
    }
    Ok(History { bucket, t, sensors })
}

type ApiError = (StatusCode, Json<Value>);

async fn get_history(
    State(conn): State<Arc<Mutex<Connection>>>,
    Query(p): Query<HistoryParams>,
) -> Result<Json<History>, ApiError> {
    let bucket = bucket_size(p.from, p.to).ok_or((
        StatusCode::BAD_REQUEST,
        Json(json!({"error": "to must be greater than from"})),
    ))?;
    let res = tokio::task::spawn_blocking(move || {
        // A panic elsewhere must not lock every later request out of the database.
        let conn = conn.lock().unwrap_or_else(PoisonError::into_inner);
        history(&conn, p.from, p.to, bucket)
    })
    .await;
    let err = match res {
        Ok(Ok(h)) => return Ok(Json(h)),
        Ok(Err(e)) => e.to_string(),
        Err(e) => e.to_string(),
    };
    eprintln!("/api/history: {err}");
    Err((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    ))
}

async fn shutdown() {
    let ctrl_c = async { tokio::signal::ctrl_c().await.expect("Ctrl-C handler") };
    #[cfg(unix)]
    let term = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}

#[tokio::main]
async fn main() {
    let mut conn = open("garden.db").expect("open garden.db");
    migrate(&mut conn).expect("run migrations");

    let app = axum::Router::new()
        .route("/api/history", axum::routing::get(get_history))
        .fallback_service(ServeDir::new("web/dist"))
        .with_state(Arc::new(Mutex::new(conn)));
    // Localhost only: tailscale serve is the sole access control (ADR 0002).
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080")
        .await
        .expect("bind 127.0.0.1:8080");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await
        .expect("server error");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(conn: &Connection) -> u32 {
        conn.query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn migrations_apply_once() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn), 1);
        let tables: u32 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name IN ('sensors', 'readings')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 2);

        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn), 1);
    }

    #[test]
    fn readings_value_is_strict() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        conn.execute("INSERT INTO sensors (id, name) VALUES (1, 'soil')", [])
            .unwrap();
        assert!(
            conn.execute("INSERT INTO readings VALUES (0, 1, 'wet')", [])
                .is_err()
        );
    }

    fn db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        conn.execute_batch(
            "INSERT INTO sensors (id, name, unit) VALUES (1, 'soil', '%'), (2, 'temp', NULL), (3, 'old', 'x');",
        )
        .unwrap();
        conn
    }

    fn add(conn: &Connection, ts: i64, sensor: i64, value: f64) {
        conn.execute(
            "INSERT INTO readings VALUES (?1, ?2, ?3)",
            (ts, sensor, value),
        )
        .unwrap();
    }

    #[test]
    fn bucket_formula() {
        assert_eq!(bucket_size(0, 600), Some(60));
        assert_eq!(bucket_size(0, 86_400), Some(87));
        let (from, to) = (1_700_000_123, 1_700_000_123 + 366 * 86_400);
        let h = history(&db(), from, to, bucket_size(from, to).unwrap()).unwrap();
        assert!(h.t.len() <= 1001, "{}", h.t.len());
    }

    #[test]
    fn to_not_after_from_is_rejected() {
        assert_eq!(bucket_size(100, 100), None);
        assert_eq!(bucket_size(100, 50), None);
        assert_eq!(bucket_size(i64::MIN, i64::MAX), None);
    }

    #[test]
    fn history_aligns_averages_and_filters() {
        let conn = db();
        add(&conn, 59, 1, 1.0); // before `from`
        add(&conn, 120, 1, 10.0);
        add(&conn, 150, 1, 20.0); // same bucket as 120
        add(&conn, 240, 1, 5.0); // bucket 180 left empty
        add(&conn, 180, 2, 7.0);
        add(&conn, 301, 2, 9.0); // after `to`
        add(&conn, 1000, 3, 1.0); // `old` has no data in range
        let h = history(&conn, 60, 300, 60).unwrap();
        assert_eq!(h.t, vec![60, 120, 180, 240, 300]);
        let got: Vec<_> = h
            .sensors
            .iter()
            .map(|s| (s.name.as_str(), s.values.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("soil", vec![None, Some(15.0), None, Some(5.0), None]),
                ("temp", vec![None, None, Some(7.0), None, None]),
            ]
        );
    }

    #[test]
    fn history_json_shape() {
        let conn = db();
        add(&conn, 60, 1, 41.0);
        add(&conn, 120, 2, 20.5);
        let h = history(&conn, 60, 120, 60).unwrap();
        assert_eq!(
            serde_json::to_value(&h).unwrap(),
            json!({"bucket": 60, "t": [60, 120], "sensors": [
                {"name": "soil", "unit": "%", "values": [41.0, null]},
                {"name": "temp", "unit": null, "values": [null, 20.5]},
            ]})
        );
    }
}
